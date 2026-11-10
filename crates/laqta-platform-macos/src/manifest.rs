//! Browser native-messaging manifests: their folders and contents. Pure.
//!
//! The host name and the allowed caller ids match the Linux and Windows adapters, so one browser
//! extension talks to the same host on every platform.

use std::path::{Path, PathBuf};

/// The native-messaging host name the browsers look up, and the callers it accepts.
const HOST_NAME: &str = "com.laqta.nmh";
const CHROME_ORIGIN: &str = "chrome-extension://nlkoigbdolhchiicbonbihbphgamnaoc/";
const FIREFOX_ID: &str = "ShareX@ShareX";

/// A browser looks for `<host name>.json` in its folder.
const MANIFEST_FILE: &str = "com.laqta.nmh.json";

/// The data folder of each Chromium-based browser below `~/Library/Application Support`. The
/// browser creates it on first run, and looks for hosts in its `NativeMessagingHosts` subfolder.
const CHROMIUM_FOLDERS: [&str; 5] = ["Google/Chrome", "Chromium", "Microsoft Edge", "BraveSoftware/Brave-Browser", "Vivaldi"];

/// Firefox reads hosts from `Mozilla/NativeMessagingHosts` but keeps its profiles in `Firefox`.
const FIREFOX_HOSTS_FOLDER: &str = "Mozilla/NativeMessagingHosts";
const FIREFOX_PROFILE_FOLDER: &str = "Firefox";

fn support_dir(home: &Path) -> PathBuf {
    home.join("Library").join("Application Support")
}

/// The `NativeMessagingHosts` folders of Chrome, Chromium, Edge, Brave, Vivaldi and Firefox
/// (in that order) below `home`.
pub(crate) fn browser_dirs(home: &Path) -> Vec<PathBuf> {
    let support = support_dir(home);
    let mut dirs: Vec<PathBuf> = CHROMIUM_FOLDERS.iter().map(|folder| support.join(folder).join("NativeMessagingHosts")).collect();
    dirs.push(support.join(FIREFOX_HOSTS_FOLDER));
    dirs
}

/// The manifest files to write for `nmh`, as (path, contents), for the browsers that have been
/// run on this Mac: `exists` says whether a folder is there. Writing for a browser that was never
/// installed would only leave stray folders behind.
pub(crate) fn manifest_files(home: &Path, nmh: &Path, exists: impl Fn(&Path) -> bool) -> Vec<(PathBuf, String)> {
    let mut dirs = browser_dirs(home);
    // `browser_dirs` ends with Firefox's folder.
    let firefox = dirs.pop();
    let mut files = Vec::new();
    for dir in dirs {
        if dir.parent().is_some_and(&exists) {
            files.push((dir.join(MANIFEST_FILE), chromium_manifest(nmh)));
        }
    }
    if let Some(dir) = firefox {
        let run = dir.parent().is_some_and(&exists) || exists(&support_dir(home).join(FIREFOX_PROFILE_FOLDER));
        if run {
            files.push((dir.join(MANIFEST_FILE), firefox_manifest(nmh)));
        }
    }
    files
}

/// The manifest Chromium-based browsers read: callers are named by extension origin.
pub(crate) fn chromium_manifest(nmh: &Path) -> String {
    manifest(nmh, &format!("\"allowed_origins\": [{}]", json_string(CHROME_ORIGIN)))
}

/// The manifest Firefox reads: callers are named by extension id.
pub(crate) fn firefox_manifest(nmh: &Path) -> String {
    manifest(nmh, &format!("\"allowed_extensions\": [{}]", json_string(FIREFOX_ID)))
}

fn manifest(nmh: &Path, allowed: &str) -> String {
    format!(
        "{{\n  \"name\": {},\n  \"description\": \"Laqta\",\n  \"path\": {},\n  \"type\": \"stdio\",\n  {allowed}\n}}\n",
        json_string(HOST_NAME),
        json_string(&nmh.to_string_lossy()),
    )
}

/// `text` as a JSON string literal, with quotes, backslashes and control characters escaped.
fn json_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if u32::from(c) < 0x20 => out.push_str(&format!("\\u{:04x}", u32::from(c))),
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::path::{Path, PathBuf};

    use super::{browser_dirs, chromium_manifest, firefox_manifest, json_string, manifest_files};

    const HOME: &str = "/Users/ana";
    const SUPPORT: &str = "/Users/ana/Library/Application Support";

    #[test]
    fn browser_dirs_list_the_six_browsers_under_application_support() {
        let dirs = browser_dirs(Path::new(HOME));
        let expected: Vec<PathBuf> = [
            "Google/Chrome/NativeMessagingHosts",
            "Chromium/NativeMessagingHosts",
            "Microsoft Edge/NativeMessagingHosts",
            "BraveSoftware/Brave-Browser/NativeMessagingHosts",
            "Vivaldi/NativeMessagingHosts",
            "Mozilla/NativeMessagingHosts",
        ]
        .iter()
        .map(|rest| Path::new(SUPPORT).join(rest))
        .collect();
        assert_eq!(dirs, expected);
    }

    #[test]
    fn chromium_manifest_names_the_host_and_the_extension_origin() {
        let text = chromium_manifest(Path::new("/Applications/Laqta.app/Contents/MacOS/laqta-nmh"));
        assert_eq!(
            text,
            "{\n  \"name\": \"com.laqta.nmh\",\n  \"description\": \"Laqta\",\n  \"path\": \"/Applications/Laqta.app/Contents/MacOS/laqta-nmh\",\n  \"type\": \"stdio\",\n  \"allowed_origins\": [\"chrome-extension://nlkoigbdolhchiicbonbihbphgamnaoc/\"]\n}\n"
        );
    }

    #[test]
    fn firefox_manifest_names_the_host_and_the_extension_id() {
        let text = firefox_manifest(Path::new("/opt/nmh"));
        assert_eq!(
            text,
            "{\n  \"name\": \"com.laqta.nmh\",\n  \"description\": \"Laqta\",\n  \"path\": \"/opt/nmh\",\n  \"type\": \"stdio\",\n  \"allowed_extensions\": [\"ShareX@ShareX\"]\n}\n"
        );
        assert!(!text.contains("allowed_origins"));
    }

    #[test]
    fn a_path_with_quotes_and_backslashes_is_escaped() {
        let text = chromium_manifest(Path::new(r#"/Volumes/a "b"/c\d/nmh"#));
        assert!(text.contains(r#""path": "/Volumes/a \"b\"/c\\d/nmh","#), "{text}");
        assert_eq!(json_string(r#"a"b\c"#), r#""a\"b\\c""#);
        assert_eq!(json_string("tab\tnl\n\u{1}"), r#""tab\u0009nl\u000a\u0001""#);
        assert_eq!(json_string("Zoë/日本"), "\"Zoë/日本\"");
    }

    #[test]
    fn manifests_are_written_only_for_browsers_that_have_been_run() {
        let present: HashSet<PathBuf> = [format!("{SUPPORT}/Google/Chrome"), format!("{SUPPORT}/Vivaldi"), format!("{SUPPORT}/Firefox")]
            .into_iter()
            .map(PathBuf::from)
            .collect();
        let files = manifest_files(Path::new(HOME), Path::new("/opt/nmh"), |dir| present.contains(dir));
        let paths: Vec<&Path> = files.iter().map(|(path, _)| path.as_path()).collect();
        assert_eq!(
            paths,
            [
                Path::new("/Users/ana/Library/Application Support/Google/Chrome/NativeMessagingHosts/com.laqta.nmh.json"),
                Path::new("/Users/ana/Library/Application Support/Vivaldi/NativeMessagingHosts/com.laqta.nmh.json"),
                Path::new("/Users/ana/Library/Application Support/Mozilla/NativeMessagingHosts/com.laqta.nmh.json"),
            ]
        );
        assert_eq!(files[0].1, chromium_manifest(Path::new("/opt/nmh")));
        assert_eq!(files[2].1, firefox_manifest(Path::new("/opt/nmh")));
    }

    #[test]
    fn no_browser_means_no_manifests() {
        assert!(manifest_files(Path::new(HOME), Path::new("/opt/nmh"), |_| false).is_empty());
    }

    #[test]
    fn firefox_counts_as_run_when_only_the_mozilla_folder_exists() {
        let mozilla = PathBuf::from(format!("{SUPPORT}/Mozilla"));
        let files = manifest_files(Path::new(HOME), Path::new("/opt/nmh"), |dir| dir == mozilla);
        assert_eq!(files.len(), 1);
        assert!(files[0].0.ends_with("Mozilla/NativeMessagingHosts/com.laqta.nmh.json"));
    }
}
