//! The recording process (M4 D3, D4, D7). A segment is one FFmpeg process. Pause ends the segment,
//! resume starts another, and stop joins the segments with the concat demuxer (animations are then
//! converted from a lossless intermediate). FFmpeg reads stdin only when an input uses it, so a
//! segment ends by EOF on stdin in that case, and by `q` otherwise.
//!
//! The handle locks `Inner` from a worker thread for each operation. `is_alive` never waits.

use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ExitStatus, Output, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex, PoisonError, TryLockError};
use std::thread;
use std::time::{Duration, Instant};

use laqta_domain::record::RecordSpec;
use laqta_domain::{Bitmap, PixelFormat};
use laqta_ports::recording::{FrameSource, OsScreen, PcmSource, RecordingHandle};
use laqta_ports::{BoxFuture, PortError};

use crate::argv::{self, AudioInputDesc, Host, VideoInputDesc, WindowsCapture};
use crate::locate::FfmpegPaths;
use crate::{ffmpeg_command, run_blocking, tail};

/// How long a segment gets to finish after its input ends. After this it is killed.
const FINISH_GRACE: Duration = Duration::from_secs(5);
/// How long a Windows `ddagrab` start is watched for an early failure (D1).
const CAPTURE_CHECK: Duration = Duration::from_secs(2);
const PCM_CHUNK_SAMPLES: usize = 4096;

/// The source that feeds FFmpeg's stdin: raw frames for a `RawFrames` video, or PCM for a `Pcm`
/// audio input on stdin. Only one of the two runs at a time.
pub(crate) enum Feed {
    Frames(Box<dyn FrameSource>),
    Pcm(Box<dyn PcmSource>),
}

impl Feed {
    fn stop(&mut self) {
        match self {
            Feed::Frames(source) => source.stop(),
            Feed::Pcm(source) => source.stop(),
        }
    }
}

/// What one recording runs: its inputs in FFmpeg's terms, and the record spec.
pub(crate) struct Plan {
    pub(crate) video: VideoInputDesc,
    pub(crate) audio: Vec<AudioInputDesc>,
    pub(crate) spec: RecordSpec,
    pub(crate) host: Host,
}

/// A recording in progress. Its `RecordingHandle` methods run the work on a worker thread.
pub struct FfmpegRecorder {
    inner: Arc<Mutex<Inner>>,
    /// Set by `stop` and `abort`, which take the handle. Then `Drop` leaves the files to them.
    closing: bool,
}

struct Inner {
    paths: FfmpegPaths,
    plan: Plan,
    capture: WindowsCapture,
    /// The folder with this recording's segments, logs and list.
    session: PathBuf,
    output: PathBuf,
    /// The stdin source while no segment runs (between a pause and a resume).
    feed: Option<Feed>,
    current: Option<Segment>,
    /// Finished segments, in order, as file names inside `session`.
    segments: Vec<String>,
    /// The log of the last segment that wrote nothing, for error messages.
    last_log: Option<PathBuf>,
    index: u32,
    /// True once a segment ended on its own (a crash or an error).
    ended: bool,
}

struct Segment {
    name: String,
    log: PathBuf,
    child: Child,
    /// Owns stdin while a feed runs.
    feeder: Option<Feeder>,
    /// Without a feed, stdin stays here so that `q` can be sent.
    stdin: Option<ChildStdin>,
}

struct Feeder {
    stop: Arc<AtomicBool>,
    /// Receives the source back once the feed has ended.
    returned: Receiver<Feed>,
}

impl FfmpegRecorder {
    /// Starts the first segment. `feed` is the stdin source, when `plan` uses stdin.
    pub(crate) fn start(paths: FfmpegPaths, plan: Plan, session: PathBuf, output: PathBuf, feed: Option<Feed>) -> Result<Self, PortError> {
        fs::create_dir_all(&session).map_err(|err| PortError::Failed(format!("create {}: {err}", session.display())))?;
        // The concat step runs inside the session folder, so the output needs an absolute path.
        let output = match std::path::absolute(&output) {
            Ok(path) => path,
            Err(_) => output,
        };
        let mut inner = Inner {
            paths,
            plan,
            capture: WindowsCapture::Ddagrab,
            session,
            output,
            feed,
            current: None,
            segments: Vec::new(),
            last_log: None,
            index: 0,
            ended: false,
        };
        if let Err(err) = inner.start_first() {
            inner.abort();
            return Err(err);
        }
        Ok(FfmpegRecorder { inner: Arc::new(Mutex::new(inner)), closing: false })
    }

    /// The FFmpeg process of the current segment, when one runs. Crash tests kill it through this.
    #[doc(hidden)]
    pub fn pid(&self) -> Option<u32> {
        match self.inner.try_lock() {
            Ok(inner) => inner.current.as_ref().map(|segment| segment.child.id()),
            Err(_) => None,
        }
    }
}

impl std::fmt::Debug for FfmpegRecorder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FfmpegRecorder").field("pid", &self.pid()).finish_non_exhaustive()
    }
}

impl Drop for FfmpegRecorder {
    fn drop(&mut self) {
        // Dropped without stop or abort: no FFmpeg process should outlive the handle.
        if !self.closing {
            locked(&self.inner, |state| {
                if let Some(mut segment) = state.current.take() {
                    kill_and_reap(&mut segment.child);
                }
            });
        }
    }
}

impl RecordingHandle for FfmpegRecorder {
    fn pause(&mut self) -> BoxFuture<'_, Result<(), PortError>> {
        let inner = Arc::clone(&self.inner);
        run_blocking(move || locked(&inner, Inner::pause))
    }

    fn resume(&mut self) -> BoxFuture<'_, Result<(), PortError>> {
        let inner = Arc::clone(&self.inner);
        run_blocking(move || locked(&inner, Inner::resume))
    }

    fn stop(mut self: Box<Self>) -> BoxFuture<'static, Result<PathBuf, PortError>> {
        self.closing = true;
        let inner = Arc::clone(&self.inner);
        run_blocking(move || locked(&inner, Inner::stop))
    }

    fn abort(mut self: Box<Self>) -> BoxFuture<'static, Result<(), PortError>> {
        self.closing = true;
        let inner = Arc::clone(&self.inner);
        run_blocking(move || {
            locked(&inner, Inner::abort);
            Ok(())
        })
    }

    fn is_alive(&mut self) -> bool {
        match self.inner.try_lock() {
            Ok(mut inner) => inner.check_alive(),
            // An operation is in flight, and it only ends a recording it has been asked to end.
            Err(TryLockError::WouldBlock) => true,
            Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner().check_alive(),
        }
    }
}

/// Runs `work` with the state locked. A poisoned lock is still used: the state is only a handle.
fn locked<T>(inner: &Mutex<Inner>, work: impl FnOnce(&mut Inner) -> T) -> T {
    let mut guard = inner.lock().unwrap_or_else(PoisonError::into_inner);
    work(&mut guard)
}

impl Inner {
    fn start_first(&mut self) -> Result<(), PortError> {
        self.spawn_segment()?;
        let windows_target = self.plan.host == Host::Windows && matches!(self.plan.video, VideoInputDesc::Target(OsScreen::Windows { .. }));
        if !windows_target {
            return Ok(());
        }
        // D1: ddagrab can fail on some adapters. gdigrab takes over when ddagrab exits early.
        let failed = self
            .current
            .as_mut()
            .and_then(|segment| wait_for_exit(&mut segment.child, CAPTURE_CHECK))
            .is_some_and(|status| !status.success());
        if !failed {
            return Ok(());
        }
        tracing::warn!("ddagrab failed to start; falling back to gdigrab");
        self.capture = WindowsCapture::Gdigrab;
        self.finish_segment();
        self.spawn_segment()?;
        let early = self.current.as_mut().and_then(|segment| wait_for_exit(&mut segment.child, CAPTURE_CHECK));
        match early {
            Some(status) if !status.success() => Err(PortError::Failed(format!("could not start the screen capture: {}", self.log_tail()))),
            _ => Ok(()),
        }
    }

    fn spawn_segment(&mut self) -> Result<(), PortError> {
        if argv::stdin_fed(&self.plan.video, &self.plan.audio) && self.feed.is_none() {
            return Err(PortError::Failed("the frame source did not come back after the pause".into()));
        }
        self.index += 1;
        let name = format!("seg-{:04}.mkv", self.index);
        let log = self.session.join(format!("seg-{:04}.log", self.index));
        let output = self.session.join(&name);
        let args = argv::record_args_on(self.plan.host, self.capture, &self.plan.spec, &self.plan.video, &self.plan.audio, &output);
        let mut child = spawn_ffmpeg(&self.paths.ffmpeg, &args, &log)?;
        let stdin = child.stdin.take();
        let (feeder, stdin) = match self.feed.take() {
            Some(feed) => {
                let Some(stdin) = stdin else {
                    return Err(PortError::Failed("FFmpeg stdin is not piped".into()));
                };
                (Some(spawn_feeder(feed, stdin)?), None)
            }
            None => (None, stdin),
        };
        self.current = Some(Segment { name, log, child, feeder, stdin });
        Ok(())
    }

    fn pause(&mut self) -> Result<(), PortError> {
        // Already paused: nothing runs, and this is a no-op.
        self.finish_segment();
        Ok(())
    }

    fn resume(&mut self) -> Result<(), PortError> {
        if self.current.is_some() {
            return Ok(());
        }
        if !self.check_alive() {
            return Err(PortError::Failed("the recording stopped unexpectedly".into()));
        }
        self.spawn_segment()
    }

    fn stop(&mut self) -> Result<PathBuf, PortError> {
        self.finish_segment();
        if let Some(mut feed) = self.feed.take() {
            feed.stop();
        }
        self.join_segments()
    }

    fn abort(&mut self) {
        if let Some(mut segment) = self.current.take() {
            kill_and_reap(&mut segment.child);
            // Dropping the receiver makes the feeder stop the source once it sees the broken pipe.
            drop(segment.feeder.take());
        }
        if let Some(mut feed) = self.feed.take() {
            feed.stop();
        }
        let _ = fs::remove_dir_all(&self.session);
    }

    /// False once a segment has ended on its own. Also true while paused.
    fn check_alive(&mut self) -> bool {
        if let Some(segment) = self.current.as_mut()
            && segment.child.try_wait().ok().flatten().is_some()
        {
            self.ended = true;
        }
        !self.ended
    }

    /// Ends the current segment and keeps its file when FFmpeg wrote something. Afterwards the
    /// feed (if any) is back in `self.feed`.
    fn finish_segment(&mut self) {
        let Some(mut segment) = self.current.take() else {
            return;
        };
        if segment.child.try_wait().ok().flatten().is_some() {
            self.ended = true;
        }
        match (&segment.feeder, segment.stdin.take()) {
            (Some(feeder), _) => feeder.stop.store(true, Ordering::SeqCst),
            (None, Some(mut stdin)) => {
                let _ = stdin.write_all(b"q\n");
            }
            (None, None) => {}
        }
        if wait_for_exit(&mut segment.child, FINISH_GRACE).is_none() {
            tracing::warn!(segment = %segment.name, "FFmpeg did not finish the segment in time; killing it");
            kill_and_reap(&mut segment.child);
        }
        if let Some(feeder) = segment.feeder.take() {
            match feeder.returned.recv_timeout(FINISH_GRACE) {
                Ok(feed) => self.feed = Some(feed),
                Err(_) => tracing::warn!("the frame source did not return; the recording cannot resume"),
            }
        }
        let written = fs::metadata(self.session.join(&segment.name)).map(|meta| meta.len()).unwrap_or(0);
        if written > 0 {
            self.segments.push(segment.name);
        } else {
            self.last_log = Some(segment.log);
        }
    }

    /// Joins the finished segments into the output (concat, `-c copy`). Animations are converted
    /// from the joined lossless intermediate. On success the session folder is deleted. On failure
    /// it stays, and the error names the partial file.
    fn join_segments(&mut self) -> Result<PathBuf, PortError> {
        // A segment paused within its first cluster is only a header. Such a file is not joined.
        let usable: Vec<String> = self.segments.iter().filter(|name| self.has_media(name)).cloned().collect();
        let Some(last) = usable.last().cloned() else {
            let partial = self.segments.last().map(|name| self.session.join(name));
            let partial = partial.map(|path| format!(" (partial file {})", path.display())).unwrap_or_default();
            return Err(PortError::Failed(format!("nothing usable was recorded{partial}: {}", self.log_tail())));
        };
        let list = self.session.join("list.txt");
        fs::write(&list, argv::concat_list(&usable)).map_err(|err| PortError::Failed(format!("write {}: {err}", list.display())))?;
        let animation = self.plan.spec.codec.is_animation();
        let joined = if animation { self.session.join("merged.mkv") } else { self.output.clone() };
        let concat = argv::concat_args("list.txt", &path_text(&joined), animation.then_some("matroska"));
        self.run_step(&concat, "join the segments", self.session.join(&last))?;
        if animation {
            let convert = argv::animation_args(self.plan.spec.codec, &joined, &self.output, self.plan.spec.fps);
            self.run_step(&convert, "convert the animation", joined)?;
        }
        let _ = fs::remove_dir_all(&self.session);
        Ok(self.output.clone())
    }

    /// True when the segment holds at least one video packet.
    fn has_media(&self, name: &str) -> bool {
        let path = self.session.join(name);
        match run_tool(&self.paths.ffprobe, &argv::packet_count_args(&path), None) {
            Ok(output) if output.status.success() => {
                String::from_utf8_lossy(&output.stdout).trim().parse::<u64>().is_ok_and(|packets| packets > 0)
            }
            _ => false,
        }
    }

    fn run_step(&self, args: &[String], what: &str, partial: PathBuf) -> Result<(), PortError> {
        let output = run_tool(&self.paths.ffmpeg, args, Some(&self.session))?;
        if output.status.success() {
            return Ok(());
        }
        Err(PortError::Failed(format!("could not {what}; the partial recording is {}: {}", partial.display(), tail(&output.stderr))))
    }

    /// The end of the log that matters now: the running segment's, else the last empty one's.
    fn log_tail(&self) -> String {
        let log = self.current.as_ref().map(|segment| segment.log.clone()).or_else(|| self.last_log.clone());
        match log {
            Some(path) => fs::read(&path).map(|bytes| tail(&bytes)).unwrap_or_default(),
            None => String::new(),
        }
    }
}

/// Runs an FFmpeg tool with stdin closed, in `cwd` when given. Stdout and stderr are captured.
pub(crate) fn run_tool(program: &Path, args: &[String], cwd: Option<&Path>) -> Result<Output, PortError> {
    let mut command = ffmpeg_command(program);
    command.args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    if let Some(dir) = cwd {
        command.current_dir(dir);
    }
    command.output().map_err(|err| PortError::Failed(format!("could not run {}: {err}", program.display())))
}

/// Maps a failed run to an error that carries the end of its stderr.
pub(crate) fn ensure_success(output: &Output, what: &str) -> Result<(), PortError> {
    if output.status.success() { Ok(()) } else { Err(PortError::Failed(format!("{what} failed: {}", tail(&output.stderr)))) }
}

fn spawn_ffmpeg(ffmpeg: &Path, args: &[String], log: &Path) -> Result<Child, PortError> {
    let stderr = File::create(log).map_err(|err| PortError::Failed(format!("create {}: {err}", log.display())))?;
    ffmpeg_command(ffmpeg)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(stderr)
        .spawn()
        .map_err(|err| PortError::Failed(format!("could not start {}: {err}", ffmpeg.display())))
}

/// Moves the feed onto a thread that writes it to stdin. The thread drops stdin when the feed
/// stops or ends, which is the end of FFmpeg's input.
fn spawn_feeder(feed: Feed, stdin: ChildStdin) -> Result<Feeder, PortError> {
    let stop = Arc::new(AtomicBool::new(false));
    let (send, returned) = mpsc::channel();
    let flag = Arc::clone(&stop);
    thread::Builder::new()
        .name("laqta-ffmpeg-feed".to_owned())
        .spawn(move || {
            let mut feed = feed;
            feed_stdin(&mut feed, stdin, &flag);
            if let Err(mpsc::SendError(mut feed)) = send.send(feed) {
                // Nobody waits for the source any more, so stop it here.
                feed.stop();
            }
        })
        .map_err(|err| PortError::Failed(format!("could not start the feed thread: {err}")))?;
    Ok(Feeder { stop, returned })
}

fn feed_stdin(feed: &mut Feed, mut stdin: ChildStdin, stop: &AtomicBool) {
    match feed {
        Feed::Frames(source) => {
            let mut scratch = Vec::new();
            while !stop.load(Ordering::SeqCst) {
                match source.next_frame() {
                    Ok(Some(frame)) => {
                        if stdin.write_all(bgra(&frame, &mut scratch)).is_err() {
                            break;
                        }
                    }
                    Ok(None) => break,
                    Err(err) => {
                        tracing::warn!(%err, "the frame source failed");
                        break;
                    }
                }
            }
        }
        Feed::Pcm(source) => {
            let mut samples = vec![0i16; PCM_CHUNK_SAMPLES];
            let mut bytes = Vec::with_capacity(PCM_CHUNK_SAMPLES * 2);
            while !stop.load(Ordering::SeqCst) {
                match source.read(&mut samples) {
                    Ok(0) => break,
                    Ok(read) => {
                        bytes.clear();
                        for sample in &samples[..read] {
                            bytes.extend_from_slice(&sample.to_le_bytes());
                        }
                        if stdin.write_all(&bytes).is_err() {
                            break;
                        }
                    }
                    Err(err) => {
                        tracing::warn!(%err, "the PCM source failed");
                        break;
                    }
                }
            }
        }
    }
}

/// The frame's pixels as BGRA. RGBA frames are swapped into `scratch`.
fn bgra<'a>(frame: &'a Bitmap, scratch: &'a mut Vec<u8>) -> &'a [u8] {
    match frame.format() {
        PixelFormat::Bgra8 => frame.pixels(),
        PixelFormat::Rgba8 => {
            scratch.clear();
            for pixel in frame.pixels().as_chunks::<4>().0 {
                scratch.extend_from_slice(&[pixel[2], pixel[1], pixel[0], pixel[3]]);
            }
            scratch.as_slice()
        }
    }
}

/// Waits up to `limit` for the child to exit. `None` means it is still running (or cannot be
/// checked).
fn wait_for_exit(child: &mut Child, limit: Duration) -> Option<ExitStatus> {
    let deadline = Instant::now() + limit;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status),
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(20)),
            _ => return None,
        }
    }
}

fn kill_and_reap(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

fn path_text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}
