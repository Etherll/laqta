//! OCR on first use (ADR 023): asks once, then downloads onnxruntime and the default PP-OCRv5
//! models through verified fetch and runs them with `laqta-ml`.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use laqta_domain::Bitmap;
use laqta_ml::catalog::{Task, catalog};
use laqta_ml::ocr::ModelOcr;
use laqta_ml::runtime::Runtime;
use laqta_ports::{BoxFuture, Notifier, Ocr, OcrResult, PortError, Prompt};

use crate::models::{ModelReleaser, ModelSlot};

/// The title of the download prompt and of the progress toasts.
const TITLE: &str = "Text recognition";

/// The detection model, recognition model and provider an engine was built for.
type EngineKey = (String, String, laqta_domain::settings::ModelProvider);

pub struct LazyOcr {
    data: PathBuf,
    prompt: Arc<dyn Prompt>,
    options: crate::models::SharedModelOptions,
    /// Where the first-use download progress is reported, as toasts.
    notifier: Arc<dyn Notifier>,
    /// The OS engine (Windows.Media.Ocr) used when the models are declined.
    fallback: Option<Arc<dyn Ocr>>,
    /// The engine and the (det, rec, provider) it was built for.
    ready: ModelSlot<EngineKey, ModelOcr>,
}

impl std::fmt::Debug for LazyOcr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LazyOcr").finish_non_exhaustive()
    }
}

fn ml(e: laqta_ml::error::MlError) -> PortError {
    PortError::Failed(e.to_string())
}

impl LazyOcr {
    /// `ui` asks before a download and shows its progress as toasts.
    pub fn new<U: Prompt + Notifier + 'static>(
        data: PathBuf,
        ui: Arc<U>,
        options: crate::models::SharedModelOptions,
        fallback: Option<Arc<dyn Ocr>>,
        releaser: Arc<ModelReleaser>,
    ) -> Self {
        LazyOcr { data, prompt: ui.clone(), options, notifier: ui, fallback, ready: ModelSlot::new(releaser) }
    }

    async fn get(&self) -> Result<Arc<ModelOcr>, PortError> {
        let options = self.options.read().map(|o| o.clone()).unwrap_or_default();
        let det_default =
            catalog().default_for(Task::OcrDet).ok_or_else(|| PortError::Unsupported("no OCR detection model in the catalog".into()))?;
        let rec_default =
            catalog().default_for(Task::OcrRec).ok_or_else(|| PortError::Unsupported("no OCR recognition model in the catalog".into()))?;
        let det_id = options.ocr_det.clone().unwrap_or_else(|| det_default.id.clone());
        let rec_id = options.ocr_rec.clone().unwrap_or_else(|| rec_default.id.clone());
        let key = (det_id.clone(), rec_id.clone(), options.provider);
        if let Some(o) = self.ready.get(&key) {
            return Ok(o);
        }
        let dir = self.data.join("models");
        let ort_entry =
            laqta_ml::install::ort_for_host().ok_or_else(|| PortError::Unsupported("no onnxruntime build for this platform".into()))?;
        let lib = laqta_ml::install::library_path(&dir, ort_entry);
        let runtime = Arc::new(Runtime::new(lib.clone(), dir.clone(), crate::models::provider(options.provider)));
        // Catalog entries to download; custom models are already on disk.
        let wanted: Vec<_> =
            [&det_id, &rec_id].into_iter().filter_map(|id| catalog().model(id)).filter(|e| !runtime.is_installed(e)).collect();
        if !lib.exists() || !wanted.is_empty() {
            let mb = (if lib.exists() { 0 } else { ort_entry.size_bytes } + wanted.iter().map(|e| e.size_bytes).sum::<u64>()) / 1_000_000;
            let ok = self
                .prompt
                .confirm(
                    TITLE,
                    &if self.fallback.is_some() {
                        format!("Download ONNX Runtime and the text models (about {mb} MB) for better accuracy? Choose No to use Windows' built-in OCR.")
                    } else {
                        format!("Text recognition needs ONNX Runtime and its models (about {mb} MB). Download them now?")
                    },
                )
                .await?;
            if !ok {
                return Err(PortError::Cancelled);
            }
            let notifier = &self.notifier;
            if !lib.exists() {
                let progress = crate::models::toast_progress(notifier, TITLE, "ONNX Runtime");
                laqta_ml::install::install_onnxruntime(&dir, progress, Arc::new(AtomicBool::new(false))).await.map_err(ml)?;
            }
            for entry in wanted {
                let progress = crate::models::toast_progress(notifier, TITLE, &entry.name);
                runtime.install(entry, progress, Arc::new(AtomicBool::new(false))).await.map_err(ml)?;
            }
        }
        let ocr = Arc::new(ModelOcr::new(runtime.clone(), det_id, rec_id));
        self.ready.store(key, ocr.clone(), runtime);
        Ok(ocr)
    }
}

impl Ocr for LazyOcr {
    fn recognize(&self, image: Bitmap, language: String) -> BoxFuture<'_, Result<OcrResult, PortError>> {
        Box::pin(async move {
            match self.get().await {
                Ok(o) => {
                    let result = o.recognize(image, language).await;
                    self.ready.ran();
                    result
                }
                Err(PortError::Cancelled) if let Some(f) = &self.fallback => f.recognize(image, language).await,
                Err(e) => Err(e),
            }
        })
    }
    fn languages(&self) -> Vec<String> {
        let mut l = self.ready.peek().map(|o| o.languages()).unwrap_or_default();
        if let Some(f) = &self.fallback {
            l.extend(f.languages());
        }
        l
    }
}
