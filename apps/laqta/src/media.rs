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
    /// The resolved FFmpeg, with the configured path it was resolved from.
    ready: Mutex<Option<(String, Arc<FfmpegMedia>)>>,
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
        // A changed path in the settings resolves again, so the next recording uses it.
        let configured = (self.configured)();
        if let Some((from, m)) = self.ready.lock().ok().and_then(|g| g.clone())
            && from == configured
        {
            return Ok(m);
        }
        let work = self.data.join("recordings-tmp");
        let paths = match laqta_media::find_ffmpeg(&configured, &self.data) {
            Some(p) => p,
            // No pinned build to download (macOS): say how to get one, not offer a download.
            None if !laqta_media::can_download_ffmpeg() => {
                return Err(PortError::Failed(
                    "Recording and video tools need FFmpeg, and none was found. Install it (with Homebrew: brew install ffmpeg), \
                     or choose its location in Settings > Recording."
                        .into(),
                ));
            }
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
            *g = Some((configured, media.clone()));
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
}

impl laqta_ports::Media for SwitchingMedia {
    fn start(
        &self,
        input: laqta_ports::RecordingInput,
        spec: laqta_domain::record::RecordSpec,
        output: std::path::PathBuf,
    ) -> laqta_ports::BoxFuture<'_, Result<Box<dyn laqta_ports::RecordingHandle>, laqta_ports::PortError>> {
        Box::pin(async move {
            let native = spec.backend == laqta_domain::record::RecorderBackend::Native;
            if let (true, Some(n)) = (native, &self.native) {
                match n.start(input, spec.clone(), output.clone()).await {
                    // A refusal: the recording needs something only FFmpeg does.
                    Err(laqta_ports::PortError::Unsupported(why)) => {
                        tracing::info!("native recorder declined ({why}); recording with FFmpeg");
                        let input = self.stream.open(spec.area, spec.show_cursor, spec.fps, &spec.audio).await?;
                        return self.ffmpeg.start(input, spec, output).await;
                    }
                    // A failure before the recording exists (no encoder, a lost device): FFmpeg gets a
                    // try, and if it fails too the error keeps the native recorder's cause.
                    Err(laqta_ports::PortError::Failed(why)) => {
                        tracing::warn!("native recorder failed to start ({why}); recording with FFmpeg");
                        let input = self.stream.open(spec.area, spec.show_cursor, spec.fps, &spec.audio).await?;
                        return match self.ffmpeg.start(input, spec, output).await {
                            Err(laqta_ports::PortError::Failed(ffmpeg)) => {
                                Err(laqta_ports::PortError::Failed(format!("{why}. FFmpeg could not record either: {ffmpeg}")))
                            }
                            other => other,
                        };
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

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use laqta_ports::BoxFuture;

    use super::*;

    struct NoPrompt;

    impl Prompt for NoPrompt {
        fn ask_text(&self, _: &str, _: &str) -> BoxFuture<'_, Result<String, PortError>> {
            Box::pin(async { Err(PortError::Cancelled) })
        }
        fn choose(&self, _: &str, _: &[String]) -> BoxFuture<'_, Result<usize, PortError>> {
            Box::pin(async { Err(PortError::Cancelled) })
        }
        fn show_text(&self, _: &str, _: &str) -> BoxFuture<'_, Result<(), PortError>> {
            Box::pin(async { Err(PortError::Cancelled) })
        }
        fn confirm(&self, _: &str, _: &str) -> BoxFuture<'_, Result<bool, PortError>> {
            Box::pin(async { Err(PortError::Cancelled) })
        }
    }

    /// A folder holding stand-ins for both binaries; nothing runs them.
    fn fake_ffmpeg(root: &Path, name: &str) -> PathBuf {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        for exe in ["ffmpeg", "ffprobe"] {
            let file = if cfg!(windows) { format!("{exe}.exe") } else { exe.to_owned() };
            std::fs::write(dir.join(file), b"").unwrap();
        }
        dir
    }

    /// A saved FFmpeg path replaces the one resolved earlier, and an unchanged path keeps it.
    #[test]
    fn a_changed_ffmpeg_path_resolves_again() {
        let root = tempfile::tempdir().unwrap();
        let (first, second) = (fake_ffmpeg(root.path(), "first"), fake_ffmpeg(root.path(), "second"));
        let setting = Arc::new(Mutex::new(first.to_string_lossy().into_owned()));
        let read = setting.clone();
        let media = LazyMedia::new(root.path().join("data"), Arc::new(move || read.lock().unwrap().clone()), Arc::new(NoPrompt));
        let ffmpeg_dir = |m: Arc<FfmpegMedia>| m.paths().ffmpeg.parent().unwrap().to_path_buf();

        let a = futures_lite::future::block_on(media.get()).unwrap();
        assert_eq!(ffmpeg_dir(a.clone()), first);
        let again = futures_lite::future::block_on(media.get()).unwrap();
        assert!(Arc::ptr_eq(&a, &again), "an unchanged path reuses the resolved FFmpeg");

        *setting.lock().unwrap() = second.to_string_lossy().into_owned();
        let b = futures_lite::future::block_on(media.get()).unwrap();
        assert_eq!(ffmpeg_dir(b), second);
    }
}
