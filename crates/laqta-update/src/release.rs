//! The GitHub release document (`GET /repos/{repo}/releases/latest`) and the `SHA256SUMS` file a
//! release publishes next to its packages.

use serde::Deserialize;

use crate::{UpdateError, Version};

/// A published release.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub version: Version,
    /// The tag as published, such as `v0.2.0`.
    pub tag: String,
    /// The release notes in Markdown. Empty when the release has none.
    pub notes: String,
    /// The release page (`html_url`).
    pub url: String,
    pub assets: Vec<Asset>,
}

/// One file attached to a release.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Asset {
    pub name: String,
    /// The direct download URL (`browser_download_url`).
    pub url: String,
    pub size: u64,
}

#[derive(Deserialize)]
struct WireRelease {
    tag_name: String,
    body: Option<String>,
    html_url: String,
    #[serde(default)]
    assets: Vec<WireAsset>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
}

#[derive(Deserialize)]
struct WireAsset {
    name: String,
    browser_download_url: String,
    #[serde(default)]
    size: u64,
}

/// Reads the body of `GET /repos/{repo}/releases/latest`. GitHub only returns published, stable
/// releases there, so a draft or pre-release body is refused rather than offered as an update.
pub fn parse_release(json: &str) -> Result<Release, UpdateError> {
    let wire: WireRelease = serde_json::from_str(json).map_err(|err| UpdateError::Parse(err.to_string()))?;
    if wire.draft || wire.prerelease {
        return Err(UpdateError::NotStable(wire.tag_name));
    }
    let version = Version::parse(&wire.tag_name)?;
    Ok(Release {
        version,
        tag: wire.tag_name,
        notes: wire.body.unwrap_or_default(),
        url: wire.html_url,
        assets: wire
            .assets
            .into_iter()
            .map(|asset| Asset { name: asset.name, url: asset.browser_download_url, size: asset.size })
            .collect(),
    })
}

/// Parses `sha256sum` output: `<hex>  <file>` in text mode or `<hex> *<file>` in binary mode. Lines
/// that are not a 64-digit hash followed by a name are skipped. Hashes come back in lowercase.
pub fn parse_sha256sums(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|line| {
            let (hash, rest) = line.split_once(' ')?;
            let name = rest.strip_prefix(' ').or_else(|| rest.strip_prefix('*'))?;
            let is_hash = hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit());
            (is_hash && !name.is_empty()).then(|| (name.to_owned(), hash.to_ascii_lowercase()))
        })
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    const LATEST: &str = include_str!("../tests/fixtures/latest-release.json");
    const SUMS: &str = include_str!("../tests/fixtures/SHA256SUMS");

    #[test]
    fn reads_the_latest_release_document() {
        let release = parse_release(LATEST).unwrap();
        assert_eq!(release.version, Version::parse("0.2.0").unwrap());
        assert_eq!(release.tag, "v0.2.0");
        assert_eq!(release.url, "https://github.com/Etherll/laqta/releases/tag/v0.2.0");
        assert!(release.notes.starts_with("## Highlights\r\n"));
        assert!(release.notes.contains("Watch folders"));
        assert_eq!(release.assets.len(), 5);
        let full = release.assets.iter().find(|asset| asset.name == "Laqta-0.2.0-win-full.nupkg").unwrap();
        assert_eq!(full.url, "https://github.com/Etherll/laqta/releases/download/v0.2.0/Laqta-0.2.0-win-full.nupkg");
        assert_eq!(full.size, 48_123_456);
    }

    #[test]
    fn null_body_is_empty_notes() {
        let json = r#"{"tag_name":"v0.2.0","body":null,"html_url":"https://example.invalid/r","assets":[]}"#;
        let release = parse_release(json).unwrap();
        assert_eq!(release.notes, "");
        assert!(release.assets.is_empty());
    }

    #[test]
    fn refuses_drafts_and_pre_releases() {
        for flag in [r#""draft":true,"prerelease":false"#, r#""draft":false,"prerelease":true"#] {
            let json = format!(r#"{{"tag_name":"v0.3.0","body":"","html_url":"x",{flag}}}"#);
            assert_eq!(parse_release(&json), Err(UpdateError::NotStable("v0.3.0".to_owned())));
        }
    }

    #[test]
    fn refuses_a_tag_that_is_not_a_version() {
        let json = r#"{"tag_name":"nightly","body":"","html_url":"x"}"#;
        assert_eq!(parse_release(json), Err(UpdateError::InvalidVersion("nightly".to_owned())));
    }

    #[test]
    fn refuses_a_document_that_is_not_json_or_lacks_fields() {
        assert!(matches!(parse_release("not json"), Err(UpdateError::Parse(_))));
        assert!(matches!(parse_release(r#"{"tag_name":"v1.0.0"}"#), Err(UpdateError::Parse(_))));
    }

    #[test]
    fn sums_accept_text_and_binary_mode_and_skip_other_lines() {
        let text = concat!(
            "# written by CI\n",
            "\n",
            "0123456789ABCDEF0123456789abcdef0123456789abcdef0123456789abcdef  Laqta-0.2.0-win-full.nupkg\r\n",
            "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210 *Laqta-win-Setup.exe\n",
            "not-a-hash  junk\n",
            "abc  too-short\n",
        );
        assert_eq!(
            parse_sha256sums(text),
            vec![
                ("Laqta-0.2.0-win-full.nupkg".to_owned(), "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".to_owned()),
                ("Laqta-win-Setup.exe".to_owned(), "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210".to_owned()),
            ]
        );
    }

    #[test]
    fn the_release_sums_file_lists_every_package() {
        let sums = parse_sha256sums(SUMS);
        let names: Vec<&str> = sums.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(names, ["Laqta-0.2.0-win-full.nupkg", "Laqta-0.2.0-win-delta.nupkg", "RELEASES", "Laqta-win-Setup.exe"]);
        assert!(sums.iter().all(|(_, hash)| hash.len() == 64));
    }
}
