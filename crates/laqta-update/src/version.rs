//! Release versions from tags such as `v1.2.3` or `1.2.3-beta.1`, ordered as SemVer 2.0 orders them.

use std::cmp::Ordering;
use std::fmt;

use crate::UpdateError;

/// A release version. A pre-release (`1.2.3-beta.1`) sorts before its release (`1.2.3`).
/// Build metadata (`+sha.abc`) does not affect the order, so it is dropped when parsing.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    /// The pre-release identifiers without the leading `-`, such as `beta.1`.
    pub pre: Option<String>,
}

impl Version {
    /// Parses `v1.2.3`, `1.2.3` or `1.2.3-beta.1`. A `+build` suffix is accepted and ignored.
    pub fn parse(text: &str) -> Result<Version, UpdateError> {
        let invalid = || UpdateError::InvalidVersion(text.to_owned());
        let body = text.strip_prefix(['v', 'V']).unwrap_or(text);
        let body = body.split_once('+').map_or(body, |(core, _build)| core);
        let (core, pre) = match body.split_once('-') {
            Some((core, pre)) => (core, Some(pre)),
            None => (body, None),
        };
        let parts: Vec<&str> = core.split('.').collect();
        let [major, minor, patch] = parts.as_slice() else {
            return Err(invalid());
        };
        if pre.is_some_and(|pre| !pre.split('.').all(valid_identifier)) {
            return Err(invalid());
        }
        Ok(Version {
            major: number(major).ok_or_else(invalid)?,
            minor: number(minor).ok_or_else(invalid)?,
            patch: number(patch).ok_or_else(invalid)?,
            pre: pre.map(str::to_owned),
        })
    }
}

/// A numeric component: digits only, no leading zero, and it must fit in `u64`.
fn number(text: &str) -> Option<u64> {
    if !is_numeric(text) || (text.len() > 1 && text.starts_with('0')) {
        return None;
    }
    text.parse().ok()
}

fn is_numeric(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit())
}

/// A pre-release identifier: ASCII alphanumerics or `-`, and a numeric one has no leading zero.
fn valid_identifier(text: &str) -> bool {
    !text.is_empty()
        && text.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        && !(is_numeric(text) && text.len() > 1 && text.starts_with('0'))
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        if let Some(pre) = &self.pre {
            write!(f, "-{pre}")?;
        }
        Ok(())
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.major, self.minor, self.patch).cmp(&(other.major, other.minor, other.patch)).then_with(|| match (&self.pre, &other.pre) {
            (None, None) => Ordering::Equal,
            (None, Some(_)) => Ordering::Greater,
            (Some(_), None) => Ordering::Less,
            (Some(left), Some(right)) => cmp_pre(left, right),
        })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Compares dot-separated pre-release identifiers: field by field, and a longer list that starts
/// with the shorter one sorts after it.
fn cmp_pre(left: &str, right: &str) -> Ordering {
    let mut left = left.split('.');
    let mut right = right.split('.');
    loop {
        let ordering = match (left.next(), right.next()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(a), Some(b)) => cmp_identifier(a, b),
        };
        if ordering != Ordering::Equal {
            return ordering;
        }
    }
}

/// Numeric identifiers compare as numbers and sort before alphanumeric ones, which compare as text.
fn cmp_identifier(left: &str, right: &str) -> Ordering {
    match (is_numeric(left), is_numeric(right)) {
        // Numbers have no leading zero, so length then text is numeric order.
        (true, true) => (left.len(), left).cmp(&(right.len(), right)),
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
        (false, false) => left.cmp(right),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn version(text: &str) -> Version {
        Version::parse(text).unwrap()
    }

    #[test]
    fn parses_tags_and_plain_versions() {
        assert_eq!(version("v1.2.3"), Version { major: 1, minor: 2, patch: 3, pre: None });
        assert_eq!(version("1.2.3"), version("v1.2.3"));
        assert_eq!(version("V1.2.3"), version("v1.2.3"));
        assert_eq!(version("1.2.3-beta.1").pre.as_deref(), Some("beta.1"));
        assert_eq!(version("v1.2.3-rc-2+build.7"), version("1.2.3-rc-2"));
        assert_eq!(version("1.2.3+sha.abc"), version("1.2.3"));
    }

    #[test]
    fn rejects_what_is_not_a_version() {
        for text in [
            "",
            "v",
            "1",
            "1.2",
            "1.2.3.4",
            "1.x.3",
            "1..3",
            "01.2.3",
            "1.2.3 ",
            "1.2.3-",
            "1.2.3-beta..1",
            "1.2.3-01",
            "99999999999999999999.0.0",
            "latest",
        ] {
            assert_eq!(Version::parse(text), Err(UpdateError::InvalidVersion(text.to_owned())), "{text:?} should be refused");
        }
    }

    #[test]
    fn displays_what_it_parsed() {
        assert_eq!(version("v1.2.3").to_string(), "1.2.3");
        assert_eq!(version("v1.2.3-beta.1+sha").to_string(), "1.2.3-beta.1");
    }

    #[test]
    fn orders_like_semver() {
        // The example order from the SemVer 2.0 specification, ascending.
        let ascending =
            ["1.0.0-alpha", "1.0.0-alpha.1", "1.0.0-alpha.beta", "1.0.0-beta", "1.0.0-beta.2", "1.0.0-beta.11", "1.0.0-rc.1", "1.0.0"];
        for pair in ascending.windows(2) {
            assert!(version(pair[0]) < version(pair[1]), "{} should sort before {}", pair[0], pair[1]);
        }
    }

    #[test]
    fn pre_release_sorts_before_its_release() {
        assert!(version("0.2.0-beta.1") < version("0.2.0"));
        assert!(version("0.1.9") < version("0.2.0-beta.1"));
        assert!(version("0.2.0") > version("0.2.0-beta.1"));
    }

    #[test]
    fn numbers_compare_as_numbers() {
        assert!(version("1.10.0") > version("1.9.0"));
        assert!(version("1.2.10") > version("1.2.9"));
        assert!(version("1.2.3-beta.10") > version("1.2.3-beta.9"));
    }

    #[test]
    fn equal_versions_compare_equal() {
        assert_eq!(version("1.2.3-rc.1").cmp(&version("v1.2.3-rc.1+build")), Ordering::Equal);
    }
}
