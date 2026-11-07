//! System sound as PCM (M4 D6): a WASAPI shared-mode loopback capture of the default playback device.
//! The capture runs on its own thread and fills a bounded buffer that FFmpeg reads through a pipe.
//! While the device delivers nothing, the thread writes silence on the wall clock, so the audio
//! stays in step with the picture. Microphones are not here: `dshow` opens them by name.

use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use laqta_domain::record::AudioDevice;
use laqta_ports::{PcmFormat, PcmSource, PortError};
use wasapi::{AudioCaptureClient, AudioClient, DeviceEnumerator, Direction, Handle, SampleType, StreamMode, WaveFormat, WasapiError};

/// The id and name of the system sound entry in the device list.
const LOOPBACK_ID: &str = "loopback";
const LOOPBACK_NAME: &str = "System sound";

const RATE: u32 = 48_000;
const CHANNELS: u16 = 2;
/// Two seconds of audio. A reader that falls further behind loses the oldest samples.
const BUFFER_FRAMES: usize = 2 * RATE as usize;
const BUFFER_SAMPLES: usize = BUFFER_FRAMES * CHANNELS as usize;
/// Silence is written only once the output is this many frames (100 ms) behind the clock, so the
/// normal buffering latency of the device adds no gaps.
const SILENCE_SLACK: u64 = (RATE / 10) as u64;
/// How long the capture thread waits for a device event before it checks for a stop.
const EVENT_WAIT_MS: u32 = 100;
/// How long `read` waits before it checks again. It keeps waiting until samples arrive or the capture
/// ends, because a 0 from `read` means the source has ended.
const READ_WAIT: Duration = Duration::from_millis(100);
/// How long `start` waits for the device to open.
const START_WAIT: Duration = Duration::from_secs(5);
/// Endpoint buffer for the event-driven shared-mode stream: 200 ms. The device's events set the
/// cadence, and the buffer only decides how much audio survives a late wake-up. The wasapi crate's
/// examples use about 20 ms.
const EVENT_BUFFER_HNS: i64 = 2_000_000;

/// Microphones (capture endpoints, named by their friendly name, which `dshow` accepts) and the
/// system sound entry. This blocks, so run it off the async worker.
pub(crate) fn list_devices() -> Result<Vec<AudioDevice>, PortError> {
    let _com = Mta::enter();
    let enumerator = DeviceEnumerator::new().map_err(|e| PortError::Failed(format!("audio devices: {e}")))?;
    let collection = enumerator
        .get_device_collection(Direction::Capture)
        .map_err(|e| PortError::Failed(format!("capture devices: {e}")))?;
    let count = collection.get_nbr_devices().map_err(|e| PortError::Failed(format!("capture devices: {e}")))?;
    let mut devices = Vec::new();
    for index in 0..count {
        match collection.get_device_at_index(index).and_then(|device| device.get_friendlyname()) {
            Ok(name) => devices.push(AudioDevice { id: name.clone(), name, loopback: false }),
            Err(e) => tracing::debug!("skipping capture device {index}: {e}"),
        }
    }
    devices.push(AudioDevice { id: LOOPBACK_ID.into(), name: LOOPBACK_NAME.into(), loopback: true });
    Ok(devices)
}

/// A running loopback capture. `read` returns interleaved 16-bit stereo at 48 kHz. Dropping it stops
/// the capture thread.
pub(crate) struct WasapiLoopback {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl WasapiLoopback {
    /// Opens the default playback device for loopback and starts the capture thread. Fails when the
    /// device cannot be opened in the requested format.
    pub(crate) fn start() -> Result<Self, PortError> {
        let shared = Arc::new(Shared::default());
        let (ready_tx, ready_rx) = mpsc::channel();
        let worker = Arc::clone(&shared);
        let thread = std::thread::Builder::new()
            .name("laqta-loopback".into())
            .spawn(move || run(&worker, &ready_tx))
            .map_err(|e| PortError::Failed(format!("system audio thread: {e}")))?;
        match ready_rx.recv_timeout(START_WAIT) {
            Ok(Ok(())) => Ok(WasapiLoopback { shared, thread: Some(thread) }),
            Ok(Err(reason)) => {
                let _ = thread.join();
                Err(PortError::Failed(format!("system audio: {reason}")))
            }
            Err(_) => {
                shared.end();
                Err(PortError::Failed("system audio did not start".into()))
            }
        }
    }
}

impl PcmSource for WasapiLoopback {
    fn format(&self) -> PcmFormat {
        PcmFormat { sample_rate: RATE, channels: CHANNELS }
    }

    /// Waits for samples and returns as many as fit in `buf`. An empty `buf` gets 0.
    fn read(&mut self, buf: &mut [i16]) -> Result<usize, PortError> {
        if buf.is_empty() {
            return Ok(0);
        }
        let mut state = self.shared.lock();
        // The clock starts with the first read, so audio from before the recording is not kept.
        state.clock_start.get_or_insert_with(Instant::now);
        loop {
            if !state.samples.is_empty() {
                let n = buf.len().min(state.samples.len());
                for (slot, sample) in buf.iter_mut().zip(state.samples.drain(..n)) {
                    *slot = sample;
                }
                return Ok(n);
            }
            if state.ended {
                return Ok(0);
            }
            state = self.shared.wake.wait_timeout(state, READ_WAIT).unwrap_or_else(PoisonError::into_inner).0;
        }
    }

    fn stop(&mut self) {
        self.shared.end();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for WasapiLoopback {
    fn drop(&mut self) {
        self.stop();
    }
}

/// The state shared by `read` and the capture thread.
#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    wake: Condvar,
}

#[derive(Default)]
struct State {
    samples: VecDeque<i16>,
    /// Set by the first `read`. Until then the capture thread drops packets.
    clock_start: Option<Instant>,
    /// Set by `stop`, or when the capture fails. Once the buffer drains, `read` returns 0.
    ended: bool,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn end(&self) {
        self.lock().ended = true;
        self.wake.notify_all();
    }
}

/// Where the capture is on the wall clock: `frames` written since `start`, silence included.
struct Clock {
    start: Instant,
    frames: u64,
}

/// The device's loopback stream: the client that owns it, its capture service and its event.
struct Capture {
    client: AudioClient,
    service: AudioCaptureClient,
    event: Handle,
}

/// The capture thread: opens the device, reports to `start`, then copies packets until the capture
/// ends.
fn run(shared: &Shared, ready: &mpsc::Sender<Result<(), String>>) {
    let _com = Mta::enter();
    let capture = match open_capture() {
        Ok(capture) => capture,
        Err(reason) => {
            let _ = ready.send(Err(reason));
            shared.end();
            return;
        }
    };
    let _ = ready.send(Ok(()));
    if let Err(e) = capture_loop(shared, &capture) {
        tracing::warn!("system audio capture stopped: {e}");
    }
    let _ = capture.client.stop_stream();
    shared.end();
}

/// Opens the default playback device for loopback. A render endpoint opened with `Direction::Capture`
/// is a loopback stream. The engine converts to 16-bit stereo at 48 kHz.
fn open_capture() -> Result<Capture, String> {
    let enumerator = DeviceEnumerator::new().map_err(|e| format!("audio devices: {e}"))?;
    let device = enumerator.get_default_device(Direction::Render).map_err(|e| format!("no playback device: {e}"))?;
    let mut client = device.get_iaudioclient().map_err(|e| format!("playback device: {e}"))?;
    let format = WaveFormat::new(16, 16, SampleType::Int, RATE as usize, usize::from(CHANNELS), None);
    let mode = StreamMode::EventsShared { autoconvert: true, buffer_duration_hns: EVENT_BUFFER_HNS };
    client.initialize_client(&format, Direction::Capture, mode, None).map_err(|e| format!("loopback: {e}"))?;
    let event = client.set_get_eventhandle().map_err(|e| format!("loopback event: {e}"))?;
    let service = client.get_audiocaptureclient().map_err(|e| format!("loopback capture: {e}"))?;
    client.start_stream().map_err(|e| format!("loopback start: {e}"))?;
    Ok(Capture { client, service, event })
}

/// Copies captured packets into the buffer until the capture ends. Packets are read but dropped until
/// the first `read` starts the clock. After that, silence fills any gap the device leaves.
fn capture_loop(shared: &Shared, capture: &Capture) -> Result<(), WasapiError> {
    let mut clock: Option<Clock> = None;
    loop {
        match capture.event.wait_for_event(EVENT_WAIT_MS) {
            Ok(()) | Err(WasapiError::EventTimeout) => {}
            Err(e) => return Err(e),
        }
        let (ended, started) = {
            let state = shared.lock();
            (state.ended, state.clock_start)
        };
        if ended {
            return Ok(());
        }
        if clock.is_none() {
            clock = started.map(|start| Clock { start, frames: 0 });
        }
        let fresh = drain_packets(&capture.service)?;
        let mut state = shared.lock();
        if state.ended {
            return Ok(());
        }
        if let Some(clock) = clock.as_mut() {
            clock.frames += fresh.len() as u64 / u64::from(CHANNELS);
            state.samples.extend(fresh);
            let due = frames_since(clock.start);
            if due > clock.frames + SILENCE_SLACK {
                let fill = (due - clock.frames).min(BUFFER_FRAMES as u64) as usize;
                state.samples.extend(std::iter::repeat_n(0, fill * usize::from(CHANNELS)));
                clock.frames = due;
            }
            let excess = state.samples.len().saturating_sub(BUFFER_SAMPLES);
            state.samples.drain(..excess);
            drop(state);
            shared.wake.notify_all();
        }
    }
}

/// Reads every packet the device has ready and returns them as interleaved samples. Silent packets
/// come back as zeros.
fn drain_packets(service: &AudioCaptureClient) -> Result<Vec<i16>, WasapiError> {
    let mut bytes = VecDeque::new();
    let mut samples = Vec::new();
    while service.get_next_packet_size()?.is_some_and(|frames| frames > 0) {
        service.read_from_device_to_deque(&mut bytes)?;
        let raw: Vec<u8> = bytes.drain(..).collect();
        samples.extend(raw.as_chunks::<2>().0.iter().map(|pair| i16::from_le_bytes(*pair)));
    }
    Ok(samples)
}

/// The number of frames the wall clock has passed since `start`.
fn frames_since(start: Instant) -> u64 {
    (start.elapsed().as_secs_f64() * f64::from(RATE)) as u64
}

/// Initialises COM for the calling thread as multi-threaded. It is uninitialised on drop when that
/// call succeeded. A thread already set up in another mode keeps its own setting.
struct Mta(bool);

impl Mta {
    fn enter() -> Self {
        Mta(wasapi::initialize_mta().is_ok())
    }
}

impl Drop for Mta {
    fn drop(&mut self) {
        if self.0 {
            wasapi::deinitialize();
        }
    }
}
