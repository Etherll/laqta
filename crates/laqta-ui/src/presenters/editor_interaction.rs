//! Editor input turned into chrome updates and background renders.
//!
//! The engine lock is held only for `handle` and `render_job()`. A render runs on the `RenderJob`
//! snapshot, so the UI thread never waits for a render.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use laqta_domain::Bitmap;
use laqta_editor::{Chrome, Editor, EditorInput, EditorOutput, EditorRequest};
use laqta_ports::BoxFuture;

use super::render_gate::{ComputeFn, RenderGate, compute};

/// What the UI does after one input: repaint the chrome, act on a request, start a render.
#[derive(Debug)]
pub struct Step {
    pub chrome: Chrome,
    pub request: Option<EditorRequest>,
    pub render_now: bool,
}

/// An open editor: the engine, the render gate and the compute pool it renders on.
pub struct EditorInteraction {
    editor: Arc<Mutex<Editor>>,
    compute: ComputeFn,
    gate: RenderGate,
}

impl EditorInteraction {
    pub fn new(editor: Editor, compute: ComputeFn) -> Self {
        Self { editor: Arc::new(Mutex::new(editor)), compute, gate: RenderGate::default() }
    }

    pub fn editor(&self) -> Arc<Mutex<Editor>> {
        Arc::clone(&self.editor)
    }

    /// Feeds one pointer or key event to the engine.
    pub fn input(&mut self, input: EditorInput) -> Step {
        let out = lock(&self.editor).handle(input);
        self.after_output(out)
    }

    /// The same rule for an output produced elsewhere, such as a command.
    pub fn after_output(&mut self, out: EditorOutput) -> Step {
        let render_now = !out.dirty.is_empty() && self.gate.request();
        Step { chrome: out.chrome, request: out.request, render_now }
    }

    /// First paint. True: start a render now.
    pub fn initial(&mut self) -> bool {
        self.gate.request()
    }

    /// Renders a snapshot of the engine on the compute pool. Resolves to `None` if the render
    /// fails.
    pub fn render(&self) -> BoxFuture<'static, Option<Bitmap>> {
        let job = lock(&self.editor).render_job();
        compute(&self.compute, move || job.run())
    }

    /// Call when a render future resolves. True: start one more render now.
    pub fn render_finished(&mut self) -> bool {
        self.gate.finished()
    }
}

impl std::fmt::Debug for EditorInteraction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EditorInteraction").field("gate", &self.gate).finish_non_exhaustive()
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::sync::{Arc, Mutex};

    use futures_channel::oneshot;
    use futures_lite::future::block_on;
    use laqta_domain::{Bitmap, PixelFormat};
    use laqta_editor::{Command, Editor, EditorInput, EditorKey, EditorRequest, Fonts, Mods, PointerButton, Tool};
    use laqta_ports::BoxFuture;

    use super::{EditorInteraction, lock};
    use crate::presenters::render_gate::ComputeFn;

    /// Jobs the gated fake holds until a test runs or drops them.
    type Held = Arc<Mutex<Vec<Box<dyn FnOnce() + Send>>>>;

    /// A `ComputeFn` that holds each job. The returned future resolves only after the job runs.
    /// A dropped job resolves the future too, so a test can simulate a failed render.
    fn gated() -> (ComputeFn, Held) {
        let held: Held = Arc::default();
        let queue = Arc::clone(&held);
        let compute: ComputeFn = Arc::new(move |work: Box<dyn FnOnce() + Send>| -> BoxFuture<'static, ()> {
            let (done, ran) = oneshot::channel::<()>();
            let job: Box<dyn FnOnce() + Send> = Box::new(move || {
                work();
                let _ = done.send(());
            });
            lock(&queue).push(job);
            Box::pin(async move {
                let _ = ran.await;
            })
        });
        (compute, held)
    }

    fn held_len(held: &Held) -> usize {
        lock(held).len()
    }

    /// Runs the oldest held job inside a rayon pool, as the compute pool would.
    fn run_next(held: &Held) {
        let job = lock(held).remove(0);
        let pool = rayon::ThreadPoolBuilder::new().num_threads(2).build().unwrap_or_else(|e| panic!("rayon pool: {e}"));
        pool.install(job);
    }

    fn base() -> Bitmap {
        let (w, h) = (2560u32, 1440u32);
        let px: Vec<u8> = std::iter::repeat_n([128u8, 128, 128, 255], (w * h) as usize).flatten().collect();
        Bitmap::new(w, h, PixelFormat::Rgba8, px.into()).unwrap()
    }

    fn editor() -> Editor {
        Editor::new(base(), Fonts::empty())
    }

    /// One rectangle, then the Select tool. A press on it is silent and each move dirties it.
    /// (A press with the Rectangle tool starts a draft, which is dirty, so it cannot be silent.)
    fn editor_with_box() -> Editor {
        let mut e = editor();
        e.handle(down(500.0, 500.0));
        e.handle(drag(900.0, 800.0));
        e.handle(up(900.0, 800.0));
        e.execute(Command::SetTool(Tool::Select));
        e
    }

    fn down(x: f32, y: f32) -> EditorInput {
        EditorInput::PointerDown { x, y, button: PointerButton::Left, mods: Mods::default() }
    }

    fn drag(x: f32, y: f32) -> EditorInput {
        EditorInput::PointerMove { x, y, mods: Mods::default() }
    }

    fn up(x: f32, y: f32) -> EditorInput {
        EditorInput::PointerUp { x, y, button: PointerButton::Left, mods: Mods::default() }
    }

    #[test]
    fn hover_moves_do_not_render() {
        let mut e = editor();
        for i in 0..25 {
            let from = (100.0 + 20.0 * i as f32, 100.0);
            let to = (from.0 + 10.0, 110.0);
            e.handle(down(from.0, from.1));
            e.handle(drag(to.0, to.1));
            e.handle(up(to.0, to.1));
        }
        let (compute, held) = gated();
        let mut ix = EditorInteraction::new(e, compute);
        let select = lock(&ix.editor()).execute(Command::SetTool(Tool::Select));
        ix.after_output(select);
        for k in 0..300 {
            let x = 2000.0 + (k % 200) as f32;
            let step = ix.input(drag(x, 1300.0));
            assert!(!step.render_now, "hover move {k} asked for a render");
        }
        assert_eq!(held_len(&held), 0);
    }

    #[test]
    fn moves_during_a_render_queue_one_more() {
        let (compute, held) = gated();
        let mut ix = EditorInteraction::new(editor_with_box(), compute);
        assert!(!ix.input(down(600.0, 600.0)).render_now);
        assert!(ix.input(drag(620.0, 610.0)).render_now);
        let first = ix.render();
        assert_eq!(held_len(&held), 1);
        for k in 1..300 {
            let step = ix.input(drag(620.0 + k as f32, 610.0));
            assert!(!step.render_now, "move {k} asked for a second render while one is held");
        }
        let engine = ix.editor();
        assert!(engine.try_lock().is_ok(), "the engine lock was held during a render");
        run_next(&held);
        assert_eq!(held_len(&held), 0);
        assert!(block_on(first).is_some());
        assert!(ix.render_finished(), "moves arrived during the render, so one more is due");
        let second = ix.render();
        assert_eq!(held_len(&held), 1);
        run_next(&held);
        assert!(block_on(second).is_some());
        assert!(!ix.render_finished());
    }

    #[test]
    fn a_dirtying_move_after_idle_renders_again() {
        let (compute, held) = gated();
        let mut ix = EditorInteraction::new(editor_with_box(), compute);
        ix.input(down(600.0, 600.0));
        assert!(ix.input(drag(620.0, 610.0)).render_now);
        let fut = ix.render();
        run_next(&held);
        assert!(block_on(fut).is_some());
        assert!(!ix.render_finished());
        assert!(ix.input(drag(650.0, 640.0)).render_now);
    }

    #[test]
    fn a_failed_render_resolves_to_none_and_frees_the_gate() {
        let (compute, held) = gated();
        let mut ix = EditorInteraction::new(editor_with_box(), compute);
        ix.input(down(600.0, 600.0));
        assert!(ix.input(drag(620.0, 610.0)).render_now);
        let fut = ix.render();
        drop(lock(&held).remove(0));
        assert!(block_on(fut).is_none());
        assert!(!ix.render_finished());
        assert!(ix.input(drag(650.0, 640.0)).render_now);
    }

    #[test]
    fn enter_with_nothing_pending_asks_to_continue() {
        let (compute, _) = gated();
        let mut ix = EditorInteraction::new(editor(), compute);
        let step = ix.input(EditorInput::Key { key: EditorKey::Enter, mods: Mods::default() });
        assert_eq!(step.request, Some(EditorRequest::Continue));
    }

    #[test]
    fn after_output_with_dirty_regions_asks_for_a_render() {
        let (compute, _) = gated();
        let mut e = editor();
        e.handle(down(500.0, 500.0));
        let out = e.handle(drag(900.0, 800.0));
        assert!(!out.dirty.is_empty());
        let mut ix = EditorInteraction::new(e, compute);
        assert!(ix.after_output(out).render_now);
    }
}
