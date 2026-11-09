//! UI translations. Fluent bundles under `locales/<tag>/laqta.ftl`, generated from ShareX's
//! translations by `scripts/resx-to-ftl.py`, so keys are ShareX's resource names. Laqta's own English
//! text is the fallback: English and any missing key show the string the caller passes.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use fluent_bundle::concurrent::FluentBundle;
use fluent_bundle::{FluentArgs, FluentResource};
use laqta_domain::Action;
use unic_langid::LanguageIdentifier;

/// (tag, native name, bundle). English first.
pub const LOCALES: &[(&str, &str, &str)] = &[
    ("en", "English", include_str!("../../../locales/en/laqta.ftl")),
    ("ar-YE", "العربية", include_str!("../../../locales/ar-YE/laqta.ftl")),
    ("cs", "Čeština", include_str!("../../../locales/cs/laqta.ftl")),
    ("da", "Dansk", include_str!("../../../locales/da/laqta.ftl")),
    ("de", "Deutsch", include_str!("../../../locales/de/laqta.ftl")),
    ("es", "Español", include_str!("../../../locales/es/laqta.ftl")),
    ("es-MX", "Español (México)", include_str!("../../../locales/es-MX/laqta.ftl")),
    ("fa-IR", "فارسی", include_str!("../../../locales/fa-IR/laqta.ftl")),
    ("fr", "Français", include_str!("../../../locales/fr/laqta.ftl")),
    ("he-IL", "עברית", include_str!("../../../locales/he-IL/laqta.ftl")),
    ("hi", "हिन्दी", include_str!("../../../locales/hi/laqta.ftl")),
    ("hu", "Magyar", include_str!("../../../locales/hu/laqta.ftl")),
    ("id-ID", "Bahasa Indonesia", include_str!("../../../locales/id-ID/laqta.ftl")),
    ("it-IT", "Italiano", include_str!("../../../locales/it-IT/laqta.ftl")),
    ("ja-JP", "日本語", include_str!("../../../locales/ja-JP/laqta.ftl")),
    ("ko-KR", "한국어", include_str!("../../../locales/ko-KR/laqta.ftl")),
    ("nl-NL", "Nederlands", include_str!("../../../locales/nl-NL/laqta.ftl")),
    ("pl", "Polski", include_str!("../../../locales/pl/laqta.ftl")),
    ("pt-BR", "Português (Brasil)", include_str!("../../../locales/pt-BR/laqta.ftl")),
    ("pt-PT", "Português (Portugal)", include_str!("../../../locales/pt-PT/laqta.ftl")),
    ("ro", "Română", include_str!("../../../locales/ro/laqta.ftl")),
    ("ru", "Русский", include_str!("../../../locales/ru/laqta.ftl")),
    ("sv", "Svenska", include_str!("../../../locales/sv/laqta.ftl")),
    ("th", "ไทย", include_str!("../../../locales/th/laqta.ftl")),
    ("tr", "Türkçe", include_str!("../../../locales/tr/laqta.ftl")),
    ("uk", "Українська", include_str!("../../../locales/uk/laqta.ftl")),
    ("vi-VN", "Tiếng Việt", include_str!("../../../locales/vi-VN/laqta.ftl")),
    ("zh-CN", "简体中文", include_str!("../../../locales/zh-CN/laqta.ftl")),
    ("zh-TW", "繁體中文", include_str!("../../../locales/zh-TW/laqta.ftl")),
];

struct Localizer {
    tag: &'static str,
    bundle: FluentBundle<FluentResource>,
    /// Lower-case id → id, so `HotkeyType_QRCode` is found from the variant `QrCode`.
    ids: HashMap<String, String>,
}

static CURRENT: RwLock<Option<Arc<Localizer>>> = RwLock::new(None);

/// The tag `requested` (or the system's languages when `None`) negotiates to: an exact tag, then the
/// same language with another region (`de-AT` → `de`, `pt` → `pt-BR`), else English.
pub fn negotiate(requested: Option<&str>) -> &'static str {
    let wanted: Vec<LanguageIdentifier> = match requested {
        Some(tag) => tag.parse().into_iter().collect(),
        None => sys_locale::get_locales().filter_map(|l| l.replace('_', "-").split('.').next().and_then(|t| t.parse().ok())).collect(),
    };
    let available: Vec<(&'static str, LanguageIdentifier)> = LOCALES.iter().filter_map(|(t, _, _)| Some((*t, t.parse().ok()?))).collect();
    for want in &wanted {
        if let Some((t, _)) = available.iter().find(|(_, l)| l.language == want.language && l.region == want.region) {
            return t;
        }
        if let Some((t, _)) = available.iter().find(|(_, l)| l.language == want.language) {
            return t;
        }
    }
    "en"
}

/// Switches the UI language; `None` follows the system. Returns the tag in use.
pub fn set_language(requested: Option<&str>) -> &'static str {
    let tag = negotiate(requested);
    let next = if tag == "en" { None } else { LOCALES.iter().find(|(t, _, _)| *t == tag).and_then(|(t, _, text)| load(t, text)) };
    if let Ok(mut current) = CURRENT.write() {
        *current = next.map(Arc::new);
    }
    tag
}

fn load(tag: &'static str, text: &str) -> Option<Localizer> {
    let text = text.replace("ShareX", "Laqta");
    let resource = match FluentResource::try_new(text) {
        Ok(r) => r,
        Err((r, errors)) => {
            tracing::warn!(tag, count = errors.len(), "translation has syntax errors; the rest is used");
            r
        }
    };
    let ids = resource
        .entries()
        .filter_map(|e| if let fluent_syntax::ast::Entry::Message(m) = e { Some(m.id.name.to_string()) } else { None })
        .map(|id| (id.to_ascii_lowercase(), id))
        .collect();
    let lang: LanguageIdentifier = tag.parse().ok()?;
    let mut bundle = FluentBundle::new_concurrent(vec![lang]);
    bundle.set_use_isolating(false);
    bundle.add_resource(resource).ok()?;
    Some(Localizer { tag, bundle, ids })
}

/// The tag in use ("en" when nothing was set).
pub fn current() -> &'static str {
    CURRENT.read().ok().and_then(|c| c.as_ref().map(|l| l.tag)).unwrap_or("en")
}

/// The translation of the ShareX resource `key` (case-insensitive), or `fallback`.
pub fn t_args(key: &str, fallback: &str, args: &[(&str, String)]) -> String {
    let Some(loc) = CURRENT.read().ok().and_then(|c| c.clone()) else { return fallback.to_string() };
    let Some(id) = loc.ids.get(&key.to_ascii_lowercase()) else { return fallback.to_string() };
    let Some(pattern) = loc.bundle.get_message(id).and_then(|m| m.value()) else { return fallback.to_string() };
    let mut fargs = FluentArgs::new();
    for (k, v) in args {
        fargs.set(*k, v.clone());
    }
    let mut errors = Vec::new();
    let out = loc.bundle.format_pattern(pattern, Some(&fargs), &mut errors);
    if out.trim().is_empty() { fallback.to_string() } else { out.into_owned() }
}

pub fn t(key: &str, fallback: &str) -> String {
    t_args(key, fallback, &[])
}

/// An action's name in the UI language: ShareX's `HotkeyType_<name>`.
pub fn action(action: Action, fallback: &str) -> String {
    let name = match action {
        Action::ExitLaqta => "ExitShareX".to_string(),
        other => format!("{other:?}"),
    };
    t(&format!("HotkeyType_{name}"), fallback)
}

pub fn action_label(a: Action) -> String {
    action(a, a.label())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_bundle_parses() {
        for (tag, _, text) in LOCALES {
            assert!(FluentResource::try_new(text.to_string()).is_ok(), "{tag} has Fluent syntax errors");
        }
    }

    #[test]
    fn translates_and_falls_back() {
        assert_eq!(negotiate(Some("de-AT")), "de");
        assert_eq!(negotiate(Some("pt-BR")), "pt-BR");
        assert_eq!(negotiate(Some("xx")), "en");
        assert_eq!(set_language(Some("de")), "de");
        assert_eq!(action_label(Action::RectangleRegion), "Bereich aufnehmen");
        assert_eq!(action(Action::QrCode, "QR code"), t("HotkeyType_QRCode", "?"));
        assert_eq!(t("NoSuchKey", "Fallback"), "Fallback");
        assert!(!action_label(Action::ExitLaqta).contains("ShareX"));
        set_language(Some("en"));
        assert_eq!(action_label(Action::RectangleRegion), Action::RectangleRegion.label());
    }
}
