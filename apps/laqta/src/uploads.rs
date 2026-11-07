//! Builds the upload registry: the bundled catalog, the user's `.sxcu` files and the native hosts.

use std::path::Path;
use std::sync::Arc;

use laqta_ports::{Prompt, Rng};
use laqta_upload::registry::Registry;
use laqta_upload::uploaders::SharedRegistry;

/// `<data>/uploaders/*.sxcu` are the user's custom uploaders.
pub fn registry(data: &Path, prompt: Arc<dyn Prompt>, rng: Arc<dyn Rng>) -> Arc<SharedRegistry> {
    let mut reg = Registry::with_catalog(prompt, rng);
    let dir = data.join("uploaders");
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for e in entries.flatten() {
            let p = e.path();
            if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("sxcu")) {
                match std::fs::read_to_string(&p).map_err(|e| e.to_string()).and_then(|j| reg.add_custom(&j).map_err(|e| e.to_string())) {
                    Ok(id) => tracing::info!(%id, "custom uploader loaded"),
                    Err(e) => tracing::warn!(path = %p.display(), "custom uploader rejected: {e}"),
                }
            }
        }
    }
    SharedRegistry::new(reg)
}
