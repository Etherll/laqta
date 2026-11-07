//! The only file-system access in laqta-app (ADR 024). Every function runs its work through
//! `Spawner::run_blocking`, so the async worker never waits on the disk.
#![allow(clippy::disallowed_methods, clippy::disallowed_types)]

use std::io;
use std::path::{Path, PathBuf};

use futures_channel::oneshot;
use laqta_ports::Spawner;

/// Runs `work` on the blocking pool and returns its result.
async fn blocking<T: Send + 'static>(spawner: &dyn Spawner, work: impl FnOnce() -> T + Send + 'static) -> T {
    let (tx, rx) = oneshot::channel();
    spawner
        .run_blocking(Box::new(move || {
            let _ = tx.send(work());
        }))
        .await;
    rx.await.expect("blocking work was dropped before it sent its result")
}

pub(crate) async fn create_dir_all(spawner: &dyn Spawner, path: PathBuf) -> io::Result<()> {
    blocking(spawner, move || std::fs::create_dir_all(&path)).await
}

/// The first candidate that does not exist yet.
pub(crate) async fn first_free(spawner: &dyn Spawner, candidates: Vec<PathBuf>) -> io::Result<PathBuf> {
    blocking(spawner, move || {
        candidates
            .into_iter()
            .find(|candidate| !candidate.exists())
            .ok_or_else(|| io::Error::new(io::ErrorKind::AlreadyExists, "no free file name"))
    })
    .await
}

/// Whether `path` exists.
pub(crate) async fn exists(spawner: &dyn Spawner, path: PathBuf) -> bool {
    blocking(spawner, move || path.exists()).await
}

/// Creates the parent folders, writes `<path>.part`, then renames it to `path`. A failed attempt
/// removes the `.part` file and returns the original error.
pub(crate) async fn save_atomic(spawner: &dyn Spawner, path: PathBuf, bytes: Vec<u8>) -> io::Result<()> {
    blocking(spawner, move || {
        let part = part_path(&path);
        let result = write_then_rename(&path, &part, &bytes);
        if result.is_err() {
            let _ = std::fs::remove_file(&part);
        }
        result
    })
    .await
}

#[cfg_attr(not(test), allow(dead_code))] // No M0 caller; the cleanup path in a later milestone uses it.
pub(crate) async fn remove(spawner: &dyn Spawner, path: PathBuf) -> io::Result<()> {
    blocking(spawner, move || std::fs::remove_file(&path)).await
}

fn part_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".part");
    PathBuf::from(name)
}

fn write_then_rename(path: &Path, part: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(part, bytes)?;
    std::fs::rename(part, path)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use futures_lite::future::block_on;
    use laqta_ports::fakes::InlineSpawner;
    use tempfile::TempDir;

    fn blocking_calls(spawner: &InlineSpawner) -> u32 {
        *spawner.blocking_calls.lock().unwrap()
    }

    #[test]
    fn save_atomic_leaves_no_part_file_when_parent_is_a_file() {
        let dir = TempDir::new().unwrap();
        let blocker = dir.path().join("blocker");
        std::fs::write(&blocker, b"a file, not a folder").unwrap();
        let spawner = InlineSpawner::default();
        let path = blocker.join("sub").join("shot.png");

        assert!(block_on(save_atomic(&spawner, path.clone(), vec![1, 2, 3])).is_err());
        assert!(!part_path(&path).exists());
        assert_eq!(blocking_calls(&spawner), 1);
    }

    #[test]
    fn save_atomic_leaves_no_part_file_when_rename_fails() {
        let dir = TempDir::new().unwrap();
        // A non-empty folder at the target makes the rename fail after the `.part` is written.
        let target = dir.path().join("shot.png");
        std::fs::create_dir(&target).unwrap();
        std::fs::write(target.join("inside"), b"x").unwrap();
        let spawner = InlineSpawner::default();

        assert!(block_on(save_atomic(&spawner, target.clone(), vec![1, 2, 3])).is_err());
        assert!(!part_path(&target).exists());
    }

    #[test]
    fn save_atomic_writes_the_file() {
        let dir = TempDir::new().unwrap();
        let spawner = InlineSpawner::default();
        let path = dir.path().join("2026-11").join("shot.png");

        block_on(save_atomic(&spawner, path.clone(), vec![7, 8])).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), vec![7, 8]);
        assert!(!part_path(&path).exists());
    }

    #[test]
    fn first_free_skips_existing_names() {
        let dir = TempDir::new().unwrap();
        let taken = dir.path().join("a.png");
        std::fs::write(&taken, b"x").unwrap();
        let spawner = InlineSpawner::default();
        let candidates = vec![taken, dir.path().join("b.png"), dir.path().join("c.png")];

        assert_eq!(block_on(first_free(&spawner, candidates)).unwrap(), dir.path().join("b.png"));
        assert_eq!(blocking_calls(&spawner), 1);
    }

    #[test]
    fn first_free_fails_when_every_candidate_is_taken() {
        let dir = TempDir::new().unwrap();
        let taken = dir.path().join("a.png");
        std::fs::write(&taken, b"x").unwrap();
        let spawner = InlineSpawner::default();

        let err = block_on(first_free(&spawner, vec![taken])).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
    }

    #[test]
    fn every_function_runs_its_work_through_run_blocking() {
        let dir = TempDir::new().unwrap();
        let spawner = InlineSpawner::default();
        let target = dir.path().join("a").join("x.png");

        block_on(create_dir_all(&spawner, dir.path().join("a"))).unwrap();
        assert_eq!(blocking_calls(&spawner), 1);
        block_on(first_free(&spawner, vec![target.clone()])).unwrap();
        assert_eq!(blocking_calls(&spawner), 2);
        block_on(save_atomic(&spawner, target.clone(), vec![0])).unwrap();
        assert_eq!(blocking_calls(&spawner), 3);
        block_on(remove(&spawner, target)).unwrap();
        assert_eq!(blocking_calls(&spawner), 4);
    }
}
