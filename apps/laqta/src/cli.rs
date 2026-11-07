use laqta_domain::Action;

pub const USAGE: &str = "usage: laqta [--action <name>]... [--version]\n  actions: print-screen, open-screenshots-folder, exit";

#[derive(Debug, Default, PartialEq)]
pub struct Args {
    pub actions: Vec<Action>,
}

#[derive(Debug, PartialEq)]
pub enum Command {
    Run(Args),
    Version,
}

/// `--action <name>` (repeatable, run in order) and `--version`. Anything else is an error.
pub fn parse(mut args: impl Iterator<Item = String>) -> Result<Command, String> {
    let mut out = Args::default();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--version" | "-V" => return Ok(Command::Version),
            "--action" => {
                let name = args.next().ok_or("--action needs a name")?;
                out.actions.push(laqta_ipc::parse_action(&name).ok_or_else(|| format!("unknown action `{name}`"))?);
            }
            other => {
                if let Some(name) = other.strip_prefix("--action=") {
                    out.actions.push(laqta_ipc::parse_action(name).ok_or_else(|| format!("unknown action `{name}`"))?);
                } else {
                    return Err(format!("unknown argument `{other}`"));
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

    #[test]
    fn parses_actions_in_order() {
        assert_eq!(p(&["--action", "print-screen", "--action=exit"]), Ok(Command::Run(Args { actions: vec![Action::PrintScreen, Action::ExitLaqta] })));
        assert_eq!(p(&["--version"]), Ok(Command::Version));
        assert!(p(&["--bogus"]).is_err());
        assert!(p(&["--action", "nope"]).is_err());
        assert!(p(&["--action"]).is_err());
    }
}
