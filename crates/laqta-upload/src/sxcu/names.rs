//! ShareX expands its file-name tokens (`%y`, `%mo`, `%rn`, …) in a custom uploader's arguments,
//! parameters and headers before the `{…}` syntax runs. The request URL and `Data` body keep their
//! `%` as written.

use std::sync::OnceLock;

use laqta_domain::naming::{NameContext, expand};
use laqta_ports::{HostInfo, Rng};

/// The user and computer names for `%un`, `%uln` and `%cn`; `main` sets them once at start-up.
static HOST: OnceLock<HostInfo> = OnceLock::new();

/// Sets the names `%un`, `%uln` and `%cn` expand to. Later calls are ignored.
pub fn set_host_info(host: HostInfo) {
    let _ = HOST.set(host);
}

/// Each value with its name tokens expanded at the current local time. Keys stay as written.
pub(super) fn expand_values(pairs: &[(String, String)], rng: &dyn Rng) -> Vec<(String, String)> {
    pairs.iter().map(|(key, value)| (key.clone(), expand_one(value, rng))).collect()
}

fn expand_one(value: &str, rng: &dyn Rng) -> String {
    if !value.contains('%') {
        return value.to_owned();
    }
    let now = jiff::Zoned::now();
    let host = HOST.get().cloned().unwrap_or_default();
    let mut next = || rng.next_u64();
    let no_files = |_: &str| -> Vec<String> { Vec::new() };
    let mut ctx = NameContext {
        now: now.datetime(),
        unix_seconds: now.timestamp().as_second(),
        window_title: "",
        process_name: "",
        width: None,
        height: None,
        counter: 0,
        user_name: &host.user_name,
        user_login: &host.user_login,
        computer_name: &host.computer_name,
        read_lines: &no_files,
        rng: &mut next,
    };
    expand(value, &mut ctx)
}
