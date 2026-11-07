//! The ScreenCast portal (ADR 019): one monitor, persisted with a restore token, and the PipeWire
//! socket that the node is read through.

use std::os::fd::OwnedFd;

use ashpd::desktop::screencast::{CursorMode, OpenPipeWireRemoteOptions, Screencast, SelectSourcesOptions, SourceType, StartCastOptions};
use ashpd::desktop::{CreateSessionOptions, PersistMode, Session};
use laqta_ports::PortError;

/// A started ScreenCast session: the PipeWire node to read, its socket, and the next restore token.
pub struct Cast {
    pub session: Session<Screencast>,
    pub node_id: u32,
    /// The stream's top-left corner in the compositor's space, when the portal reports one.
    pub position: Option<(i32, i32)>,
    pub fd: OwnedFd,
    pub restore_token: Option<String>,
}

/// Starts a monitor stream. A `restore_token` from an earlier session lets the portal skip its
/// picker. The session is closed again if any step after it fails.
pub async fn start(restore_token: Option<&str>, show_cursor: bool) -> Result<Cast, PortError> {
    let proxy = Screencast::new().await.map_err(|e| PortError::Unsupported(format!("ScreenCast portal: {e}")))?;
    let session = proxy.create_session(CreateSessionOptions::default()).await.map_err(portal_error)?;
    match open_stream(&proxy, &session, restore_token, show_cursor).await {
        Ok(started) => {
            Ok(Cast { session, node_id: started.node_id, position: started.position, fd: started.fd, restore_token: started.restore_token })
        }
        Err(e) => {
            let _ = session.close().await;
            Err(e)
        }
    }
}

struct Started {
    node_id: u32,
    position: Option<(i32, i32)>,
    restore_token: Option<String>,
    fd: OwnedFd,
}

async fn open_stream(
    proxy: &Screencast,
    session: &Session<Screencast>,
    restore_token: Option<&str>,
    show_cursor: bool,
) -> Result<Started, PortError> {
    let cursor = if show_cursor { CursorMode::EMBEDDED } else { CursorMode::HIDDEN };
    let select = SelectSourcesOptions::default()
        .set_sources(SourceType::MONITOR)
        .set_multiple(false)
        .set_cursor_mode(cursor)
        .set_persist_mode(PersistMode::ExplicitlyRevoked)
        .set_restore_token(restore_token);
    proxy.select_sources(session, select).await.map_err(portal_error)?.response().map_err(portal_error)?;
    let streams = proxy.start(session, None, StartCastOptions::default()).await.map_err(portal_error)?.response().map_err(portal_error)?;
    let stream = streams.streams().first().ok_or_else(|| PortError::Failed("ScreenCast returned no stream".into()))?;
    let (node_id, position, size) = (stream.pipe_wire_node_id(), stream.position(), stream.size());
    tracing::debug!(node_id, ?position, ?size, "ScreenCast stream started");
    let fd = proxy.open_pipe_wire_remote(session, OpenPipeWireRemoteOptions::default()).await.map_err(portal_error)?;
    Ok(Started { node_id, position, restore_token: streams.restore_token().map(str::to_owned), fd })
}

/// A user's refusal is `Cancelled`; any other portal failure is a failed recording.
fn portal_error(e: ashpd::Error) -> PortError {
    match e {
        ashpd::Error::Response(ashpd::desktop::ResponseError::Cancelled) => PortError::Cancelled,
        other => PortError::Failed(format!("ScreenCast portal: {other}")),
    }
}
