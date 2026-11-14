//! Requests from the command line, a secondary instance or the native-messaging host.
//! Every one ends in `App::dispatch` or an Upload job; `.sxcu` and `.sxie` files are imported instead.

use std::path::{Path, PathBuf};

use laqta_app::UploadSource;
use laqta_domain::workflow::WorkflowId;
use laqta_domain::{Action, Settings};
use laqta_ipc::Request;

#[derive(Debug)]
pub enum Route {
    Dispatch(Action, Option<WorkflowId>),
    /// An action on a file (`-ImageEditor shot.png`).
    DispatchFile(Action, PathBuf, Option<WorkflowId>),
    Upload(UploadSource, Option<WorkflowId>),
    ImportUploader(PathBuf),
    ImportEffect(PathBuf),
    /// A ShareX `.sxb` backup.
    ImportBackup(PathBuf),
}

fn has_ext(path: &Path, ext: &str) -> bool {
    path.extension().is_some_and(|x| x.eq_ignore_ascii_case(ext))
}

/// The workflow `-task` names, matched as a hotkey's name is.
fn named<'a>(settings: &'a Settings, workflow: &str) -> Result<&'a laqta_domain::workflow::Workflow, String> {
    settings
        .workflows
        .iter()
        .find(|w| w.name.eq_ignore_ascii_case(workflow.trim()))
        .ok_or_else(|| format!("no workflow named `{workflow}`"))
}

/// What a request does. An action runs the first enabled workflow bound to it, as a hotkey does.
pub fn route(request: Request, settings: &Settings) -> Result<Route, String> {
    Ok(match request {
        Request::Action(action) => Route::Dispatch(action, settings.workflow_for_action(action).map(|w| w.id)),
        Request::Workflow(name) => {
            let w = settings
                .workflows
                .iter()
                .find(|w| w.name.eq_ignore_ascii_case(name.trim()))
                .ok_or_else(|| format!("no workflow named `{name}`"))?;
            Route::Dispatch(w.action, Some(w.id))
        }
        Request::Upload(path) if has_ext(&path, "sxcu") => Route::ImportUploader(path),
        Request::Upload(path) if has_ext(&path, "sxie") => Route::ImportEffect(path),
        Request::Upload(path) if has_ext(&path, "sxb") => Route::ImportBackup(path),
        Request::Upload(path) => {
            let path = std::path::absolute(&path).unwrap_or(path);
            Route::Upload(UploadSource::File(path), None)
        }
        Request::UploadText(text) => Route::Upload(UploadSource::Text(text), None),
        Request::UploadUrl(url) => Route::Upload(UploadSource::Url(url), None),
        Request::ShortenUrl(url) => Route::Upload(UploadSource::ShortenUrl(url), None),
        Request::ActionFile(action, path) => {
            let workflow = settings.workflow_for_action(action).map(|w| w.id);
            Route::DispatchFile(action, std::path::absolute(&path).unwrap_or(path), workflow)
        }
        Request::UploadWith { workflow, path } => {
            let w = named(settings, &workflow)?;
            Route::Upload(UploadSource::File(std::path::absolute(&path).unwrap_or(path)), Some(w.id))
        }
        Request::UploadUrlWith { workflow, url } => Route::Upload(UploadSource::Url(url), Some(named(settings, &workflow)?.id)),
    })
}

/// Whether a request from another program may run while Laqta has administrator rights. Any program
/// running as the user can send one, so one that names a file or a URL to fetch is refused: it would
/// let that program read through Laqta what only an administrator can. Actions, workflows, text and
/// short links only do what the user could do from the tray.
pub fn allowed_when_elevated(request: &Request) -> bool {
    match request {
        Request::Action(_) | Request::Workflow(_) | Request::UploadText(_) | Request::ShortenUrl(_) => true,
        Request::Upload(_)
        | Request::ActionFile(..)
        | Request::UploadWith { .. }
        | Request::UploadUrl(_)
        | Request::UploadUrlWith { .. } => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_elevated_copy_refuses_requests_that_name_files_or_urls_to_fetch() {
        assert!(allowed_when_elevated(&Request::Action(Action::RectangleRegion)));
        assert!(allowed_when_elevated(&Request::Workflow("Region".into())));
        assert!(allowed_when_elevated(&Request::UploadText("hi".into())));
        assert!(allowed_when_elevated(&Request::ShortenUrl("https://example.com".into())));
        assert!(!allowed_when_elevated(&Request::Upload("C:/Windows/System32/config/SAM".into())));
        assert!(!allowed_when_elevated(&Request::ActionFile(Action::ImageEditor, "a.png".into())));
        assert!(!allowed_when_elevated(&Request::UploadWith { workflow: "x".into(), path: "a.png".into() }));
        assert!(!allowed_when_elevated(&Request::UploadUrl("file:///C:/secret".into())));
        assert!(!allowed_when_elevated(&Request::UploadUrlWith { workflow: "x".into(), url: "file:///C:/secret".into() }));
    }
    use laqta_domain::workflow::{TaskSettingsPatch, Workflow};

    fn settings() -> Settings {
        let mut s = Settings::default_for("/p".into());
        s.workflows = vec![Workflow {
            id: WorkflowId(7),
            name: "Region to Imgur".into(),
            action: Action::RectangleRegion,
            hotkey: None,
            enabled: true,
            patch: TaskSettingsPatch::default(),
        }];
        s
    }

    #[test]
    fn cli_routes_requests() {
        let s = settings();
        assert!(matches!(
            route(Request::Action(Action::RectangleRegion), &s),
            Ok(Route::Dispatch(Action::RectangleRegion, Some(WorkflowId(7))))
        ));
        assert!(matches!(route(Request::Action(Action::PrintScreen), &s), Ok(Route::Dispatch(Action::PrintScreen, None))));
        assert!(matches!(
            route(Request::Workflow("region to imgur".into()), &s),
            Ok(Route::Dispatch(Action::RectangleRegion, Some(WorkflowId(7))))
        ));
        assert!(route(Request::Workflow("nope".into()), &s).is_err());
        assert!(matches!(route(Request::Upload("/a/b.SXCU".into()), &s), Ok(Route::ImportUploader(_))));
        assert!(matches!(route(Request::Upload("/a/b.sxie".into()), &s), Ok(Route::ImportEffect(_))));
        assert!(matches!(route(Request::Upload("/a/b.sxb".into()), &s), Ok(Route::ImportBackup(_))));
        let png = if cfg!(windows) { r"C:\a\b.png" } else { "/a/b.png" };
        assert!(matches!(route(Request::Upload(png.into()), &s), Ok(Route::Upload(UploadSource::File(p), None)) if p == Path::new(png)));
        assert!(matches!(route(Request::ShortenUrl("https://x".into()), &s), Ok(Route::Upload(UploadSource::ShortenUrl(_), None))));
        assert!(matches!(route(Request::UploadText("hi".into()), &s), Ok(Route::Upload(UploadSource::Text(_), None))));
        let url = Request::UploadUrlWith { workflow: "region to imgur".into(), url: "https://x/y.png".into() };
        assert!(matches!(route(url, &s), Ok(Route::Upload(UploadSource::Url(u), Some(WorkflowId(7)))) if u == "https://x/y.png"));
        assert!(route(Request::UploadUrlWith { workflow: "nope".into(), url: "https://x".into() }, &s).is_err());
    }
}
