//! Runs an effect preset with what the kernels can't get themselves: rendered text and the image
//! files a preset names.
//!
//! `main` attaches the text renderer once (fonts are injected, ADR 024). Asset files are read
//! through `files` and decoded before the kernels start, so the compute pool does no I/O.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use laqta_domain::Bitmap;
use laqta_domain::effects::draw::Draw;
use laqta_domain::effects::{Effect, Preset, TextSpec};
use laqta_imaging::effects::EffectContext;
use laqta_ports::AppError;

use crate::dispatcher::{App, Ports};
use crate::{files, task};

/// Renders the text of a drawing effect. `None` when it can't (no usable font).
pub type EffectText = Arc<dyn Fn(&TextSpec) -> Option<Bitmap> + Send + Sync>;

/// What every preset run shares. `App` and Task hold the same one.
#[derive(Default)]
pub(crate) struct EffectRunner {
    text: OnceLock<EffectText>,
}

impl std::fmt::Debug for EffectRunner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EffectRunner").field("text", &self.text.get().is_some()).finish()
    }
}

/// The asset paths the enabled effects of `preset` name, without repeats or empty paths.
fn asset_paths(preset: &Preset) -> Vec<String> {
    let mut paths: Vec<String> = Vec::new();
    for item in preset.effects.iter().filter(|item| item.enabled) {
        let path = match &item.effect {
            Effect::Draw(Draw::BackgroundImage { path, .. } | Draw::Image { path, .. } | Draw::Particles { path, .. }) => path,
            _ => continue,
        };
        if !path.is_empty() && !paths.contains(path) {
            paths.push(path.clone());
        }
    }
    paths
}

impl EffectRunner {
    pub(crate) fn attach_text(&self, text: EffectText) {
        // The first one stays: fonts do not change while the process runs.
        let _ = self.text.set(text);
    }

    /// Applies the enabled effects of `preset` to `image` on the compute pool. An asset that can't
    /// be read or decoded is left out, and the effect that names it reports it missing.
    pub(crate) async fn apply(&self, ports: &Ports, image: Bitmap, preset: Preset) -> Result<Bitmap, AppError> {
        let mut assets: HashMap<String, Bitmap> = HashMap::new();
        for path in asset_paths(&preset) {
            let Ok(bytes) = files::read(ports.spawner.as_ref(), PathBuf::from(&path)).await else { continue };
            let codecs = ports.codecs.clone();
            if let Ok(bitmap) =
                task::compute(ports.spawner.as_ref(), move || codecs.decode(&bytes).map_err(|e| AppError::Io(e.to_string()))).await
            {
                assets.insert(path, bitmap);
            }
        }
        let text = self.text.get().cloned();
        task::compute(ports.spawner.as_ref(), move || {
            let asset = |path: &str| assets.get(path).cloned();
            let render = |spec: &TextSpec| text.as_ref().and_then(|render| render(spec));
            let cx = EffectContext { asset: &asset, text: &render };
            laqta_imaging::effects::apply_preset(&image, &preset, &cx).map_err(|error| AppError::Io(error.to_string()))
        })
        .await
    }
}

impl App {
    /// Gives the application the text renderer for drawing effects. `main` calls it once.
    pub fn attach_effect_text(&self, text: EffectText) {
        self.effects.attach_text(text);
    }

    /// Applies an effect preset as a capture would get it: with text and the image files it names.
    pub async fn apply_preset(&self, image: Bitmap, preset: Preset) -> Result<Bitmap, AppError> {
        self.effects.apply(&self.ports, image, preset).await
    }
}

#[cfg(test)]
mod tests {
    use laqta_domain::effects::EffectItem;
    use laqta_domain::effects::draw::{Placement, SizeMode};

    use super::*;

    fn item(effect: Draw, enabled: bool) -> EffectItem {
        EffectItem { effect: Effect::Draw(effect), enabled }
    }

    #[test]
    fn asset_paths_are_the_enabled_named_ones_once() {
        let image = |path: &str| Draw::Image {
            path: path.into(),
            placement: Placement::TopLeft,
            offset: (0, 0),
            size_mode: SizeMode::DontResize,
            opacity: 1.0,
        };
        let preset = Preset {
            name: "p".into(),
            effects: vec![
                item(image("a.png"), true),
                item(Draw::BackgroundImage { path: "b.png".into(), center: false, tile: true }, true),
                item(image("a.png"), true),
                item(image(""), true),
                item(image("off.png"), false),
                item(Draw::Particles { path: "c.png".into(), count: 1, min_size: 1, max_size: 2, random_angle: false, seed: 1 }, true),
            ],
        };
        assert_eq!(asset_paths(&preset), ["a.png", "b.png", "c.png"]);
    }
}
