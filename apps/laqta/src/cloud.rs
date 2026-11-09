//! The background remover and image analyzer the app uses: the settings pick a local
//! model family or remove.bg, and the analyzer's endpoint and model, at each call.

use std::sync::{Arc, RwLock};

use laqta_domain::Bitmap;
use laqta_domain::settings::{BackgroundProvider, CloudOptions};
use laqta_ports::{BackgroundRemover, BoxFuture, ImageAnalyzer, PortError, SecretStore, Spawner};

pub type SharedCloudOptions = Arc<RwLock<CloudOptions>>;

/// Runs `work` on the compute pool (the codecs require it) and returns its result.
async fn compute<T: Send + 'static>(spawner: &Arc<dyn Spawner>, work: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    let slot = Arc::new(std::sync::Mutex::new(None));
    let out = slot.clone();
    spawner
        .run_compute(Box::new(move || {
            let v = work();
            if let Ok(mut s) = out.lock() {
                *s = Some(v);
            }
        }))
        .await;
    slot.lock().ok().and_then(|mut s| s.take())
}

fn codecs_encode(spawner: Arc<dyn Spawner>) -> laqta_cloud::Encode {
    Arc::new(move |b: Bitmap| {
        let spawner = spawner.clone();
        Box::pin(async move {
            compute(&spawner, move || laqta_imaging::png::encode_png(&b).map_err(|e| e.to_string()))
                .await
                .unwrap_or_else(|| Err("the compute pool stopped".into()))
        })
    })
}

fn codecs_decode(spawner: Arc<dyn Spawner>, codecs: laqta_imaging::Codecs) -> laqta_cloud::Decode {
    Arc::new(move |bytes: Vec<u8>| {
        let (spawner, codecs) = (spawner.clone(), codecs.clone());
        Box::pin(async move {
            compute(&spawner, move || codecs.decode(&bytes).map_err(|e| e.to_string()))
                .await
                .unwrap_or_else(|| Err("the compute pool stopped".into()))
        })
    })
}

pub struct Background {
    pub options: SharedCloudOptions,
    pub local: Arc<dyn BackgroundRemover>,
    pub remove_bg: laqta_cloud::RemoveBg,
}

impl Background {
    pub fn new(
        options: SharedCloudOptions,
        local: Arc<dyn BackgroundRemover>,
        secrets: Arc<dyn SecretStore>,
        spawner: Arc<dyn Spawner>,
        codecs: laqta_imaging::Codecs,
    ) -> Self {
        let remove_bg = laqta_cloud::RemoveBg::new(secrets, codecs_encode(spawner.clone()), codecs_decode(spawner, codecs));
        Background { options, local, remove_bg }
    }
}

impl BackgroundRemover for Background {
    fn remove(&self, image: Bitmap) -> BoxFuture<'_, Result<Bitmap, PortError>> {
        let provider = self.options.read().map(|o| o.background).unwrap_or_default();
        match provider {
            BackgroundProvider::Local => self.local.remove(image),
            BackgroundProvider::RemoveBg => self.remove_bg.remove(image),
        }
    }
}

pub struct Analyzer {
    pub options: SharedCloudOptions,
    pub secrets: Arc<dyn SecretStore>,
    pub spawner: Arc<dyn Spawner>,
}

impl ImageAnalyzer for Analyzer {
    fn analyze(&self, image: Bitmap, prompt: String) -> BoxFuture<'_, Result<String, PortError>> {
        let a = self.options.read().map(|o| o.analyzer.clone()).unwrap_or_default();
        let client = laqta_cloud::OpenAiCompatible {
            base_url: a.base_url,
            model: a.model,
            secrets: self.secrets.clone(),
            transport: laqta_cloud::http(),
            encode: codecs_encode(self.spawner.clone()),
        };
        Box::pin(async move { client.analyze(image, prompt).await })
    }
}
