//! The bundled `.sxcu` catalog and the destination registry: every file parses, ids are unique,
//! every template builds a request, and the registry routes each kind to the right map.
#![allow(clippy::unwrap_used)]

use std::collections::{BTreeSet, HashSet};
use std::path::Path;
use std::sync::Arc;

use futures_lite::future::block_on;
use laqta_ports::fakes::{CountingRng, FakeDestination, FakePrompt};
use laqta_upload::catalog::{CATALOG, catalog};
use laqta_upload::registry::Registry;
use laqta_upload::sxcu::{CustomUploader, DestinationType, Env, Input, SxcuError, build_request};

/// The M2 catalog hosts (FEATURES.md), by the `Name` each file gives.
const M2_NAMES: &[&str] = &[
    "Chevereto",
    "vgy.me",
    "ImageBin",
    "Img1",
    "Pastebin",
    "Paste.ee",
    "Hastebin",
    "OneTimeSecret",
    "Pastie",
    "uPaste",
    "Slexy",
    "Paste2",
    "Pomf",
    "Uguu",
    "transfer.sh",
    "FileBin",
    "Plik",
    "Lambda",
    "s-ul",
    "img.fish",
    "Puush",
    "Immich",
    "Seafile",
    "Streamable",
    "YOURLS",
    "Polr",
    "Kutt",
    "Zero Width",
    "is.gd",
    "v.gd",
    "TinyURL",
    "TURL",
    "2.gp",
    "qr.net",
    "vurl",
    "nlcm",
];

/// The catalog ids of the shorteners.
const SHORTENER_IDS: &[&str] = &[
    "sxcu:yourls",
    "sxcu:polr",
    "sxcu:kutt",
    "sxcu:zero-width",
    "sxcu:is-gd",
    "sxcu:v-gd",
    "sxcu:tinyurl",
    "sxcu:turl",
    "sxcu:2-gp",
    "sxcu:qr-net",
    "sxcu:vurl",
    "sxcu:nlcm",
];

fn registry() -> Registry {
    Registry::with_catalog(Arc::new(FakePrompt::default()), Arc::new(CountingRng::default()))
}

const CUSTOM: &str = r#"{
  "Version": "15.0.0",
  "Name": "My Host",
  "DestinationType": "ImageUploader",
  "RequestURL": "https://example.org/up",
  "Body": "MultipartFormData",
  "FileFormName": "f",
  "URL": "{json:link}"
}"#;

#[test]
fn every_catalog_file_parses_and_names_a_destination_type() {
    for (stem, json) in CATALOG {
        let uploader = CustomUploader::parse(json).unwrap_or_else(|error| panic!("{stem}: {error}"));
        assert!(!uploader.destination_types().is_empty(), "{stem} names no DestinationType");
    }
}

#[test]
fn stems_and_ids_are_unique() {
    let stems: HashSet<&str> = CATALOG.iter().map(|(stem, _)| *stem).collect();
    assert_eq!(stems.len(), CATALOG.len());
    let ids: HashSet<String> = catalog().into_iter().map(|(id, _)| id).collect();
    assert_eq!(ids.len(), CATALOG.len());
    assert!(ids.iter().all(|id| id.starts_with("sxcu:")));
}

#[test]
fn every_file_in_the_catalog_directory_is_in_the_table() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("catalog");
    let on_disk: BTreeSet<String> = std::fs::read_dir(&dir)
        .expect("the catalog directory")
        .map(|entry| entry.expect("a directory entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "sxcu"))
        .map(|path| path.file_stem().unwrap().to_string_lossy().into_owned())
        .collect();
    let in_table: BTreeSet<String> = CATALOG.iter().map(|(stem, _)| stem.to_string()).collect();
    assert_eq!(on_disk, in_table);
}

#[test]
fn the_catalog_has_every_m2_host() {
    let names: BTreeSet<String> = catalog().into_iter().filter_map(|(_, uploader)| uploader.name).collect();
    let expected: BTreeSet<String> = M2_NAMES.iter().map(|name| name.to_string()).collect();
    assert_eq!(names, expected);
}

#[test]
fn every_template_builds_a_request() {
    let prompt = FakePrompt::default();
    let rng = CountingRng::default();
    let env = Env { prompt: &prompt, rng: &rng };
    let input = Input { file_name: "shot.txt", data: b"hello", mime: "text/plain", text: Some("hello") };
    for (id, uploader) in catalog() {
        let built = block_on(build_request(&uploader, &input, &env));
        assert!(built.is_ok(), "{id}: {:?}", built.err());
    }
}

#[test]
fn with_catalog_routes_each_kind_to_its_map() {
    let registry = registry();
    assert_eq!(registry.destination_ids().len(), 24, "12 image and text hosts and 12 file hosts");
    assert_eq!(registry.destination("sxcu:vgy-me").unwrap().name(), "vgy.me");
    assert!(registry.destination("sxcu:is-gd").is_none(), "a shortener is not a destination");
    assert!(registry.destination("sxcu:nope").is_none());
    for id in SHORTENER_IDS {
        assert!(registry.shortener(id).is_some(), "{id} is not a shortener");
        assert!(registry.destination(id).is_none(), "{id} is also a destination");
    }
}

#[test]
fn every_catalog_id_resolves_for_its_kinds() {
    let registry = registry();
    for (id, uploader) in catalog() {
        let kinds = uploader.destination_types();
        if kinds.iter().any(|kind| matches!(kind, DestinationType::Image | DestinationType::Text | DestinationType::File)) {
            assert!(registry.destination(&id).is_some(), "{id} is not a destination");
        }
        if kinds.contains(&DestinationType::UrlShortener) {
            assert!(registry.shortener(&id).is_some(), "{id} is not a shortener");
        }
    }
}

#[test]
fn add_custom_registers_the_file_under_its_name() {
    let mut registry = registry();
    let id = registry.add_custom(CUSTOM).unwrap();
    assert_eq!(id, "custom:My Host");
    assert_eq!(registry.destination(&id).expect("registered").name(), "My Host");
    assert!(registry.destination_ids().contains(&(id.clone(), "My Host".to_string())));
}

#[test]
fn add_custom_with_the_same_name_replaces_the_entry() {
    let mut registry = registry();
    registry.add_custom(CUSTOM).unwrap();
    registry.add_custom(CUSTOM).unwrap();
    let copies = registry.destination_ids().iter().filter(|(id, _)| id == "custom:My Host").count();
    assert_eq!(copies, 1);
}

#[test]
fn add_custom_accepts_a_shortener_file() {
    let mut registry = registry();
    let id = registry
        .add_custom(r#"{"Name": "Short", "DestinationType": "UrlShortener", "RequestURL": "https://s.example/api?u={input}", "URL": "{json:link}"}"#)
        .unwrap();
    assert!(registry.shortener(&id).is_some());
    assert!(registry.destination(&id).is_none());
}

#[test]
fn add_custom_rejects_files_it_cannot_register() {
    let mut registry = registry();
    assert!(matches!(registry.add_custom("not json"), Err(SxcuError::Json(_))));
    assert!(matches!(registry.add_custom(r#"{"RequestURL": "https://x"}"#), Err(SxcuError::Json(_))), "no Name");
    assert!(
        matches!(registry.add_custom(r#"{"Name": "X", "RequestURL": "https://x"}"#), Err(SxcuError::Unsupported(_))),
        "no DestinationType"
    );
    assert!(
        matches!(
            registry.add_custom(r#"{"Name": "X", "RequestURL": "https://x", "DestinationType": "UrlSharingService"}"#),
            Err(SxcuError::Unsupported(_))
        ),
        "a sharing type has no registry map"
    );
    assert_eq!(registry.destination_ids().len(), 24, "nothing was added");
}

#[test]
fn add_destination_replaces_an_entry_with_the_same_id() {
    let mut registry = registry();
    registry.add_destination(Arc::new(FakeDestination::new("fake", Vec::new())));
    registry.add_destination(Arc::new(FakeDestination::new("fake", Vec::new())));
    assert!(registry.destination("fake").is_some());
    assert_eq!(registry.destination_ids().iter().filter(|(id, _)| id == "fake").count(), 1);
}
