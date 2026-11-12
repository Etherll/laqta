pub mod capture_kwin;
pub mod capture_portal;
pub mod capture_wayshot;
pub mod hover_kwin;
pub mod hover_sway;
pub mod layout;
pub mod shortcuts_portal;

use laqta_domain::Frame;
use laqta_ports::{BoxFuture, PortError, ScreenSource};

/// KWin on Plasma, else `libwayshot` per output, then the Screenshot portal.
#[derive(Debug)]
pub struct WaylandScreen;

impl ScreenSource for WaylandScreen {
    fn capture_all(&self) -> BoxFuture<'_, Result<Vec<Frame>, PortError>> {
        Box::pin(async {
            // Plasma's portal draws the pointer into the picture; KWin itself can leave it out.
            if hover_kwin::is_kde(std::env::var("XDG_CURRENT_DESKTOP").ok().as_deref()) {
                match capture_kwin::capture_all().await {
                    Ok(frames) => return Ok(frames),
                    Err(e) => tracing::info!("KWin capture failed ({e}); trying the other sources"),
                }
            }
            match blocking::unblock(capture_wayshot::capture_all).await {
                Ok(frames) => Ok(frames),
                Err(e) => {
                    tracing::info!("wayshot capture failed ({e}); trying the Screenshot portal");
                    capture_portal::capture_all().await
                }
            }
        })
    }
}
