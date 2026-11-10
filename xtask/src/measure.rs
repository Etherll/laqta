//! `cargo xtask measure-idle`: idle RSS, CPU time and wakeups of the release
//! binary, with and without GPUI.

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

pub fn measure_idle(root: &Path) -> Result<(), String> {
    super::run(Command::new("cargo").current_dir(root).args(["build", "--release", "-p", "laqta"]))?;
    let exe = root.join("target/release").join(if cfg!(windows) { "laqta.exe" } else { "laqta" });
    for (label, no_gpui) in [("with GPUI", false), ("LAQTA_NO_GPUI=1", true)] {
        let mut cmd = Command::new(&exe);
        cmd.stdout(Stdio::null()).stderr(Stdio::null());
        // Quit any running Laqta first, or this one becomes a secondary instance and exits.
        if no_gpui {
            cmd.env("LAQTA_NO_GPUI", "1");
        }
        let mut child = cmd.spawn().map_err(|e| format!("start {}: {e}", exe.display()))?;
        std::thread::sleep(Duration::from_secs(10));
        let before = sample(child.id());
        std::thread::sleep(Duration::from_secs(30));
        let after = sample(child.id());
        let _ = child.kill();
        let _ = child.wait();
        match (before, after) {
            (Some(b), Some(a)) => println!(
                "{label}: RSS {:.1} MB, CPU over 30 s {} ms, voluntary context switches over 30 s {}",
                a.rss_kb as f64 / 1024.0,
                a.cpu_ms.saturating_sub(b.cpu_ms),
                a.switches.saturating_sub(b.switches)
            ),
            _ => println!("{label}: process exited early or sampling unsupported on this OS"),
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Copy)]
struct Sample {
    rss_kb: u64,
    cpu_ms: u64,
    switches: u64,
}

#[cfg(target_os = "linux")]
fn sample(pid: u32) -> Option<Sample> {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    let field =
        |text: &str, key: &str| -> Option<u64> { text.lines().find(|l| l.starts_with(key))?.split_whitespace().nth(1)?.parse().ok() };
    let rss_kb = field(&status, "VmRSS:")?;
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let rest = stat.rsplit_once(')')?.1;
    let f: Vec<&str> = rest.split_whitespace().collect();
    // Fields after the command name start at field 3 (state); utime is 14, stime 15.
    let ticks: u64 = f.get(11)?.parse::<u64>().ok()? + f.get(12)?.parse::<u64>().ok()?;
    let cpu_ms = ticks * 10; // USER_HZ is 100 on Linux
    let mut switches = 0;
    for t in std::fs::read_dir(format!("/proc/{pid}/task")).ok()?.flatten() {
        if let Ok(s) = std::fs::read_to_string(t.path().join("status")) {
            switches += field(&s, "voluntary_ctxt_switches:").unwrap_or(0);
        }
    }
    Some(Sample { rss_kb, cpu_ms, switches })
}

/// macOS has no `/proc`: `ps` gives the resident size in KB and the CPU time as `[H:]MM:SS.ss`.
/// It does not report context switches, so those stay 0.
#[cfg(target_os = "macos")]
fn sample(pid: u32) -> Option<Sample> {
    let out = Command::new("ps").args(["-o", "rss=,cputime=", "-p", &pid.to_string()]).output().ok()?;
    let text = String::from_utf8(out.stdout).ok()?;
    let mut fields = text.split_whitespace();
    let rss_kb = fields.next()?.parse().ok()?;
    let seconds = fields.next()?.split(':').try_fold(0.0, |total, part| part.parse::<f64>().ok().map(|n| total * 60.0 + n))?;
    Some(Sample { rss_kb, cpu_ms: (seconds * 1000.0) as u64, switches: 0 })
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn sample(_pid: u32) -> Option<Sample> {
    // Windows: read WorkingSetSize in Task Manager by hand.
    None
}
