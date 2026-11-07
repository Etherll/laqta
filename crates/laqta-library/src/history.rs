//! History in SQLite (ADR 006, M1 D7): entries, tags, a full-text index over file names, and the
//! task-event subscriber that writes entries.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use laqta_domain::history::{
    HistoryItem, HistoryKind, HistoryQuery, HistoryStats, NewHistoryItem, kind_for_path,
};
use laqta_ports::{Clock, TaskEvent};
use rusqlite::types::{Type, Value};
use rusqlite::{Connection, OptionalExtension, Row, params_from_iter};

use crate::LibraryError;

/// `MIGRATIONS[i]` moves `PRAGMA user_version` from `i` to `i + 1`. Never edit a shipped entry.
const MIGRATIONS: [&str; 1] = [
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
];

/// Every column `map_row` reads, in its order.
const COLUMNS: &str = "id, time, kind, file_name, path, url, thumbnail_url, deletion_url, host, width, height, \
                       size_bytes, thumbnail_path";

const KINDS: [HistoryKind; 4] = [
    HistoryKind::Image,
    HistoryKind::Text,
    HistoryKind::File,
    HistoryKind::Url,
];

/// The history database. The connection sits behind a mutex, so the value is `Send + Sync`.
#[derive(Debug)]
pub struct History {
    conn: Mutex<Connection>,
}

impl History {
    /// Opens or creates the database at `path` in WAL mode and migrates its schema.
    pub fn open(path: &Path) -> Result<History, LibraryError> {
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent)?;
        }
        Self::setup(Connection::open(path)?)
    }

    pub fn open_in_memory() -> Result<History, LibraryError> {
        Self::setup(Connection::open_in_memory()?)
    }

    fn setup(mut conn: Connection) -> Result<History, LibraryError> {
        conn.busy_timeout(Duration::from_secs(5))?;
        conn.pragma_update(None, "foreign_keys", true)?;
        // Returns the mode it reached: "wal" on disk, "memory" in memory.
        conn.pragma_update_and_check(None, "journal_mode", "WAL", |row| row.get::<_, String>(0))?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        migrate(&mut conn)?;
        Ok(History {
            conn: Mutex::new(conn),
        })
    }

    fn lock(&self) -> MutexGuard<'_, Connection> {
        // A panic while the lock was held leaves SQLite itself consistent, so keep using it.
        self.conn.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Stores an entry with its tags and returns its id.
    pub fn add(&self, item: &NewHistoryItem) -> Result<i64, LibraryError> {
        let path = item.path.as_deref().map(text_of).transpose()?;
        let thumbnail_path = item.thumbnail_path.as_deref().map(text_of).transpose()?;
        let size_bytes = item.size_bytes.map(signed).transpose()?;
        let time = stored_time(item.time).to_string();

        let mut conn = self.lock();
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO history (time, kind, file_name, path, url, thumbnail_url, deletion_url, host, width, \
             height, size_bytes, thumbnail_path) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
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
            ],
        )?;
        let id = tx.last_insert_rowid();
        insert_tags(&tx, id, &item.tags)?;
        tx.commit()?;
        Ok(id)
    }

    pub fn get(&self, id: i64) -> Result<Option<HistoryItem>, LibraryError> {
        let conn = self.lock();
        let item = conn
            .query_row(
                &format!("SELECT {COLUMNS} FROM history WHERE id = ?1"),
                [id],
                map_row,
            )
            .optional()?;
        match item {
            Some(mut item) => {
                item.tags = load_tags(&conn, item.id)?;
                Ok(Some(item))
            }
            None => Ok(None),
        }
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
        params.push(Value::Integer(if q.limit == 0 {
            -1
        } else {
            i64::from(q.limit)
        }));
        params.push(Value::Integer(i64::from(q.offset)));

        let conn = self.lock();
        let mut stmt = conn.prepare(&sql)?;
        let mut items = stmt
            .query_map(params_from_iter(params.iter()), map_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(stmt);
        for item in &mut items {
            item.tags = load_tags(&conn, item.id)?;
        }
        Ok(items)
    }

    /// Deletes an entry and its tags. An unknown id is not an error.
    pub fn delete(&self, id: i64) -> Result<(), LibraryError> {
        self.lock()
            .execute("DELETE FROM history WHERE id = ?1", [id])?;
        Ok(())
    }

    /// Replaces the tags of an entry. An unknown id is not an error.
    pub fn set_tags(&self, id: i64, tags: &[String]) -> Result<(), LibraryError> {
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        let exists = tx.query_row(
            "SELECT EXISTS (SELECT 1 FROM history WHERE id = ?1)",
            [id],
            |row| row.get::<_, bool>(0),
        )?;
        if exists {
            tx.execute("DELETE FROM history_tags WHERE item_id = ?1", [id])?;
            insert_tags(&tx, id, tags)?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Sets the thumbnail path of an entry. An unknown id is not an error.
    pub fn set_thumbnail(&self, id: i64, path: &Path) -> Result<(), LibraryError> {
        let path = text_of(path)?;
        self.lock().execute(
            "UPDATE history SET thumbnail_path = ?1 WHERE id = ?2",
            rusqlite::params![path, id],
        )?;
        Ok(())
    }

    pub fn stats(&self) -> Result<HistoryStats, LibraryError> {
        let conn = self.lock();
        let (total, total_bytes): (u64, u64) = conn.query_row(
            "SELECT COUNT(*), COALESCE(SUM(size_bytes), 0) FROM history",
            [],
            |row| Ok((unsigned(row.get(0)?, 0)?, unsigned(row.get(1)?, 1)?)),
        )?;

        let mut by_kind: Vec<(HistoryKind, u64)> = KINDS.iter().map(|&kind| (kind, 0)).collect();
        let mut stmt = conn.prepare("SELECT kind, COUNT(*) FROM history GROUP BY kind")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, unsigned(row.get(1)?, 1)?))
        })?;
        for row in rows {
            let (kind, count) = row?;
            if let Some(entry) =
                parse_kind(&kind).and_then(|kind| by_kind.iter_mut().find(|(k, _)| *k == kind))
            {
                entry.1 = count;
            }
        }
        Ok(HistoryStats {
            total,
            by_kind,
            total_bytes,
        })
    }

    /// Deletes the oldest entries beyond `max_items`. Returns how many were deleted. 0 keeps everything.
    pub fn trim(&self, max_items: u32) -> Result<u64, LibraryError> {
        if max_items == 0 {
            return Ok(0);
        }
        let deleted = self.lock().execute(
            "DELETE FROM history WHERE id NOT IN (SELECT id FROM history ORDER BY time DESC, id DESC LIMIT ?1)",
            [i64::from(max_items)],
        )?;
        Ok(deleted as u64)
    }
}

/// Writes a history entry for each `TaskEvent::Saved` until the channel closes. A failure is logged
/// and that event is skipped. Each entry is timestamped with `clock.now()`.
pub async fn record_task_events(
    history: Arc<History>,
    mut events: async_broadcast::Receiver<TaskEvent>,
    clock: Arc<dyn Clock>,
) {
    loop {
        match events.recv().await {
            Ok(TaskEvent::Saved { path, .. }) => {
                if let Err(error) = record_saved(&history, &path, clock.now()) {
                    tracing::warn!(path = %path.display(), %error, "history: could not record a saved file");
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

fn record_saved(
    history: &History,
    path: &Path,
    now: jiff::civil::DateTime,
) -> Result<(), LibraryError> {
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
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "saved path has no UTF-8 file name",
            )
        })?;
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
        host: None,
        width,
        height,
        size_bytes,
        thumbnail_path: None,
        tags: Vec::new(),
    })?;
    Ok(())
}

/// Width and height from the IHDR chunk of a PNG file. `None` for other files, or when the header
/// does not read. Only the header is decoded.
fn png_dimensions(path: &Path) -> Option<(u32, u32)> {
    if !path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("png"))
    {
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

fn migrate(conn: &mut Connection) -> Result<(), LibraryError> {
    let found = conn.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))?;
    let supported = i64::try_from(MIGRATIONS.len()).unwrap_or(i64::MAX);
    if found > supported {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            format!("history database schema {found} is newer than {supported}"),
        )
        .into());
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
    let mut stmt =
        conn.prepare_cached("INSERT INTO history_tags (item_id, pos, tag) VALUES (?1, ?2, ?3)")?;
    for (pos, tag) in tags.iter().enumerate() {
        stmt.execute(rusqlite::params![
            id,
            i64::try_from(pos).unwrap_or(i64::MAX),
            tag
        ])?;
    }
    Ok(())
}

fn load_tags(conn: &Connection, id: i64) -> rusqlite::Result<Vec<String>> {
    let mut stmt =
        conn.prepare_cached("SELECT tag FROM history_tags WHERE item_id = ?1 ORDER BY pos")?;
    let tags = stmt
        .query_map([id], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(tags)
}

fn map_row(row: &Row<'_>) -> rusqlite::Result<HistoryItem> {
    let time: String = row.get(1)?;
    let kind: String = row.get(2)?;
    Ok(HistoryItem {
        id: row.get(0)?,
        time: time.parse::<jiff::civil::DateTime>().map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(1, Type::Text, Box::new(error))
        })?,
        kind: parse_kind(&kind).ok_or_else(|| {
            rusqlite::Error::FromSqlConversionFailure(
                2,
                Type::Text,
                format!("unknown kind {kind:?}").into(),
            )
        })?,
        file_name: row.get(3)?,
        path: row.get::<_, Option<String>>(4)?.map(PathBuf::from),
        url: row.get(5)?,
        thumbnail_url: row.get(6)?,
        deletion_url: row.get(7)?,
        host: row.get(8)?,
        width: row.get(9)?,
        height: row.get(10)?,
        size_bytes: row
            .get::<_, Option<i64>>(11)?
            .map(|size| unsigned(size, 11))
            .transpose()?,
        thumbnail_path: row.get::<_, Option<String>>(12)?.map(PathBuf::from),
        tags: Vec::new(),
    })
}

/// SQLite integers are signed; counts and sizes never go negative, so a negative value is a bad row.
fn unsigned(value: i64, column: usize) -> rusqlite::Result<u64> {
    u64::try_from(value).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(column, Type::Integer, Box::new(error))
    })
}

/// A size for storage. Fails only beyond `i64::MAX`, which no file reaches.
fn signed(value: u64) -> Result<i64, LibraryError> {
    i64::try_from(value).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "size exceeds the SQLite integer range",
        )
        .into()
    })
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
    path.to_str().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("path is not UTF-8: {}", path.display()),
        )
        .into()
    })
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
