#![allow(clippy::unwrap_used)]

use std::fs;

use laqta_library::data_dir;

#[test]
fn portable_marker_moves_data_next_to_the_executable() {
    let exe_dir = tempfile::tempdir().unwrap();
    let default = tempfile::tempdir()
        .unwrap()
        .path()
        .join("AppData")
        .join("Laqta");

    assert_eq!(data_dir(Some(exe_dir.path()), default.clone()), default);
    assert_eq!(data_dir(None, default.clone()), default);

    fs::write(exe_dir.path().join("portable"), "").unwrap();
    assert_eq!(
        data_dir(Some(exe_dir.path()), default.clone()),
        exe_dir.path().join("LaqtaData")
    );
    assert_eq!(data_dir(None, default.clone()), default);
}
