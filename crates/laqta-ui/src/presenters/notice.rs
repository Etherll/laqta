//! Short notices for a window's toast: a headline, and for a failure the reason in plain words with
//! the raw error kept as copyable detail. The reason comes from the error's kind (`AppError`), set
//! by the adapter that saw the failure, never from reading the error's text.

use laqta_ports::AppError;

/// One toast.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    /// What happened: "Saved photo.png", "Couldn't describe the image".
    pub title: String,
    /// Why, in plain words, for a failure.
    pub reason: Option<String>,
    /// The raw error, for "Copy details", when it says more than `reason`.
    pub detail: Option<String>,
    /// A failure stays until dismissed; anything else fades.
    pub error: bool,
}

impl Notice {
    pub fn info(title: impl Into<String>) -> Notice {
        Notice { title: title.into(), reason: None, detail: None, error: false }
    }

    /// A failure with no more to say than its title.
    pub fn error(title: impl Into<String>) -> Notice {
        Notice { title: title.into(), reason: None, detail: None, error: true }
    }

    /// `title` failed with `error`.
    pub fn failure(title: impl Into<String>, error: &AppError) -> Notice {
        let (reason, message) = match error {
            AppError::Unreachable(m) => {
                ("The service isn't answering. Check that it's running and that its address in Settings is right.".to_owned(), m.as_str())
            }
            AppError::TimedOut => ("The service took too long to answer.".to_owned(), ""),
            AppError::Cancelled => ("It was cancelled.".to_owned(), ""),
            // Callers drop a Busy request without a toast; this only keeps the match total.
            AppError::Busy => ("A selection is already open.".to_owned(), ""),
            AppError::PermissionDenied => ("Permission was denied.".to_owned(), ""),
            AppError::Unsupported(m)
            | AppError::Io(m)
            | AppError::Network(m)
            | AppError::Auth(m)
            | AppError::Quota(m)
            | AppError::Panic(m) => (sentence(m), m.as_str()),
            // A kind added later still says what it can.
            other => (sentence(&other.to_string()), ""),
        };
        Notice::with_reason(title, reason, message)
    }

    /// `title` failed with `message`, a sentence written by this app (a file error, say).
    pub fn failure_text(title: impl Into<String>, message: &str) -> Notice {
        Notice::with_reason(title, sentence(message), message)
    }

    /// The detail is worth copying only when the reason left something of `message` out.
    fn with_reason(title: impl Into<String>, reason: String, message: &str) -> Notice {
        let message = message.trim();
        let said = reason.eq_ignore_ascii_case(message) || message.is_empty();
        let reason = (!reason.is_empty()).then_some(reason);
        Notice { title: title.into(), reason, detail: (!said).then(|| message.to_owned()), error: true }
    }
}

/// How long `n` stays up: info fades after five seconds; a failure stays until it is dismissed.
pub fn lifetime(n: &Notice) -> Option<std::time::Duration> {
    if n.error { None } else { Some(std::time::Duration::from_millis(5000)) }
}

/// The shortest an info notice may stay up: long enough to read its headline.
pub const MIN_INFO_MS: u32 = 4000;

/// The longest reason shown; the rest is in the detail.
const REASON_MAX: usize = 140;

/// `message` as a sentence: a capital first letter, cut at a word under `REASON_MAX`.
fn sentence(message: &str) -> String {
    let s = message.trim();
    let mut out: String = s.chars().take(REASON_MAX).collect();
    if out.len() < s.len() {
        // Cut at the last space past half the limit, counted in characters like the limit.
        let half = out.char_indices().nth(REASON_MAX / 2).map_or(out.len(), |(i, _)| i);
        let cut = out.rfind(' ').filter(|&i| i > half).unwrap_or(out.len());
        out.truncate(cut);
        out = out.trim_end_matches([',', ':', ';', ' ']).to_owned() + "…";
    }
    let mut chars = out.chars();
    let first = chars.next().map(|c| c.to_uppercase().collect::<String>()).unwrap_or_default();
    first + chars.as_str()
}

/// The editor's AI tools, by what their notices say.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AiTool {
    CopyText,
    RemoveBackground,
    Erase,
    Describe,
}

impl AiTool {
    /// The status while the tool works.
    pub fn busy(self) -> &'static str {
        match self {
            AiTool::CopyText => "Reading the text…",
            AiTool::RemoveBackground => "Removing the background…",
            AiTool::Erase => "Erasing…",
            AiTool::Describe => "Describing the image…",
        }
    }

    /// The toast's headline when the tool fails.
    pub fn failed(self) -> &'static str {
        match self {
            AiTool::CopyText => "Couldn't read the text",
            AiTool::RemoveBackground => "Couldn't remove the background",
            AiTool::Erase => "Couldn't erase that",
            AiTool::Describe => "Couldn't describe the image",
        }
    }
}

/// One line for a status bar: "`title`. `message` as a sentence." The message is cut at a word
/// past `REASON_MAX`, so a long OS error never floods the line.
pub fn failure_line(title: &str, message: &str) -> String {
    let reason = sentence(message);
    if reason.is_empty() { format!("{title}.") } else { format!("{title}. {reason}") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failure_line_is_the_title_then_the_reason_as_a_sentence() {
        assert_eq!(failure_line("Couldn't save", "permission denied"), "Couldn't save. Permission denied");
        assert_eq!(failure_line("Couldn't save", "  "), "Couldn't save.");
        let long = "word ".repeat(60);
        assert!(failure_line("Export failed", &long).ends_with('…'));
    }

    #[test]
    fn an_unreachable_service_says_so_and_keeps_the_raw_error() {
        let raw = "error sending request for url (http://localhost:11434/v1/chat/completions): tcp connect error";
        let n = Notice::failure(AiTool::Describe.failed(), &AppError::Unreachable(raw.into()));
        assert!(n.error);
        assert!(n.reason.as_deref().is_some_and(|r| r.starts_with("The service isn't answering")));
        assert_eq!(n.detail.as_deref(), Some(raw), "the raw error stays copyable");
        let n = Notice::failure("x", &AppError::TimedOut);
        assert_eq!(n.reason.as_deref(), Some("The service took too long to answer."));
        assert_eq!(n.detail, None);
    }

    #[test]
    fn a_service_s_own_message_is_the_reason_without_the_kind_prefix() {
        let n = Notice::failure("x", &AppError::Io("The analyzer answered 404: model 'llava' not found".into()));
        assert_eq!(n.reason.as_deref(), Some("The analyzer answered 404: model 'llava' not found"));
        assert_eq!(n.detail, None, "nothing more to copy");
        let n = Notice::failure_text("x", "set the endpoint (Settings → Models → Cloud)");
        assert_eq!(n.reason.as_deref(), Some("Set the endpoint (Settings → Models → Cloud)"));
        assert_eq!(n.detail, None);
    }

    #[test]
    fn a_long_message_is_cut_at_a_word_and_kept_whole_in_the_detail() {
        let raw = "word ".repeat(60);
        let n = Notice::failure_text("x", &raw);
        let r = n.reason.expect("a reason");
        assert!(r.chars().count() <= REASON_MAX + 1 && r.ends_with("word…"), "{r}");
        assert_eq!(n.detail.as_deref(), Some(raw.trim()));
    }

    #[test]
    fn a_long_message_in_another_script_keeps_most_of_its_words() {
        let raw = "خطأ في الاتصال ".repeat(20);
        let r = Notice::failure_text("x", &raw).reason.expect("a reason");
        assert!(r.chars().count() > REASON_MAX / 2, "cut by characters, not bytes: {} chars", r.chars().count());
    }

    #[test]
    fn info_fades_and_failures_stay() {
        assert!(!Notice::info("Saved").error);
        assert!(Notice::error("The command failed.").error);
    }

    #[test]
    fn info_fades_after_five_seconds_and_never_sooner_than_the_minimum() {
        let ms = lifetime(&Notice::info("Saved photo.png")).expect("info fades");
        assert_eq!(ms, std::time::Duration::from_millis(5000));
        assert!(ms.as_millis() >= u128::from(MIN_INFO_MS));
    }

    #[test]
    fn a_failure_never_fades() {
        assert_eq!(lifetime(&Notice::error("The command failed.")), None);
        assert_eq!(lifetime(&Notice::failure("x", &AppError::TimedOut)), None);
        assert_eq!(lifetime(&Notice::failure_text("x", "set the endpoint")), None);
    }
}
