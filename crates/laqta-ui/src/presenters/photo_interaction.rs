//! Photo editor commands turned into layer-panel updates and background renders.
//!
//! The engine lock is held only while a command runs or a render job is taken. Commands and renders
//! run on the compute pool. The panel paints a `PhotoView` cached at the last change, so a command
//! in progress never blocks the UI thread.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError, TryLockError};

use laqta_domain::Bitmap;
use laqta_photo::{BlendMode, LayerId, PhotoEditor};
use laqta_ports::BoxFuture;

use super::render_gate::{ComputeFn, RenderGate, compute};

/// One top-level layer as the panel shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct LayerRow {
    pub id: LayerId,
    pub name: String,
    pub visible: bool,
    pub opacity: f32,
    pub blend: BlendMode,
    pub locked: bool,
}

/// What the panels paint. It changes when a command finishes, or when a refresh finds the engine free.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PhotoView {
    pub revision: u64,
    /// Bottom first, as the document stores them.
    pub layers: Vec<LayerRow>,
    /// An index into `layers`. `None` when no top-level layer is active.
    pub active: Option<usize>,
}

fn view_of(editor: &PhotoEditor) -> PhotoView {
    let doc = editor.document();
    let layers: Vec<LayerRow> = doc
        .layers
        .iter()
        .map(|l| LayerRow { id: l.id, name: l.name.clone(), visible: l.visible, opacity: l.opacity, blend: l.blend, locked: l.locked })
        .collect();
    let active = doc.active.and_then(|id| layers.iter().position(|row| row.id == id));
    PhotoView { revision: editor.revision(), layers, active }
}

/// An open photo editor: the engine, the cached panel view, the render gate and the compute pool.
pub struct PhotoInteraction {
    editor: Arc<Mutex<PhotoEditor>>,
    compute: ComputeFn,
    gate: RenderGate,
    view: PhotoView,
    busy: bool,
}

impl PhotoInteraction {
    pub fn new(editor: PhotoEditor, compute: ComputeFn) -> Self {
        let view = view_of(&editor);
        Self { editor: Arc::new(Mutex::new(editor)), compute, gate: RenderGate::default(), view, busy: false }
    }

    pub fn editor(&self) -> Arc<Mutex<PhotoEditor>> {
        Arc::clone(&self.editor)
    }

    pub fn view(&self) -> &PhotoView {
        &self.view
    }

    /// False while a command runs.
    pub fn idle(&self) -> bool {
        !self.busy
    }

    /// Runs `f` under the engine lock on the compute pool. Resolves to its result, or to `None` if it
    /// panicked. Call only while [`idle`](Self::idle) is true, and call
    /// [`command_finished`](Self::command_finished) when the future resolves.
    pub fn command<R: Send + 'static>(&mut self, f: impl FnOnce(&mut PhotoEditor) -> R + Send + 'static) -> BoxFuture<'static, Option<R>> {
        self.busy = true;
        let editor = Arc::clone(&self.editor);
        compute(&self.compute, move || {
            let mut engine = lock(&editor);
            f(&mut engine)
        })
    }

    /// Call when a command future resolves. Refreshes the view. True: the revision moved, so start a
    /// render now. False: nothing changed, or a render is running and will be asked again.
    pub fn command_finished(&mut self) -> bool {
        self.busy = false;
        let before = self.view.revision;
        self.view = view_of(&lock(&self.editor));
        self.view.revision != before && self.gate.request()
    }

    /// Refreshes the view from the engine if no command runs and the lock is free. Otherwise keeps the
    /// cached view. Never waits.
    pub fn refresh_view(&mut self) {
        if self.busy {
            return;
        }
        match self.editor.try_lock() {
            Ok(engine) => self.view = view_of(&engine),
            Err(TryLockError::Poisoned(poisoned)) => self.view = view_of(&poisoned.into_inner()),
            Err(TryLockError::WouldBlock) => {}
        }
    }

    /// First paint. True: start a render now.
    pub fn initial(&mut self) -> bool {
        self.gate.request()
    }

    /// Takes a snapshot of the engine and renders it on the compute pool. Resolves to `None` if the
    /// render fails. Call while idle: during a command, the lock is held until the command ends.
    pub fn render(&self) -> BoxFuture<'static, Option<Bitmap>> {
        let job = lock(&self.editor).render_job();
        compute(&self.compute, move || job.run())
    }

    /// Call when a render future resolves. True: start one more render now.
    pub fn render_finished(&mut self) -> bool {
        self.gate.finished()
    }
}

impl std::fmt::Debug for PhotoInteraction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PhotoInteraction")
            .field("view", &self.view)
            .field("gate", &self.gate)
            .field("busy", &self.busy)
            .finish_non_exhaustive()
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::sync::{Arc, Mutex, mpsc};

    use futures_channel::oneshot;
    use futures_lite::future::block_on;
    use laqta_domain::color::Rgba;
    use laqta_photo::{Command, Document, Fill, LayerId, LayerKind, PhotoEditor, PhotoError};
    use laqta_ports::BoxFuture;

    use super::{PhotoInteraction, lock};
    use crate::presenters::render_gate::ComputeFn;

    /// Jobs the gated fake holds until a test runs or drops them.
    type Held = Arc<Mutex<Vec<Box<dyn FnOnce() + Send>>>>;

    /// A `ComputeFn` that holds each job. The returned future resolves only after the job runs.
    /// A dropped job resolves the future too, so a test can simulate a failed command or render.
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

    /// Runs the job at `at` inside a rayon pool, as the compute pool would.
    fn run_at(held: &Held, at: usize) {
        let job = lock(held).remove(at);
        let pool = rayon::ThreadPoolBuilder::new().num_threads(2).build().unwrap_or_else(|e| panic!("rayon pool: {e}"));
        pool.install(job);
    }

    fn run_next(held: &Held) {
        run_at(held, 0);
    }

    fn editor() -> PhotoEditor {
        PhotoEditor::new(Document::new(64, 48).unwrap())
    }

    fn add(name: &str) -> Command {
        Command::AddLayer { name: name.into(), kind: LayerKind::Fill(Fill::Solid(Rgba { r: 0, g: 0, b: 0, a: 255 })) }
    }

    /// Renders on the pool, then checks that no further render is due.
    fn draw(pi: &mut PhotoInteraction, held: &Held) {
        let fut = pi.render();
        run_next(held);
        assert!(block_on(fut).is_some());
        assert!(!pi.render_finished());
    }

    #[test]
    fn a_pending_command_keeps_idle_false_and_the_cached_view() {
        let (compute, held) = gated();
        let mut pi = PhotoInteraction::new(editor(), compute);
        assert!(pi.idle());
        let before = pi.view().clone();
        let fut = pi.command(|e| e.execute(add("a")));
        assert!(!pi.idle());
        assert_eq!(pi.view(), &before);
        run_next(&held);
        assert!(matches!(block_on(fut), Some(Ok(()))));
        assert_eq!(pi.view(), &before, "the view changes only at command_finished");
        assert!(pi.command_finished());
        assert!(pi.idle());
        assert_eq!(pi.view().layers.len(), 1);
    }

    #[test]
    fn refresh_view_keeps_the_cache_while_the_engine_is_locked() {
        let (compute, _) = gated();
        let mut pi = PhotoInteraction::new(editor(), compute);
        let engine = pi.editor();
        let (locked_tx, locked_rx) = mpsc::channel::<()>();
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let during = std::thread::scope(|s| {
            s.spawn(move || {
                let mut guard = lock(&engine);
                guard.execute(add("a")).unwrap();
                locked_tx.send(()).unwrap();
                release_rx.recv().unwrap();
            });
            locked_rx.recv().unwrap();
            pi.refresh_view();
            let during = pi.view().layers.len();
            release_tx.send(()).unwrap();
            during
        });
        assert_eq!(during, 0, "the view changed while the engine lock was held");
        pi.refresh_view();
        assert_eq!(pi.view().layers.len(), 1);
    }

    #[test]
    fn refresh_view_waits_for_command_finished_to_see_a_change() {
        let (compute, held) = gated();
        let mut pi = PhotoInteraction::new(editor(), compute);
        let fut = pi.command(|e| e.execute(add("a")));
        run_next(&held);
        assert!(block_on(fut).is_some());
        pi.refresh_view();
        assert_eq!(pi.view().revision, 0, "refresh_view advanced the cache during a command");
        assert!(pi.command_finished());
        assert_eq!(pi.view().layers.len(), 1);
    }

    #[test]
    fn command_finished_is_true_after_a_change_and_false_after_a_failed_one() {
        let (compute, held) = gated();
        let mut pi = PhotoInteraction::new(editor(), compute);
        let fut = pi.command(|e| e.execute(add("a")));
        run_next(&held);
        assert!(block_on(fut).is_some());
        assert!(pi.command_finished());
        draw(&mut pi, &held);

        let fut = pi.command(|e| e.execute(Command::RemoveLayer { layer: LayerId(999) }));
        run_next(&held);
        assert!(matches!(block_on(fut), Some(Err(PhotoError::NoSuchLayer(LayerId(999))))));
        assert!(!pi.command_finished());

        let fut = pi.command(|e| e.execute(Command::Deselect));
        run_next(&held);
        assert!(matches!(block_on(fut), Some(Ok(()))));
        assert!(!pi.command_finished(), "a no-op command must not count as a change");
    }

    #[test]
    fn a_dropped_command_resolves_to_none() {
        let (compute, held) = gated();
        let mut pi = PhotoInteraction::new(editor(), compute);
        let fut = pi.command(|e| e.execute(add("a")));
        drop(lock(&held).remove(0));
        assert!(block_on(fut).is_none());
        assert!(!pi.command_finished());
        assert!(pi.idle());
        assert_eq!(pi.view().layers.len(), 0);
    }

    #[test]
    fn renders_coalesce_while_one_is_held() {
        let (compute, held) = gated();
        let mut pi = PhotoInteraction::new(editor(), compute);
        assert!(pi.initial());
        let first = pi.render();
        assert_eq!(held_len(&held), 1);
        let fut = pi.command(|e| e.execute(add("a")));
        run_at(&held, 1);
        assert!(block_on(fut).is_some());
        assert!(!pi.command_finished(), "a render is held, so the change waits for it");
        run_next(&held);
        assert!(block_on(first).is_some());
        assert!(pi.render_finished(), "the change arrived during the render, so one more is due");
        draw(&mut pi, &held);
    }

    #[test]
    fn undo_and_redo_go_through_command() {
        let (compute, held) = gated();
        let mut pi = PhotoInteraction::new(editor(), compute);
        let fut = pi.command(|e| e.execute(add("a")));
        run_next(&held);
        assert!(matches!(block_on(fut), Some(Ok(()))));
        assert!(pi.command_finished());
        draw(&mut pi, &held);

        let fut = pi.command(|e| e.undo());
        run_next(&held);
        assert_eq!(block_on(fut), Some(true));
        assert!(pi.command_finished());
        assert_eq!(pi.view().layers.len(), 0);
        draw(&mut pi, &held);

        let fut = pi.command(|e| e.redo());
        run_next(&held);
        assert_eq!(block_on(fut), Some(true));
        assert!(pi.command_finished());
        assert_eq!(pi.view().layers.len(), 1);
        draw(&mut pi, &held);
    }
}
