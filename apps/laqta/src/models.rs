//! The model manager's service (ADR 023): catalog and custom models, downloads, removal and
//! custom-model validation, over `laqta-ml`.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use laqta_domain::settings::{CustomModel, ModelOptions, ModelProvider};
use laqta_ml::families::Family;
use laqta_ml::runtime::{ExecutionProvider, Runtime};
use laqta_ml::{Task, catalog};
use laqta_ports::BoxFuture;
use laqta_ui::{ModelProgress, ModelRow, ModelService};

/// The settings' model choices, shared with the OCR adapter and updated on save.
pub type SharedModelOptions = Arc<std::sync::RwLock<ModelOptions>>;

pub fn provider(p: ModelProvider) -> ExecutionProvider {
    match p {
        ModelProvider::Auto => ExecutionProvider::Auto,
        ModelProvider::Cpu => ExecutionProvider::Cpu,
        ModelProvider::Gpu => ExecutionProvider::Gpu,
    }
}

pub fn models_dir(data: &Path) -> PathBuf {
    data.join("models")
}

/// A runtime over `<data>/models` with the chosen provider. ONNX Runtime may still be missing.
pub fn runtime(data: &Path, options: &ModelOptions) -> Option<Arc<Runtime>> {
    let dir = models_dir(data);
    let ort = laqta_ml::install::ort_for_host()?;
    Some(Arc::new(Runtime::new(laqta_ml::install::library_path(&dir, ort), dir, provider(options.provider))))
}

/// Downloads ONNX Runtime when it is not there yet.
pub async fn ensure_onnxruntime(data: &Path) -> Result<(), String> {
    let dir = models_dir(data);
    let ort = laqta_ml::install::ort_for_host().ok_or("no ONNX Runtime build for this platform")?;
    if laqta_ml::install::library_path(&dir, ort).exists() {
        return Ok(());
    }
    let none: laqta_ml::Progress = Arc::new(|_, _| {});
    laqta_ml::install::install_onnxruntime(&dir, none, Arc::new(AtomicBool::new(false))).await.map(|_| ()).map_err(|e| e.to_string())
}

pub struct Models {
    pub data: PathBuf,
    pub options: SharedModelOptions,
}

impl Models {
    fn options(&self) -> ModelOptions {
        self.options.read().map(|o| o.clone()).unwrap_or_default()
    }
}

impl ModelService for Models {
    fn rows(&self) -> Vec<ModelRow> {
        let options = self.options();
        let rt = runtime(&self.data, &options);
        let mut rows: Vec<ModelRow> = [Task::OcrDet, Task::OcrRec, Task::Background, Task::Inpaint]
            .into_iter()
            .flat_map(|t| catalog().models_for(t))
            .map(|e| ModelRow {
                id: e.id.clone(),
                task: e.task.as_str().to_owned(),
                name: e.name.clone(),
                description: e.description.clone(),
                size_bytes: e.size_bytes,
                licence: e.licence.clone(),
                commercial: e.commercial,
                installed: rt.as_ref().is_some_and(|r| r.is_installed(e)),
                custom: false,
                available: !e.is_pending(),
            })
            .collect();
        let dir = models_dir(&self.data);
        rows.extend(options.custom.iter().map(|c| ModelRow {
            id: c.id.clone(),
            task: c.task.clone(),
            name: c.name.clone(),
            description: format!("Custom model ({})", c.family),
            size_bytes: std::fs::metadata(dir.join(format!("{}.onnx", c.id))).map(|m| m.len()).unwrap_or(0),
            licence: String::new(),
            commercial: true,
            installed: dir.join(format!("{}.onnx", c.id)).is_file(),
            custom: true,
            available: true,
        }));
        rows
    }

    fn families(&self) -> Vec<(String, String)> {
        Family::ALL.iter().map(|f| (f.task().as_str().to_owned(), f.name().to_owned())).collect()
    }

    fn install(&self, id: String, progress: ModelProgress) -> BoxFuture<'static, Result<(), String>> {
        let data = self.data.clone();
        let options = self.options();
        Box::pin(async move {
            let entry = catalog().model(&id).ok_or_else(|| format!("no model {id}"))?;
            let rt = runtime(&data, &options).ok_or("no ONNX Runtime build for this platform")?;
            let p: laqta_ml::Progress = Arc::new(move |a, b| progress(a, b));
            rt.install(entry, p, Arc::new(AtomicBool::new(false))).await.map_err(|e| e.to_string())
        })
    }

    fn remove(&self, id: &str) -> Result<(), String> {
        let dir = models_dir(&self.data);
        for f in [format!("{id}.onnx"), format!("{id}.dict.txt")] {
            match std::fs::remove_file(dir.join(&f)) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(format!("{f}: {e}")),
            }
        }
        Ok(())
    }

    fn add_custom(&self, path: PathBuf, family: String, name: String) -> BoxFuture<'static, Result<CustomModel, String>> {
        let data = self.data.clone();
        let options = self.options();
        Box::pin(async move {
            let fam = Family::parse(&family).ok_or_else(|| format!("unknown family {family}"))?;
            ensure_onnxruntime(&data).await?;
            let rt = runtime(&data, &options).ok_or("no ONNX Runtime build for this platform")?;
            rt.ensure_ort().map_err(|e| e.to_string())?;
            laqta_ml::validate_custom(&path, &family).map_err(|e| e.to_string())?;
            let slug: String = name.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' }).collect();
            let id = format!("custom-{}-{}", fam.name(), slug.trim_matches('-'));
            let dir = models_dir(&data);
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            std::fs::copy(&path, dir.join(format!("{id}.onnx"))).map_err(|e| e.to_string())?;
            Ok(CustomModel { id, name, task: fam.task().as_str().to_owned(), family })
        })
    }
}
