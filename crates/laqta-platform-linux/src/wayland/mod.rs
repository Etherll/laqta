pub mod capture_portal;
pub mod capture_wayshot;
pub mod hover_kwin;
pub mod hover_sway;
pub mod layout;
pub mod shortcuts_portal;

use laqta_domain::Frame;
use laqta_ports::{BoxFuture, PortError, ScreenSource};

/// `libwayshot` per output, then the Screenshot portal (M0 §5.3).
#[derive(Debug)]
pub struct WaylandScreen;

impl ScreenSource for WaylandScreen {
    fn capture_all(&self) -> BoxFuture<'_, Result<Vec<Frame>, PortError>> {
        Box::pin(async {
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
