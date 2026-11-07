//! Builds the upload registry: the bundled catalog, the user's `.sxcu` files and the native hosts.

use std::path::Path;
use std::sync::Arc;

use laqta_domain::hosts::HostAccount;
use laqta_ports::{Prompt, Rng, SecretStore, Shell};
use laqta_upload::native::{HostDeps, native_hosts};
use laqta_upload::registry::Registry;
use laqta_upload::uploaders::SharedRegistry;

/// OS randomness for PKCE verifiers and OAuth state.
#[derive(Debug)]
pub struct OsRng;

impl Rng for OsRng {
    fn next_u64(&self) -> u64 {
        let mut b = [0u8; 8];
        if getrandom::fill(&mut b).is_err() {
            // getrandom only fails if the OS has no entropy source; fall back to the clock-seeded generator.
            return fastrand::u64(..);
        }
        u64::from_le_bytes(b)
    }
}

pub fn host_deps(secrets: Arc<dyn SecretStore>, shell: Arc<dyn Shell>) -> HostDeps {
    HostDeps {
        secrets,
        shell,
        csprng: Arc::new(OsRng),
        now_unix: Arc::new(|| std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)),
    }
}

/// Replaces the native hosts with the ones `accounts` describe.
pub fn apply_accounts(registry: &SharedRegistry, accounts: &[HostAccount], deps: &HostDeps) {
    let hosts = native_hosts(accounts, deps);
    registry.update(|r| {
        for d in hosts.destinations {
            r.add_destination(d);
        }
        for s in hosts.shorteners {
            r.add_shortener(s);
        }
    });
}

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

/// Registers a custom uploader and keeps a copy in `<data>/uploaders/<id>.sxcu`.
pub fn add_custom(registry: &SharedRegistry, dir: &Path, json: &str) -> Result<String, String> {
    let id = registry.update(|r| r.add_custom(json)).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let file: String = id.trim_start_matches("custom:").chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
    std::fs::write(dir.join(format!("{file}.sxcu")), json).map_err(|e| e.to_string())?;
    Ok(id)
}
