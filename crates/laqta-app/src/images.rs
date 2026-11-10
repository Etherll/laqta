//! Image files read and decoded for the app. The file read goes through `files`; the decode and the
//! thumbnail run on the compute pool.

use std::path::PathBuf;

use laqta_domain::Bitmap;
use laqta_ports::AppError;

use crate::dispatcher::App;
use crate::files;
use crate::task::compute;

impl App {
    /// Reads the image file at `path` and decodes it.
    pub async fn decode_file(&self, path: PathBuf) -> Result<Bitmap, AppError> {
        let bytes = files::read(self.ports.spawner.as_ref(), path).await.map_err(|error| AppError::Io(error.to_string()))?;
        let codecs = self.ports.codecs.clone();
        let work = move || -> Result<Bitmap, AppError> { codecs.decode(&bytes).map_err(|error| AppError::Io(error.to_string())) };
        compute(self.ports.spawner.as_ref(), work).await
    }

    /// Reads the image file at `path` and returns it scaled to fit `max_side` by `max_side`, keeping its aspect ratio.
    pub async fn thumbnail_file(&self, path: PathBuf, max_side: u32) -> Result<Bitmap, AppError> {
        let bytes = files::read(self.ports.spawner.as_ref(), path).await.map_err(|error| AppError::Io(error.to_string()))?;
        let codecs = self.ports.codecs.clone();
        let work = move || -> Result<Bitmap, AppError> {
            let decoded = codecs.decode(&bytes).map_err(|error| AppError::Io(error.to_string()))?;
            Ok(laqta_imaging::thumbnail(&decoded, max_side, max_side))
        };
        compute(self.ports.spawner.as_ref(), work).await
    }

    /// Puts `image` on the clipboard through the `Clipboard` port, as a capture's copy step does, so
    /// every program that pastes pictures can paste it. The port call blocks, so it runs on the
    /// blocking pool.
    pub async fn copy_image(&self, image: Bitmap) -> Result<(), AppError> {
        let clipboard = self.ports.clipboard.clone();
        let (tx, rx) = futures_channel::oneshot::channel();
        self.ports
            .spawner
            .run_blocking(Box::new(move || {
                let _ = tx.send(clipboard.set_image(&image));
            }))
            .await;
        rx.await.map_err(|_| AppError::Io("the clipboard call did not finish".to_owned()))?.map_err(AppError::from)
    }

    /// Reads the image file at `path` and puts the picture on the clipboard.
    pub async fn copy_image_file(&self, path: PathBuf) -> Result<(), AppError> {
        let image = self.decode_file(path).await?;
        self.copy_image(image).await
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::disallowed_methods, clippy::disallowed_types, clippy::unwrap_used)]

    use std::sync::Arc;

    use futures_lite::future::block_on;
    use laqta_domain::{Bitmap, PixelFormat, Settings};
    use laqta_ports::fakes::{
        ClipboardBehaviour, CountingRng, FakeClipboard, FakeHoverSource, FakeNotifier, FakeOverlay, FakeScreen, FakeShell, InlineSpawner,
        ManualClock,
    };
    use laqta_ports::{AppError, HostInfo};
    use tempfile::TempDir;

    use super::App;
    use crate::dispatcher::Ports;

    fn in_compute_pool<R: Send>(body: impl FnOnce() -> R + Send) -> R {
        rayon::ThreadPoolBuilder::new().num_threads(2).build().unwrap().install(body)
    }

    fn app() -> Arc<App> {
        App::new(
            Ports {
                screen: Arc::new(FakeScreen::new(Vec::new())),
                clipboard: Arc::new(FakeClipboard::new(ClipboardBehaviour::Succeed)),
                shell: Arc::new(FakeShell::default()),
                clock: Arc::new(ManualClock::new(jiff::civil::date(2026, 11, 7).at(14, 3, 22, 0))),
                spawner: Arc::new(InlineSpawner::default()),
                overlay: Arc::new(FakeOverlay::default()),
                tools: Arc::new(laqta_ports::fakes::FakeToolHost::default()),
                prompt: Arc::new(laqta_ports::fakes::FakePrompt::default()),
                editor: Arc::new(laqta_ports::fakes::FakeEditorHost::default()),
                media: Arc::new(laqta_ports::fakes::FakeMedia::default()),
                stream: Arc::new(laqta_ports::fakes::FakeScreenStream::default()),
                ocr: Arc::new(laqta_ports::fakes::FakeOcr::default()),
                window_ops: Arc::new(laqta_ports::fakes::FakeWindowOps::default()),
                input: Arc::new(laqta_ports::fakes::FakeInputSynth::default()),
                highlighter: Arc::new(laqta_ports::fakes::FakeHighlighter::default()),
                background: Arc::new(laqta_ports::fakes::FakeBackgroundRemover::default()),
                analyzer: Arc::new(laqta_ports::fakes::FakeAnalyzer::default()),
                inpainter: Arc::new(laqta_ports::fakes::FakeInpainter),
                watcher: Arc::new(laqta_ports::fakes::FakeFileWatcher::default()),
                uploaders: Arc::new(laqta_ports::fakes::FakeUploaders::default()),
                hover: Arc::new(FakeHoverSource::new(Vec::new())),
                notifier: Arc::new(FakeNotifier::default()),
                rng: Arc::new(CountingRng::default()),
                host: HostInfo::default(),
                codecs: laqta_imaging::Codecs::builtin(),
            },
            Settings::default(),
        )
    }

    fn solid(width: u32, height: u32) -> Bitmap {
        let pixels: Vec<u8> = std::iter::repeat_n([10, 20, 30, 255], width as usize * height as usize).flatten().collect();
        Bitmap::new(width, height, PixelFormat::Rgba8, pixels.into()).unwrap()
    }

    #[test]
    fn decode_file_round_trips_a_png() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("pixels.png");
        let pixels: Vec<u8> = (0..24).collect();
        let original = Bitmap::new(3, 2, PixelFormat::Rgba8, pixels.into()).unwrap();
        let png = in_compute_pool(|| laqta_imaging::encode_png(&original).unwrap());
        std::fs::write(&path, png).unwrap();

        let decoded = in_compute_pool(|| block_on(app().decode_file(path))).unwrap();

        assert_eq!((decoded.width(), decoded.height()), (3, 2));
        assert_eq!(decoded.pixels(), original.pixels());
    }

    #[test]
    fn thumbnail_file_fits_the_side_and_keeps_the_aspect() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("wide.png");
        let png = in_compute_pool(|| laqta_imaging::encode_png(&solid(4000, 3000)).unwrap());
        std::fs::write(&path, png).unwrap();

        let thumb = in_compute_pool(|| block_on(app().thumbnail_file(path, 256))).unwrap();

        assert_eq!((thumb.width(), thumb.height()), (256, 192));
    }

    #[test]
    fn thumbnail_file_keeps_an_image_that_already_fits() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("small.png");
        let png = in_compute_pool(|| laqta_imaging::encode_png(&solid(4, 3)).unwrap());
        std::fs::write(&path, png).unwrap();

        let thumb = in_compute_pool(|| block_on(app().thumbnail_file(path, 256))).unwrap();

        assert_eq!((thumb.width(), thumb.height()), (4, 3));
    }

    #[test]
    fn missing_file_is_an_io_error() {
        let dir = TempDir::new().unwrap();
        let missing = dir.path().join("missing.png");

        let result = in_compute_pool(|| block_on(app().decode_file(missing)));

        assert!(matches!(result, Err(AppError::Io(_))));
    }

    #[test]
    fn copy_image_reaches_the_clipboard_port() {
        let image = Bitmap::new(1, 1, PixelFormat::Rgba8, Arc::from(vec![0u8; 4])).unwrap();
        assert!(block_on(app().copy_image(image)).is_ok());
    }
}
