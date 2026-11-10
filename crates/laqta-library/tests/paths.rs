#![allow(clippy::unwrap_used)]

use std::fs;
use std::path::Path;

use laqta_library::data_dir;

#[test]
fn portable_marker_moves_data_next_to_the_executable() {
    let exe_dir = tempfile::tempdir().unwrap();
    let exe = exe_dir.path().join("laqta");
    let default = tempfile::tempdir().unwrap().path().join("AppData").join("Laqta");

    assert_eq!(data_dir(Some(&exe), default.clone()), default);
    assert_eq!(data_dir(None, default.clone()), default);

    fs::write(exe_dir.path().join("portable"), "").unwrap();
    assert_eq!(data_dir(Some(&exe), default.clone()), exe_dir.path().join("LaqtaData"));
    assert_eq!(data_dir(None, default.clone()), default);
}

#[test]
fn a_portable_named_executable_keeps_its_data_beside_it() {
    let default = Path::new("/home/me/.local/share/Laqta").to_path_buf();
    let exe = Path::new("/usb/laqta-windows-x64-portable.exe");
    assert_eq!(data_dir(Some(exe), default.clone()), Path::new("/usb/LaqtaData"));
    assert_eq!(data_dir(Some(Path::new("/opt/laqta-nowhere/laqta")), default.clone()), default);
}
