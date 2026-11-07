//! Local background removal and inpainting (M7 D4, D6): the chosen catalog or custom model,
//! downloaded with ONNX Runtime on first use after the user agrees, then run through `laqta-ml`.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use laqta_domain::Bitmap;
use laqta_ml::{ModelBackground, ModelInpainter, Task, catalog};
use laqta_ports::{BackgroundRemover, BoxFuture, Inpainter, PortError, Prompt};

use crate::models::SharedModelOptions;

/// Downloads ONNX Runtime and `id` when missing (after asking), and returns a runtime for it.
async fn prepare(
    data: &std::path::Path,
    options: &SharedModelOptions,
    prompt: &Arc<dyn Prompt>,
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
        crate::models::ensure_onnxruntime(data).await.map_err(PortError::Failed)?;
        if let Some(e) = entry.filter(|_| !model_ok) {
            let none: laqta_ml::Progress = Arc::new(|_, _| {});
            rt.install(e, none, Arc::new(AtomicBool::new(false))).await.map_err(|e| PortError::Failed(e.to_string()))?;
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
    ready: Mutex<Option<(String, Arc<ModelBackground>)>>,
}

impl LocalBackground {
    pub fn new(data: PathBuf, options: SharedModelOptions, prompt: Arc<dyn Prompt>) -> Self {
        LocalBackground { data, options, prompt, ready: Mutex::new(None) }
    }
}

impl BackgroundRemover for LocalBackground {
    fn remove(&self, image: Bitmap) -> BoxFuture<'_, Result<Bitmap, PortError>> {
        Box::pin(async move {
            let chosen = self.options.read().ok().and_then(|o| o.background.clone());
            let id = match chosen {
                Some(id) => id,
                None => default_id(Task::Background)?,
            };
            let cached = self.ready.lock().ok().and_then(|g| g.clone()).filter(|(k, _)| *k == id);
            let engine = match cached {
                Some((_, e)) => e,
                None => {
                    let rt = prepare(&self.data, &self.options, &self.prompt, &id, "Background removal").await?;
                    // A custom model (not in the catalog) runs with the default model's numbers.
                    let e = if catalog().model(&id).is_some() {
                        Arc::new(ModelBackground::new(rt, id.clone()))
                    } else {
                        let params = ModelBackground::default_params()
                            .ok_or_else(|| PortError::Unsupported("no background model in the catalog".into()))?;
                        Arc::new(ModelBackground::custom(rt, id.clone(), params))
                    };
                    if let Ok(mut g) = self.ready.lock() {
                        *g = Some((id, e.clone()));
                    }
                    e
                }
            };
            engine.remove(image).await
        })
    }
}

pub struct LocalInpainter {
    pub data: PathBuf,
    pub options: SharedModelOptions,
    pub prompt: Arc<dyn Prompt>,
    ready: Mutex<Option<Arc<ModelInpainter>>>,
}

impl LocalInpainter {
    pub fn new(data: PathBuf, options: SharedModelOptions, prompt: Arc<dyn Prompt>) -> Self {
        LocalInpainter { data, options, prompt, ready: Mutex::new(None) }
    }
}

impl Inpainter for LocalInpainter {
    fn inpaint(&self, image: Bitmap, mask: Vec<u8>) -> BoxFuture<'_, Result<Bitmap, PortError>> {
        Box::pin(async move {
            let engine = match self.ready.lock().ok().and_then(|g| g.clone()) {
                Some(e) => e,
                None => {
                    let id = default_id(Task::Inpaint)?;
                    let rt = prepare(&self.data, &self.options, &self.prompt, &id, "Inpainting").await?;
                    let e = Arc::new(ModelInpainter::new(rt, id));
                    if let Ok(mut g) = self.ready.lock() {
                        *g = Some(e.clone());
                    }
                    e
                }
            };
            engine.inpaint(image, mask).await
        })
    }
}
