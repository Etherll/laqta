use std::sync::Arc;

use laqta_domain::{Bitmap, Frame, Monitor, MonitorId, PhysRect, PixelFormat};
use laqta_ports::PortError;
use libwayshot::WayshotConnection;

fn fail(e: impl std::fmt::Display) -> PortError {
    PortError::Failed(format!("wayshot: {e}"))
}

/// One output's layout: its logical rect in the compositor's space.
#[derive(Clone, Debug)]
pub struct OutputLayout {
    pub name: String,
    pub description: String,
    pub logical: PhysRect,
}

pub fn layout() -> Result<Vec<OutputLayout>, PortError> {
    let conn = WayshotConnection::new().map_err(fail)?;
    Ok(conn.get_all_outputs().iter().map(to_layout).collect())
}

fn to_layout(o: &libwayshot::output::OutputInfo) -> OutputLayout {
    let r = o.logical_region.inner;
    OutputLayout {
        name: o.name.clone(),
        description: o.description.clone(),
        logical: PhysRect { x: r.position.x, y: r.position.y, width: r.size.width, height: r.size.height },
    }
}

/// Captures every output. Monitor rects keep the logical origin and the captured (physical) size;
/// `scale` is physical / logical width, so `compose_monitors`' mixed-scale rule applies.
pub fn capture_all() -> Result<Vec<Frame>, PortError> {
    let conn = WayshotConnection::new().map_err(fail)?;
    let outputs = conn.get_all_outputs().to_vec();
    if outputs.is_empty() {
        return Err(PortError::Failed("wayshot: no outputs".into()));
    }
    let mut frames = Vec::with_capacity(outputs.len());
    for (i, o) in outputs.iter().enumerate() {
        let img = conn.screenshot_single_output(o, false).map_err(fail)?.into_rgba8();
        let (w, h) = (img.width(), img.height());
        let l = to_layout(o);
        let scale = if l.logical.width > 0 { w as f32 / l.logical.width as f32 } else { 1.0 };
        let image = Bitmap::new(w, h, PixelFormat::Rgba8, Arc::from(img.into_raw())).map_err(fail)?;
        frames.push(Frame {
            monitor: Monitor {
                id: MonitorId(l.name.clone()),
                name: if l.description.is_empty() { l.name.clone() } else { l.description.clone() },
                rect: PhysRect { x: l.logical.x, y: l.logical.y, width: w, height: h },
                scale,
                primary: i == 0,
            },
            image,
        });
    }
    Ok(frames)
}
