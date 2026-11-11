//! History in SQLite (ADR 006): entries, tags, a full-text index over file names, and the
//! task-event subscriber that writes entries.

use std::collections::VecDeque;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError, TryLockError};
use std::time::Duration;

use laqta_domain::history::{HistoryItem, HistoryKind, HistoryQuery, HistoryStats, NewHistoryItem, kind_for_path};
use laqta_ports::{Clock, HistoryEdit, PortError, Spawner, TaskEvent, TaskId};
use rusqlite::types::{Type, Value};
use rusqlite::{Connection, OptionalExtension, Row, params_from_iter};

use crate::LibraryError;

/// `MIGRATIONS[i]` moves `PRAGMA user_version` from `i` to `i + 1`. Never edit a shipped entry.
const MIGRATIONS: [&str; 2] = [
    "
CREATE TABLE history (
    id             INTEGER PRIMARY KEY,
    time           TEXT NOT NULL,
    kind           TEXT NOT NULL,
    file_name      TEXT NOT NULL,
    path           TEXT,
    url            TEXT,
    thumbnail_url  TEXT,
    deletion_url   TEXT,
    host           TEXT,
    width          INTEGER,
    height         INTEGER,
    size_bytes     INTEGER,
    thumbnail_path TEXT
);
CREATE INDEX history_time ON history (time, id);

CREATE TABLE history_tags (
    item_id INTEGER NOT NULL REFERENCES history (id) ON DELETE CASCADE,
    pos     INTEGER NOT NULL,
    tag     TEXT NOT NULL,
    PRIMARY KEY (item_id, pos)
);
CREATE INDEX history_tags_tag ON history_tags (tag, item_id);

CREATE VIRTUAL TABLE history_fts USING fts5 (file_name, content = 'history', content_rowid = 'id');
CREATE TRIGGER history_fts_insert AFTER INSERT ON history BEGIN
    INSERT INTO history_fts (rowid, file_name) VALUES (new.id, new.file_name);
END;
CREATE TRIGGER history_fts_delete AFTER DELETE ON history BEGIN
    INSERT INTO history_fts (history_fts, rowid, file_name) VALUES ('delete', old.id, old.file_name);
END;
CREATE TRIGGER history_fts_update AFTER UPDATE OF file_name ON history BEGIN
    INSERT INTO history_fts (history_fts, rowid, file_name) VALUES ('delete', old.id, old.file_name);
    INSERT INTO history_fts (rowid, file_name) VALUES (new.id, new.file_name);
END;
",
    "
ALTER TABLE history ADD COLUMN short_url TEXT;
",
];

/// Every column `map_row` reads, in its order.
const COLUMNS: &str = "id, time, kind, file_name, path, url, thumbnail_url, deletion_url, host, width, height, \
                       size_bytes, thumbnail_path, short_url";

const KINDS: [HistoryKind; 4] = [HistoryKind::Image, HistoryKind::Text, HistoryKind::File, HistoryKind::Url];

/// How long the connection stays open with no use before the release timer closes it (ARCHITECTURE §1.4).
const RELEASE_AFTER: Duration = Duration::from_secs(60);

/// The history database. The connection sits behind a mutex, so the value is `Send + Sync`. On disk it is a lazy
/// resource: `release_when_idle` makes it close after a minute with no use, and the next call opens it again.
pub struct History {
    /// The database file. `None` for an in-memory database, which cannot be reopened and so is never released.
    path: Option<PathBuf>,
    state: Arc<Mutex<State>>,
    /// The clock and spawner of the release timer, once `release_when_idle` has set them.
    idle: OnceLock<Idle>,
}

impl fmt::Debug for History {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("History").field("path", &self.path).field("connected", &self.connected()).finish_non_exhaustive()
    }
}

struct State {
    conn: Option<Connection>,
    /// Calls so far. The release timer compares it with the count it saw when it last slept.
    uses: u64,
    timer_armed: bool,
}

struct Idle {
    clock: Arc<dyn Clock>,
    spawner: Arc<dyn Spawner>,
}

impl History {
    /// Opens or creates the database at `path` in WAL mode and migrates its schema. It opens now, so a bad file fails
    /// at start; `release_when_idle` lets the connection go again when it is not in use.
    pub fn open(path: &Path) -> Result<History, LibraryError> {
        if let Some(parent) = path.parent().filter(|parent| !parent.as_os_str().is_empty()) {
            fs::create_dir_all(parent)?;
        }
        Ok(Self::holding(Some(path.to_path_buf()), connect(path)?))
    }

    pub fn open_in_memory() -> Result<History, LibraryError> {
        Ok(Self::holding(None, configure(Connection::open_in_memory()?)?))
    }

    fn holding(path: Option<PathBuf>, conn: Connection) -> History {
        History { path, state: Arc::new(Mutex::new(State { conn: Some(conn), uses: 0, timer_armed: false })), idle: OnceLock::new() }
    }

    /// Closes the connection after `RELEASE_AFTER` with no use, using one one-shot timer on `clock` that exists only
    /// while the connection is open (ARCHITECTURE §1.4). The next call reopens the file. An in-memory database keeps
    /// its connection, and the first call wins if this is called twice.
    pub fn release_when_idle(&self, clock: Arc<dyn Clock>, spawner: Arc<dyn Spawner>) {
        if self.path.is_none() || self.idle.set(Idle { clock, spawner }).is_err() {
            return;
        }
        let mut state = lock_state(&self.state);
        self.arm(&mut state);
    }

    /// Whether the connection is open right now.
    pub fn connected(&self) -> bool {
        lock_state(&self.state).conn.is_some()
    }

    /// Runs `f` on the connection, opened again if the release timer closed it. Every method comes through here, and
    /// each call counts as a use.
    fn with<T>(&self, f: impl FnOnce(&mut Connection) -> Result<T, LibraryError>) -> Result<T, LibraryError> {
        // A panic while the lock was held leaves SQLite itself consistent, so keep using it.
        let mut state = lock_state(&self.state);
        state.uses += 1;
        if state.conn.is_none() {
            let path = self.path.as_deref().ok_or_else(|| io::Error::other("the in-memory history connection is gone"))?;
            state.conn = Some(connect(path)?);
            tracing::debug!("history database reopened");
        }
        self.arm(&mut state);
        match state.conn.as_mut() {
            Some(conn) => f(conn),
            None => Err(io::Error::other("the history connection is closed").into()),
        }
    }

    /// Starts the release timer if it is configured and not already running.
    fn arm(&self, state: &mut State) {
        let Some(idle) = self.idle.get() else { return };
        if state.timer_armed || state.conn.is_none() {
            return;
        }
        state.timer_armed = true;
        let shared = Arc::clone(&self.state);
        let clock = Arc::clone(&idle.clock);
        let mut seen = state.uses;
        idle.spawner.spawn(Box::pin(async move {
            loop {
                clock.sleep(RELEASE_AFTER).await;
                // A call that holds the connection is a use, so it is not interrupted: look again later.
                let mut state = match shared.try_lock() {
                    Ok(state) => state,
                    Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
                    Err(TryLockError::WouldBlock) => continue,
                };
                if state.uses == seen {
                    state.conn = None;
                    state.timer_armed = false;
                    tracing::debug!("history database released");
                    return;
                }
                seen = state.uses;
            }
        }));
    }

    /// Stores an entry with its tags and returns its id.
    pub fn add(&self, item: &NewHistoryItem) -> Result<i64, LibraryError> {
        let path = item.path.as_deref().map(text_of).transpose()?;
        let thumbnail_path = item.thumbnail_path.as_deref().map(text_of).transpose()?;
        let size_bytes = item.size_bytes.map(signed).transpose()?;
        let time = stored_time(item.time).to_string();

        self.with(|conn| {
            let tx = conn.transaction()?;
            tx.execute(
                "INSERT INTO history (time, kind, file_name, path, url, thumbnail_url, deletion_url, host, width, \
             height, size_bytes, thumbnail_path, short_url) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
                rusqlite::params![
                    time,
                    kind_text(item.kind),
                    item.file_name,
                    path,
                    item.url,
                    item.thumbnail_url,
                    item.deletion_url,
                    item.host,
                    item.width,
                    item.height,
                    size_bytes,
                    thumbnail_path,
                    item.short_url,
                ],
            )?;
            let id = tx.last_insert_rowid();
            insert_tags(&tx, id, &item.tags)?;
            tx.commit()?;
            Ok(id)
        })
    }

    pub fn get(&self, id: i64) -> Result<Option<HistoryItem>, LibraryError> {
        self.with(|conn| {
            let item = conn.query_row(&format!("SELECT {COLUMNS} FROM history WHERE id = ?1"), [id], map_row).optional()?;
            match item {
                Some(mut item) => {
                    item.tags = load_tags(conn, item.id)?;
                    Ok(Some(item))
                }
                None => Ok(None),
            }
        })
    }

    /// Entries matching `q`, newest first, with the given paging.
    pub fn query(&self, q: &HistoryQuery) -> Result<Vec<HistoryItem>, LibraryError> {
        let mut sql = format!("SELECT {COLUMNS} FROM history AS h WHERE 1 = 1");
        let mut params: Vec<Value> = Vec::new();

        // Each word must match somewhere: a file-name prefix (FTS5), or a substring of the URL or host.
        for word in q.text.as_deref().unwrap_or_default().split_whitespace() {
            sql.push_str(
                " AND (h.id IN (SELECT rowid FROM history_fts WHERE history_fts MATCH ?) \
                 OR h.url LIKE ? ESCAPE '\\' OR h.host LIKE ? ESCAPE '\\')",
            );
            let like = format!("%{}%", escape_like(word));
            params.push(Value::Text(fts_phrase(word)));
            params.push(Value::Text(like.clone()));
            params.push(Value::Text(like));
        }
        if let Some(kind) = q.kind {
            sql.push_str(" AND h.kind = ?");
            params.push(Value::Text(kind_text(kind).to_owned()));
        }
        if let Some(tag) = &q.tag {
            sql.push_str(" AND EXISTS (SELECT 1 FROM history_tags AS t WHERE t.item_id = h.id AND t.tag = ?)");
            params.push(Value::Text(tag.clone()));
        }
        if let Some(from) = q.from {
            sql.push_str(" AND h.time >= ?");
            params.push(Value::Text(stored_time(from).to_string()));
        }
        if let Some(to) = q.to {
            sql.push_str(" AND h.time <= ?");
            params.push(Value::Text(stored_time(to).to_string()));
        }
        sql.push_str(" ORDER BY h.time DESC, h.id DESC LIMIT ? OFFSET ?");
        // LIMIT -1 means no limit in SQLite.
        params.push(Value::Integer(if q.limit == 0 { -1 } else { i64::from(q.limit) }));
        params.push(Value::Integer(i64::from(q.offset)));

        self.with(|conn| {
            let mut stmt = conn.prepare(&sql)?;
            let mut items = stmt.query_map(params_from_iter(params.iter()), map_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
            drop(stmt);
            for item in &mut items {
                item.tags = load_tags(conn, item.id)?;
            }
            Ok(items)
        })
    }

    /// Deletes an entry and its tags. An unknown id is not an error.
    pub fn delete(&self, id: i64) -> Result<(), LibraryError> {
        self.with(|conn| {
            conn.execute("DELETE FROM history WHERE id = ?1", [id])?;
            Ok(())
        })
    }

    /// Replaces the tags of an entry. An unknown id is not an error.
    pub fn set_tags(&self, id: i64, tags: &[String]) -> Result<(), LibraryError> {
        self.with(|conn| {
            let tx = conn.transaction()?;
            let exists = tx.query_row("SELECT EXISTS (SELECT 1 FROM history WHERE id = ?1)", [id], |row| row.get::<_, bool>(0))?;
            if exists {
                tx.execute("DELETE FROM history_tags WHERE item_id = ?1", [id])?;
                insert_tags(&tx, id, tags)?;
            }
            tx.commit()?;
            Ok(())
        })
    }

    /// Sets the thumbnail path of an entry. An unknown id is not an error.
    pub fn set_thumbnail(&self, id: i64, path: &Path) -> Result<(), LibraryError> {
        let path = text_of(path)?;
        self.with(|conn| {
            conn.execute("UPDATE history SET thumbnail_path = ?1 WHERE id = ?2", rusqlite::params![path, id])?;
            Ok(())
        })
    }

    /// Sets the short link of an entry. Returns `false` when no entry has the id.
    pub fn set_short_url(&self, id: i64, short_url: &str) -> Result<bool, LibraryError> {
        self.with(|conn| Ok(conn.execute("UPDATE history SET short_url = ?1 WHERE id = ?2", rusqlite::params![short_url, id])? > 0))
    }

    /// Sets the upload links and host of an entry. Returns `false` when no entry has the id.
    pub fn set_upload(
        &self,
        id: i64,
        url: &str,
        thumbnail_url: Option<&str>,
        deletion_url: Option<&str>,
        host: Option<&str>,
    ) -> Result<bool, LibraryError> {
        self.with(|conn| {
            let changed = conn.execute(
                "UPDATE history SET url = ?1, thumbnail_url = ?2, deletion_url = ?3, host = ?4 WHERE id = ?5",
                rusqlite::params![url, thumbnail_url, deletion_url, host, id],
            )?;
            Ok(changed > 0)
        })
    }

    pub fn stats(&self) -> Result<HistoryStats, LibraryError> {
        self.with(|conn| {
            let (total, total_bytes): (u64, u64) =
                conn.query_row("SELECT COUNT(*), COALESCE(SUM(size_bytes), 0) FROM history", [], |row| {
                    Ok((unsigned(row.get(0)?, 0)?, unsigned(row.get(1)?, 1)?))
                })?;

            let mut by_kind: Vec<(HistoryKind, u64)> = KINDS.iter().map(|&kind| (kind, 0)).collect();
            let mut stmt = conn.prepare("SELECT kind, COUNT(*) FROM history GROUP BY kind")?;
            let rows = stmt.query_map([], |row| Ok((row.get::<_, String>(0)?, unsigned(row.get(1)?, 1)?)))?;
            for row in rows {
                let (kind, count) = row?;
                if let Some(entry) = parse_kind(&kind).and_then(|kind| by_kind.iter_mut().find(|(k, _)| *k == kind)) {
                    entry.1 = count;
                }
            }
            Ok(HistoryStats { total, by_kind, total_bytes })
        })
    }

    /// Moves the write-ahead log into `history.db` and empties the log, so the file alone holds every
    /// committed entry. Fails when another connection blocks the checkpoint.
    pub fn checkpoint(&self) -> Result<(), LibraryError> {
        // The row is (busy, log frames, checkpointed frames). busy is 1 when SQLite could not finish.
        let busy: i64 = self.with(|conn| Ok(conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| row.get(0))?))?;
        if busy == 0 {
            Ok(())
        } else {
            Err(io::Error::new(io::ErrorKind::WouldBlock, "history checkpoint was blocked by another connection").into())
        }
    }

    /// Deletes the oldest entries beyond `max_items`. Returns how many were deleted. 0 keeps everything.
    pub fn trim(&self, max_items: u32) -> Result<u64, LibraryError> {
        if max_items == 0 {
            return Ok(0);
        }
        let deleted = self.with(|conn| {
            Ok(conn.execute(
                "DELETE FROM history WHERE id NOT IN (SELECT id FROM history ORDER BY time DESC, id DESC LIMIT ?1)",
                [i64::from(max_items)],
            )?)
        })?;
        Ok(deleted as u64)
    }
}

/// The port the application edits history through (`laqta-ports`). Both calls are the inherent ones above.
impl HistoryEdit for History {
    fn set_tags(&self, id: i64, tags: &[String]) -> Result<(), PortError> {
        History::set_tags(self, id, tags).map_err(|error| PortError::Failed(error.to_string()))
    }

    fn delete(&self, id: i64) -> Result<(), PortError> {
        History::delete(self, id).map_err(|error| PortError::Failed(error.to_string()))
    }
}

/// How many recent saves an upload can still link to. An upload of an older save adds its own entry.
const LINKABLE_SAVES: usize = 64;

/// Writes a history entry for each `TaskEvent::Saved` and records each `TaskEvent::Uploaded` until
/// the channel closes. An upload of a saved file fills in that file's entry; an upload of a file from
/// disk adds an entry for that file; an upload with no file (text, a URL) adds an entry of kind `Url`. A failure is logged and that event is skipped.
/// Each entry is timestamped with `clock.now()`.
///
/// After each entry, the oldest entries beyond `max_items()` are deleted (the setting
/// `history.max_items`, read at each save; 0 keeps everything).
pub async fn record_task_events(
    history: Arc<History>,
    mut events: async_broadcast::Receiver<TaskEvent>,
    clock: Arc<dyn Clock>,
    max_items: impl Fn() -> u32,
) {
    // Saves not yet matched by an upload: task id and history entry id, oldest first.
    let mut saves: VecDeque<(TaskId, i64)> = VecDeque::with_capacity(LINKABLE_SAVES);
    // Uploads a shortened link may still follow: task id and history entry id, oldest first.
    let mut uploads: VecDeque<(TaskId, i64)> = VecDeque::with_capacity(LINKABLE_SAVES);
    loop {
        match events.recv().await {
            Ok(TaskEvent::Saved { id, path }) => {
                match record_saved(&history, &path, clock.now()) {
                    Ok(entry) => {
                        if saves.len() == LINKABLE_SAVES {
                            saves.pop_front();
                        }
                        saves.push_back((id, entry));
                    }
                    Err(error) => tracing::warn!(path = %path.display(), %error, "history: could not record a saved file"),
                }
                if let Err(error) = history.trim(max_items()) {
                    tracing::warn!(%error, "history: could not remove the oldest entries");
                }
            }
            Ok(TaskEvent::Uploaded { id, url, thumbnail_url, deletion_url, path }) => {
                let mut saved = saves.iter().rposition(|(task, _)| *task == id).and_then(|at| saves.remove(at)).map(|(_, entry)| entry);
                // A file uploaded from disk had no Saved event: its entry is made now, as a save's would be.
                if saved.is_none()
                    && let Some(path) = &path
                {
                    match record_saved(&history, path, clock.now()) {
                        Ok(entry) => saved = Some(entry),
                        Err(error) => tracing::warn!(path = %path.display(), %error, "history: could not record an uploaded file"),
                    }
                }
                match record_upload(&history, saved, &url, thumbnail_url.as_deref(), deletion_url.as_deref(), clock.now()) {
                    Ok(entry) => {
                        if uploads.len() == LINKABLE_SAVES {
                            uploads.pop_front();
                        }
                        uploads.push_back((id, entry));
                    }
                    Err(error) => tracing::warn!(%error, "history: could not record an upload"),
                }
                if let Err(error) = history.trim(max_items()) {
                    tracing::warn!(%error, "history: could not remove the oldest entries");
                }
            }
            Ok(TaskEvent::Shortened { id, url }) => {
                let upload = uploads.iter().rposition(|(task, _)| *task == id).and_then(|at| uploads.remove(at)).map(|(_, entry)| entry);
                if let Err(error) = record_short(&history, upload, &url, clock.now()) {
                    tracing::warn!(%error, "history: could not record a shortened link");
                }
                if let Err(error) = history.trim(max_items()) {
                    tracing::warn!(%error, "history: could not remove the oldest entries");
                }
            }
            Ok(_) => {}
            Err(async_broadcast::RecvError::Overflowed(skipped)) => {
                tracing::warn!(skipped, "history: task events were skipped");
            }
            Err(async_broadcast::RecvError::Closed) => break,
        }
    }
}

fn record_saved(history: &History, path: &Path, now: jiff::civil::DateTime) -> Result<i64, LibraryError> {
    let size_bytes = match fs::metadata(path) {
        Ok(metadata) => Some(metadata.len()),
        Err(error) => {
            // A missing size does not lose the entry: the save happened, so keep the record.
            tracing::warn!(path = %path.display(), %error, "history: no size for a saved file");
            None
        }
    };
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "saved path has no UTF-8 file name"))?;
    let kind = kind_for_path(path);
    let (width, height) = match png_dimensions(path) {
        Some((width, height)) if kind == HistoryKind::Image => (Some(width), Some(height)),
        _ => (None, None),
    };
    history.add(&NewHistoryItem {
        time: now,
        kind,
        file_name: file_name.to_owned(),
        path: Some(path.to_path_buf()),
        url: None,
        thumbnail_url: None,
        deletion_url: None,
        short_url: None,
        host: None,
        width,
        height,
        size_bytes,
        thumbnail_path: None,
        tags: Vec::new(),
    })
}

/// Adds a short link to its upload's entry when that entry still exists. Otherwise (a link shortened
/// on its own) adds an entry of kind `Url` whose link and short link are both the short one.
fn record_short(history: &History, upload: Option<i64>, short_url: &str, now: jiff::civil::DateTime) -> Result<(), LibraryError> {
    if let Some(entry) = upload
        && history.set_short_url(entry, short_url)?
    {
        return Ok(());
    }
    history.add(&NewHistoryItem {
        time: now,
        kind: HistoryKind::Url,
        file_name: upload_file_name(short_url),
        path: None,
        url: Some(short_url.to_owned()),
        thumbnail_url: None,
        deletion_url: None,
        short_url: Some(short_url.to_owned()),
        host: host_of(short_url).map(str::to_owned),
        width: None,
        height: None,
        size_bytes: None,
        thumbnail_path: None,
        tags: Vec::new(),
    })?;
    Ok(())
}

/// Links an upload to the entry of its saved file when that entry still exists. Otherwise adds an
/// entry of kind `Url`, named after the last segment of the URL. Returns the entry's id.
fn record_upload(
    history: &History,
    saved: Option<i64>,
    url: &str,
    thumbnail_url: Option<&str>,
    deletion_url: Option<&str>,
    now: jiff::civil::DateTime,
) -> Result<i64, LibraryError> {
    let host = host_of(url);
    if let Some(entry) = saved
        && history.set_upload(entry, url, thumbnail_url, deletion_url, host)?
    {
        return Ok(entry);
    }
    history.add(&NewHistoryItem {
        time: now,
        kind: HistoryKind::Url,
        file_name: upload_file_name(url),
        path: None,
        url: Some(url.to_owned()),
        thumbnail_url: thumbnail_url.map(str::to_owned),
        deletion_url: deletion_url.map(str::to_owned),
        short_url: None,
        host: host.map(str::to_owned),
        width: None,
        height: None,
        size_bytes: None,
        thumbnail_path: None,
        tags: Vec::new(),
    })
}

/// The host of a URL, without scheme, userinfo, port or path. `None` when the URL has no `://`.
fn host_of(url: &str) -> Option<&str> {
    let authority = url.split_once("://")?.1.split(['/', '?', '#']).next()?;
    let host = authority.rsplit_once('@').map_or(authority, |(_, host)| host);
    let host = match host.strip_prefix('[') {
        Some(bracketed) => bracketed.split_once(']').map_or(bracketed, |(inner, _)| inner),
        None => host.split_once(':').map_or(host, |(name, _)| name),
    };
    (!host.is_empty()).then_some(host)
}

/// The last path segment of a URL, or the whole URL when it has none.
fn upload_file_name(url: &str) -> String {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    path.rsplit('/').find(|segment| !segment.is_empty()).unwrap_or(url).to_owned()
}

/// Width and height from the IHDR chunk of a PNG file. `None` for other files, or when the header
/// does not read. Only the header is decoded.
fn png_dimensions(path: &Path) -> Option<(u32, u32)> {
    if !path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("png")) {
        return None;
    }
    let file = fs::File::open(path).ok()?;
    match png::Decoder::new(io::BufReader::new(file)).read_info() {
        Ok(reader) => {
            let info = reader.info();
            Some((info.width, info.height))
        }
        Err(error) => {
            tracing::debug!(path = %path.display(), %error, "history: no PNG header to read");
            None
        }
    }
}

/// Opens the database file and sets it up.
fn connect(path: &Path) -> Result<Connection, LibraryError> {
    configure(Connection::open(path)?)
}

/// The pragmas and the schema, for a fresh connection.
fn configure(mut conn: Connection) -> Result<Connection, LibraryError> {
    conn.busy_timeout(Duration::from_secs(5))?;
    conn.pragma_update(None, "foreign_keys", true)?;
    // Returns the mode it reached: "wal" on disk, "memory" in memory.
    conn.pragma_update_and_check(None, "journal_mode", "WAL", |row| row.get::<_, String>(0))?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    migrate(&mut conn)?;
    Ok(conn)
}

fn lock_state(state: &Mutex<State>) -> MutexGuard<'_, State> {
    state.lock().unwrap_or_else(PoisonError::into_inner)
}

fn migrate(conn: &mut Connection) -> Result<(), LibraryError> {
    let found = conn.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))?;
    let supported = i64::try_from(MIGRATIONS.len()).unwrap_or(i64::MAX);
    if found > supported {
        return Err(io::Error::new(io::ErrorKind::Unsupported, format!("history database schema {found} is newer than {supported}")).into());
    }
    for (index, sql) in MIGRATIONS.iter().enumerate() {
        let version = i64::try_from(index + 1).unwrap_or(i64::MAX);
        if version <= found {
            continue;
        }
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", version)?;
        tx.commit()?;
    }
    Ok(())
}

fn insert_tags(conn: &Connection, id: i64, tags: &[String]) -> rusqlite::Result<()> {
    let mut stmt = conn.prepare_cached("INSERT INTO history_tags (item_id, pos, tag) VALUES (?1, ?2, ?3)")?;
    for (pos, tag) in tags.iter().enumerate() {
        stmt.execute(rusqlite::params![id, i64::try_from(pos).unwrap_or(i64::MAX), tag])?;
    }
    Ok(())
}

fn load_tags(conn: &Connection, id: i64) -> rusqlite::Result<Vec<String>> {
    let mut stmt = conn.prepare_cached("SELECT tag FROM history_tags WHERE item_id = ?1 ORDER BY pos")?;
    let tags = stmt.query_map([id], |row| row.get::<_, String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(tags)
}

fn map_row(row: &Row<'_>) -> rusqlite::Result<HistoryItem> {
    let time: String = row.get(1)?;
    let kind: String = row.get(2)?;
    Ok(HistoryItem {
        id: row.get(0)?,
        time: time
            .parse::<jiff::civil::DateTime>()
            .map_err(|error| rusqlite::Error::FromSqlConversionFailure(1, Type::Text, Box::new(error)))?,
        kind: parse_kind(&kind)
            .ok_or_else(|| rusqlite::Error::FromSqlConversionFailure(2, Type::Text, format!("unknown kind {kind:?}").into()))?,
        file_name: row.get(3)?,
        path: row.get::<_, Option<String>>(4)?.map(PathBuf::from),
        url: row.get(5)?,
        thumbnail_url: row.get(6)?,
        deletion_url: row.get(7)?,
        host: row.get(8)?,
        width: row.get(9)?,
        height: row.get(10)?,
        size_bytes: row.get::<_, Option<i64>>(11)?.map(|size| unsigned(size, 11)).transpose()?,
        thumbnail_path: row.get::<_, Option<String>>(12)?.map(PathBuf::from),
        short_url: row.get(13)?,
        tags: Vec::new(),
    })
}

/// SQLite integers are signed; counts and sizes never go negative, so a negative value is a bad row.
fn unsigned(value: i64, column: usize) -> rusqlite::Result<u64> {
    u64::try_from(value).map_err(|error| rusqlite::Error::FromSqlConversionFailure(column, Type::Integer, Box::new(error)))
}

/// A size for storage. Fails only beyond `i64::MAX`, which no file reaches.
fn signed(value: u64) -> Result<i64, LibraryError> {
    i64::try_from(value).map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "size exceeds the SQLite integer range").into())
}

/// Times are stored with whole-second precision, so the text sorts in time order.
fn stored_time(time: jiff::civil::DateTime) -> jiff::civil::DateTime {
    time.with().subsec_nanosecond(0).build().unwrap_or(time)
}

fn kind_text(kind: HistoryKind) -> &'static str {
    match kind {
        HistoryKind::Image => "image",
        HistoryKind::Text => "text",
        HistoryKind::File => "file",
        HistoryKind::Url => "url",
    }
}

fn parse_kind(text: &str) -> Option<HistoryKind> {
    match text {
        "image" => Some(HistoryKind::Image),
        "text" => Some(HistoryKind::Text),
        "file" => Some(HistoryKind::File),
        "url" => Some(HistoryKind::Url),
        _ => None,
    }
}

/// SQL needs UTF-8 text, so a path that is not UTF-8 is refused rather than stored lossily.
fn text_of(path: &Path) -> Result<&str, LibraryError> {
    path.to_str().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, format!("path is not UTF-8: {}", path.display())).into())
}

/// One FTS5 phrase for a word: quoted so that FTS operators in file names are literal, with a
/// prefix match.
fn fts_phrase(word: &str) -> String {
    format!("\"{}\"*", word.replace('"', "\"\""))
}

/// Escapes LIKE wildcards for use with `ESCAPE '\'`.
fn escape_like(word: &str) -> String {
    let mut escaped = String::with_capacity(word.len());
    for c in word.chars() {
        if matches!(c, '\\' | '%' | '_') {
            escaped.push('\\');
        }
        escaped.push(c);
    }
    escaped
}
