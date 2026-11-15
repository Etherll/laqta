//! Small wording rules shared by the views: counts with their noun, the platform modifier key.

/// `n` with its noun: `one` for exactly one, `many` for any other count ("0 files", "3 files").
pub fn count(n: usize, one: &str, many: &str) -> String {
    let noun = if n == 1 { one } else { many };
    format!("{n} {noun}")
}

/// The modifier key in shortcut text: ⌘ on macOS, Ctrl elsewhere.
pub fn modifier(mac: bool) -> &'static str {
    if mac { "⌘" } else { "Ctrl" }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_count_takes_the_singular_only_for_one() {
        assert_eq!(count(1, "file", "files"), "1 file");
        assert_eq!(count(3, "file", "files"), "3 files");
        assert_eq!(count(0, "file", "files"), "0 files");
    }

    #[test]
    fn the_modifier_is_the_platforms() {
        assert_eq!(modifier(true), "⌘");
        assert_eq!(modifier(false), "Ctrl");
    }
}
