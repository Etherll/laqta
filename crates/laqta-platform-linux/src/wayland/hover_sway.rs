//! sway hover targets (ADR 019, M1 D1): the visible windows from sway's IPC, in physical pixels (D2).

use std::ffi::OsStr;
use std::fmt::Display;

use laqta_domain::PhysRect;
use laqta_domain::region::{HoverTarget, TargetKind};
use laqta_ports::{BoxFuture, HoverSource, PortError};
use swayipc::{Connection, Node, NodeType, Rect};

use super::layout::{OutputLayout, max_scale, to_physical};

/// A window as the walk reports it: logical rect, title, process.
type View = (PhysRect, String, String);

#[derive(Debug, Default)]
pub struct SwayHover;

impl HoverSource for SwayHover {
    fn targets(&self) -> BoxFuture<'_, Result<Vec<HoverTarget>, PortError>> {
        Box::pin(async { blocking::unblock(sway_targets).await })
    }
    fn active_window(&self) -> BoxFuture<'_, Result<Option<HoverTarget>, PortError>> {
        Box::pin(async { blocking::unblock(sway_active).await })
    }
}

fn fail(e: impl Display) -> PortError {
    PortError::Failed(format!("sway ipc: {e}"))
}

/// Hover targets exist only where sway exports its IPC socket.
fn check_socket(sock: Option<&OsStr>) -> Result<(), PortError> {
    match sock {
        Some(s) if !s.is_empty() => Ok(()),
        _ => Err(PortError::Unsupported("hover targets need sway (SWAYSOCK unset)".into())),
    }
}

/// The window tree, and the largest active output scale to map its logical rects with (D2).
fn snapshot() -> Result<(Node, f32), PortError> {
    check_socket(std::env::var_os("SWAYSOCK").as_deref())?;
    let mut conn = Connection::new().map_err(fail)?;
    let outputs: Vec<OutputLayout> = conn
        .get_outputs()
        .map_err(fail)?
        .iter()
        .filter(|o| o.active)
        .map(|o| OutputLayout {
            name: o.name.clone(),
            description: format!("{} {}", o.make, o.model).trim().to_owned(),
            logical: phys_rect(o.rect),
            scale: o.scale.unwrap_or(1.0) as f32,
        })
        .collect();
    let tree = conn.get_tree().map_err(fail)?;
    Ok((tree, max_scale(&outputs)))
}

fn sway_targets() -> Result<Vec<HoverTarget>, PortError> {
    let (tree, s_max) = snapshot()?;
    Ok(collect_views(&tree, &proc_comm).into_iter().map(|v| hover_target(v, s_max)).collect())
}

fn sway_active() -> Result<Option<HoverTarget>, PortError> {
    let (tree, s_max) = snapshot()?;
    Ok(focused_view(&tree, &proc_comm).map(|v| hover_target(v, s_max)))
}

fn hover_target((rect, title, process): View, s_max: f32) -> HoverTarget {
    HoverTarget { handle: 0, rect: to_physical(rect, s_max), title, process, kind: TargetKind::Window }
}

/// Every visible window, front to back. Per workspace the floating windows come first (sway lists
/// them bottom to top, so the last is frontmost), then the tiled ones. Rects are logical.
/// `comm` resolves a pid to its executable name, for windows with neither an app id nor a class.
pub fn collect_views(root: &Node, comm: &dyn Fn(i32) -> Option<String>) -> Vec<View> {
    let workspaces = root.nodes.iter().flat_map(|output| output.nodes.iter()).filter(|n| n.node_type == NodeType::Workspace);
    let mut out = Vec::new();
    for workspace in workspaces {
        for floating in workspace.floating_nodes.iter().rev() {
            push_visible(floating, comm, &mut out);
        }
        for tiled in &workspace.nodes {
            push_visible(tiled, comm, &mut out);
        }
    }
    out
}

fn push_visible(node: &Node, comm: &dyn Fn(i32) -> Option<String>, out: &mut Vec<View>) {
    if node.visible == Some(true)
        && is_view(node)
        && let Some(found) = view(node, comm)
    {
        out.push(found);
    }
    for child in node.nodes.iter().chain(&node.floating_nodes) {
        push_visible(child, comm, out);
    }
}

/// The focused window, when the focused node is a view.
fn focused_view(root: &Node, comm: &dyn Fn(i32) -> Option<String>) -> Option<View> {
    let node = root.find_as_ref(|n| n.focused && is_view(n))?;
    view(node, comm)
}

/// Views are the nodes sway reports with a pid, an app id or X11 window properties.
fn is_view(node: &Node) -> bool {
    node.pid.is_some() || node.app_id.is_some() || node.window_properties.is_some()
}

/// A view's logical rect (title bar included), title and process. Zero-area views give None.
fn view(node: &Node, comm: &dyn Fn(i32) -> Option<String>) -> Option<View> {
    let rect = with_title_bar(phys_rect(node.rect), phys_rect(node.deco_rect));
    if rect.width == 0 || rect.height == 0 {
        return None;
    }
    let class = node.window_properties.as_ref().and_then(|p| p.class.as_deref());
    let process = process_name(node.app_id.as_deref(), class, || node.pid.and_then(comm));
    Some((rect, node.name.clone().unwrap_or_default(), process))
}

/// `rect` leaves out the title bar, which sits directly above it. swayipc documents `deco_rect` as
/// relative to the parent node, so a union with `rect` would reach the parent's origin. Only the
/// bar's height is taken.
fn with_title_bar(rect: PhysRect, deco: PhysRect) -> PhysRect {
    if deco.height == 0 {
        return rect;
    }
    PhysRect { y: rect.y.saturating_sub_unsigned(deco.height), height: rect.height.saturating_add(deco.height), ..rect }
}

/// The app id, else the X11 class, else the executable name.
fn process_name(app_id: Option<&str>, class: Option<&str>, comm: impl FnOnce() -> Option<String>) -> String {
    app_id.filter(|s| !s.is_empty()).or(class.filter(|s| !s.is_empty())).map(str::to_owned).or_else(comm).unwrap_or_default()
}

/// The executable name of process `pid`, from `/proc/<pid>/comm`.
fn proc_comm(pid: i32) -> Option<String> {
    let comm = std::fs::read_to_string(format!("/proc/{pid}/comm")).ok()?;
    Some(comm.trim_end().to_owned())
}

/// swayipc's rect as a physical rect, with negative sizes clamped to zero.
fn phys_rect(r: Rect) -> PhysRect {
    PhysRect { x: r.x, y: r.y, width: r.width.max(0) as u32, height: r.height.max(0) as u32 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn socket_must_be_set_and_non_empty() {
        let err = check_socket(None).expect_err("an unset socket is unsupported");
        assert!(matches!(err, PortError::Unsupported(ref m) if m == "hover targets need sway (SWAYSOCK unset)"));
        assert!(check_socket(Some(OsStr::new(""))).is_err());
        assert!(check_socket(Some(OsStr::new("/run/user/1000/sway-ipc.sock"))).is_ok());
    }

    #[test]
    fn title_bar_sits_above_the_content() {
        let rect = PhysRect { x: 10, y: 40, width: 300, height: 200 };
        assert_eq!(with_title_bar(rect, PhysRect::default()), rect);
        let bar = PhysRect { x: 0, y: 0, width: 300, height: 25 };
        assert_eq!(with_title_bar(rect, bar), PhysRect { x: 10, y: 15, width: 300, height: 225 });
    }

    #[test]
    fn process_prefers_app_id_then_class_then_comm() {
        assert_eq!(process_name(Some("firefox"), Some("Firefox"), || Some("x".into())), "firefox");
        assert_eq!(process_name(Some(""), Some("Gimp"), || None), "Gimp");
        assert_eq!(process_name(None, None, || Some("kitty".into())), "kitty");
        assert_eq!(process_name(None, None, || None), "");
    }
}
