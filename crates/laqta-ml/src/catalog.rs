//! The model catalog (ADR 023): the entries that ship with a release, parsed from
//! `catalog/models.toml` and checked on load.

use std::sync::OnceLock;

use serde::Deserialize;

use crate::error::MlError;
use crate::families::Family;

const CATALOG_TOML: &str = include_str!("../catalog/models.toml");

/// The SHA-256 placeholder for an entry whose hash is not yet confirmed. Downloads are refused.
pub const PENDING_HASH: &str = "pending-owner";

/// What a model is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Task {
    OcrDet,
    OcrRec,
    Background,
}

impl Task {
    /// The name used in the catalog and in the session cache.
    pub fn as_str(self) -> &'static str {
        match self {
            Task::OcrDet => "ocr-det",
            Task::OcrRec => "ocr-rec",
            Task::Background => "background",
        }
    }
}

/// One model in the catalog.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ModelEntry {
    pub id: String,
    pub task: Task,
    pub family: String,
    #[serde(default)]
    pub default: bool,
    pub name: String,
    pub description: String,
    pub url: String,
    pub sha256: String,
    pub size_bytes: u64,
    pub licence: String,
    pub commercial: bool,
    #[serde(default)]
    pub precisions: Vec<String>,
    #[serde(default)]
    pub languages: Vec<String>,
    #[serde(default)]
    pub dict_url: Option<String>,
    #[serde(default)]
    pub dict_sha256: Option<String>,
    #[serde(default)]
    pub dict_size_bytes: Option<u64>,
}

impl ModelEntry {
    /// True if the hash is still the placeholder, so the file cannot be downloaded yet.
    pub fn is_pending(&self) -> bool {
        self.sha256 == PENDING_HASH || self.dict_sha256.as_deref() == Some(PENDING_HASH)
    }
}

/// One ONNX Runtime archive, per operating system.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct OrtArchive {
    pub os: String,
    pub arch: String,
    pub version: String,
    pub url: String,
    pub sha256: String,
    pub size_bytes: u64,
    pub archive: String,
    pub library: String,
}

/// The parsed catalog.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Catalog {
    #[serde(rename = "model", default)]
    pub models: Vec<ModelEntry>,
    #[serde(rename = "onnxruntime", default)]
    pub onnxruntime: Vec<OrtArchive>,
}

impl Catalog {
    /// Parses and checks a catalog. The checks are the ones the shipped file must pass: unique ids
    /// with file-safe characters, known families for the task, HTTPS URLs, valid hashes, dictionaries
    /// on recognisers only, and one default per task.
    pub fn parse(text: &str) -> Result<Self, MlError> {
        let catalog: Catalog = toml::from_str(text).map_err(|error| MlError::Catalog(error.to_string()))?;
        catalog.validate()?;
        Ok(catalog)
    }

    /// The entry with this id.
    pub fn model(&self, id: &str) -> Option<&ModelEntry> {
        self.models.iter().find(|model| model.id == id)
    }

    /// The entries for one task, in catalog order.
    pub fn models_for(&self, task: Task) -> impl Iterator<Item = &ModelEntry> {
        self.models.iter().filter(move |model| model.task == task)
    }

    /// The default entry for a task.
    pub fn default_for(&self, task: Task) -> Option<&ModelEntry> {
        self.models_for(task).find(|model| model.default)
    }

    /// The ONNX Runtime archive for an OS and architecture.
    pub fn onnxruntime_for(&self, os: &str, arch: &str) -> Option<&OrtArchive> {
        self.onnxruntime.iter().find(|archive| archive.os == os && archive.arch == arch)
    }

    fn validate(&self) -> Result<(), MlError> {
        let mut ids = std::collections::HashSet::new();
        for model in &self.models {
            let fail = |detail: String| MlError::Catalog(format!("model `{}`: {detail}", model.id));
            if !is_file_safe_id(&model.id) {
                return Err(fail("the id may only use a-z, 0-9 and '-'".into()));
            }
            if !ids.insert(model.id.as_str()) {
                return Err(fail("duplicate id".into()));
            }
            let family = Family::parse(&model.family).ok_or_else(|| fail(format!("unknown family `{}`", model.family)))?;
            if family.task() != model.task {
                return Err(fail(format!("family `{}` serves {}, not {}", model.family, family.task().as_str(), model.task.as_str())));
            }
            check_url(&model.url).map_err(fail)?;
            check_hash(&model.sha256).map_err(fail)?;
            if model.size_bytes == 0 {
                return Err(fail("size_bytes must be set".into()));
            }
            match model.task {
                Task::OcrRec => {
                    let dict_url = model.dict_url.as_deref().ok_or_else(|| fail("a recogniser needs dict_url".into()))?;
                    check_url(dict_url).map_err(fail)?;
                    let dict_hash = model.dict_sha256.as_deref().ok_or_else(|| fail("a recogniser needs dict_sha256".into()))?;
                    check_hash(dict_hash).map_err(fail)?;
                    if model.dict_size_bytes.is_none() {
                        return Err(fail("a recogniser needs dict_size_bytes".into()));
                    }
                    if model.languages.is_empty() {
                        return Err(fail("a recogniser needs at least one language or script".into()));
                    }
                }
                _ => {
                    if model.dict_url.is_some() || model.dict_sha256.is_some() || model.dict_size_bytes.is_some() {
                        return Err(fail("only a recogniser has a dictionary".into()));
                    }
                }
            }
        }
        for task in [Task::OcrDet, Task::OcrRec, Task::Background] {
            let entries: Vec<&ModelEntry> = self.models_for(task).collect();
            let defaults = entries.iter().filter(|model| model.default).count();
            if !entries.is_empty() && defaults != 1 {
                return Err(MlError::Catalog(format!("task {} needs exactly one default, found {defaults}", task.as_str())));
            }
        }
        for archive in &self.onnxruntime {
            let fail = |detail: String| MlError::Catalog(format!("onnxruntime {}-{}: {detail}", archive.os, archive.arch));
            check_url(&archive.url).map_err(fail)?;
            check_hash(&archive.sha256).map_err(fail)?;
            if !matches!(archive.archive.as_str(), "tgz" | "zip") {
                return Err(fail(format!("unknown archive `{}`", archive.archive)));
            }
            if archive.library.is_empty() {
                return Err(fail("library path is empty".into()));
            }
        }
        Ok(())
    }
}

fn is_file_safe_id(id: &str) -> bool {
    !id.is_empty() && id.bytes().all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn check_url(url: &str) -> Result<(), String> {
    if url.starts_with("https://") { Ok(()) } else { Err(format!("the URL must be HTTPS, got `{url}`")) }
}

fn check_hash(hash: &str) -> Result<(), String> {
    if hash == PENDING_HASH || (hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit())) {
        Ok(())
    } else {
        Err(format!("sha256 must be 64 hex digits or \"{PENDING_HASH}\", got `{hash}`"))
    }
}

/// The catalog that ships in this build. It is checked by the tests, so a bad file fails there and
/// not at run time.
pub fn catalog() -> &'static Catalog {
    static CATALOG: OnceLock<Catalog> = OnceLock::new();
    CATALOG.get_or_init(|| match Catalog::parse(CATALOG_TOML) {
        Ok(catalog) => catalog,
        Err(error) => panic!("the embedded model catalog is invalid: {error}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shipped_catalog_is_valid() {
        let catalog = catalog();
        assert!(!catalog.models.is_empty());
        assert!(catalog.default_for(Task::OcrDet).is_some());
        assert!(catalog.default_for(Task::OcrRec).is_some());
    }

    #[test]
    fn detection_has_no_dictionary_and_recognisers_do() {
        for model in &catalog().models {
            match model.task {
                Task::OcrRec => assert!(model.dict_url.is_some(), "{} needs a dictionary", model.id),
                _ => assert!(model.dict_url.is_none(), "{} has a stray dictionary", model.id),
            }
        }
    }

    #[test]
    fn the_shipped_onnxruntime_is_listed_for_both_systems() {
        assert!(catalog().onnxruntime_for("linux", "x86_64").is_some());
        assert!(catalog().onnxruntime_for("windows", "x86_64").is_some());
    }

    #[test]
    fn a_pending_hash_is_reported_as_pending() {
        let text = CATALOG_TOML.replacen(
            "sha256 = \"a431985659dc921974177a95adcfbb90fd9e51989a5e04d70d0b75f597b6e61d\"",
            "sha256 = \"pending-owner\"",
            1,
        );
        let catalog = Catalog::parse(&text).expect("pending is a valid placeholder");
        assert!(catalog.model("ppocr-v5-mobile-det").expect("entry").is_pending());
        assert!(!catalog.model("ppocr-v5-mobile-rec-multi").expect("entry").is_pending());
    }

    #[test]
    fn a_bad_hash_is_rejected() {
        let text = CATALOG_TOML.replacen(
            "sha256 = \"a431985659dc921974177a95adcfbb90fd9e51989a5e04d70d0b75f597b6e61d\"",
            "sha256 = \"not-a-hash\"",
            1,
        );
        assert!(matches!(Catalog::parse(&text), Err(MlError::Catalog(_))));
    }

    #[test]
    fn a_recogniser_without_a_dictionary_is_rejected() {
        let text = CATALOG_TOML.replacen("dict_size_bytes = 2616\n", "", 1);
        assert!(matches!(Catalog::parse(&text), Err(MlError::Catalog(_))));
    }

    #[test]
    fn a_family_for_the_wrong_task_is_rejected() {
        let text = CATALOG_TOML.replacen("family = \"o1-det\"", "family = \"o1-rec\"", 1);
        assert!(matches!(Catalog::parse(&text), Err(MlError::Catalog(_))));
    }

    #[test]
    fn a_plain_http_url_is_rejected() {
        let text = CATALOG_TOML.replacen(
            "url = \"https://huggingface.co/PaddlePaddle/PP-OCRv5_mobile_det_onnx",
            "url = \"http://example.test/x",
            1,
        );
        assert!(matches!(Catalog::parse(&text), Err(MlError::Catalog(_))));
    }

    #[test]
    fn two_defaults_for_one_task_are_rejected() {
        let text = CATALOG_TOML.replacen("default = false", "default = true", 1);
        assert!(matches!(Catalog::parse(&text), Err(MlError::Catalog(_))));
    }
}
