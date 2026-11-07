//! Command-line arguments (M0 §5.5, M6 D1). Laqta's own `--action <name>` plus ShareX's
//! single-dash flags (`-RectangleRegion`, `-silent`, `-workflow "<name>"`, …) and bare file paths.

use std::path::PathBuf;

use laqta_domain::Action;
use laqta_ipc::Request;

pub const USAGE: &str = "usage: laqta [--action <name>]... [-<ShareXAction>]... [-workflow <name>] [file]...
       [-silent] [-portable] [-sandbox] [-multi] [-NoHotkeys] [-autoclose] [-StartupDelay <s>]
       [-CustomUploader <file.sxcu>] [-ImageEffect <file.sxie>] [--version]
  actions: print-screen, rectangle-region, active-window, … (see the hotkey actions), exit";

#[derive(Debug, Default, PartialEq)]
pub struct Args {
    /// What to do, in order: actions, uploads, workflows.
    pub requests: Vec<Request>,
    pub silent: bool,
    pub portable: bool,
    pub sandbox: bool,
    pub multi: bool,
    pub no_hotkeys: bool,
    pub autoclose: bool,
    pub startup_delay_secs: u32,
    pub import_uploaders: Vec<PathBuf>,
    pub import_effects: Vec<PathBuf>,
}

impl Args {
    #[cfg(test)]
    pub fn actions(&self) -> Vec<Action> {
        self.requests.iter().filter_map(|r| if let Request::Action(a) = r { Some(*a) } else { None }).collect()
    }
}

#[derive(Debug, PartialEq)]
pub enum Command {
    Run(Args),
    Version,
}

/// ShareX's names that differ from Laqta's variant names.
fn sharex_alias(name: &str) -> Option<Action> {
    Some(match name {
        "exitsharex" => Action::ExitLaqta,
        "screenrecordergif" => Action::ScreenRecorderGif,
        "screenrecordergifactivewindow" => Action::ScreenRecorderGifActiveWindow,
        "screenrecordergifcustomregion" => Action::ScreenRecorderGifCustomRegion,
        "startscreenrecordergif" => Action::StartScreenRecorderGif,
        "uploadurl" => Action::UploadUrl,
        "shortenurl" => Action::ShortenUrl,
        "qrcode" => Action::QrCode,
        "qrcodedecodefromscreen" => Action::QrCodeDecodeFromScreen,
        "qrcodescanregion" => Action::QrCodeScanRegion,
        "ocr" => Action::Ocr,
        _ => return None,
    })
}

/// `-RectangleRegion` → the action whose variant name matches, case-insensitively.
fn action_flag(flag: &str) -> Option<Action> {
    let lower = flag.to_ascii_lowercase();
    sharex_alias(&lower).or_else(|| Action::ALL.iter().copied().find(|a| format!("{a:?}").to_ascii_lowercase() == lower))
}

pub fn parse(mut args: impl Iterator<Item = String>) -> Result<Command, String> {
    let mut out = Args::default();
    let action = |name: &str| laqta_ipc::parse_action(name).ok_or_else(|| format!("unknown action `{name}`"));
    while let Some(arg) = args.next() {
        let lower = arg.to_ascii_lowercase();
        match lower.as_str() {
            "--version" | "-v" | "-version" => return Ok(Command::Version),
            "--action" => {
                let name = args.next().ok_or("--action needs a name")?;
                out.requests.push(Request::Action(action(&name)?));
            }
            "-silent" | "--silent" => out.silent = true,
            "-portable" | "--portable" => out.portable = true,
            "-sandbox" | "--sandbox" => out.sandbox = true,
            "-multi" | "--multi" => out.multi = true,
            "-nohotkeys" | "--no-hotkeys" => out.no_hotkeys = true,
            "-autoclose" | "--autoclose" => out.autoclose = true,
            "-startupdelay" | "--startup-delay" => {
                let v = args.next().ok_or("-StartupDelay needs seconds")?;
                out.startup_delay_secs = v.trim().parse::<f32>().map_err(|_| format!("bad delay `{v}`"))?.max(0.0) as u32;
            }
            "-workflow" | "-task" | "--workflow" => {
                let name = args.next().ok_or("-workflow needs a name")?;
                out.requests.push(Request::Workflow(name));
            }
            "-customuploader" | "--custom-uploader" => out.import_uploaders.push(args.next().ok_or("-CustomUploader needs a file")?.into()),
            "-imageeffect" | "--image-effect" => out.import_effects.push(args.next().ok_or("-ImageEffect needs a file")?.into()),
            "-nativemessaginginput" => {
                // Handled by laqta-nmh; accept and ignore the file for compatibility.
                let _ = args.next();
            }
            _ => {
                if let Some(name) = arg.strip_prefix("--action=") {
                    out.requests.push(Request::Action(action(name)?));
                } else if let Some(flag) = arg.strip_prefix('-').filter(|f| !f.starts_with('-') && !f.is_empty()) {
                    out.requests.push(Request::Action(action_flag(flag).ok_or_else(|| format!("unknown argument `{arg}`"))?));
                } else if arg.starts_with("--") {
                    return Err(format!("unknown argument `{arg}`"));
                } else {
                    out.requests.push(Request::Upload(PathBuf::from(arg)));
                }
            }
        }
    }
    Ok(Command::Run(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(a: &[&str]) -> Result<Command, String> {
        parse(a.iter().map(|s| s.to_string()))
    }

    fn run(a: &[&str]) -> Args {
        match p(a) {
            Ok(Command::Run(args)) => args,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn parses_actions_in_order() {
        assert_eq!(run(&["--action", "print-screen", "--action=exit"]).actions(), vec![Action::PrintScreen, Action::ExitLaqta]);
        assert_eq!(p(&["--version"]), Ok(Command::Version));
        assert!(p(&["--bogus"]).is_err());
        assert!(p(&["--action", "nope"]).is_err());
        assert!(p(&["--action"]).is_err());
    }

    #[test]
    fn sharex_flags() {
        let a = run(&["-RectangleRegion", "-silent", "-autoclose", "-NoHotkeys", "-ExitShareX"]);
        assert_eq!(a.actions(), vec![Action::RectangleRegion, Action::ExitLaqta]);
        assert!(a.silent && a.autoclose && a.no_hotkeys);
        assert!(p(&["-NotAThing"]).is_err());
        let w = run(&["-workflow", "Region to Imgur", "-StartupDelay", "2.5", "-portable", "-multi"]);
        assert_eq!(w.requests, vec![Request::Workflow("Region to Imgur".into())]);
        assert_eq!(w.startup_delay_secs, 2);
        assert!(w.portable && w.multi);
    }

    #[test]
    fn files_and_imports() {
        let a = run(&["/tmp/a.png", "-CustomUploader", "x.sxcu", "-ImageEffect", "y.sxie", "b.txt"]);
        assert_eq!(a.requests, vec![Request::Upload("/tmp/a.png".into()), Request::Upload("b.txt".into())]);
        assert_eq!(a.import_uploaders, vec![PathBuf::from("x.sxcu")]);
        assert_eq!(a.import_effects, vec![PathBuf::from("y.sxie")]);
    }
}
