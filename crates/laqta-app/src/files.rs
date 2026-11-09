//! The only file-system access in laqta-app (ADR 024). Every function runs its work through
//! `Spawner::run_blocking`, so the async worker never waits on the disk.
#![allow(clippy::disallowed_methods, clippy::disallowed_types)]

use std::io::{self, Read};
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

#[cfg_attr(not(test), allow(dead_code))] // Not called outside tests yet; the cleanup path will use it.
pub(crate) async fn remove(spawner: &dyn Spawner, path: PathBuf) -> io::Result<()> {
    blocking(spawner, move || std::fs::remove_file(&path)).await
}

/// Moves the file at `from` into `dir` under its own name, or `name (n).ext` for the first free `n`, and returns the new
/// path. A file already in `dir` stays where it is. The new name is claimed with an empty file before the move, so a
/// concurrent move cannot take it. Across volumes the bytes are copied and the original removed.
pub(crate) async fn move_to_free_name(spawner: &dyn Spawner, from: PathBuf, dir: PathBuf) -> io::Result<PathBuf> {
    blocking(spawner, move || move_into(&from, &dir)).await
}

/// The size of the file at `path`, in bytes.
pub(crate) async fn size(spawner: &dyn Spawner, path: PathBuf) -> io::Result<u64> {
    blocking(spawner, move || std::fs::metadata(&path).map(|metadata| metadata.len())).await
}

/// The bytes of the file at `path`.
pub(crate) async fn read(spawner: &dyn Spawner, path: PathBuf) -> io::Result<Vec<u8>> {
    blocking(spawner, move || std::fs::read(&path)).await
}

/// The files directly inside `dir`, sorted by path. Folders are skipped, and so is anything inside them.
pub(crate) async fn list_dir(spawner: &dyn Spawner, dir: PathBuf) -> io::Result<Vec<PathBuf>> {
    blocking(spawner, move || {
        let mut files = Vec::new();
        for entry in std::fs::read_dir(&dir)? {
            let path = entry?.path();
            if path.is_file() {
                files.push(path);
            }
        }
        files.sort();
        Ok(files)
    })
    .await
}

/// The first `max` bytes of the file at `path`, or the whole file when it is shorter.
pub(crate) async fn read_prefix(spawner: &dyn Spawner, path: PathBuf, max: u64) -> io::Result<Vec<u8>> {
    blocking(spawner, move || {
        let mut bytes = Vec::new();
        std::fs::File::open(&path)?.take(max).read_to_end(&mut bytes)?;
        Ok(bytes)
    })
    .await
}

/// One entry of `walk`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct WalkEntry {
    /// The file or folder name, without its parents.
    pub(crate) name: String,
    /// 0 for the entries directly inside the walked folder.
    pub(crate) depth: u32,
    pub(crate) is_dir: bool,
    /// The file's length in bytes; 0 for a folder.
    pub(crate) size: u64,
}

/// The folders and files inside `root`, in walk order: a folder is followed by its contents, and the entries of one
/// folder come folders first, then by name. Only `max_depth` levels are walked (`None` walks them all, `Some(0)` none).
/// Hidden entries (a leading dot, or the hidden attribute on Windows) are skipped unless `include_hidden`. Symbolic
/// links are listed as files and not followed. A folder that cannot be read is listed without its contents; an
/// unreadable `root` is an error.
pub(crate) async fn walk(spawner: &dyn Spawner, root: PathBuf, max_depth: Option<u32>, include_hidden: bool) -> io::Result<Vec<WalkEntry>> {
    blocking(spawner, move || {
        let mut out = Vec::new();
        walk_level(&root, 0, WalkOptions { max_depth, include_hidden }, &mut out)?;
        Ok(out)
    })
    .await
}

#[derive(Clone, Copy)]
struct WalkOptions {
    max_depth: Option<u32>,
    include_hidden: bool,
}

struct Child {
    name: String,
    path: PathBuf,
    is_dir: bool,
    hidden: bool,
    size: u64,
}

fn walk_level(dir: &Path, depth: u32, options: WalkOptions, out: &mut Vec<WalkEntry>) -> io::Result<()> {
    if !options.max_depth.is_none_or(|max| depth < max) {
        return Ok(());
    }
    let mut children = read_children(dir)?;
    children.sort_by_cached_key(|child| (!child.is_dir, child.name.to_lowercase()));
    for child in children {
        if child.hidden && !options.include_hidden {
            continue;
        }
        out.push(WalkEntry { name: child.name, depth, is_dir: child.is_dir, size: child.size });
        if child.is_dir {
            // An unreadable folder stays listed, without its contents.
            let _ = walk_level(&child.path, depth + 1, options, out);
        }
    }
    Ok(())
}

fn read_children(dir: &Path) -> io::Result<Vec<Child>> {
    let mut children = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        // An entry that cannot be read while listing is skipped.
        let Ok(entry) = entry else { continue };
        let Ok(metadata) = entry.metadata() else { continue };
        let name = entry.file_name().to_string_lossy().into_owned();
        let is_dir = entry.file_type().is_ok_and(|kind| kind.is_dir());
        children.push(Child {
            hidden: name.starts_with('.') || hidden_attribute(&metadata),
            is_dir,
            size: if is_dir { 0 } else { metadata.len() },
            path: entry.path(),
            name,
        });
    }
    Ok(children)
}

#[cfg(windows)]
fn hidden_attribute(metadata: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
    (metadata.file_attributes() & FILE_ATTRIBUTE_HIDDEN) != 0
}

#[cfg(not(windows))]
fn hidden_attribute(_metadata: &std::fs::Metadata) -> bool {
    false
}

/// A file read in fixed-size chunks. Each `next_chunk` reads on the blocking pool: the file handle moves into the job
/// and back out, so the reader keeps no thread of its own.
#[derive(Debug)]
pub(crate) struct ChunkReader {
    file: Option<std::fs::File>,
    chunk: usize,
}

/// Opens `path` for chunked reading, `chunk` bytes at a time.
pub(crate) async fn open_chunks(spawner: &dyn Spawner, path: PathBuf, chunk: usize) -> io::Result<ChunkReader> {
    let file = blocking(spawner, move || std::fs::File::open(&path)).await?;
    Ok(ChunkReader { file: Some(file), chunk })
}

impl ChunkReader {
    /// The next chunk, or `None` at the end of the file. Only the last chunk can be shorter than `chunk`.
    pub(crate) async fn next_chunk(&mut self, spawner: &dyn Spawner) -> io::Result<Option<Vec<u8>>> {
        let Some(mut file) = self.file.take() else { return Ok(None) };
        let chunk = self.chunk;
        let (file, read) = blocking(spawner, move || {
            let mut bytes = Vec::with_capacity(chunk);
            let read = file.by_ref().take(chunk as u64).read_to_end(&mut bytes);
            (file, read.map(|_| bytes))
        })
        .await;
        let bytes = read?;
        if bytes.is_empty() {
            return Ok(None);
        }
        self.file = Some(file);
        Ok(Some(bytes))
    }
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

/// The most names `move_into` tries before it gives up.
const MAX_FREE_NAMES: u32 = 10_000;

fn move_into(from: &Path, dir: &Path) -> io::Result<PathBuf> {
    if from.parent() == Some(dir) {
        return Ok(from.to_path_buf());
    }
    std::fs::create_dir_all(dir)?;
    let to = reserve_free_name(from, dir)?;
    match rename_or_copy(from, &to) {
        Ok(()) => Ok(to),
        Err(error) => {
            let _ = std::fs::remove_file(&to);
            Err(error)
        }
    }
}

/// Creates an empty file at the first free name in `dir` (the file's own name, then `name (1).ext`, `name (2).ext`, …)
/// and returns its path.
fn reserve_free_name(from: &Path, dir: &Path) -> io::Result<PathBuf> {
    let name = from.file_name().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no file name"))?;
    let base = Path::new(name);
    let stem = base.file_stem().unwrap_or(name);
    for n in 0..MAX_FREE_NAMES {
        let candidate = if n == 0 {
            dir.join(name)
        } else {
            let mut numbered = stem.to_os_string();
            numbered.push(format!(" ({n})"));
            if let Some(ext) = base.extension() {
                numbered.push(".");
                numbered.push(ext);
            }
            dir.join(numbered)
        };
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&candidate) {
            Ok(_) => return Ok(candidate),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(io::ErrorKind::AlreadyExists, "no free file name"))
}

/// Renames `from` to `to`. A rename fails across volumes, so the bytes are copied and the original removed then.
fn rename_or_copy(from: &Path, to: &Path) -> io::Result<()> {
    if std::fs::rename(from, to).is_ok() {
        return Ok(());
    }
    std::fs::copy(from, to)?;
    std::fs::remove_file(from)
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
    fn list_dir_takes_the_files_directly_inside_sorted() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("b.txt"), b"b").unwrap();
        std::fs::write(dir.path().join("a.txt"), b"a").unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("sub").join("deep.txt"), b"d").unwrap();
        let spawner = InlineSpawner::default();

        let listed = block_on(list_dir(&spawner, dir.path().to_path_buf())).unwrap();
        assert_eq!(listed, vec![dir.path().join("a.txt"), dir.path().join("b.txt")]);
        assert_eq!(block_on(read(&spawner, dir.path().join("a.txt"))).unwrap(), b"a".to_vec());
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

    fn walked(entries: &[WalkEntry]) -> Vec<(&str, u32)> {
        entries.iter().map(|e| (e.name.as_str(), e.depth)).collect()
    }

    #[test]
    fn walk_lists_folders_first_with_depth_and_skips_hidden() {
        let dir = TempDir::new().unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        std::fs::write(dir.path().join("sub").join("deep.txt"), b"ddd").unwrap();
        std::fs::create_dir(dir.path().join("Alpha")).unwrap();
        std::fs::write(dir.path().join("b.txt"), b"bb").unwrap();
        std::fs::write(dir.path().join(".secret"), b"s").unwrap();
        let spawner = InlineSpawner::default();
        let root = dir.path().to_path_buf();

        let all = block_on(walk(&spawner, root.clone(), None, false)).unwrap();
        assert_eq!(walked(&all), vec![("Alpha", 0), ("sub", 0), ("deep.txt", 1), ("b.txt", 0)]);
        assert_eq!(all[2].size, 3);

        let hidden = block_on(walk(&spawner, root.clone(), None, true)).unwrap();
        assert_eq!(walked(&hidden), vec![("Alpha", 0), ("sub", 0), ("deep.txt", 1), (".secret", 0), ("b.txt", 0)]);

        let one_level = block_on(walk(&spawner, root.clone(), Some(1), false)).unwrap();
        assert_eq!(walked(&one_level), vec![("Alpha", 0), ("sub", 0), ("b.txt", 0)]);

        assert!(block_on(walk(&spawner, root.clone(), Some(0), false)).unwrap().is_empty());
        assert!(block_on(walk(&spawner, dir.path().join("missing"), None, false)).is_err());
    }

    #[test]
    fn chunks_read_back_the_whole_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("data.bin");
        let data: Vec<u8> = (0..2_500u32).map(|i| (i % 251) as u8).collect();
        std::fs::write(&path, &data).unwrap();
        let spawner = InlineSpawner::default();

        let mut reader = block_on(open_chunks(&spawner, path, 1000)).unwrap();
        let mut back = Vec::new();
        let mut sizes = Vec::new();
        while let Some(chunk) = block_on(reader.next_chunk(&spawner)).unwrap() {
            sizes.push(chunk.len());
            back.extend(chunk);
        }
        assert_eq!(back, data);
        assert_eq!(sizes, vec![1000, 1000, 500]);
        assert_eq!(block_on(reader.next_chunk(&spawner)).unwrap(), None);
    }

    #[test]
    fn read_prefix_stops_at_the_limit() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("long.bin");
        std::fs::write(&path, (0..100u8).collect::<Vec<u8>>()).unwrap();
        let spawner = InlineSpawner::default();

        assert_eq!(block_on(read_prefix(&spawner, path.clone(), 10)).unwrap(), (0..10u8).collect::<Vec<u8>>());
        assert_eq!(block_on(read_prefix(&spawner, path, 1000)).unwrap().len(), 100);
    }
}
