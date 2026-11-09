//! The ScreenCast portal (ADR 019): one monitor, persisted with a restore token, and the PipeWire
//! socket that the node is read through.

use std::os::fd::OwnedFd;

use ashpd::desktop::screencast::{CursorMode, OpenPipeWireRemoteOptions, Screencast, SelectSourcesOptions, SourceType, StartCastOptions};
use ashpd::desktop::{CreateSessionOptions, PersistMode, Session};
use laqta_ports::PortError;

/// A started ScreenCast session: the PipeWire node to read, its socket, and the next restore token.
pub struct Cast {
    pub session: SessionGuard,
    pub node_id: u32,
    /// The stream's top-left corner in the compositor's space, when the portal reports one.
    pub position: Option<(i32, i32)>,
    /// The stream's size in the compositor's (logical) space, when the portal reports one.
    pub size: Option<(i32, i32)>,
    pub fd: OwnedFd,
    pub restore_token: Option<String>,
}

/// A portal session that is closed when it is dropped unclaimed: a start dropped midway (a stop
/// while the picker is open) or a cast that never reaches PipeWire. `into_inner` hands it over.
pub struct SessionGuard(Option<Session<Screencast>>);

impl SessionGuard {
    pub fn into_inner(mut self) -> Session<Screencast> {
        self.0.take().expect("a guard holds its session until it is handed over")
    }
}

impl Drop for SessionGuard {
    fn drop(&mut self) {
        if let Some(session) = self.0.take() {
            // Drop can run on the async worker, and closing blocks on D-Bus.
            let _ = std::thread::Builder::new().name("laqta-portal-close".into()).spawn(move || {
                let _ = async_io::block_on(session.close());
            });
        }
    }
}

/// Starts a monitor stream. A `restore_token` from an earlier session lets the portal skip its
/// picker. The session is closed again if any step after it fails or the start is dropped.
pub async fn start(restore_token: Option<&str>, show_cursor: bool) -> Result<Cast, PortError> {
    let proxy = Screencast::new().await.map_err(|e| PortError::Unsupported(format!("ScreenCast portal: {e}")))?;
    let session = SessionGuard(Some(proxy.create_session(CreateSessionOptions::default()).await.map_err(portal_error)?));
    let open = session.0.as_ref().expect("just created");
    // On an error, or when this future is dropped, the guard closes the session.
    let started = open_stream(&proxy, open, restore_token, show_cursor).await?;
    Ok(Cast {
        session,
        node_id: started.node_id,
        position: started.position,
        size: started.size,
        fd: started.fd,
        restore_token: started.restore_token,
    })
}

struct Started {
    node_id: u32,
    position: Option<(i32, i32)>,
    size: Option<(i32, i32)>,
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
    Ok(Started { node_id, position, size, restore_token: streams.restore_token().map(str::to_owned), fd })
}

/// A user's refusal is `Cancelled`; any other portal failure is a failed recording.
fn portal_error(e: ashpd::Error) -> PortError {
    match e {
        ashpd::Error::Response(ashpd::desktop::ResponseError::Cancelled) => PortError::Cancelled,
        other => PortError::Failed(format!("ScreenCast portal: {other}")),
    }
}
