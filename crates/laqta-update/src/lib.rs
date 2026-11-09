//! Update check and install: the latest GitHub release of the Laqta repository. On Windows
//! the Velopack package is fetched through `laqta-http`'s verified fetch, with the hash taken from the
//! release's `SHA256SUMS`, and applied with Velopack's `Update.exe`. On Linux the caller shows a
//! notice instead. Nothing here runs the install: `velopack_apply_command` only builds the command.
#![forbid(unsafe_code)]

mod release;
mod version;

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use laqta_http::{HttpError, Method, Request};

pub use release::{Asset, Release, parse_release, parse_sha256sums};
pub use version::Version;

/// The GitHub repository that publishes Laqta releases. The owner is still to be confirmed,
/// so change it here only.
pub const REPOSITORY: &str = "Etherll/laqta";

/// The release asset that lists the SHA-256 of every other file in the release.
const SUMS_ASSET: &str = "SHA256SUMS";

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum UpdateError {
    #[error(transparent)]
    Http(#[from] HttpError),
    #[error("GitHub answered HTTP {0}")]
    Status(u16),
    #[error("not a version: {0:?}")]
    InvalidVersion(String),
    #[error("malformed release document: {0}")]
    Parse(String),
    #[error("release {0} is a draft or a pre-release")]
    NotStable(String),
    #[error("the release has no Windows package")]
    NoPackage,
    #[error("the release publishes no SHA-256 for the update package")]
    Unverified,
    #[error("the release names a file that is not a plain file name: {0:?}")]
    UnsafeName(String),
}

/// The network the update check uses. The app passes an `HttpFetcher`; tests pass canned responses.
pub trait Fetcher: Send + Sync {
    /// GETs `url` and returns the body as text. A non-2xx status is an error.
    fn get_text<'a>(&'a self, url: &'a str) -> BoxFuture<'a, Result<String, UpdateError>>;

    /// Downloads `url` to `dest`. The file appears at `dest` only if its SHA-256 equals `sha256`.
    fn download<'a>(&'a self, url: &'a str, dest: &'a Path, sha256: &'a str) -> BoxFuture<'a, Result<(), UpdateError>>;
}

/// The `Fetcher` over `laqta-http`.
#[derive(Clone, Debug)]
pub struct HttpFetcher {
    user_agent: String,
}

impl HttpFetcher {
    /// `current` is the running app's version, sent in the User-Agent that the GitHub API requires.
    pub fn new(current: &Version) -> HttpFetcher {
        HttpFetcher { user_agent: format!("Laqta/{current}") }
    }
}

impl Fetcher for HttpFetcher {
    fn get_text<'a>(&'a self, url: &'a str) -> BoxFuture<'a, Result<String, UpdateError>> {
        Box::pin(async move {
            let mut request = Request::new(Method::Get, url);
            request.headers =
                vec![("User-Agent".to_owned(), self.user_agent.clone()), ("Accept".to_owned(), "application/vnd.github+json".to_owned())];
            let response = laqta_http::send(request).await?;
            if !(200..300).contains(&response.status) {
                return Err(UpdateError::Status(response.status));
            }
            Ok(response.text())
        })
    }

    fn download<'a>(&'a self, url: &'a str, dest: &'a Path, sha256: &'a str) -> BoxFuture<'a, Result<(), UpdateError>> {
        Box::pin(async move {
            let no_progress = Arc::new(|_received: u64, _total: Option<u64>| {});
            laqta_http::fetch_verified(url, dest, sha256, no_progress, Arc::new(AtomicBool::new(false))).await?;
            Ok(())
        })
    }
}

/// The latest published release, if it is newer than `current`; `None` when the app is up to date.
pub async fn check(fetcher: &dyn Fetcher, current: &Version) -> Result<Option<Release>, UpdateError> {
    let body = fetcher.get_text(&latest_url()).await?;
    let release = parse_release(&body)?;
    tracing::debug!(latest = %release.version, current = %current, "update check");
    Ok((release.version > *current).then_some(release))
}

fn latest_url() -> String {
    format!("https://api.github.com/repos/{REPOSITORY}/releases/latest")
}

/// The Velopack full package for Windows, `<Pack>-<version>-<channel>-full.nupkg`. Delta packages
/// are skipped. If several full packages are attached, the one carrying this release's version wins.
pub fn windows_package(release: &Release) -> Option<&Asset> {
    let candidates: Vec<&Asset> = release.assets.iter().filter(|asset| is_full_package(&asset.name)).collect();
    match candidates.as_slice() {
        [only] => Some(*only),
        _ => {
            let marker = format!("-{}-", release.version);
            candidates.iter().copied().find(|asset| asset.name.contains(&marker))
        }
    }
}

fn is_full_package(name: &str) -> bool {
    name.to_ascii_lowercase().ends_with("-full.nupkg")
}

/// Downloads the release's Windows package into `dir` and returns its path. The package's hash comes
/// from the release's `SHA256SUMS`. A release without a listed hash is refused (`Unverified`) before
/// anything is downloaded.
pub async fn download_windows_update(fetcher: &dyn Fetcher, release: &Release, dir: &Path) -> Result<PathBuf, UpdateError> {
    let package = windows_package(release).ok_or(UpdateError::NoPackage)?;
    if !is_plain_name(&package.name) {
        return Err(UpdateError::UnsafeName(package.name.clone()));
    }
    let sums_asset = release.assets.iter().find(|asset| asset.name == SUMS_ASSET).ok_or(UpdateError::Unverified)?;
    let sums = fetcher.get_text(&sums_asset.url).await?;
    let sha256 =
        parse_sha256sums(&sums).into_iter().find(|(file, _)| *file == package.name).map(|(_, hash)| hash).ok_or(UpdateError::Unverified)?;
    let dest = dir.join(&package.name);
    fetcher.download(&package.url, &dest, &sha256).await?;
    Ok(dest)
}

/// A name that stays inside the directory it is joined to.
fn is_plain_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\\', ':'])
}

/// The command that installs a downloaded Velopack package and restarts the app:
/// `Update.exe apply --package <package> --restart`. The caller runs it. A path that is not UTF-8 is
/// converted lossily.
pub fn velopack_apply_command(update_exe: &Path, package: &Path) -> (PathBuf, Vec<String>) {
    let package = package.to_string_lossy().into_owned();
    let args = vec!["apply".to_owned(), "--package".to_owned(), package, "--restart".to_owned()];
    (update_exe.to_path_buf(), args)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Mutex;
    use std::task::{Context, Poll, Wake, Waker};

    use super::*;

    const LATEST: &str = include_str!("../tests/fixtures/latest-release.json");
    const SUMS: &str = include_str!("../tests/fixtures/SHA256SUMS");
    const FULL: &str = "Laqta-0.2.0-win-full.nupkg";
    const FULL_HASH: &str = "20b1abfd526cc381c0cef72563d4867003c5ba0f57332c60849417c9c501d715";
    const SUMS_URL: &str = "https://github.com/Etherll/laqta/releases/download/v0.2.0/SHA256SUMS";

    /// Serves canned bodies by URL (anything else is a 404) and records every download.
    #[derive(Debug, Default)]
    struct FakeFetcher {
        bodies: HashMap<String, String>,
        requests: Mutex<Vec<String>>,
        downloads: Mutex<Vec<(String, PathBuf, String)>>,
    }

    impl FakeFetcher {
        fn serving(bodies: &[(&str, &str)]) -> FakeFetcher {
            FakeFetcher { bodies: bodies.iter().map(|(url, body)| (url.to_string(), body.to_string())).collect(), ..FakeFetcher::default() }
        }
    }

    impl Fetcher for FakeFetcher {
        fn get_text<'a>(&'a self, url: &'a str) -> BoxFuture<'a, Result<String, UpdateError>> {
            Box::pin(async move {
                self.requests.lock().unwrap().push(url.to_owned());
                self.bodies.get(url).cloned().ok_or(UpdateError::Status(404))
            })
        }

        fn download<'a>(&'a self, url: &'a str, dest: &'a Path, sha256: &'a str) -> BoxFuture<'a, Result<(), UpdateError>> {
            Box::pin(async move {
                self.downloads.lock().unwrap().push((url.to_owned(), dest.to_path_buf(), sha256.to_owned()));
                Ok(())
            })
        }
    }

    /// Runs a future that never waits on I/O: the fake fetcher is always ready.
    fn block_on<F: Future>(future: F) -> F::Output {
        struct Unpark(std::thread::Thread);
        impl Wake for Unpark {
            fn wake(self: Arc<Self>) {
                self.0.unpark();
            }
        }
        let waker = Waker::from(Arc::new(Unpark(std::thread::current())));
        let mut context = Context::from_waker(&waker);
        let mut future = std::pin::pin!(future);
        loop {
            if let Poll::Ready(output) = future.as_mut().poll(&mut context) {
                return output;
            }
            std::thread::park();
        }
    }

    fn version(text: &str) -> Version {
        Version::parse(text).unwrap()
    }

    fn latest_release() -> Release {
        parse_release(LATEST).unwrap()
    }

    fn serving_latest(body: &str) -> FakeFetcher {
        let url = latest_url();
        FakeFetcher::serving(&[(url.as_str(), body)])
    }

    fn asset(name: &str) -> Asset {
        Asset { name: name.to_owned(), url: format!("https://example.invalid/{name}"), size: 1 }
    }

    #[test]
    fn check_offers_a_newer_release() {
        let fetcher = serving_latest(LATEST);
        let release = block_on(check(&fetcher, &version("0.1.0"))).unwrap().unwrap();
        assert_eq!(release.tag, "v0.2.0");
        assert_eq!(fetcher.requests.lock().unwrap().as_slice(), [latest_url()]);
    }

    #[test]
    fn check_says_nothing_when_up_to_date() {
        let fetcher = serving_latest(LATEST);
        assert_eq!(block_on(check(&fetcher, &version("v0.2.0"))).unwrap(), None);
    }

    #[test]
    fn check_says_nothing_when_ahead_of_the_latest_release() {
        let fetcher = serving_latest(LATEST);
        assert_eq!(block_on(check(&fetcher, &version("0.3.0-dev"))).unwrap(), None);
    }

    #[test]
    fn check_offers_the_release_to_a_pre_release_build() {
        let fetcher = serving_latest(LATEST);
        assert!(block_on(check(&fetcher, &version("0.2.0-beta.1"))).unwrap().is_some());
    }

    #[test]
    fn check_reports_fetch_and_parse_errors() {
        let missing = FakeFetcher::default();
        assert_eq!(block_on(check(&missing, &version("0.1.0"))), Err(UpdateError::Status(404)));
        let garbled = serving_latest("<html>");
        assert!(matches!(block_on(check(&garbled, &version("0.1.0"))), Err(UpdateError::Parse(_))));
    }

    #[test]
    fn windows_package_is_the_full_package_not_the_delta() {
        let release = latest_release();
        assert_eq!(windows_package(&release).map(|asset| asset.name.as_str()), Some(FULL));
    }

    #[test]
    fn windows_package_is_none_without_a_full_package() {
        let mut release = latest_release();
        release.assets.retain(|asset| !asset.name.ends_with("-full.nupkg"));
        assert_eq!(windows_package(&release), None);
    }

    #[test]
    fn windows_package_prefers_this_releases_version_when_several_are_attached() {
        let mut release = latest_release();
        release.assets.push(asset("Laqta-0.1.0-win-full.nupkg"));
        assert_eq!(windows_package(&release).map(|asset| asset.name.as_str()), Some(FULL));
    }

    #[test]
    fn windows_update_downloads_the_package_with_the_hash_from_sha256sums() {
        let fetcher = FakeFetcher::serving(&[(SUMS_URL, SUMS)]);
        let release = latest_release();
        let dir = tempfile::tempdir().unwrap();
        let path = block_on(download_windows_update(&fetcher, &release, dir.path())).unwrap();
        assert_eq!(path, dir.path().join(FULL));
        let downloads = fetcher.downloads.lock().unwrap();
        assert_eq!(
            downloads.as_slice(),
            [(format!("https://github.com/Etherll/laqta/releases/download/v0.2.0/{FULL}"), path, FULL_HASH.to_owned())]
        );
    }

    #[test]
    fn windows_update_is_refused_when_the_package_has_no_hash() {
        let sums_without_package: String = SUMS.lines().filter(|line| !line.ends_with(FULL)).collect::<Vec<_>>().join("\n");
        let fetcher = FakeFetcher::serving(&[(SUMS_URL, sums_without_package.as_str())]);
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(block_on(download_windows_update(&fetcher, &latest_release(), dir.path())), Err(UpdateError::Unverified));
        assert!(fetcher.downloads.lock().unwrap().is_empty());
    }

    #[test]
    fn windows_update_is_refused_without_a_sha256sums_asset() {
        let mut release = latest_release();
        release.assets.retain(|asset| asset.name != SUMS_ASSET);
        let fetcher = FakeFetcher::serving(&[(SUMS_URL, SUMS)]);
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(block_on(download_windows_update(&fetcher, &release, dir.path())), Err(UpdateError::Unverified));
        assert!(fetcher.downloads.lock().unwrap().is_empty());
    }

    #[test]
    fn windows_update_needs_a_package() {
        let mut release = latest_release();
        release.assets.retain(|asset| !asset.name.ends_with("-full.nupkg"));
        let fetcher = FakeFetcher::serving(&[(SUMS_URL, SUMS)]);
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(block_on(download_windows_update(&fetcher, &release, dir.path())), Err(UpdateError::NoPackage));
    }

    #[test]
    fn windows_update_refuses_a_name_that_leaves_the_directory() {
        let mut release = latest_release();
        release.assets = vec![asset("..\\..\\Startup\\evil-win-full.nupkg")];
        let fetcher = FakeFetcher::serving(&[(SUMS_URL, SUMS)]);
        let dir = tempfile::tempdir().unwrap();
        let err = block_on(download_windows_update(&fetcher, &release, dir.path())).unwrap_err();
        assert!(matches!(err, UpdateError::UnsafeName(_)));
        assert!(fetcher.downloads.lock().unwrap().is_empty());
    }

    #[test]
    fn apply_command_runs_update_exe_with_the_package() {
        let (program, args) = velopack_apply_command(
            Path::new(r"C:\Users\me\AppData\Local\Laqta\Update.exe"),
            Path::new(r"C:\Temp\Laqta-0.2.0-win-full.nupkg"),
        );
        assert_eq!(program, PathBuf::from(r"C:\Users\me\AppData\Local\Laqta\Update.exe"));
        assert_eq!(args, ["apply", "--package", r"C:\Temp\Laqta-0.2.0-win-full.nupkg", "--restart"]);
    }

    #[test]
    fn http_fetcher_identifies_the_app_version() {
        assert_eq!(HttpFetcher::new(&version("0.2.0")).user_agent, "Laqta/0.2.0");
    }
}
