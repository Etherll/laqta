//! The wire format and the text names of actions. One UTF-8 line per request and per reply.

use laqta_domain::Action;
use std::io::{self, BufRead};

/// Longest request or reply line, in bytes, without its line break.
const MAX_LINE_BYTES: usize = 4096;

const REQUEST_PREFIX: &str = "action ";

/// The text name of an action, as used by `--action` and the protocol.
pub fn action_name(action: Action) -> &'static str {
    match action {
        Action::PrintScreen => "print-screen",
        Action::OpenScreenshotsFolder => "open-screenshots-folder",
        Action::ExitLaqta => "exit",
        _ => "unknown",
    }
}

pub fn parse_action(name: &str) -> Option<Action> {
    match name {
        "print-screen" => Some(Action::PrintScreen),
        "open-screenshots-folder" => Some(Action::OpenScreenshotsFolder),
        "exit" => Some(Action::ExitLaqta),
        _ => None,
    }
}

/// Reads one line without its line break. `Ok(None)` means the stream ended. A line that is too
/// long, or not UTF-8, is an `InvalidData` error.
pub(crate) fn read_line(reader: &mut impl BufRead) -> io::Result<Option<String>> {
    let mut line = String::new();
    let mut limited = io::Read::take(&mut *reader, MAX_LINE_BYTES as u64);
    let read = limited.read_line(&mut line)?;
    if read == 0 {
        return Ok(None);
    }
    if !line.ends_with('\n') && read >= MAX_LINE_BYTES {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "line too long"));
    }
    Ok(Some(line.trim_end_matches(['\r', '\n']).to_owned()))
}

pub(crate) fn encode_request(action: Action) -> String {
    format!("{REQUEST_PREFIX}{}\n", action_name(action))
}

/// Parses a request line (without its line break). The error text is the reply's message.
pub(crate) fn parse_request(line: &str) -> Result<Action, String> {
    let name = line
        .strip_prefix(REQUEST_PREFIX)
        .filter(|name| !name.is_empty())
        .ok_or_else(|| "malformed request".to_owned())?;
    parse_action(name).ok_or_else(|| format!("unknown action {name}"))
}

/// Longest error message sent. Keeps a reply within the line limit the reader enforces.
const MAX_MESSAGE_BYTES: usize = 1024;

/// Encodes a reply as one line. Line breaks in an error message become spaces, and a long message
/// is cut at a character boundary.
pub(crate) fn encode_reply(result: &Result<(), String>) -> String {
    match result {
        Ok(()) => "ok\n".to_owned(),
        Err(message) => {
            let mut flat = message.replace(['\r', '\n'], " ");
            let mut end = flat.len().min(MAX_MESSAGE_BYTES);
            while !flat.is_char_boundary(end) {
                end -= 1;
            }
            flat.truncate(end);
            format!("err {flat}\n")
        }
    }
}

/// Parses a reply line (without its line break). An `err` reply becomes `Err` with its message.
pub(crate) fn parse_reply(line: &str) -> Result<(), String> {
    if line == "ok" {
        Ok(())
    } else if let Some(message) = line.strip_prefix("err ") {
        Err(message.to_owned())
    } else {
        Err(format!("malformed reply: {line}"))
    }
}
