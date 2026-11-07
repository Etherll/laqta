use interprocess::local_socket::{GenericFilePath, prelude::*};
use laqta_domain::Action;
use laqta_ipc::{Claim, action_name, claim_at, parse_action};
use std::collections::HashSet;
use std::io::{BufRead, BufReader, Write};

/// A unique endpoint inside `dir`. Windows gets a pipe name derived from the directory's name.
fn endpoint(dir: &tempfile::TempDir) -> String {
    if cfg!(windows) {
        let id = dir
            .path()
            .file_name()
            .expect("temp dir has a name")
            .to_string_lossy()
            .into_owned();
        format!(r"\\.\pipe\laqta-test-{id}")
    } else {
        dir.path().join("laqta.sock").to_string_lossy().into_owned()
    }
}

fn handler(action: Action) -> Result<(), String> {
    match action {
        Action::PrintScreen => Ok(()),
        Action::ExitLaqta => Err("nope".to_owned()),
        _ => Err("unexpected action".to_owned()),
    }
}

#[test]
fn primary_and_secondary_in_one_process() {
    let dir = tempfile::tempdir().expect("temp dir");
    let endpoint = endpoint(&dir);
    let Claim::Primary(server) = claim_at(&endpoint).expect("first claim") else {
        panic!("first claim should be primary");
    };
    let Claim::Secondary(mut client) = claim_at(&endpoint).expect("second claim") else {
        panic!("second claim should be secondary");
    };
    server.serve(handler);
    assert_eq!(client.send(Action::PrintScreen), Ok(()));
    assert_eq!(client.send(Action::ExitLaqta), Err("nope".to_owned()));
}

#[test]
fn handler_panic_becomes_an_error_reply() {
    let dir = tempfile::tempdir().expect("temp dir");
    let endpoint = endpoint(&dir);
    let Claim::Primary(server) = claim_at(&endpoint).expect("claim") else {
        panic!("claim should be primary");
    };
    server.serve(|action| match action {
        Action::OpenScreenshotsFolder => panic!("handler panics"),
        _ => Ok(()),
    });
    let Claim::Secondary(mut client) = claim_at(&endpoint).expect("second claim") else {
        panic!("second claim should be secondary");
    };
    assert_eq!(
        client.send(Action::OpenScreenshotsFolder),
        Err("internal error".to_owned())
    );
    assert_eq!(client.send(Action::PrintScreen), Ok(()));
}

#[cfg(unix)]
#[test]
fn stale_socket_is_removed_and_claimed() {
    let dir = tempfile::tempdir().expect("temp dir");
    let endpoint = endpoint(&dir);
    // std's listener leaves its socket file behind when dropped, so nothing answers at the path.
    drop(std::os::unix::net::UnixListener::bind(&endpoint).expect("bind stale socket"));
    let Claim::Primary(server) = claim_at(&endpoint).expect("claim over stale socket") else {
        panic!("claim over a stale socket should be primary");
    };
    server.serve(handler);
    let Claim::Secondary(mut client) = claim_at(&endpoint).expect("second claim") else {
        panic!("second claim should be secondary");
    };
    assert_eq!(client.send(Action::PrintScreen), Ok(()));
}

#[test]
fn raw_requests_get_protocol_replies() {
    let dir = tempfile::tempdir().expect("temp dir");
    let endpoint = endpoint(&dir);
    let Claim::Primary(server) = claim_at(&endpoint).expect("claim") else {
        panic!("claim should be primary");
    };
    server.serve(handler);
    let name = endpoint
        .as_str()
        .to_fs_name::<GenericFilePath>()
        .expect("endpoint name");
    let mut stream = BufReader::new(LocalSocketStream::connect(name).expect("connect"));
    for (request, reply) in [
        ("action bogus\n", "err unknown action bogus\n"),
        ("hello\n", "err malformed request\n"),
        ("action\n", "err malformed request\n"),
        ("action print-screen\r\n", "ok\n"),
    ] {
        stream
            .get_mut()
            .write_all(request.as_bytes())
            .expect("write request");
        let mut line = String::new();
        stream.read_line(&mut line).expect("read reply");
        assert_eq!(line, reply, "reply to {request:?}");
    }
}

#[test]
fn long_error_message_is_cut_to_one_line() {
    let dir = tempfile::tempdir().expect("temp dir");
    let endpoint = endpoint(&dir);
    let Claim::Primary(server) = claim_at(&endpoint).expect("claim") else {
        panic!("claim should be primary");
    };
    server.serve(|action| match action {
        Action::OpenScreenshotsFolder => Err("x".repeat(5000)),
        _ => Ok(()),
    });
    let Claim::Secondary(mut client) = claim_at(&endpoint).expect("second claim") else {
        panic!("second claim should be secondary");
    };
    let error = client
        .send(Action::OpenScreenshotsFolder)
        .expect_err("error reply");
    assert!(
        error.len() < 5000,
        "message should be cut, got {} bytes",
        error.len()
    );
    assert_eq!(client.send(Action::PrintScreen), Ok(()));
}

#[cfg(unix)]
#[test]
fn overlong_request_gets_malformed_reply_and_hangs_up() {
    let dir = tempfile::tempdir().expect("temp dir");
    let endpoint = endpoint(&dir);
    let Claim::Primary(server) = claim_at(&endpoint).expect("claim") else {
        panic!("claim should be primary");
    };
    server.serve(handler);
    let name = endpoint
        .as_str()
        .to_fs_name::<GenericFilePath>()
        .expect("endpoint name");
    let mut stream = BufReader::new(LocalSocketStream::connect(name).expect("connect"));
    let request = format!("action {}\n", "x".repeat(8192));
    stream
        .get_mut()
        .write_all(request.as_bytes())
        .expect("write request");
    let mut line = String::new();
    stream.read_line(&mut line).expect("read reply");
    assert_eq!(line, "err malformed request\n");
    // The server hangs up with unread bytes queued, so the peer may see a reset instead of EOF.
    line.clear();
    let _ = stream.read_line(&mut line);
    assert!(line.is_empty(), "connection should be closed, got {line:?}");
}

/// CamelCase to kebab-case. `action_name` follows this rule for every action except `ExitLaqta`.
fn kebab(camel: &str) -> String {
    let mut out = String::new();
    for (index, ch) in camel.chars().enumerate() {
        if ch.is_ascii_uppercase() {
            if index > 0 {
                out.push('-');
            }
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push(ch);
        }
    }
    out
}

#[test]
fn action_names_round_trip() {
    let mut seen = HashSet::new();
    for &action in Action::ALL {
        let name = action_name(action);
        assert_eq!(parse_action(name), Some(action), "round trip of {name}");
        assert_eq!(
            parse_action(&name.to_uppercase()),
            Some(action),
            "case-insensitive {name}"
        );
        assert!(seen.insert(name), "duplicate name {name}");
        assert!(
            name.bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
                && !name.starts_with('-')
                && !name.ends_with('-')
                && !name.contains("--"),
            "{name} is not lowercase kebab-case"
        );
        let expected = if action == Action::ExitLaqta {
            "exit".to_owned()
        } else {
            kebab(&format!("{action:?}"))
        };
        assert_eq!(name, expected, "name of {action:?}");
    }
    assert_eq!(action_name(Action::PrintScreen), "print-screen");
    assert_eq!(
        action_name(Action::OpenScreenshotsFolder),
        "open-screenshots-folder"
    );
    assert_eq!(action_name(Action::ExitLaqta), "exit");
    assert_eq!(parse_action("PRINT-SCREEN"), Some(Action::PrintScreen));
    assert_eq!(parse_action("bogus"), None);
    assert_eq!(parse_action("unknown"), None);
}
