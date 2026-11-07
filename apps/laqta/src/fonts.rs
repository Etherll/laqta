//! The font loader `main` hands to the Editor engine (ADR 024). It runs on first use, not at start.

use std::sync::Arc;

pub fn fonts() -> laqta_editor::Fonts {
    laqta_editor::Fonts::new(Arc::new(|| {
        let mut db = cosmic_text::fontdb::Database::new();
        db.load_system_fonts();
        let locale = std::env::var("LC_ALL")
            .or_else(|_| std::env::var("LANG"))
            .ok()
            .and_then(|l| l.split('.').next().map(|s| s.replace('_', "-")))
            .filter(|l| !l.is_empty() && l != "C" && l != "POSIX")
            .unwrap_or_else(|| "en-US".into());
        (db, locale)
    }))
}
