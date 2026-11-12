//! The AI features page: what each feature card says about its engine, download and model
//! choice, and the analyzer's service presets. The view owns the widgets; the rules live here.

use super::history::format_size;

/// What a card needs on disk to work: one entry per model file it uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Need {
    pub installed: bool,
    /// False while the catalog has no pinned hash, so it can't be downloaded.
    pub available: bool,
    pub size_bytes: u64,
    /// A model the user added: when its file is gone there is nothing to download.
    pub custom: bool,
}

/// A download the page started for a card.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Download {
    #[default]
    Idle,
    /// Bytes done and the total when the server sent one.
    Running {
        done: u64,
        total: Option<u64>,
    },
    Failed(String),
}

/// A card's state, shown as a badge beside its title.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    Ready,
    /// Something still has to be downloaded: this many bytes, ONNX Runtime included.
    Missing {
        bytes: u64,
    },
    Downloading(String),
    Failed(String),
    /// A model it needs has no pinned download yet.
    Unavailable,
    /// The file of a model the user added is gone: only removing or re-adding it helps.
    FileMissing,
    /// The work goes to a service (remove.bg) or the system (Windows OCR): nothing to download.
    Elsewhere(String),
}

/// How a badge reads; the view maps it to its own tones.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Good,
    Neutral,
    Busy,
    Bad,
}

pub fn status(needs: &[Need], runtime_ready: bool, runtime_size: u64, download: &Download) -> Status {
    match download {
        Download::Running { done, total } => return Status::Downloading(progress(*done, *total)),
        Download::Failed(e) => return Status::Failed(e.clone()),
        Download::Idle => {}
    }
    let missing: Vec<_> = needs.iter().filter(|n| !n.installed).collect();
    if missing.iter().any(|n| n.custom) {
        return Status::FileMissing;
    }
    if missing.iter().any(|n| !n.available) {
        return Status::Unavailable;
    }
    let mut bytes: u64 = missing.iter().map(|n| n.size_bytes).sum();
    if !runtime_ready && !needs.is_empty() {
        bytes += runtime_size;
    }
    if missing.is_empty() && (runtime_ready || needs.is_empty()) { Status::Ready } else { Status::Missing { bytes } }
}

/// The badge text and tone.
pub fn badge(status: &Status) -> (String, Tone) {
    match status {
        Status::Ready => ("Ready".into(), Tone::Good),
        // The size is on the Download button beside it.
        Status::Missing { .. } => ("Not downloaded".into(), Tone::Neutral),
        Status::Downloading(p) => (format!("Downloading · {p}"), Tone::Busy),
        Status::Failed(_) => ("Download failed".into(), Tone::Bad),
        Status::Unavailable => ("Not available yet".into(), Tone::Neutral),
        Status::FileMissing => ("Model file missing".into(), Tone::Bad),
        Status::Elsewhere(what) => (what.clone(), Tone::Good),
    }
}

/// "38% · 44.0 MB of 115.0 MB", or just the bytes when the size is unknown.
pub fn progress(done: u64, total: Option<u64>) -> String {
    match total {
        Some(t) if t > 0 => format!("{}% · {} of {}", done.saturating_mul(100) / t, format_size(done), format_size(t)),
        _ => format_size(done),
    }
}

/// The model in use for a task: the chosen id when it is still listed, else the catalog default,
/// else the first.
pub fn in_use<'a>(chosen: Option<&str>, ids: &[(&'a str, bool)]) -> Option<&'a str> {
    chosen
        .and_then(|c| ids.iter().find(|(id, _)| *id == c))
        .or_else(|| ids.iter().find(|(_, default)| *default))
        .or_else(|| ids.first())
        .map(|(id, _)| *id)
}

/// The OCR preset whose detector and recogniser are the pair in use; `None` is a hand-picked pair.
pub fn active_preset<'a>(presets: &'a [(String, String, String)], det: &str, rec: &str) -> Option<&'a str> {
    presets.iter().find(|(_, d, r)| d == det && r == rec).map(|(id, _, _)| id.as_str())
}

pub fn speed_label(speed: &str) -> &'static str {
    match speed {
        "fast" => "Fast",
        "balanced" => "Balanced",
        "best" => "Best quality",
        _ => "",
    }
}

/// Catalog language codes as words: "Chinese, English and Latin script".
pub fn languages(codes: &[String]) -> String {
    let names: Vec<&str> = codes
        .iter()
        .map(|c| match c.as_str() {
            "zh" => "Chinese",
            "en" => "English",
            "ja" => "Japanese",
            "ko" => "Korean",
            "el" => "Greek",
            "ar" => "Arabic",
            "ru" | "cyrillic" => "Cyrillic script",
            "latin" => "Latin script",
            other => other,
        })
        .collect();
    match names.as_slice() {
        [] => String::new(),
        [one] => (*one).to_owned(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// The question before removing a model.
pub fn remove_question(name: &str, in_use: bool) -> String {
    if in_use {
        format!("Remove {name}? It's in use, so Laqta asks to download it again the next time you use this feature.")
    } else {
        format!("Remove {name}? It can be downloaded again later.")
    }
}

/// A custom model family as the user knows it, instead of its code name.
pub fn family_label(task: &str, family: &str) -> String {
    match (task, family) {
        ("background", "matting-parametric") => "Background removal: BiRefNet, BEN2 or U²-Net style".into(),
        ("ocr-det", _) => "Text detection: PP-OCR (DBNet)".into(),
        ("ocr-rec", _) => "Text recognition: PP-OCR, with its dictionary file beside it".into(),
        ("inpaint", "lama") => "Object removal: LaMa".into(),
        _ => format!("{family} ({task})"),
    }
}

/// An OpenAI-compatible service the analyzer can be pointed at in one click.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Service {
    pub name: &'static str,
    pub base_url: &'static str,
    /// The model filled in on pick; empty when the server decides (pick from its list).
    pub model: &'static str,
    pub needs_key: bool,
}

pub const SERVICES: [Service; 5] = [
    Service { name: "OpenAI", base_url: "https://api.openai.com/v1", model: "gpt-4.1-mini", needs_key: true },
    Service {
        name: "Gemini",
        base_url: "https://generativelanguage.googleapis.com/v1beta/openai",
        model: "gemini-2.5-flash",
        needs_key: true,
    },
    Service { name: "OpenRouter", base_url: "https://openrouter.ai/api/v1", model: "openai/gpt-4.1-mini", needs_key: true },
    Service { name: "Ollama", base_url: "http://localhost:11434/v1", model: "", needs_key: false },
    Service { name: "LM Studio", base_url: "http://localhost:1234/v1", model: "", needs_key: false },
];

/// The preset a base URL belongs to; `None` is a custom address.
pub fn service_for(base_url: &str) -> Option<usize> {
    let url = base_url.trim().trim_end_matches('/');
    SERVICES.iter().position(|s| s.base_url.eq_ignore_ascii_case(url))
}

#[cfg(test)]
mod tests {
    use super::*;

    const HAVE: Need = Need { installed: true, available: true, size_bytes: 10, custom: false };
    const LACK: Need = Need { installed: false, available: true, size_bytes: 10, custom: false };

    #[test]
    fn a_card_is_ready_only_with_its_models_and_the_runtime() {
        assert_eq!(status(&[HAVE, HAVE], true, 30, &Download::Idle), Status::Ready);
        assert_eq!(status(&[HAVE, LACK], true, 30, &Download::Idle), Status::Missing { bytes: 10 });
        assert_eq!(status(&[HAVE], false, 30, &Download::Idle), Status::Missing { bytes: 30 }, "the runtime counts");
        assert_eq!(status(&[LACK], false, 30, &Download::Idle), Status::Missing { bytes: 40 });
        let pending = Need { available: false, ..LACK };
        assert_eq!(status(&[pending], true, 0, &Download::Idle), Status::Unavailable);
        let gone = Need { custom: true, size_bytes: 0, ..LACK };
        assert_eq!(status(&[HAVE, gone], true, 0, &Download::Idle), Status::FileMissing, "nothing to download for a lost file");
        assert_eq!(status(&[], false, 30, &Download::Idle), Status::Ready, "nothing to run, nothing to fetch");
    }

    #[test]
    fn a_running_or_failed_download_wins() {
        let running = Download::Running { done: 50, total: Some(200) };
        assert!(matches!(status(&[LACK], true, 0, &running), Status::Downloading(p) if p.starts_with("25%")));
        assert_eq!(status(&[LACK], true, 0, &Download::Failed("offline".into())), Status::Failed("offline".into()));
        assert_eq!(badge(&Status::Failed("x".into())).1, Tone::Bad);
    }

    #[test]
    fn the_default_model_is_in_use_until_one_is_chosen() {
        let ids = [("fp32", false), ("fp16", true), ("tiny", false)];
        assert_eq!(in_use(None, &ids), Some("fp16"));
        assert_eq!(in_use(Some("tiny"), &ids), Some("tiny"));
        assert_eq!(in_use(Some("gone"), &ids), Some("fp16"), "a removed custom model falls back");
        assert_eq!(in_use(None, &[("a", false)]), Some("a"));
        assert_eq!(in_use(None, &[]), None);
    }

    #[test]
    fn a_preset_is_active_only_for_its_exact_pair() {
        let presets = [("accurate".to_owned(), "d6".to_owned(), "r6".to_owned()), ("fast".to_owned(), "d5".to_owned(), "r5".to_owned())];
        assert_eq!(active_preset(&presets, "d5", "r5"), Some("fast"));
        assert_eq!(active_preset(&presets, "d6", "r5"), None);
    }

    #[test]
    fn languages_read_as_a_list() {
        let codes = |c: &[&str]| c.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        assert_eq!(languages(&codes(&["zh", "en", "latin"])), "Chinese, English and Latin script");
        assert_eq!(languages(&codes(&["latin"])), "Latin script");
        assert_eq!(languages(&[]), "");
    }

    #[test]
    fn a_service_is_found_by_its_address() {
        assert_eq!(service_for("http://localhost:11434/v1/"), Some(3));
        assert_eq!(service_for(" https://API.openai.com/v1"), Some(0));
        assert_eq!(service_for("http://my-box:8080/v1"), None);
    }

    #[test]
    fn progress_reads_percent_and_sizes() {
        assert_eq!(progress(0, None), "0 B");
        assert!(progress(1024 * 1024, Some(4 * 1024 * 1024)).starts_with("25% · 1.0 MB of 4.0 MB"));
    }
}
