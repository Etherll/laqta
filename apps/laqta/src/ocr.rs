//! OCR on first use (ADR 023, M5 D2): asks once, then downloads onnxruntime and the default PP-OCRv5
//! models through verified fetch and runs them with `laqta-ml`.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use laqta_domain::Bitmap;
use laqta_ml::catalog::{Task, catalog};
use laqta_ml::ocr::ModelOcr;
use laqta_ml::runtime::{ExecutionProvider, Runtime};
use laqta_ports::{BoxFuture, Ocr, OcrResult, PortError, Prompt};

pub struct LazyOcr {
    data: PathBuf,
    prompt: Arc<dyn Prompt>,
    ready: Mutex<Option<Arc<ModelOcr>>>,
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
    pub fn new(data: PathBuf, prompt: Arc<dyn Prompt>) -> Self {
        LazyOcr { data, prompt, ready: Mutex::new(None) }
    }

    async fn get(&self) -> Result<Arc<ModelOcr>, PortError> {
        if let Some(o) = self.ready.lock().ok().and_then(|g| g.clone()) {
            return Ok(o);
        }
        let dir = self.data.join("models");
        let det = catalog().default_for(Task::OcrDet).ok_or_else(|| PortError::Unsupported("no OCR detection model in the catalog".into()))?;
        let rec = catalog().default_for(Task::OcrRec).ok_or_else(|| PortError::Unsupported("no OCR recognition model in the catalog".into()))?;
        let ort_entry = laqta_ml::install::ort_for_host().ok_or_else(|| PortError::Unsupported("no onnxruntime build for this platform".into()))?;
        let lib = laqta_ml::install::library_path(&dir, ort_entry);
        let runtime = Arc::new(Runtime::new(lib.clone(), dir.clone(), ExecutionProvider::Auto));
        let installed = lib.exists() && runtime.is_installed(det) && runtime.is_installed(rec);
        if !installed {
            let mb = (ort_entry.size_bytes + det.size_bytes + rec.size_bytes) / 1_000_000;
            let ok = self.prompt.confirm("Text recognition", &format!("Text recognition needs ONNX Runtime and the PP-OCRv5 models (about {mb} MB). Download them now?")).await?;
            if !ok {
                return Err(PortError::Cancelled);
            }
            let none: laqta_ml::runtime::Progress = Arc::new(|_, _| {});
            laqta_ml::install::install_onnxruntime(&dir, none.clone(), Arc::new(AtomicBool::new(false))).await.map_err(ml)?;
            for entry in [det, rec] {
                if !runtime.is_installed(entry) {
                    runtime.install(entry, none.clone(), Arc::new(AtomicBool::new(false))).await.map_err(ml)?;
                }
            }
        }
        let ocr = Arc::new(ModelOcr::new(runtime, det.id.clone(), rec.id.clone()));
        if let Ok(mut g) = self.ready.lock() {
            *g = Some(ocr.clone());
        }
        Ok(ocr)
    }
}

impl Ocr for LazyOcr {
    fn recognize(&self, image: Bitmap, language: String) -> BoxFuture<'_, Result<OcrResult, PortError>> {
        Box::pin(async move {
            let o = self.get().await?;
            o.recognize(image, language).await
        })
    }
    fn languages(&self) -> Vec<String> {
        self.ready.lock().ok().and_then(|g| g.as_ref().map(|o| o.languages())).unwrap_or_default()
    }
}
