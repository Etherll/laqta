//! One variant per hotkey action. CLI and IPC names live in `laqta-ipc`.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub enum Action {
    PrintScreen,
    OpenScreenshotsFolder,
    ExitLaqta,
}
