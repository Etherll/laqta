//! The annotation editor window. Filled in next (M3 lead task).

use futures_channel::oneshot;
use gpui::App;
use laqta_domain::Bitmap;
use laqta_ports::PortError;

pub(crate) type EditReply = oneshot::Sender<Result<Bitmap, PortError>>;

pub(crate) fn open(_image: Option<Bitmap>, reply: Option<EditReply>, _path: Option<std::path::PathBuf>, _cx: &mut App) {
    if let Some(r) = reply {
        let _ = r.send(Err(PortError::Unsupported("editor window".into())));
    }
}

pub(crate) fn open_file(path: Option<std::path::PathBuf>, cx: &mut App) {
    open(None, None, path, cx);
}
