//! KDE Plasma hover targets and the active window, from KWin's scripting API. KWin has no window
//! list on D-Bus, so each query loads a one-shot script that reads `workspace` and calls back on
//! this connection's unique name, then unloads it. Rects come back logical and are mapped to
//! physical pixels like sway's.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use ashpd::zbus::message::Type;
use ashpd::zbus::{Connection, MessageStream};
use futures_lite::{StreamExt, future};
use laqta_domain::PhysRect;
use laqta_domain::region::{HoverTarget, TargetKind};
use laqta_ports::{BoxFuture, HoverSource, PortError};

use super::layout::to_physical;

/// How long one query may take, script load to callback.
const BUDGET: Duration = Duration::from_secs(2);
const KWIN: &str = "org.kde.KWin";
const CALLBACK_PATH: &str = "/laqta/kwin";
const CALLBACK_IFACE: &str = "org.laqta.KWin";
const CALLBACK_MEMBER: &str = "windows";

/// Lists the visible windows front to back as tab-separated lines (`W`, x, y, width, height,
/// active, class, title) after an `S` line with the largest screen scale.
const SCRIPT: &str = r#"(function () {
  const clean = (s) => String(s || "").replace(/[\t\r\n]/g, " ");
  const shown = (w) => !w.minimized && (w.normalWindow || w.dialog)
    && (w.onAllDesktops || w.desktops.indexOf(workspace.currentDesktop) >= 0);
  const active = workspace.activeWindow;
  const scales = workspace.screens.map((s) => s.devicePixelRatio).filter((s) => s > 0);
  const lines = ["S\t" + (scales.length ? Math.max.apply(null, scales) : 1)];
  workspace.stackingOrder.filter(shown).reverse().forEach((w) => {
    const g = w.frameGeometry;
    lines.push(["W", Math.round(g.x), Math.round(g.y), Math.round(g.width), Math.round(g.height),
      w === active ? 1 : 0, clean(w.resourceClass), clean(w.caption)].join("\t"));
  });
  callDBus("@NAME@", "@PATH@", "@IFACE@", "@MEMBER@", lines.join("\n"));
})();
"#;

#[derive(Debug, Default)]
pub struct KwinHover;

impl HoverSource for KwinHover {
    fn targets(&self) -> BoxFuture<'_, Result<Vec<HoverTarget>, PortError>> {
        Box::pin(async { Ok(parse(&query().await?).into_iter().map(|(target, _)| target).collect()) })
    }
    fn active_window(&self) -> BoxFuture<'_, Result<Option<HoverTarget>, PortError>> {
        Box::pin(async { Ok(parse(&query().await?).into_iter().find_map(|(target, active)| active.then_some(target))) })
    }
}

/// Whether this session is KDE Plasma, so KWin is the window source.
pub fn is_kde(current_desktop: Option<&str>) -> bool {
    current_desktop.is_some_and(|desktops| desktops.split(':').any(|d| d.eq_ignore_ascii_case("KDE")))
}

fn fail(e: impl std::fmt::Display) -> PortError {
    PortError::Failed(format!("KWin: {e}"))
}

/// Runs the script once and returns what it sent back. The whole query, cleanup included, has
/// the budget, so a KWin that stopped answering cannot hold up the overlay or a recording.
async fn query() -> Result<String, PortError> {
    within(BUDGET, async {
        let conn = Connection::session().await.map_err(fail)?;
        let name = conn.unique_name().ok_or_else(|| fail("no D-Bus name"))?.to_string();
        let mut stream = MessageStream::from(&conn);
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let plugin = format!("laqta-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed));
        let path = script_dir().join(format!("{plugin}.js"));
        write_private(&path, &script(&name)).map_err(fail)?;
        let result = run(&conn, &mut stream, &path, &plugin).await;
        let _ = conn.call_method(Some(KWIN), "/Scripting", Some("org.kde.kwin.Scripting"), "unloadScript", &plugin).await;
        let _ = std::fs::remove_file(&path);
        result
    })
    .await
}

async fn within<T>(budget: Duration, work: impl std::future::Future<Output = Result<T, PortError>>) -> Result<T, PortError> {
    future::or(work, async {
        async_io::Timer::after(budget).await;
        Err(fail("no answer in time"))
    })
    .await
}

/// A new file only this user can read, so nothing else can swap the script KWin runs.
fn write_private(path: &std::path::Path, text: &str) -> std::io::Result<()> {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt as _;
    let mut file = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(path)?;
    file.write_all(text.as_bytes())
}

async fn run(conn: &Connection, stream: &mut MessageStream, path: &std::path::Path, plugin: &str) -> Result<String, PortError> {
    let script = path.to_str().ok_or_else(|| fail("script path is not UTF-8"))?;
    // Only KWin's answer counts: any peer on the bus could call this connection.
    let owner: String = conn
        .call_method(Some("org.freedesktop.DBus"), "/org/freedesktop/DBus", Some("org.freedesktop.DBus"), "GetNameOwner", &KWIN)
        .await
        .map_err(fail)?
        .body()
        .deserialize()
        .map_err(fail)?;
    let reply =
        conn.call_method(Some(KWIN), "/Scripting", Some("org.kde.kwin.Scripting"), "loadScript", &(script, plugin)).await.map_err(fail)?;
    let id: i32 = reply.body().deserialize().map_err(fail)?;
    conn.call_method(Some(KWIN), format!("/Scripting/Script{id}").as_str(), Some("org.kde.kwin.Script"), "run", &()).await.map_err(fail)?;
    while let Some(message) = stream.next().await {
        let message = message.map_err(fail)?;
        let header = message.header();
        let ours = header.message_type() == Type::MethodCall
            && header.sender().is_some_and(|s| s.as_str() == owner)
            && header.interface().is_some_and(|i| i.as_str() == CALLBACK_IFACE)
            && header.member().is_some_and(|m| m.as_str() == CALLBACK_MEMBER);
        if ours {
            let text: String = message.body().deserialize().map_err(fail)?;
            let _ = conn.reply(&header, &()).await;
            return Ok(text);
        }
    }
    Err(fail("the D-Bus connection closed"))
}

fn script(name: &str) -> String {
    SCRIPT.replace("@NAME@", name).replace("@PATH@", CALLBACK_PATH).replace("@IFACE@", CALLBACK_IFACE).replace("@MEMBER@", CALLBACK_MEMBER)
}

/// KWin reads the script from a file: the runtime dir, else the temp dir.
fn script_dir() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR").map_or_else(std::env::temp_dir, PathBuf::from)
}

/// The script's lines as targets in physical pixels, front to back, each with whether it is active.
fn parse(text: &str) -> Vec<(HoverTarget, bool)> {
    let mut scale = 1.0f32;
    let mut out = Vec::new();
    for line in text.lines() {
        let fields: Vec<&str> = line.split('\t').collect();
        match fields.as_slice() {
            ["S", s] => scale = s.parse().ok().filter(|s: &f32| s.is_finite() && *s > 0.0).unwrap_or(1.0),
            ["W", x, y, w, h, active, class, title] => {
                let (Ok(x), Ok(y), Ok(width), Ok(height)) = (x.parse(), y.parse(), w.parse(), h.parse()) else { continue };
                if width == 0 || height == 0 {
                    continue;
                }
                let rect = PhysRect { x, y, width, height };
                out.push((rect, *active == "1", (*class).to_owned(), (*title).to_owned()));
            }
            _ => {}
        }
    }
    out.into_iter()
        .map(|(rect, active, process, title)| {
            (HoverTarget { handle: 0, rect: to_physical(rect, scale), title, process, kind: TargetKind::Window }, active)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kde_is_found_in_the_desktop_list() {
        assert!(is_kde(Some("KDE")));
        assert!(is_kde(Some("ubuntu:kde")));
        assert!(!is_kde(Some("GNOME")));
        assert!(!is_kde(None));
    }

    #[test]
    fn lines_become_physical_targets_front_to_back() {
        let text = "S\t1.25\nW\t0\t0\t2048\t1120\t1\torg.gnome.TextEditor\tNotes\nW\t100\t40\t800\t600\t0\tkitty\tshell\nW\t1\t2\t0\t5\t0\tx\tempty";
        let found = parse(text);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].0.rect, PhysRect { x: 0, y: 0, width: 2560, height: 1400 });
        assert!(found[0].1);
        assert_eq!((found[0].0.process.as_str(), found[0].0.title.as_str()), ("org.gnome.TextEditor", "Notes"));
        assert_eq!(found[1].0.rect, PhysRect { x: 125, y: 50, width: 1000, height: 750 });
        assert!(!found[1].1);
    }

    #[test]
    fn a_bad_scale_or_line_is_ignored() {
        let found = parse("S\tNaN\nW\t1\t2\t3\nW\t-10\t5\t20\t30\t0\t\t");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].0.rect, PhysRect { x: -10, y: 5, width: 20, height: 30 });
    }

    #[test]
    fn the_script_names_this_connection() {
        let script = script(":1.42");
        assert!(script.contains(r#"callDBus(":1.42", "/laqta/kwin", "org.laqta.KWin", "windows""#));
        assert!(!script.contains('@'));
    }

    /// Runs against the live KWin when `LAQTA_KWIN=1` (read-only: it lists windows).
    #[test]
    fn live_kwin_lists_windows() {
        if std::env::var_os("LAQTA_KWIN").is_none() {
            return;
        }
        let text = future::block_on(query()).expect("KWin answers");
        let found = parse(&text);
        assert!(!found.is_empty(), "{text}");
        eprintln!("{found:#?}");
    }
}
