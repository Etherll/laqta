//! Editor input turned into chrome updates and background renders.
//!
//! The engine lock is held only for `handle` and `render_job()`. A render runs on the `RenderJob`
//! snapshot, so the UI thread never waits for a render.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use laqta_domain::Bitmap;
use laqta_editor::{Chrome, Command, Editor, EditorInput, EditorOutput, EditorRequest};
use laqta_ports::BoxFuture;

use super::render_gate::{ComputeFn, RenderGate, compute};

/// What the UI does after one input: repaint the chrome, act on a request, start a render.
#[derive(Debug)]
pub struct Step {
    pub chrome: Chrome,
    pub request: Option<EditorRequest>,
    pub render_now: bool,
}

/// One piece of work for the engine, kept in the order the user produced it.
#[derive(Debug)]
pub enum Work {
    Input(EditorInput),
    Command(Command),
    /// Export the image as it is at this point in the order, and send it here.
    Export(futures_channel::oneshot::Sender<Bitmap>),
}

/// A batch of work running on the compute pool. Resolves to one output per piece of work, or to
/// `None` if the engine panicked.
pub type Batch = BoxFuture<'static, Option<Vec<EditorOutput>>>;

/// What [`EditorInteraction::submit`] did with the work.
pub enum Submitted {
    /// It ran at once (an input with nothing running).
    Step(Box<Step>),
    /// It started a batch. Await it, then call [`EditorInteraction::batch_finished`].
    Started(Batch),
    /// A batch is running; this work runs in the next one.
    Queued,
}

/// What the UI does when a batch ends.
pub struct Finished {
    /// The newest chrome and whether to render. `None` if the batch failed.
    pub step: Option<Step>,
    /// Every request the batch produced, in order.
    pub requests: Vec<EditorRequest>,
    /// The work queued meanwhile, already started. Await it like the first batch.
    pub next: Option<Batch>,
    /// The engine panicked, so the batch's work is lost.
    pub failed: bool,
}

impl std::fmt::Debug for Submitted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Submitted::Step(_) => "Step",
            Submitted::Started(_) => "Started",
            Submitted::Queued => "Queued",
        })
    }
}

impl std::fmt::Debug for Finished {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Finished").field("step", &self.step).field("requests", &self.requests).field("next", &self.next.is_some()).finish()
    }
}

/// An open editor: the engine, the render gate and the compute pool it renders on.
pub struct EditorInteraction {
    editor: Arc<Mutex<Editor>>,
    compute: ComputeFn,
    gate: RenderGate,
    queue: VecDeque<Work>,
    running: bool,
}

impl EditorInteraction {
    pub fn new(editor: Editor, compute: ComputeFn) -> Self {
        Self { editor: Arc::new(Mutex::new(editor)), compute, gate: RenderGate::default(), queue: VecDeque::new(), running: false }
    }

    pub fn editor(&self) -> Arc<Mutex<Editor>> {
        Arc::clone(&self.editor)
    }

    /// Feeds one pointer or key event to the engine.
    pub fn input(&mut self, input: EditorInput) -> Step {
        let out = lock(&self.editor).handle(input);
        self.after_output(out)
    }

    /// Runs work without waiting for it. Commands always run on the compute pool. An input runs
    /// inline (state math only) unless a batch is running, in which case it queues behind it so the
    /// engine sees everything in order and the caller never waits for the engine lock.
    pub fn submit(&mut self, work: Work) -> Submitted {
        match work {
            // `try_lock`: a render snapshot or an export may hold the engine for a moment, and the
            // caller is the UI thread. A busy engine sends the input to the pool instead.
            Work::Input(input) if !self.running => {
                let handled = match self.editor.try_lock() {
                    Ok(mut engine) => Ok(engine.handle(input)),
                    Err(std::sync::TryLockError::Poisoned(poisoned)) => Ok(poisoned.into_inner().handle(input)),
                    Err(std::sync::TryLockError::WouldBlock) => Err(input),
                };
                match handled {
                    Ok(out) => Submitted::Step(Box::new(self.after_output(out))),
                    Err(input) => {
                        self.queue.push_back(Work::Input(input));
                        Submitted::Started(self.start_batch())
                    }
                }
            }
            work => {
                self.queue.push_back(work);
                if self.running { Submitted::Queued } else { Submitted::Started(self.start_batch()) }
            }
        }
    }

    /// Runs a command that only changes engine state (such as `SetTool`) at once when the engine is
    /// free, so it costs no pool job. A busy engine queues it like any other command. Never waits.
    pub fn submit_light(&mut self, command: Command) -> Submitted {
        if !self.running {
            let out = match self.editor.try_lock() {
                Ok(mut engine) => Some(engine.execute(command.clone())),
                Err(std::sync::TryLockError::Poisoned(poisoned)) => Some(poisoned.into_inner().execute(command.clone())),
                Err(std::sync::TryLockError::WouldBlock) => None,
            };
            if let Some(out) = out {
                return Submitted::Step(Box::new(self.after_output(out)));
            }
        }
        self.submit(Work::Command(command))
    }

    /// False while a batch runs. Work submitted then is queued, so an export started now could miss it.
    pub fn idle(&self) -> bool {
        !self.running
    }

    fn start_batch(&mut self) -> Batch {
        self.running = true;
        let batch: Vec<Work> = self.queue.drain(..).collect();
        let editor = Arc::clone(&self.editor);
        compute(&self.compute, move || {
            let mut engine = lock(&editor);
            batch
                .into_iter()
                .filter_map(|work| match work {
                    Work::Input(input) => Some(engine.handle(input)),
                    Work::Command(command) => Some(engine.execute(command)),
                    Work::Export(image) => {
                        let _ = image.send(engine.export());
                        None
                    }
                })
                .collect()
        })
    }

    /// Call when a batch resolves, with what it resolved to.
    pub fn batch_finished(&mut self, outputs: Option<Vec<EditorOutput>>) -> Finished {
        self.running = false;
        let failed = outputs.is_none();
        let mut step: Option<Step> = None;
        let mut requests = Vec::new();
        for out in outputs.into_iter().flatten() {
            let mut one = self.after_output(out);
            requests.extend(one.request.take());
            one.render_now |= step.as_ref().is_some_and(|s| s.render_now);
            step = Some(one);
        }
        let next = (!self.queue.is_empty()).then(|| self.start_batch());
        Finished { step, requests, next, failed }
    }

    /// The finished image, rendered on the compute pool. Resolves to `None` if the export fails.
    /// The export takes its place in the work order: it holds every edit submitted before this call
    /// and none submitted after. Handle the `Submitted` as for [`Self::submit`].
    pub fn export(&mut self) -> (BoxFuture<'static, Option<Bitmap>>, Submitted) {
        let (send, image) = futures_channel::oneshot::channel();
        let submitted = self.submit(Work::Export(send));
        (Box::pin(async move { image.await.ok() }), submitted)
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

    use super::{EditorInteraction, Submitted, Work, lock};
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

    #[test]
    fn an_export_holds_the_edits_before_it_and_none_after() {
        let (compute, held) = gated();
        let mut ix = EditorInteraction::new(editor(), compute);
        let (image, submitted) = ix.export();
        let Submitted::Started(batch) = submitted else { panic!("an idle export starts a batch") };
        // Submitted after the export, while it waits for the pool.
        let small = Bitmap::new(4, 4, PixelFormat::Rgba8, vec![0u8; 64].into()).unwrap();
        assert!(matches!(ix.submit(Work::Command(Command::ReplaceImage(small))), Submitted::Queued));

        run_next(&held);
        assert_eq!(block_on(image).map(|b| b.width()), Some(2560), "the later edit is not in the export");
        let done = ix.batch_finished(block_on(batch));
        assert!(!done.failed && done.step.is_none(), "an export alone is not a failed command");
        let next = done.next.unwrap_or_else(|| panic!("the queued edit starts next"));
        run_next(&held);
        ix.batch_finished(block_on(next));
        assert_eq!(lock(&ix.editor()).export().width(), 4);
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

    /// A command never runs on the caller's thread, and work submitted while it runs keeps its order.
    #[test]
    fn commands_run_on_the_pool_and_later_work_queues_in_order() {
        use super::{Submitted, Work};
        let (compute, held) = gated();
        let mut ix = EditorInteraction::new(editor(), compute);
        let Submitted::Started(first) = ix.submit(Work::Command(laqta_editor::Command::SetTool(Tool::Rectangle))) else {
            panic!("a command starts a batch");
        };
        assert_eq!(held_len(&held), 1, "the command waits for the pool");
        assert!(matches!(ix.submit(Work::Input(down(10.0, 10.0))), Submitted::Queued));
        assert!(matches!(ix.submit(Work::Input(drag(60.0, 60.0))), Submitted::Queued));
        assert_eq!(held_len(&held), 1, "queued work starts no job of its own");
        run_next(&held);
        let done = ix.batch_finished(block_on(first));
        assert!(done.step.is_some());
        let next = done.next.unwrap();
        run_next(&held);
        let outputs = block_on(next).unwrap();
        assert_eq!(outputs.len(), 2, "both inputs ran in one batch");
        let done = ix.batch_finished(Some(outputs));
        assert!(done.next.is_none());
        assert!(done.step.unwrap().render_now, "the drag drew a draft");
        // Idle again: an input runs inline.
        assert!(matches!(ix.submit(Work::Input(up(60.0, 60.0))), Submitted::Step(_)));
    }

    #[test]
    fn a_failed_batch_reports_no_step() {
        let (compute, _held) = gated();
        let mut ix = EditorInteraction::new(editor(), compute);
        let _ = ix.submit(super::Work::Command(laqta_editor::Command::Undo));
        let done = ix.batch_finished(None);
        assert!(done.step.is_none() && done.requests.is_empty() && done.next.is_none());
    }
}
