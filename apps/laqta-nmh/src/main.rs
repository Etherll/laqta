//! The browser native-messaging host (M6 D2). Chrome and Firefox start it and send one message:
//! a 4-byte little-endian length and UTF-8 JSON. It forwards the request to the running Laqta over
//! IPC (starting Laqta with `-silent` when none runs) and replies the same way.

use std::io::{self, Read, Write};
use std::process::ExitCode;
use std::time::Duration;

use laqta_ipc::{Claim, Request};

/// ShareX's extension message.
#[derive(Debug, Default, serde::Deserialize)]
#[serde(default, rename_all = "PascalCase")]
struct Message {
    /// 1 = upload an image URL, 2 = upload text, 3 = shorten a URL, 4 = upload a URL (download and reupload).
    action: u32,
    #[serde(rename = "URL")]
    url: String,
    text: String,
}

const MAX_MESSAGE: u32 = 1024 * 1024;

fn read_message(input: &mut impl Read) -> io::Result<Vec<u8>> {
    let mut len = [0u8; 4];
    input.read_exact(&mut len)?;
    let len = u32::from_le_bytes(len);
    if len > MAX_MESSAGE {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "message too large"));
    }
    let mut buf = vec![0u8; len as usize];
    input.read_exact(&mut buf)?;
    Ok(buf)
}

fn write_message(out: &mut impl Write, value: &serde_json::Value) -> io::Result<()> {
    let bytes = serde_json::to_vec(value)?;
    out.write_all(&(bytes.len() as u32).to_le_bytes())?;
    out.write_all(&bytes)?;
    out.flush()
}

fn to_request(m: &Message) -> Result<Request, String> {
    match m.action {
        1 | 4 if !m.url.is_empty() => Ok(Request::UploadUrl(m.url.clone())),
        2 if !m.text.is_empty() => Ok(Request::UploadText(m.text.clone())),
        3 if !m.url.is_empty() => Ok(Request::ShortenUrl(m.url.clone())),
        _ => Err(format!("unsupported message (action {})", m.action)),
    }
}

fn client() -> Result<laqta_ipc::Client, String> {
    for attempt in 0..20 {
        match laqta_ipc::claim().map_err(|e| e.to_string())? {
            Claim::Secondary(c) => return Ok(c),
            Claim::Primary(server) => {
                // No Laqta is running: release the endpoint and start one.
                drop(server);
                if attempt == 0 {
                    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
                    let laqta = exe.with_file_name(if cfg!(windows) { "laqta.exe" } else { "laqta" });
                    std::process::Command::new(laqta).arg("-silent").spawn().map_err(|e| format!("cannot start Laqta: {e}"))?;
                }
                std::thread::sleep(Duration::from_millis(250));
            }
        }
    }
    Err("Laqta did not start".into())
}

fn run() -> Result<(), String> {
    let raw = read_message(&mut io::stdin().lock()).map_err(|e| e.to_string())?;
    let message: Message = serde_json::from_slice(&raw).map_err(|e| format!("bad message: {e}"))?;
    let request = to_request(&message)?;
    client()?.send_request(&request)
}

fn main() -> ExitCode {
    let reply = match run() {
        Ok(()) => serde_json::json!({ "Status": "OK" }),
        Err(e) => serde_json::json!({ "Status": "Error", "Message": e }),
    };
    match write_message(&mut io::stdout().lock(), &reply) {
        Ok(()) => ExitCode::SUCCESS,
        Err(_) => ExitCode::FAILURE,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn framing_round_trip() {
        let mut buf = Vec::new();
        write_message(&mut buf, &serde_json::json!({"Action": 2, "Text": "hi"})).unwrap();
        let raw = read_message(&mut buf.as_slice()).unwrap();
        let m: Message = serde_json::from_slice(&raw).unwrap();
        assert_eq!(to_request(&m).unwrap(), Request::UploadText("hi".into()));
    }

    #[test]
    fn maps_actions() {
        let m = |a: u32, url: &str| Message { action: a, url: url.into(), text: String::new() };
        assert_eq!(to_request(&m(1, "https://a/b.png")).unwrap(), Request::UploadUrl("https://a/b.png".into()));
        assert_eq!(to_request(&m(3, "https://a")).unwrap(), Request::ShortenUrl("https://a".into()));
        assert!(to_request(&m(9, "x")).is_err());
        assert!(to_request(&m(2, "")).is_err());
    }

    #[test]
    fn rejects_huge_messages() {
        let mut buf = (MAX_MESSAGE + 1).to_le_bytes().to_vec();
        buf.extend([0; 8]);
        assert!(read_message(&mut buf.as_slice()).is_err());
    }
}
