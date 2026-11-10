//! One native recording. The video thread reads WGC frames, paces them on the recording timeline and writes
//! them to the sink writer. The audio thread writes system sound while the recording is not paused. The
//! `Recording` handle pauses, resumes, finishes and aborts the pair, and reports whether they still run.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use laqta_ports::PortError;
use laqta_ports::recording::PcmSource;
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};

use super::capture::{Capture, Monitor};
use super::encoder::{AUDIO_CHANNELS, AUDIO_RATE, Sink};
use super::pure::{Pacer, Tick, Timeline, time_of};

/// How long `start` waits for the recording thread to open the capture and the sink.
const START_WAIT: Duration = Duration::from_secs(10);
/// Frames per audio read: about 85 ms at 48 kHz.
const AUDIO_CHUNK_FRAMES: usize = 4096;
/// The video loop checks for a new frame at least this often, and at most this rarely.
const MIN_POLL: Duration = Duration::from_millis(1);
const MAX_POLL: Duration = Duration::from_millis(4);

/// What a recording needs to start.
pub(crate) struct Job {
    pub(crate) output: PathBuf,
    pub(crate) monitor: Monitor,
    pub(crate) fps: u32,
    pub(crate) bitrate: u32,
    pub(crate) show_cursor: bool,
    pub(crate) audio: Option<Box<dyn PcmSource>>,
}

/// The state the recording threads and the handle share.
struct Session {
    base: Instant,
    timeline: Mutex<Timeline>,
    sink: Mutex<Option<Sink>>,
    halt: AtomicBool,
    failure: Mutex<Option<String>>,
}

impl Session {
    fn new() -> Self {
        Session {
            base: Instant::now(),
            timeline: Mutex::new(Timeline::new(0)),
            sink: Mutex::new(None),
            halt: AtomicBool::new(false),
            failure: Mutex::new(None),
        }
    }

    /// Wall time since the session began, in 100 ns units.
    fn now(&self) -> u64 {
        u64::try_from(self.base.elapsed().as_nanos() / 100).unwrap_or(u64::MAX)
    }

    fn recording_time(&self) -> u64 {
        lock(&self.timeline).recording_time(self.now())
    }

    /// Starts the recording clock now: recording time 0 is this moment.
    fn restart_clock(&self) {
        *lock(&self.timeline) = Timeline::new(self.now());
    }

    fn pause(&self) {
        let now = self.now();
        lock(&self.timeline).pause(now);
    }

    fn resume(&self) {
        let now = self.now();
        lock(&self.timeline).resume(now);
    }

    fn paused(&self) -> bool {
        lock(&self.timeline).is_paused()
    }

    fn halt(&self) {
        self.halt.store(true, Ordering::SeqCst);
    }

    fn halted(&self) -> bool {
        self.halt.load(Ordering::SeqCst)
    }

    /// Records the first failure. Later ones are dropped, since the first is the cause.
    fn fail(&self, reason: String) {
        let mut failure = lock(&self.failure);
        if failure.is_none() {
            *failure = Some(reason);
        }
    }

    fn failure(&self) -> Option<String> {
        lock(&self.failure).clone()
    }

    fn install(&self, sink: Sink) {
        *lock(&self.sink) = Some(sink);
    }

    fn take_sink(&self) -> Option<Sink> {
        lock(&self.sink).take()
    }

    fn write_video(&self, frame: &[u8], tick: Tick) -> Result<(), PortError> {
        let sink = lock(&self.sink);
        sink.as_ref().ok_or_else(sink_closed)?.write_video(frame, tick.time, tick.duration)
    }

    fn write_audio(&self, samples: &[u8], time: u64, duration: u64) -> Result<(), PortError> {
        let sink = lock(&self.sink);
        sink.as_ref().ok_or_else(sink_closed)?.write_audio(samples, time, duration)
    }
}

fn sink_closed() -> PortError {
    PortError::Failed("the recording is already finished".into())
}

/// Locks a mutex, ignoring poisoning. The state stays usable after a panic elsewhere.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Starts a recording and waits until its threads are running. A failure to set up is returned as the
/// error (`Unsupported` where that is the cause), and no file is left behind.
pub(crate) fn start(job: Job) -> Result<Recording, PortError> {
    let session = Arc::new(Session::new());
    let (ready_tx, ready_rx) = mpsc::channel();
    let worker = Arc::clone(&session);
    let output = job.output.clone();
    let thread = std::thread::Builder::new()
        .name("laqta-wgc".into())
        .spawn(move || record(&worker, job, &ready_tx))
        .map_err(|error| PortError::Failed(format!("recording thread: {error}")))?;
    match ready_rx.recv_timeout(START_WAIT) {
        Ok(Ok(())) => Ok(Recording { session, video: Some(thread), output }),
        Ok(Err(error)) => {
            let _ = thread.join();
            let _ = std::fs::remove_file(&output);
            Err(error)
        }
        Err(_) => {
            session.halt();
            Err(PortError::Failed("the native recorder did not start in time".into()))
        }
    }
}

/// The video thread: sets up the capture, the sink and the audio thread, reports to `start`, then writes
/// frames until the recording halts. The sink is finished here, on the thread that wrote it. Errors during
/// the recording are kept for `is_alive`, and the partial file stays playable.
fn record(session: &Arc<Session>, job: Job, ready: &mpsc::Sender<Result<(), PortError>>) -> Result<(), PortError> {
    let _apartment = Apartment::enter();
    let Job { output, monitor, fps, bitrate, show_cursor, audio } = job;
    let (width, height) = (monitor.area.width, monitor.area.height);
    let has_audio = audio.is_some();
    // The capture comes first, so a refusal (no WGC, area off its monitor) never creates the file.
    let mut capture = match Capture::start(monitor, show_cursor) {
        Ok(capture) => capture,
        Err(error) => {
            stop_source(audio);
            let _ = ready.send(Err(error));
            return Ok(());
        }
    };
    // `start` gave up waiting and the caller has moved on to another recorder for this path.
    if session.halted() {
        capture.close();
        stop_source(audio);
        return Ok(());
    }
    if let Err(error) = Sink::create(&output, (width, height), fps, bitrate, has_audio).map(|sink| session.install(sink)) {
        capture.close();
        stop_source(audio);
        let _ = ready.send(Err(error));
        return Ok(());
    }
    // The clock starts once both outputs are ready, before the audio thread reads anything, so the first
    // frame and the first sample both sit at recording time 0.
    session.restart_clock();
    let audio_thread = match audio {
        None => None,
        Some(source) => {
            let worker = Arc::clone(session);
            match std::thread::Builder::new().name("laqta-wgc-audio".into()).spawn(move || audio_loop(&worker, source)) {
                Ok(thread) => Some(thread),
                Err(error) => {
                    capture.close();
                    drop(session.take_sink());
                    let _ = ready.send(Err(PortError::Failed(format!("system audio thread: {error}"))));
                    return Ok(());
                }
            }
        }
    };
    let _ = ready.send(Ok(()));

    if let Err(error) = run(session, &mut capture, fps) {
        session.fail(error.to_string());
    }
    session.halt();
    if let Some(thread) = audio_thread {
        match thread.join() {
            Ok(Ok(())) => {}
            Ok(Err(error)) => session.fail(error.to_string()),
            Err(_) => session.fail("the system audio thread panicked".into()),
        }
    }
    capture.close();
    match session.take_sink() {
        Some(sink) => sink.finish(),
        None => Ok(()),
    }
}

/// Writes frames at the recording's frame rate until the recording halts. A frame is due once the recording
/// time reaches its time, so a pause holds the frames back, and a late loop writes the missed frames at once.
/// A new capture is read back only when a frame is due, so a fast source costs no more than the output rate.
fn run(session: &Session, capture: &mut Capture, fps: u32) -> Result<(), PortError> {
    let mut pacer = Pacer::new(fps);
    let mut repeated = 0u64;
    while !session.halted() {
        if capture.poll()? {
            pacer.captured();
        }
        let now = session.recording_time();
        if pacer.until_next(now) == 0 && capture.refresh()? {
            while let Some(tick) = pacer.next_tick(now) {
                session.write_video(capture.latest(), tick)?;
                repeated += u64::from(tick.repeat);
            }
        }
        std::thread::sleep(poll_wait(pacer.until_next(now)));
    }
    tracing::debug!(repeated, "native recording stopped");
    Ok(())
}

/// Stops a source that will not be recorded, so its capture thread ends.
fn stop_source(source: Option<Box<dyn PcmSource>>) {
    if let Some(mut source) = source {
        source.stop();
    }
}

/// How long the video loop sleeps, given the recording time left until the next frame.
fn poll_wait(ticks: u64) -> Duration {
    Duration::from_nanos(ticks.saturating_mul(100)).clamp(MIN_POLL, MAX_POLL)
}

/// The audio thread: reads system sound and writes it while the recording runs. Samples read during a
/// pause are dropped, so the audio timeline stays in step with the video.
fn audio_loop(session: &Session, mut source: Box<dyn PcmSource>) -> Result<(), PortError> {
    let _apartment = Apartment::enter();
    let channels = usize::from(AUDIO_CHANNELS);
    let mut samples = vec![0i16; AUDIO_CHUNK_FRAMES * channels];
    let mut frames = 0u64;
    let outcome = loop {
        if session.halted() {
            break Ok(());
        }
        let read = match source.read(&mut samples) {
            Ok(read) => read,
            Err(error) => break Err(error),
        };
        if read == 0 {
            break if session.halted() { Ok(()) } else { Err(PortError::Failed("system audio stopped".into())) };
        }
        if session.paused() {
            continue;
        }
        let whole = read - read % channels;
        let count = u64::try_from(whole / channels).unwrap_or(0);
        let bytes: Vec<u8> = samples[..whole].iter().flat_map(|sample| sample.to_le_bytes()).collect();
        let time = time_of(frames, AUDIO_RATE);
        let duration = time_of(frames + count, AUDIO_RATE) - time;
        if let Err(error) = session.write_audio(&bytes, time, duration) {
            break Err(error);
        }
        frames += count;
    };
    source.stop();
    outcome
}

/// A recording in progress, as the app holds it.
pub(crate) struct Recording {
    session: Arc<Session>,
    video: Option<JoinHandle<Result<(), PortError>>>,
    output: PathBuf,
}

impl Recording {
    pub(crate) fn pause_now(&self) {
        self.session.pause();
    }

    pub(crate) fn resume_now(&self) {
        self.session.resume();
    }

    /// False once a thread has failed or has ended.
    pub(crate) fn alive(&self) -> bool {
        self.session.failure().is_none() && self.video.as_ref().is_some_and(|thread| !thread.is_finished())
    }

    /// Stops the recording, finishes the file and returns its path. Calling it again returns the path.
    pub(crate) fn finish(&mut self) -> Result<PathBuf, PortError> {
        self.session.halt();
        match self.video.take().map(JoinHandle::join) {
            None | Some(Ok(Ok(()))) => Ok(self.output.clone()),
            Some(Ok(Err(error))) => Err(error),
            Some(Err(_)) => Err(PortError::Failed("the recording thread panicked".into())),
        }
    }

    /// Finishes the recording, then deletes its file.
    pub(crate) fn discard(&mut self) -> Result<(), PortError> {
        let finished = self.finish();
        match std::fs::remove_file(&self.output) {
            Ok(()) => finished.map(|_| ()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => finished.map(|_| ()),
            Err(error) => Err(PortError::Failed(format!("delete {}: {error}", self.output.display()))),
        }
    }
}

/// Initializes COM for the calling thread as multithreaded. It is uninitialized on drop when this call
/// succeeded. A thread that was set up in another mode keeps its setting.
struct Apartment(bool);

impl Apartment {
    fn enter() -> Self {
        // SAFETY: plain COM initialization for the calling thread.
        let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        Apartment(hr.is_ok())
    }
}

impl Drop for Apartment {
    fn drop(&mut self) {
        if self.0 {
            // SAFETY: this pairs with the successful CoInitializeEx on this thread.
            unsafe { CoUninitialize() };
        }
    }
}
