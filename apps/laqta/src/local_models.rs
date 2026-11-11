//! Local background removal and inpainting: the chosen catalog or custom model,
//! downloaded with ONNX Runtime on first use after the user agrees, then run through `laqta-ml`.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use laqta_domain::Bitmap;
use laqta_domain::settings::ModelProvider;
use laqta_ml::{ModelBackground, ModelInpainter, Task, catalog};
use laqta_ports::{BackgroundRemover, BoxFuture, Inpainter, Notifier, PortError, Prompt};

use crate::models::{ModelReleaser, ModelSlot, SharedModelOptions};

/// Downloads ONNX Runtime and `id` when missing (after asking), and returns a runtime for it. The
/// downloads report their progress through `notifier`.
async fn prepare(
    data: &std::path::Path,
    options: &SharedModelOptions,
    prompt: &Arc<dyn Prompt>,
    notifier: &Arc<dyn Notifier>,
    id: &str,
    what: &str,
) -> Result<Arc<laqta_ml::Runtime>, PortError> {
    let opts = options.read().map(|o| o.clone()).unwrap_or_default();
    let rt = crate::models::runtime(data, &opts).ok_or_else(|| PortError::Unsupported("no ONNX Runtime build for this platform".into()))?;
    let dir = crate::models::models_dir(data);
    let ort = laqta_ml::install::ort_for_host().ok_or_else(|| PortError::Unsupported("no ONNX Runtime build for this platform".into()))?;
    let lib_ok = laqta_ml::install::library_path(&dir, ort).exists();
    let entry = catalog().model(id);
    let model_ok = match entry {
        Some(e) => rt.is_installed(e),
        None => rt.model_path(id).is_file(), // a custom model
    };
    if !lib_ok || !model_ok {
        let mb = (if lib_ok { 0 } else { ort.size_bytes } + entry.filter(|_| !model_ok).map_or(0, |e| e.size_bytes)) / 1_000_000;
        let ok = prompt.confirm(what, &format!("{what} needs ONNX Runtime and its model (about {mb} MB). Download them now?")).await?;
        if !ok {
            return Err(PortError::Cancelled);
        }
        crate::models::ensure_onnxruntime(data, crate::models::toast_progress(notifier, what, "ONNX Runtime"))
            .await
            .map_err(PortError::Failed)?;
        if let Some(e) = entry.filter(|_| !model_ok) {
            let progress = crate::models::toast_progress(notifier, what, &e.name);
            rt.install(e, progress, Arc::new(AtomicBool::new(false))).await.map_err(|e| PortError::Failed(e.to_string()))?;
        }
    }
    Ok(rt)
}

fn default_id(task: Task) -> Result<String, PortError> {
    catalog()
        .default_for(task)
        .map(|e| e.id.clone())
        .ok_or_else(|| PortError::Unsupported(format!("no {} model in the catalog", task.as_str())))
}

pub struct LocalBackground {
    pub data: PathBuf,
    pub options: SharedModelOptions,
    pub prompt: Arc<dyn Prompt>,
    notifier: Arc<dyn Notifier>,
    /// The engine and the (model, provider) it was built for.
    ready: ModelSlot<(String, ModelProvider), ModelBackground>,
}

impl LocalBackground {
    /// `ui` asks before a download and shows its progress as toasts.
    pub fn new<U: Prompt + Notifier + 'static>(
        data: PathBuf,
        options: SharedModelOptions,
        ui: Arc<U>,
        releaser: Arc<ModelReleaser>,
    ) -> Self {
        LocalBackground { data, options, prompt: ui.clone(), notifier: ui, ready: ModelSlot::new(releaser) }
    }
}

/// The provider chosen in the settings now.
fn provider(options: &SharedModelOptions) -> ModelProvider {
    options.read().map(|o| o.provider).unwrap_or_default()
}

impl BackgroundRemover for LocalBackground {
    fn remove(&self, image: Bitmap) -> BoxFuture<'_, Result<Bitmap, PortError>> {
        Box::pin(async move {
            let chosen = self.options.read().ok().and_then(|o| o.background.clone());
            let id = match chosen {
                Some(id) => id,
                None => default_id(Task::Background)?,
            };
            let key = (id.clone(), provider(&self.options));
            let engine = match self.ready.get(&key) {
                Some(e) => e,
                None => {
                    let rt = prepare(&self.data, &self.options, &self.prompt, &self.notifier, &id, "Background removal").await?;
                    // A custom model (not in the catalog) runs with the default model's numbers.
                    let e = if catalog().model(&id).is_some() {
                        Arc::new(ModelBackground::new(rt.clone(), id.clone()))
                    } else {
                        let params = ModelBackground::default_params()
                            .ok_or_else(|| PortError::Unsupported("no background model in the catalog".into()))?;
                        Arc::new(ModelBackground::custom(rt.clone(), id.clone(), params))
                    };
                    self.ready.store(key, e.clone(), rt);
                    e
                }
            };
            let result = engine.remove(image).await;
            self.ready.ran();
            result
        })
    }
}

pub struct LocalInpainter {
    pub data: PathBuf,
    pub options: SharedModelOptions,
    pub prompt: Arc<dyn Prompt>,
    notifier: Arc<dyn Notifier>,
    /// The engine and the (model, provider) it was built for.
    ready: ModelSlot<(String, ModelProvider), ModelInpainter>,
}

impl LocalInpainter {
    /// `ui` asks before a download and shows its progress as toasts.
    pub fn new<U: Prompt + Notifier + 'static>(
        data: PathBuf,
        options: SharedModelOptions,
        ui: Arc<U>,
        releaser: Arc<ModelReleaser>,
    ) -> Self {
        LocalInpainter { data, options, prompt: ui.clone(), notifier: ui, ready: ModelSlot::new(releaser) }
    }
}

impl Inpainter for LocalInpainter {
    fn inpaint(&self, image: Bitmap, mask: Vec<u8>) -> BoxFuture<'_, Result<Bitmap, PortError>> {
        Box::pin(async move {
            let key = (default_id(Task::Inpaint)?, provider(&self.options));
            let engine = match self.ready.get(&key) {
                Some(e) => e,
                None => {
                    let rt = prepare(&self.data, &self.options, &self.prompt, &self.notifier, &key.0, "Inpainting").await?;
                    let e = Arc::new(ModelInpainter::new(rt.clone(), key.0.clone()));
                    self.ready.store(key, e.clone(), rt);
                    e
                }
            };
            let result = engine.inpaint(image, mask).await;
            self.ready.ran();
            result
        })
    }
}
