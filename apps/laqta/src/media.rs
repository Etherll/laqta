//! FFmpeg on first use (ADR 005): found on PATH, at the configured path, or in the data folder;
//! otherwise Laqta asks once and downloads the pinned build through verified fetch.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use laqta_domain::record::RecordSpec;
use laqta_media::FfmpegMedia;
use laqta_ports::{BoxFuture, Media, MediaInfo, PortError, Prompt, RecordingHandle, RecordingInput, TranscodeSpec};

pub struct LazyMedia {
    data: PathBuf,
    configured: Arc<dyn Fn() -> String + Send + Sync>,
    prompt: Arc<dyn Prompt>,
    ready: Mutex<Option<Arc<FfmpegMedia>>>,
}

impl std::fmt::Debug for LazyMedia {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LazyMedia").finish_non_exhaustive()
    }
}

impl LazyMedia {
    pub fn new(data: PathBuf, configured: Arc<dyn Fn() -> String + Send + Sync>, prompt: Arc<dyn Prompt>) -> Self {
        LazyMedia { data, configured, prompt, ready: Mutex::new(None) }
    }

    async fn get(&self) -> Result<Arc<FfmpegMedia>, PortError> {
        if let Some(m) = self.ready.lock().ok().and_then(|g| g.clone()) {
            return Ok(m);
        }
        let work = self.data.join("recordings-tmp");
        let paths = match laqta_media::find_ffmpeg(&(self.configured)(), &self.data) {
            Some(p) => p,
            None => {
                let ok = self
                    .prompt
                    .confirm(
                        "FFmpeg is needed",
                        "Recording and video tools use FFmpeg. Download the pinned FFmpeg 9.0.2 build now (about 190 MB)?",
                    )
                    .await?;
                if !ok {
                    return Err(PortError::Cancelled);
                }
                laqta_media::download_ffmpeg(&self.data, Arc::new(|_, _| {}), Arc::new(AtomicBool::new(false))).await?
            }
        };
        let media = Arc::new(FfmpegMedia::new(paths, work));
        if let Ok(mut g) = self.ready.lock() {
            *g = Some(media.clone());
        }
        Ok(media)
    }
}

impl Media for LazyMedia {
    fn start(
        &self,
        input: RecordingInput,
        spec: RecordSpec,
        output: PathBuf,
    ) -> BoxFuture<'_, Result<Box<dyn RecordingHandle>, PortError>> {
        Box::pin(async move {
            let m = self.get().await?;
            m.start(input, spec, output).await
        })
    }
    fn transcode(&self, input: &Path, output: &Path, spec: TranscodeSpec) -> BoxFuture<'_, Result<(), PortError>> {
        let (i, o) = (input.to_path_buf(), output.to_path_buf());
        Box::pin(async move {
            let m = self.get().await?;
            m.transcode(&i, &o, spec).await
        })
    }
    fn probe(&self, input: &Path) -> BoxFuture<'_, Result<MediaInfo, PortError>> {
        let i = input.to_path_buf();
        Box::pin(async move {
            let m = self.get().await?;
            m.probe(&i).await
        })
    }
}

/// FFmpeg, or the native Windows recorder when the settings choose it. A recording the
/// native recorder refuses (an area, another codec, the camera, custom arguments) or fails to
/// start reopens the screen stream and records with FFmpeg instead.
pub struct SwitchingMedia {
    pub ffmpeg: std::sync::Arc<dyn laqta_ports::Media>,
    pub native: Option<std::sync::Arc<dyn laqta_ports::Media>>,
    pub stream: std::sync::Arc<dyn laqta_ports::ScreenStream>,
    pub backend: std::sync::Arc<std::sync::RwLock<laqta_domain::record::RecorderBackend>>,
}

impl laqta_ports::Media for SwitchingMedia {
    fn start(
        &self,
        input: laqta_ports::RecordingInput,
        spec: laqta_domain::record::RecordSpec,
        output: std::path::PathBuf,
    ) -> laqta_ports::BoxFuture<'_, Result<Box<dyn laqta_ports::RecordingHandle>, laqta_ports::PortError>> {
        Box::pin(async move {
            let native = self.backend.read().map(|b| *b == laqta_domain::record::RecorderBackend::Native).unwrap_or(false);
            if let (true, Some(n)) = (native, &self.native) {
                match n.start(input, spec.clone(), output.clone()).await {
                    // A refusal, or a failure before the recording exists (no encoder, a lost device).
                    Err(laqta_ports::PortError::Unsupported(why) | laqta_ports::PortError::Failed(why)) => {
                        tracing::warn!("native recorder did not start ({why}); recording with FFmpeg");
                        let input = self.stream.open(spec.area, spec.show_cursor, spec.fps, &spec.audio).await?;
                        return self.ffmpeg.start(input, spec, output).await;
                    }
                    other => return other,
                }
            }
            self.ffmpeg.start(input, spec, output).await
        })
    }
    fn transcode(
        &self,
        input: &std::path::Path,
        output: &std::path::Path,
        spec: laqta_ports::TranscodeSpec,
    ) -> laqta_ports::BoxFuture<'_, Result<(), laqta_ports::PortError>> {
        self.ffmpeg.transcode(input, output, spec)
    }
    fn probe(&self, input: &std::path::Path) -> laqta_ports::BoxFuture<'_, Result<laqta_ports::MediaInfo, laqta_ports::PortError>> {
        self.ffmpeg.probe(input)
    }
}
