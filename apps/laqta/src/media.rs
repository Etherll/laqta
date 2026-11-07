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
