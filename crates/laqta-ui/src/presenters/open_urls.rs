//! Files the OS hands to a running Laqta (macOS "Open With", or a file dropped on the Dock icon).

use std::path::PathBuf;

/// The local paths among `urls`. The OS sends `file://` URLs with percent-escapes; anything else
/// is left out.
pub fn file_paths(urls: &[String]) -> Vec<PathBuf> {
    urls.iter()
        .filter_map(|url| url.strip_prefix("file://"))
        .map(|rest| rest.strip_prefix("localhost").unwrap_or(rest))
        .filter(|path| path.starts_with('/'))
        .map(|path| PathBuf::from(unescape(path)))
        .collect()
}

/// Turns `%XX` back into its byte. A `%` that is not followed by two hex digits stays as it is.
fn unescape(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = (bytes[i] == b'%').then(|| text.get(i + 1..i + 3)).flatten().and_then(|h| u8::from_str_radix(h, 16).ok());
        match hex {
            Some(byte) => {
                out.push(byte);
                i += 3;
            }
            None => {
                out.push(bytes[i]);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_urls_become_paths_with_escapes_decoded() {
        let urls = ["file:///Users/a/My%20Shot.png".to_owned(), "file://localhost/tmp/%C3%A9.png".to_owned()];
        assert_eq!(file_paths(&urls), [PathBuf::from("/Users/a/My Shot.png"), PathBuf::from("/tmp/é.png")]);
    }

    #[test]
    fn other_urls_and_broken_escapes_are_handled() {
        assert!(file_paths(&["https://example.com/a.png".to_owned(), "file://host/share/a.png".to_owned()]).is_empty());
        assert_eq!(file_paths(&["file:///tmp/100%.png".to_owned()]), [PathBuf::from("/tmp/100%.png")]);
    }
}
