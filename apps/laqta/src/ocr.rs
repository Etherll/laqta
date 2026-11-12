//! OCR on first use (ADR 023): asks once, then downloads onnxruntime and the default PP-OCRv5
//! models through verified fetch and runs them with `laqta-ml`.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use laqta_domain::Bitmap;
use laqta_domain::settings::OcrEngine;
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
    /// The OS engine's name, as the download question names it.
    system_name: &'static str,
    /// The engine and the (det, rec, provider) it was built for.
    ready: ModelSlot<EngineKey, ModelOcr>,
    /// The user turned the download down and the OS engine reads instead. Not asked again until
    /// Laqta restarts; the models can still be installed from the settings.
    declined: AtomicBool,
}

/// What `LazyOcr` runs with the current settings, and what of it is not on disk yet.
struct Plan {
    key: EngineKey,
    dir: PathBuf,
    lib: PathBuf,
    ort: &'static laqta_ml::catalog::OrtArchive,
    runtime: Arc<Runtime>,
    /// Catalog entries to download; custom models are already on disk.
    wanted: Vec<&'static laqta_ml::catalog::ModelEntry>,
    /// A chosen custom model whose file is gone: nothing can download it.
    custom_missing: bool,
}

impl Plan {
    fn complete(&self) -> bool {
        self.lib.exists() && self.wanted.is_empty() && !self.custom_missing
    }
}

impl std::fmt::Debug for LazyOcr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LazyOcr").finish_non_exhaustive()
    }
}

fn ml(e: laqta_ml::error::MlError) -> PortError {
    PortError::Failed(e.to_string())
}

/// The OS engine reads when the OS has one and the user chose it or turned the models down.
fn reads_with_system(engine: OcrEngine, declined: bool, has_system: bool) -> bool {
    has_system && (engine == OcrEngine::System || declined)
}

impl LazyOcr {
    /// `ui` asks before a download and shows its progress as toasts.
    pub fn new<U: Prompt + Notifier + 'static>(
        data: PathBuf,
        ui: Arc<U>,
        options: crate::models::SharedModelOptions,
        fallback: Option<Arc<dyn Ocr>>,
        system_name: &'static str,
        releaser: Arc<ModelReleaser>,
    ) -> Self {
        LazyOcr {
            data,
            prompt: ui.clone(),
            options,
            notifier: ui,
            fallback,
            system_name,
            ready: ModelSlot::new(releaser),
            declined: AtomicBool::new(false),
        }
    }

    /// The OS engine reads instead of the models: chosen in settings, or the models were turned down
    /// and are still missing (once they are on disk, say from the settings page, they read again).
    fn uses_fallback(&self) -> bool {
        let engine = self.options.read().map(|o| o.ocr_engine).unwrap_or_default();
        let declined = self.declined.load(Ordering::Relaxed) && !self.plan().is_ok_and(|plan| plan.complete());
        reads_with_system(engine, declined, self.fallback.is_some())
    }

    fn plan(&self) -> Result<Plan, PortError> {
        let options = self.options.read().map(|o| o.clone()).unwrap_or_default();
        let det_default =
            catalog().default_for(Task::OcrDet).ok_or_else(|| PortError::Unsupported("no OCR detection model in the catalog".into()))?;
        let rec_default =
            catalog().default_for(Task::OcrRec).ok_or_else(|| PortError::Unsupported("no OCR recognition model in the catalog".into()))?;
        let det_id = options.ocr_det.clone().unwrap_or_else(|| det_default.id.clone());
        let rec_id = options.ocr_rec.clone().unwrap_or_else(|| rec_default.id.clone());
        let key = (det_id.clone(), rec_id.clone(), options.provider);
        let dir = self.data.join("models");
        let ort =
            laqta_ml::install::ort_for_host().ok_or_else(|| PortError::Unsupported("no onnxruntime build for this platform".into()))?;
        let lib = laqta_ml::install::runtime_library(&dir, ort);
        let runtime = Arc::new(Runtime::new(lib.clone(), dir.clone(), crate::models::provider(options.provider)));
        let wanted: Vec<_> =
            [&det_id, &rec_id].into_iter().filter_map(|id| catalog().model(id)).filter(|e| !runtime.is_installed(e)).collect();
        let custom_missing =
            [&det_id, &rec_id].into_iter().any(|id| catalog().model(id).is_none() && !dir.join(format!("{id}.onnx")).is_file());
        Ok(Plan { key, dir, lib, ort, runtime, wanted, custom_missing })
    }

    async fn get(&self) -> Result<Arc<ModelOcr>, PortError> {
        // Asked and answered already: the caller reads with the OS engine.
        if self.uses_fallback() {
            return Err(PortError::Cancelled);
        }
        let Plan { key, dir, lib, ort: ort_entry, runtime, wanted, .. } = self.plan()?;
        if let Some(o) = self.ready.get(&key) {
            return Ok(o);
        }
        let (det_id, rec_id) = (key.0.clone(), key.1.clone());
        if !lib.exists() || !wanted.is_empty() {
            let mb = (if lib.exists() { 0 } else { ort_entry.size_bytes } + wanted.iter().map(|e| e.size_bytes).sum::<u64>()) / 1_000_000;
            let ok = self
                .prompt
                .confirm(
                    TITLE,
                    &if self.fallback.is_some() {
                        format!("Download ONNX Runtime and the text models (about {mb} MB) for better accuracy? Choose No to use {} instead. You can change this in Settings → AI features.", self.system_name)
                    } else {
                        format!("Text recognition needs ONNX Runtime and its models (about {mb} MB). Download them now?")
                    },
                )
                .await?;
            if !ok {
                self.declined.store(true, Ordering::Relaxed);
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
    /// True once nothing stands between a call and its answer: the models are on disk, or the user
    /// chose the OS engine. Until then `recognize` opens the download question first.
    fn ready(&self) -> bool {
        self.uses_fallback() || self.plan().is_ok_and(|plan| plan.complete())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_os_engine_reads_only_when_present_and_chosen_or_declined() {
        // (engine, declined, has_system, reads)
        let cases = [
            (OcrEngine::Models, false, false, false),
            (OcrEngine::Models, true, false, false),
            (OcrEngine::System, false, false, false),
            (OcrEngine::System, true, false, false),
            (OcrEngine::Models, false, true, false),
            (OcrEngine::Models, true, true, true),
            (OcrEngine::System, false, true, true),
            (OcrEngine::System, true, true, true),
        ];
        for (engine, declined, has_system, reads) in cases {
            assert_eq!(reads_with_system(engine, declined, has_system), reads, "{engine:?}, declined {declined}, system {has_system}");
        }
    }
}
