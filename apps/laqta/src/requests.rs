//! Requests from the command line, a secondary instance or the native-messaging host (M6 D1, D2).
//! Every one ends in `App::dispatch` or an Upload job; `.sxcu` and `.sxie` files are imported instead.

use std::path::{Path, PathBuf};

use laqta_app::UploadSource;
use laqta_domain::workflow::WorkflowId;
use laqta_domain::{Action, Settings};
use laqta_ipc::Request;

#[derive(Debug)]
pub enum Route {
    Dispatch(Action, Option<WorkflowId>),
    Upload(UploadSource, Option<WorkflowId>),
    ImportUploader(PathBuf),
    ImportEffect(PathBuf),
}

fn has_ext(path: &Path, ext: &str) -> bool {
    path.extension().is_some_and(|x| x.eq_ignore_ascii_case(ext))
}

/// What a request does. An action runs the first enabled workflow bound to it, as a hotkey does.
pub fn route(request: Request, settings: &Settings) -> Result<Route, String> {
    Ok(match request {
        Request::Action(action) => {
            Route::Dispatch(action, settings.workflows.iter().find(|w| w.enabled && w.action == action).map(|w| w.id))
        }
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
        Request::Upload(path) => {
            let path = std::path::absolute(&path).unwrap_or(path);
            Route::Upload(UploadSource::File(path), None)
        }
        Request::UploadText(text) => Route::Upload(UploadSource::Text(text), None),
        Request::UploadUrl(url) => Route::Upload(UploadSource::Url(url), None),
        Request::ShortenUrl(url) => Route::Upload(UploadSource::ShortenUrl(url), None),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
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
        assert!(
            matches!(route(Request::Upload("/a/b.png".into()), &s), Ok(Route::Upload(UploadSource::File(p), None)) if p == Path::new("/a/b.png"))
        );
        assert!(matches!(route(Request::ShortenUrl("https://x".into()), &s), Ok(Route::Upload(UploadSource::ShortenUrl(_), None))));
        assert!(matches!(route(Request::UploadText("hi".into()), &s), Ok(Route::Upload(UploadSource::Text(_), None))));
    }
}
