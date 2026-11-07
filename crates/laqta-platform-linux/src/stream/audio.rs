//! Audio devices from PulseAudio's `pactl`, which PipeWire's pulse server answers too (M4 D6). A
//! monitor source is system sound (loopback). Every other source is a microphone.

use std::io::ErrorKind;
use std::process::{Command, Stdio};

use laqta_domain::record::AudioDevice;
use laqta_ports::{AudioInput, PortError};

/// Every source `pactl` lists. The JSON listing gives the descriptions; the short listing is the
/// fallback, and it has no descriptions, so the name stands in for them.
pub fn list_devices() -> Result<Vec<AudioDevice>, PortError> {
    if let Ok(json) = pactl(&["-f", "json", "list", "sources"]) {
        let devices = parse_json(&json);
        if !devices.is_empty() {
            return Ok(devices);
        }
    }
    Ok(parse_short(&pactl(&["list", "short", "sources"])?))
}

/// One `AudioInput::Device` per requested device.
pub fn inputs(devices: &[AudioDevice]) -> Vec<AudioInput> {
    devices.iter().cloned().map(AudioInput::Device).collect()
}

fn pactl(args: &[&str]) -> Result<String, PortError> {
    let output = Command::new("pactl").args(args).stdin(Stdio::null()).stderr(Stdio::null()).output().map_err(|e| {
        if e.kind() == ErrorKind::NotFound {
            PortError::Unsupported("pactl is not installed".into())
        } else {
            PortError::Failed(format!("pactl: {e}"))
        }
    })?;
    if !output.status.success() {
        return Err(PortError::Failed(format!("pactl {}: {}", args.join(" "), output.status)));
    }
    String::from_utf8(output.stdout).map_err(|e| PortError::Failed(format!("pactl output: {e}")))
}

/// Devices from `pactl -f json list sources`. Only the `name` and `description` keys of the objects
/// directly inside the top-level array count. Keys of nested objects (`ports`, `properties`) are
/// skipped, and so are braces and quotes inside strings.
fn parse_json(text: &str) -> Vec<AudioDevice> {
    let bytes = text.as_bytes();
    let mut devices = Vec::new();
    let mut depth = 0usize;
    let mut name: Option<String> = None;
    let mut description: Option<String> = None;
    let mut i = 0;
    while let Some(&byte) = bytes.get(i) {
        match byte {
            b'"' => {
                let (key, after_key) = read_string(bytes, i);
                i = after_key;
                if depth != 2 {
                    continue;
                }
                let Some(value_at) = after_colon(bytes, after_key) else { continue };
                if !matches!(key.as_str(), "name" | "description") || bytes.get(value_at) != Some(&b'"') {
                    continue;
                }
                let (value, after_value) = read_string(bytes, value_at);
                if key == "name" {
                    name = Some(value);
                } else {
                    description = Some(value);
                }
                i = after_value;
            }
            b'{' | b'[' => {
                depth += 1;
                if byte == b'{' && depth == 2 {
                    name = None;
                    description = None;
                }
                i += 1;
            }
            b'}' | b']' => {
                if byte == b'}' && depth == 2 {
                    let (finished_name, finished_description) = (name.take(), description.take());
                    if let Some(finished_name) = finished_name {
                        devices.push(device(finished_name, finished_description));
                    }
                }
                depth = depth.saturating_sub(1);
                i += 1;
            }
            _ => i += 1,
        }
    }
    devices
}

/// Devices from `pactl list short sources`: `index<TAB>name<TAB>driver<TAB>spec<TAB>state` per line.
fn parse_short(text: &str) -> Vec<AudioDevice> {
    text.lines().filter_map(|line| line.split_whitespace().nth(1)).map(|name| device(name.to_owned(), None)).collect()
}

fn device(name: String, description: Option<String>) -> AudioDevice {
    let loopback = name.ends_with(".monitor");
    let shown = description.filter(|text| !text.is_empty()).unwrap_or_else(|| name.clone());
    AudioDevice { id: name, name: shown, loopback }
}

/// The JSON string that starts at `start` (an opening quote), decoded, and the index after its
/// closing quote. An unterminated string runs to the end of the input.
fn read_string(bytes: &[u8], start: usize) -> (String, usize) {
    let mut out = Vec::new();
    let mut i = start + 1;
    while let Some(&byte) = bytes.get(i) {
        match byte {
            b'"' => return (String::from_utf8_lossy(&out).into_owned(), i + 1),
            b'\\' => {
                let Some(&escaped) = bytes.get(i + 1) else { break };
                i += 2;
                match escaped {
                    b'n' => out.push(b'\n'),
                    b't' => out.push(b'\t'),
                    b'r' => out.push(b'\r'),
                    b'b' => out.push(0x08),
                    b'f' => out.push(0x0c),
                    b'u' => {
                        let code = bytes
                            .get(i..i + 4)
                            .and_then(|hex| std::str::from_utf8(hex).ok())
                            .and_then(|hex| u32::from_str_radix(hex, 16).ok());
                        i += 4;
                        let ch = code.and_then(char::from_u32).unwrap_or(char::REPLACEMENT_CHARACTER);
                        let mut buf = [0u8; 4];
                        out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                    }
                    // `\"`, `\\` and `\/` stand for themselves.
                    other => out.push(other),
                }
            }
            _ => {
                out.push(byte);
                i += 1;
            }
        }
    }
    (String::from_utf8_lossy(&out).into_owned(), bytes.len())
}

/// The index of the value after a `:` that follows `from` (whitespace allowed), or `None` when no
/// colon follows, which means the string before it was a value and not a key.
fn after_colon(bytes: &[u8], from: usize) -> Option<usize> {
    let colon = skip_whitespace(bytes, from);
    if bytes.get(colon) != Some(&b':') {
        return None;
    }
    Some(skip_whitespace(bytes, colon + 1))
}

fn skip_whitespace(bytes: &[u8], mut i: usize) -> usize {
    while bytes.get(i).is_some_and(|byte| byte.is_ascii_whitespace()) {
        i += 1;
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two sources with ports and properties that repeat `name` and `description`, and one source
    /// whose name and description contain escapes.
    const PACTL_JSON: &str = r#"[
        {
            "index": 0,
            "state": "SUSPENDED",
            "name": "alsa_input.pci-0000_00_1f.3.analog-stereo",
            "description": "Built-in Audio Analog Stereo",
            "driver": "alsa.c",
            "properties": {"device.description": "Built-in", "node.name": "x"},
            "ports": [{"name": "analog-input-mic", "description": "Microphone", "priority": 8900}],
            "active_port": "analog-input-mic"
        },
        {
            "index": 1,
            "name": "alsa_output.pci-0000_00_1f.3.analog-stereo.monitor",
            "description": "Monitor of Built-in Audio Analog Stereo",
            "monitor_of_sink": "alsa_output.pci-0000_00_1f.3.analog-stereo",
            "ports": [{"name": "monitor_FL", "description": "Monitor"}]
        },
        {"index": 2, "name": "bluez_input.\"quoted\"", "description": "Headset \\ é {}", "ports": []},
        {"index": 3, "description": "No name, so no device", "ports": []}
    ]"#;

    #[test]
    fn json_keeps_only_the_top_level_name_and_description() {
        let devices = parse_json(PACTL_JSON);
        assert_eq!(devices.len(), 3);
        assert_eq!(devices[0].id, "alsa_input.pci-0000_00_1f.3.analog-stereo");
        assert_eq!(devices[0].name, "Built-in Audio Analog Stereo");
        assert!(!devices[0].loopback);
        assert_eq!(devices[1].id, "alsa_output.pci-0000_00_1f.3.analog-stereo.monitor");
        assert_eq!(devices[1].name, "Monitor of Built-in Audio Analog Stereo");
        assert!(devices[1].loopback);
        assert_eq!(devices[2].id, "bluez_input.\"quoted\"");
        assert_eq!(devices[2].name, "Headset \\ é {}");
    }

    #[test]
    fn json_without_description_uses_the_name() {
        let devices = parse_json(r#"[{"name": "a.monitor", "description": ""}]"#);
        assert_eq!(devices, vec![device("a.monitor".into(), None)]);
    }

    #[test]
    fn json_garbage_gives_no_devices() {
        assert!(parse_json("").is_empty());
        assert!(parse_json("Failure: not supported").is_empty());
    }

    #[test]
    fn short_listing_uses_the_name_for_both_fields() {
        let text = "0\talsa_input.pci.analog-stereo\tmodule-alsa-card.c\ts16le 2ch 44100Hz\tSUSPENDED\n\
                    1\talsa_output.pci.analog-stereo.monitor\tmodule-alsa-card.c\ts16le 2ch 44100Hz\tRUNNING\n\
                    \n";
        let devices = parse_short(text);
        assert_eq!(devices.len(), 2);
        assert_eq!(devices[0].name, "alsa_input.pci.analog-stereo");
        assert_eq!(devices[0].id, devices[0].name);
        assert!(!devices[0].loopback);
        assert!(devices[1].loopback);
    }

    #[test]
    fn inputs_keep_every_device_in_order() {
        let devices = parse_short("0\ta\tx\ty\tz\n1\tb.monitor\tx\ty\tz\n");
        let inputs = inputs(&devices);
        let ids: Vec<&str> = inputs
            .iter()
            .filter_map(|input| match input {
                AudioInput::Device(device) => Some(device.id.as_str()),
                AudioInput::Pcm(_) => None,
            })
            .collect();
        assert_eq!(ids, ["a", "b.monitor"]);
    }
}
