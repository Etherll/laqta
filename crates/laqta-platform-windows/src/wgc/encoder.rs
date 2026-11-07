//! The Media Foundation sink writer of the native recorder: H.264 from BGRA frames and, when system sound
//! is recorded, AAC from 48 kHz stereo PCM. The file is fragmented MP4, so a crash leaves a playable file.

use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::sync::OnceLock;

use laqta_ports::PortError;
use windows::Win32::Media::MediaFoundation::{
    IMFAttributes, IMFByteStream, IMFMediaType, IMFSinkWriter, MF_MT_AUDIO_AVG_BYTES_PER_SECOND, MF_MT_AUDIO_BITS_PER_SAMPLE,
    MF_MT_AUDIO_BLOCK_ALIGNMENT, MF_MT_AUDIO_NUM_CHANNELS, MF_MT_AUDIO_SAMPLES_PER_SECOND, MF_MT_AVG_BITRATE, MF_MT_DEFAULT_STRIDE,
    MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE, MF_MT_INTERLACE_MODE, MF_MT_MAJOR_TYPE, MF_MT_PIXEL_ASPECT_RATIO, MF_MT_SUBTYPE,
    MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, MF_TRANSCODE_CONTAINERTYPE, MF_VERSION, MFAudioFormat_AAC, MFAudioFormat_PCM,
    MFCreateAttributes, MFCreateMediaType, MFCreateMemoryBuffer, MFCreateSample, MFCreateSinkWriterFromURL, MFMediaType_Audio,
    MFMediaType_Video, MFSTARTUP_FULL, MFStartup, MFTranscodeContainerType_FMPEG4, MFVideoFormat_ARGB32, MFVideoFormat_H264,
    MFVideoInterlace_Progressive,
};
use windows::core::PCWSTR;

use super::failed;

/// The audio the native recorder writes: 48 kHz stereo.
pub(crate) const AUDIO_RATE: u32 = 48_000;
pub(crate) const AUDIO_CHANNELS: u16 = 2;
/// AAC at 160 kbit/s, in bytes per second.
const AAC_BYTES_PER_SECOND: u32 = 20_000;

/// An open sink writer with its video stream and, when recording sound, its AAC stream. It is created,
/// written and finished on the recording threads, all in the multithreaded apartment.
pub(crate) struct Sink {
    writer: IMFSinkWriter,
    video: u32,
    audio: Option<u32>,
}

// SAFETY: the writer is only used by threads in the multithreaded apartment, and every call to it is
// serialized by the session's mutex. It moves between the video and audio threads and is finished by the
// video thread, after the audio thread has been joined.
unsafe impl Send for Sink {}

impl Sink {
    /// Creates `path` with an H.264 stream of `size` at `fps` and `bitrate` bits per second, and an AAC
    /// stream when `audio` is set. Writing starts at once.
    pub(crate) fn create(path: &Path, (width, height): (u32, u32), fps: u32, bitrate: u32, audio: bool) -> Result<Self, PortError> {
        startup()?;
        let writer = open(path)?;
        let (video_output, video_input) = video_types(width, height, fps, bitrate).map_err(failed("video format"))?;
        // SAFETY: the media types are live for each call, and the stream indexes come from the writer.
        let video = unsafe { writer.AddStream(&video_output) }.map_err(failed("video stream"))?;
        unsafe { writer.SetInputMediaType(video, &video_input, None::<&IMFAttributes>) }.map_err(failed("video input"))?;
        let audio = if audio {
            let (audio_output, audio_input) = audio_types().map_err(failed("audio format"))?;
            // SAFETY: as above.
            let index = unsafe { writer.AddStream(&audio_output) }.map_err(failed("audio stream"))?;
            unsafe { writer.SetInputMediaType(index, &audio_input, None::<&IMFAttributes>) }.map_err(failed("audio input"))?;
            Some(index)
        } else {
            None
        };
        // SAFETY: every stream is configured.
        unsafe { writer.BeginWriting() }.map_err(failed("start writing"))?;
        Ok(Sink { writer, video, audio })
    }

    /// Writes one BGRA frame at `time` that lasts `duration`, both in 100 ns units.
    pub(crate) fn write_video(&self, frame: &[u8], time: u64, duration: u64) -> Result<(), PortError> {
        self.write(self.video, frame, time, duration)
    }

    /// Writes interleaved 16-bit 48 kHz stereo samples at `time` that last `duration`.
    pub(crate) fn write_audio(&self, samples: &[u8], time: u64, duration: u64) -> Result<(), PortError> {
        let Some(stream) = self.audio else {
            return Err(PortError::Failed("the recording has no audio stream".into()));
        };
        self.write(stream, samples, time, duration)
    }

    /// Writes the end of the file. Nothing can be written after this.
    pub(crate) fn finish(&self) -> Result<(), PortError> {
        // SAFETY: the writer is live until this struct is dropped.
        unsafe { self.writer.Finalize() }.map_err(failed("finish the recording"))
    }

    fn write(&self, stream: u32, bytes: &[u8], time: u64, duration: u64) -> Result<(), PortError> {
        let len = u32::try_from(bytes.len()).map_err(|_| PortError::Failed("a sample is too large".into()))?;
        // SAFETY: the buffer is new and is locked only while the bytes are copied into it. The sample owns
        // a reference to it, and the writer copies the sample's data during WriteSample.
        unsafe {
            let buffer = MFCreateMemoryBuffer(len).map_err(failed("sample buffer"))?;
            let mut data: *mut u8 = std::ptr::null_mut();
            buffer.Lock(&raw mut data, None, None).map_err(failed("lock the sample buffer"))?;
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), data, bytes.len());
            buffer.Unlock().map_err(failed("unlock the sample buffer"))?;
            buffer.SetCurrentLength(len).map_err(failed("sample length"))?;
            let sample = MFCreateSample().map_err(failed("sample"))?;
            sample.AddBuffer(&buffer).map_err(failed("sample buffer"))?;
            sample.SetSampleTime(i64::try_from(time).unwrap_or(i64::MAX)).map_err(failed("sample time"))?;
            sample.SetSampleDuration(i64::try_from(duration).unwrap_or(i64::MAX)).map_err(failed("sample duration"))?;
            self.writer.WriteSample(stream, &sample).map_err(failed("write a sample"))
        }
    }
}

/// Starts Media Foundation for the process, once. It needs the calling thread to be in a COM apartment.
fn startup() -> Result<(), PortError> {
    static STARTED: OnceLock<Result<(), String>> = OnceLock::new();
    let started = STARTED.get_or_init(|| {
        // SAFETY: a plain start-up call. Media Foundation stays up until the process exits.
        unsafe { MFStartup(MF_VERSION, MFSTARTUP_FULL) }.map_err(|error| error.to_string())
    });
    started.clone().map_err(PortError::Failed)
}

/// The sink writer for `path`: fragmented MP4, with hardware transforms allowed for the encoder.
fn open(path: &Path) -> Result<IMFSinkWriter, PortError> {
    let mut attributes: Option<IMFAttributes> = None;
    // SAFETY: the out-pointer receives a new attribute store, which lives in `attributes`.
    unsafe { MFCreateAttributes(&raw mut attributes, 2) }.map_err(failed("sink attributes"))?;
    let attributes = attributes.ok_or_else(|| PortError::Failed("sink attributes: none were returned".into()))?;
    // SAFETY: the store is live, and the GUIDs are statics.
    unsafe {
        attributes.SetGUID(&MF_TRANSCODE_CONTAINERTYPE, &MFTranscodeContainerType_FMPEG4).map_err(failed("container type"))?;
        attributes.SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, 1).map_err(failed("hardware transforms"))?;
    }
    let url: Vec<u16> = path.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    // SAFETY: `url` is NUL-terminated and outlives the call. No byte stream is given, so the writer opens the file.
    unsafe { MFCreateSinkWriterFromURL(PCWSTR(url.as_ptr()), None::<&IMFByteStream>, Some(&attributes)) }
        .map_err(failed("MP4 sink"))
}

/// Packs two 32-bit values into one `MF_MT_*` attribute value, high word first.
fn pack(high: u32, low: u32) -> u64 {
    (u64::from(high) << 32) | u64::from(low)
}

/// The H.264 output type and the BGRA input type (`ARGB32` in Media Foundation's byte order).
fn video_types(width: u32, height: u32, fps: u32, bitrate: u32) -> windows::core::Result<(IMFMediaType, IMFMediaType)> {
    // SAFETY: the media types are new objects owned here, and each call takes live objects and GUIDs.
    unsafe {
        let output = MFCreateMediaType()?;
        output.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        output.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)?;
        output.SetUINT32(&MF_MT_AVG_BITRATE, bitrate)?;
        describe_video(&output, width, height, fps)?;

        let input = MFCreateMediaType()?;
        input.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        input.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_ARGB32)?;
        input.SetUINT32(&MF_MT_DEFAULT_STRIDE, width * 4)?;
        describe_video(&input, width, height, fps)?;
        Ok((output, input))
    }
}

fn describe_video(media: &IMFMediaType, width: u32, height: u32, fps: u32) -> windows::core::Result<()> {
    // SAFETY: the media type is live for each call.
    unsafe {
        media.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
        media.SetUINT64(&MF_MT_FRAME_SIZE, pack(width, height))?;
        media.SetUINT64(&MF_MT_FRAME_RATE, pack(fps, 1))?;
        media.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack(1, 1))?;
    }
    Ok(())
}

/// The AAC output type (160 kbit/s) and the 16-bit PCM input type, both 48 kHz stereo.
fn audio_types() -> windows::core::Result<(IMFMediaType, IMFMediaType)> {
    let channels = u32::from(AUDIO_CHANNELS);
    let block_align = channels * 2;
    // SAFETY: the media types are new objects owned here, and each call takes live objects and GUIDs.
    unsafe {
        let output = MFCreateMediaType()?;
        output.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
        output.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_AAC)?;
        output.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
        output.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, AUDIO_RATE)?;
        output.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, channels)?;
        output.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, AAC_BYTES_PER_SECOND)?;

        let input = MFCreateMediaType()?;
        input.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
        input.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM)?;
        input.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
        input.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, AUDIO_RATE)?;
        input.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, channels)?;
        input.SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, block_align)?;
        input.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, AUDIO_RATE * block_align)?;
        Ok((output, input))
    }
}
