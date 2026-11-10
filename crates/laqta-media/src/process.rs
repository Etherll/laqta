//! The recording process. A segment is one FFmpeg process. Pause ends the segment,
//! resume starts another, and stop joins the segments with the concat demuxer (animations are then
//! converted from a lossless intermediate). FFmpeg reads stdin only when an input uses it, so a
//! segment ends by EOF on stdin in that case, and by `q` otherwise.
//!
//! The handle locks `Inner` from a worker thread for each operation. `is_alive` never waits.

use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, ExitStatus, Output, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex, PoisonError, TryLockError};
use std::thread;
use std::time::{Duration, Instant};

use laqta_domain::record::{RecordSpec, VideoCodec};
use laqta_domain::{Bitmap, PixelFormat};
use laqta_ports::recording::{FrameSource, OsScreen, PcmSource, RecordingHandle};
use laqta_ports::{BoxFuture, PortError};

use crate::argv::{self, AudioInputDesc, Host, VideoInputDesc, WindowsCapture};
use crate::locate::FfmpegPaths;
use crate::{ffmpeg_command, run_blocking, tail};

/// How long a segment gets to finish after its input ends. After this it is killed.
const FINISH_GRACE: Duration = Duration::from_secs(5);
/// How long a Windows `ddagrab` start is watched for an early failure.
const CAPTURE_CHECK: Duration = Duration::from_secs(2);
const PCM_CHUNK_SAMPLES: usize = 4096;
/// The longest silence put before the sound for FFmpeg's start-up. A longer wait is not a start-up.
const MAX_LEAD_SILENCE: Duration = Duration::from_secs(5);
/// The pipe name a PCM input carries when raw frames have stdin. The recorder replaces it with a
/// named pipe inside the session folder.
pub(crate) const SIDE_PIPE: &str = "laqta-side-pcm";

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

/// Refuses a plan this host cannot run, before FFmpeg is spawned. A codec the OS lacks is
/// `Unsupported`. A Windows camera needs a DirectShow name, so an enabled camera with none is refused.
fn validate(plan: &Plan) -> Result<(), PortError> {
    if !plan.spec.codec.available_on(plan.host.os()) {
        return Err(PortError::Unsupported(format!("{} is not available on this system", plan.spec.codec.label())));
    }
    let camera_unnamed = plan.spec.camera.as_ref().is_some_and(|cam| cam.enabled && cam.device.trim().is_empty());
    if plan.host == Host::Windows && camera_unnamed {
        return Err(PortError::Failed("Choose a camera in Settings → Recording, or turn the camera overlay off".into()));
    }
    Ok(())
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
    /// The PCM source that has a named pipe of its own, while no segment runs.
    side: Option<Box<dyn PcmSource>>,
    /// That named pipe, when the plan has one.
    side_pipe: Option<PathBuf>,
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
    /// Writes the side PCM source to its named pipe.
    side: Option<SideFeeder>,
}

struct SideFeeder {
    stop: Arc<AtomicBool>,
    /// Receives the source back once the feed has ended.
    returned: Receiver<Box<dyn PcmSource>>,
}

struct Feeder {
    stop: Arc<AtomicBool>,
    /// Receives the source back once the feed has ended.
    returned: Receiver<Feed>,
}

impl FfmpegRecorder {
    /// Starts the first segment. `feed` is the stdin source, when `plan` uses stdin. `side` is the
    /// PCM source of the plan's `SIDE_PIPE` input.
    pub(crate) fn start(
        paths: FfmpegPaths,
        mut plan: Plan,
        session: PathBuf,
        output: PathBuf,
        mut feed: Option<Feed>,
        mut side: Option<Box<dyn PcmSource>>,
    ) -> Result<Self, PortError> {
        let stop_sources = |feed: &mut Option<Feed>, side: &mut Option<Box<dyn PcmSource>>| {
            if let Some(feed) = feed.as_mut() {
                feed.stop();
            }
            if let Some(side) = side.as_mut() {
                side.stop();
            }
        };
        if let Err(err) = validate(&plan) {
            stop_sources(&mut feed, &mut side);
            return Err(err);
        }
        if let Err(err) = fs::create_dir_all(&session) {
            stop_sources(&mut feed, &mut side);
            return Err(PortError::Failed(format!("create {}: {err}", session.display())));
        }
        let side_pipe = place_side_pipe(&mut plan, &session, &mut side);
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
            side,
            side_pipe,
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
                    // The sound thread may still wait for FFmpeg to open its pipe.
                    state.side = segment.side.take().and_then(|side| side.finish(state.side_pipe.as_deref()));
                }
                if let Some(mut side) = state.side.take() {
                    side.stop();
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
        // ddagrab can fail on some adapters. gdigrab takes over when ddagrab exits early.
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
        // The ddagrab segment ending is why gdigrab takes over, not the end of the recording.
        self.ended = false;
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
        // FFmpeg would wait for a writer on the pipe for ever.
        if self.side_pipe.is_some() && self.side.is_none() {
            return Err(PortError::Failed("the sound source did not come back after the pause".into()));
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
                (Some(spawn_feeder(feed, stdin, raw_fps(&self.plan.video))?), None)
            }
            None => (None, stdin),
        };
        let side = match (self.side.take(), &self.side_pipe) {
            (Some(source), Some(pipe)) => match spawn_side_feeder(source, pipe.clone()) {
                Ok(side) => Some(side),
                Err(err) => {
                    kill_and_reap(&mut child);
                    drop(feeder);
                    return Err(err);
                }
            },
            _ => None,
        };
        self.current = Some(Segment { name, log, child, feeder, stdin, side });
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
        if let Some(mut side) = self.side.take() {
            side.stop();
        }
        self.join_segments()
    }

    fn abort(&mut self) {
        if let Some(mut segment) = self.current.take() {
            kill_and_reap(&mut segment.child);
            // Dropping the receiver makes the feeder stop the source once it sees the broken pipe.
            drop(segment.feeder.take());
            self.side = segment.side.take().and_then(|side| side.finish(self.side_pipe.as_deref()));
        }
        if let Some(mut feed) = self.feed.take() {
            feed.stop();
        }
        if let Some(mut side) = self.side.take() {
            side.stop();
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
        if let Some(side) = &segment.side {
            side.stop.store(true, Ordering::SeqCst);
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
        if let Some(side) = segment.side.take() {
            self.side = side.finish(self.side_pipe.as_deref());
            if self.side.is_none() {
                tracing::warn!("the sound source did not return; the recording cannot resume");
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
    /// the error names the partial file: the last segment when the join failed, or the intermediate
    /// when the conversion did (see `convert_animation`).
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
            self.convert_animation(&joined)?;
        }
        let _ = fs::remove_dir_all(&self.session);
        Ok(self.output.clone())
    }

    /// Converts the lossless `merged` file to the animation at `self.output`. The animation is
    /// written under a `.part` name and renamed when complete, so a failed or killed conversion
    /// never leaves a broken file at the output. A GIF takes two runs (palette, then palette use),
    /// WebP and APNG one.
    ///
    /// On failure the `.part` file is deleted and the intermediate is kept as the usable result:
    /// it moves next to the output as `<output name>.mkv` (`kept_path`), and the error names it.
    fn convert_animation(&self, merged: &Path) -> Result<(), PortError> {
        let part = part_path(&self.output);
        let reason = match self.convert_into(merged, &part) {
            Ok(()) => match fs::rename(&part, &self.output) {
                Ok(()) => return Ok(()),
                Err(err) => format!("rename {} to {}: {err}", part.display(), self.output.display()),
            },
            Err(reason) => reason,
        };
        let _ = fs::remove_file(&part);
        let kept = self.keep_intermediate(merged);
        Err(PortError::Failed(format!("could not convert the animation; the partial recording is {}: {reason}", kept.display())))
    }

    /// Runs the conversion into `part`. The error is FFmpeg's last lines, or what went wrong.
    fn convert_into(&self, merged: &Path, part: &Path) -> Result<(), String> {
        let spec = &self.plan.spec;
        let palette = self.session.join("palette.png");
        let runs = match spec.codec {
            VideoCodec::Gif => {
                vec![argv::palette_args(merged, spec.fps, &palette), argv::paletteuse_args(merged, &palette, spec.fps, part)]
            }
            codec => vec![argv::animation_args(codec, merged, part)],
        };
        for args in &runs {
            let output = run_tool(&self.paths.ffmpeg, args, Some(&self.session)).map_err(|err| err.to_string())?;
            if !output.status.success() {
                return Err(tail(&output.stderr));
            }
        }
        match fs::metadata(part) {
            Ok(meta) if meta.len() > 0 => Ok(()),
            _ => Err("FFmpeg wrote no frames".to_owned()),
        }
    }

    /// Moves the intermediate next to the output and deletes the session folder (with its palette
    /// and logs). Returns where the intermediate is now. When it cannot be moved, it stays in the
    /// session folder, which is then kept too.
    fn keep_intermediate(&self, merged: &Path) -> PathBuf {
        let kept = kept_path(&self.output);
        match move_file(merged, &kept) {
            Ok(()) => {
                let _ = fs::remove_dir_all(&self.session);
                kept
            }
            Err(err) => {
                tracing::warn!(%err, to = %kept.display(), "could not move the intermediate next to the output");
                merged.to_path_buf()
            }
        }
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

/// Makes the named pipe for the plan's `SIDE_PIPE` input and points the input at it. When the pipe
/// cannot be made, the recording goes on without that sound: the input is removed and the source
/// stopped.
fn place_side_pipe(plan: &mut Plan, session: &Path, side: &mut Option<Box<dyn PcmSource>>) -> Option<PathBuf> {
    let is_side = |input: &AudioInputDesc| matches!(input, AudioInputDesc::Pcm { pipe, .. } if pipe == SIDE_PIPE);
    if !plan.audio.iter().any(is_side) {
        return None;
    }
    let path = session.join("sound.pcm");
    let made = side.is_some() && cfg!(unix) && Command::new("mkfifo").arg(&path).status().is_ok_and(|status| status.success());
    if !made {
        tracing::warn!("no pipe for the system sound; recording without it");
        plan.audio.retain(|input| !is_side(input));
        if let Some(mut source) = side.take() {
            source.stop();
        }
        return None;
    }
    for input in &mut plan.audio {
        if let AudioInputDesc::Pcm { pipe, .. } = input
            && pipe == SIDE_PIPE
        {
            *pipe = path.to_string_lossy().into_owned();
        }
    }
    Some(path)
}

impl SideFeeder {
    /// Stops the feed and takes the source back. The feed thread may still be waiting in `open`
    /// for FFmpeg to read the pipe (FFmpeg failed before it got there); opening the pipe for
    /// reading and writing here never blocks and lets that `open` return.
    fn finish(self, pipe: Option<&Path>) -> Option<Box<dyn PcmSource>> {
        self.stop.store(true, Ordering::SeqCst);
        let _reader = pipe.and_then(|pipe| fs::OpenOptions::new().read(true).write(true).open(pipe).ok());
        self.returned.recv_timeout(FINISH_GRACE).ok()
    }
}

/// Moves the side PCM source onto a thread that writes it to the named pipe. Opening the pipe waits
/// until FFmpeg opens it for reading. The thread closes the pipe when the feed stops or ends.
fn spawn_side_feeder(source: Box<dyn PcmSource>, pipe: PathBuf) -> Result<SideFeeder, PortError> {
    let stop = Arc::new(AtomicBool::new(false));
    let (send, returned) = mpsc::channel();
    let flag = Arc::clone(&stop);
    thread::Builder::new()
        .name("laqta-ffmpeg-sound".to_owned())
        .spawn(move || {
            let mut source = source;
            // The picture's clock starts now, with this segment; FFmpeg opens the pipe later.
            let started = Instant::now();
            match fs::OpenOptions::new().write(true).open(&pipe) {
                Ok(mut file) => {
                    let format = source.format();
                    let lead = vec![0u8; lead_silence_bytes(started.elapsed(), format.sample_rate, format.channels)];
                    // Stopped already: `finish` opened the pipe, not FFmpeg, and nothing reads it.
                    if !flag.load(Ordering::SeqCst) && file.write_all(&lead).is_ok() {
                        feed_pcm(source.as_mut(), &mut file, &flag);
                    }
                }
                Err(err) => tracing::warn!(%err, "could not open the sound pipe"),
            }
            if let Err(mpsc::SendError(mut source)) = send.send(source) {
                source.stop();
            }
        })
        .map_err(|err| PortError::Failed(format!("could not start the sound thread: {err}")))?;
    Ok(SideFeeder { stop, returned })
}

/// Bytes of 16-bit silence that last `waited`, in whole sample frames and at most
/// `MAX_LEAD_SILENCE`. A source starts its sound when it is first read, which is when FFmpeg opens
/// the pipe; the picture started `waited` earlier, and the frames from that wait are in the file.
/// Without this silence the sound would play that much ahead of the picture.
fn lead_silence_bytes(waited: Duration, sample_rate: u32, channels: u16) -> usize {
    let frames = (waited.min(MAX_LEAD_SILENCE).as_secs_f64() * f64::from(sample_rate)) as usize;
    frames * usize::from(channels) * 2
}

/// Writes the source's samples as little-endian 16-bit PCM until `stop` is set, the source ends or
/// the reader goes away.
fn feed_pcm(source: &mut dyn PcmSource, out: &mut dyn Write, stop: &AtomicBool) {
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
                if out.write_all(&bytes).is_err() {
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

/// The rate FFmpeg reads raw frames at (`-r`), which the feeder paces them to. 0 without them.
fn raw_fps(video: &VideoInputDesc) -> u32 {
    match video {
        VideoInputDesc::RawFrames { fps, .. } => *fps,
        VideoInputDesc::Target(_) => 0,
    }
}

/// Moves the feed onto a thread that writes it to stdin. The thread drops stdin when the feed
/// stops or ends, which is the end of FFmpeg's input. Frames are paced to `fps`.
fn spawn_feeder(feed: Feed, stdin: ChildStdin, fps: u32) -> Result<Feeder, PortError> {
    let stop = Arc::new(AtomicBool::new(false));
    let (send, returned) = mpsc::channel();
    let flag = Arc::clone(&stop);
    thread::Builder::new()
        .name("laqta-ffmpeg-feed".to_owned())
        .spawn(move || {
            let mut feed = feed;
            feed_stdin(&mut feed, stdin, &flag, fps);
            if let Err(mpsc::SendError(mut feed)) = send.send(feed) {
                // Nobody waits for the source any more, so stop it here.
                feed.stop();
            }
        })
        .map_err(|err| PortError::Failed(format!("could not start the feed thread: {err}")))?;
    Ok(Feeder { stop, returned })
}

fn feed_stdin(feed: &mut Feed, mut stdin: ChildStdin, stop: &AtomicBool, fps: u32) {
    match feed {
        Feed::Frames(source) => feed_frames(source.as_mut(), &mut stdin, stop, fps),
        Feed::Pcm(source) => feed_pcm(source.as_mut(), &mut stdin, stop),
    }
}

/// Writes the source's frames at `fps`, whatever rate they arrive at. FFmpeg stamps the n-th frame
/// at n/fps, so the n-th frame is due at `start + n/fps` (absolute deadlines, no drift). An early
/// frame waits for its deadline. When the source is late, the latest frame is repeated for the
/// periods missed, at most `fps` copies at a time.
fn feed_frames(source: &mut dyn FrameSource, stdin: &mut ChildStdin, stop: &AtomicBool, fps: u32) {
    let fps = fps.max(1);
    let period = Duration::from_secs(1) / fps;
    let mut scratch = Vec::new();
    // Starts with the first frame, so a resumed segment does not fill the pause.
    let mut next_due: Option<Instant> = None;
    while !stop.load(Ordering::SeqCst) {
        let frame = match source.next_frame() {
            Ok(Some(frame)) => frame,
            Ok(None) => return,
            Err(err) => {
                tracing::warn!(%err, "the frame source failed");
                return;
            }
        };
        let due = *next_due.get_or_insert_with(Instant::now);
        if !sleep_until(due, stop) {
            return;
        }
        let copies = copies_due(due, Instant::now(), period, fps);
        let pixels = bgra(&frame, &mut scratch);
        for _ in 0..copies {
            if stop.load(Ordering::SeqCst) || stdin.write_all(pixels).is_err() {
                return;
            }
        }
        next_due = Some(due + period * copies);
    }
}

/// How many frames to write at `now` for the deadline `due`: one, plus one per whole period
/// missed, up to `cap`.
fn copies_due(due: Instant, now: Instant, period: Duration, cap: u32) -> u32 {
    let missed = now.saturating_duration_since(due).as_nanos() / period.as_nanos().max(1);
    u32::try_from(missed.saturating_add(1)).unwrap_or(u32::MAX).min(cap)
}

/// Sleeps until `due`. False when `stop` was set meanwhile.
fn sleep_until(due: Instant, stop: &AtomicBool) -> bool {
    loop {
        if stop.load(Ordering::SeqCst) {
            return false;
        }
        let left = due.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return true;
        }
        thread::sleep(left.min(Duration::from_millis(50)));
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

/// The name an animation is written under until it is complete: `<output name>.part`.
fn part_path(output: &Path) -> PathBuf {
    with_suffix(output, ".part")
}

/// Where a failed animation conversion leaves its lossless intermediate: `<output name>.mkv` in the
/// output's folder. `laqta-app` (recording/drive.rs) looks for this name to report the partial
/// file, because `RecordingHandle::stop` has no way to return a path with an error.
fn kept_path(output: &Path) -> PathBuf {
    with_suffix(output, ".mkv")
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

/// Renames `from` to `to`, or copies and deletes it when they are on different file systems.
fn move_file(from: &Path, to: &Path) -> std::io::Result<()> {
    if fs::rename(from, to).is_ok() {
        return Ok(());
    }
    fs::copy(from, to).inspect_err(|_| {
        let _ = fs::remove_file(to);
    })?;
    let _ = fs::remove_file(from);
    Ok(())
}

#[cfg(test)]
mod tests {
    use laqta_domain::geometry::PhysRect;
    use laqta_domain::record::{CameraOverlay, Quality};

    use super::*;

    /// A stand-in FFmpeg that fails for `ddagrab` and runs until killed otherwise.
    #[cfg(unix)]
    #[test]
    fn a_recording_that_fell_back_to_gdigrab_is_still_alive() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().expect("tempdir");
        let ffmpeg = dir.path().join("ffmpeg");
        fs::write(&ffmpeg, "#!/bin/sh\ncase \"$*\" in *ddagrab*) exit 1;; esac\nexec sleep 30\n").expect("script");
        fs::set_permissions(&ffmpeg, fs::Permissions::from_mode(0o755)).expect("chmod");
        let mut plan = plan(VideoCodec::X264, Host::Windows, None);
        plan.video = VideoInputDesc::Target(OsScreen::Windows {
            adapter_index: 0,
            output_index: 0,
            offset_x: 0,
            offset_y: 0,
            width: 640,
            height: 480,
            desktop_x: 0,
            desktop_y: 0,
        });
        let paths = FfmpegPaths { ffmpeg: ffmpeg.clone(), ffprobe: ffmpeg };
        let mut recorder =
            FfmpegRecorder::start(paths, plan, dir.path().join("session"), dir.path().join("out.mp4"), None, None).expect("start");
        assert!(RecordingHandle::is_alive(&mut recorder), "the failed ddagrab attempt must not end the recording");
        locked(&recorder.inner, Inner::abort);
    }

    #[test]
    fn the_sound_gets_silence_for_the_time_ffmpeg_took_to_open_the_pipe() {
        // 250 ms of 48 kHz stereo: 12 000 frames of 4 bytes.
        assert_eq!(lead_silence_bytes(Duration::from_millis(250), 48_000, 2), 48_000);
        assert_eq!(lead_silence_bytes(Duration::ZERO, 48_000, 2), 0);
        // Whole frames only, so the channels stay in step.
        assert_eq!(lead_silence_bytes(Duration::from_micros(30), 48_000, 2) % 4, 0);
        assert_eq!(lead_silence_bytes(Duration::from_secs(60), 48_000, 2), 5 * 48_000 * 4);
    }

    #[test]
    fn copies_follow_the_periods_missed_up_to_the_cap() {
        let due = Instant::now();
        let period = Duration::from_millis(100);
        assert_eq!(copies_due(due, due, period, 10), 1);
        assert_eq!(copies_due(due, due + Duration::from_millis(99), period, 10), 1);
        assert_eq!(copies_due(due, due + Duration::from_millis(100), period, 10), 2);
        assert_eq!(copies_due(due, due + Duration::from_millis(350), period, 10), 4);
        assert_eq!(copies_due(due, due + Duration::from_secs(60), period, 10), 10);
    }

    #[test]
    fn temporary_and_kept_names_sit_next_to_the_output() {
        let output = Path::new("/videos/Screencast 12-00-00.gif");
        assert_eq!(part_path(output), Path::new("/videos/Screencast 12-00-00.gif.part"));
        assert_eq!(kept_path(output), Path::new("/videos/Screencast 12-00-00.gif.mkv"));
    }

    fn plan(codec: VideoCodec, host: Host, camera: Option<CameraOverlay>) -> Plan {
        let spec = RecordSpec {
            area: PhysRect { x: 0, y: 0, width: 640, height: 480 },
            codec,
            fps: 30,
            quality: Quality::Default,
            show_cursor: true,
            audio: Vec::new(),
            custom_args: String::new(),
            camera,
        };
        Plan { video: VideoInputDesc::RawFrames { width: 64, height: 48, fps: 30 }, audio: Vec::new(), spec, host }
    }

    fn camera(enabled: bool, device: &str) -> Option<CameraOverlay> {
        Some(CameraOverlay { enabled, device: device.into(), ..CameraOverlay::default() })
    }

    #[test]
    fn vaapi_is_refused_on_windows_and_passes_on_linux() {
        let on_windows = validate(&plan(VideoCodec::Vaapi, Host::Windows, None));
        assert!(matches!(on_windows, Err(PortError::Unsupported(_))));
        assert!(validate(&plan(VideoCodec::Vaapi, Host::Linux, None)).is_ok());
    }

    #[test]
    fn enabled_camera_without_a_name_is_refused_on_windows_only() {
        for name in ["", "   "] {
            let windows = validate(&plan(VideoCodec::X264, Host::Windows, camera(true, name)));
            assert!(matches!(windows, Err(PortError::Failed(_))), "{name:?}");
        }
        assert!(validate(&plan(VideoCodec::X264, Host::Linux, camera(true, ""))).is_ok());
    }

    #[test]
    fn disabled_camera_or_a_named_one_passes_on_windows() {
        assert!(validate(&plan(VideoCodec::X264, Host::Windows, camera(false, ""))).is_ok());
        assert!(validate(&plan(VideoCodec::X264, Host::Windows, camera(true, "Integrated Camera"))).is_ok());
        assert!(validate(&plan(VideoCodec::X264, Host::Windows, None)).is_ok());
    }
}
