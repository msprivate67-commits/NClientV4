//! SQLite-backed persistence.
//!
//! SQLite persistence. Tables:
//! - `favorites`   : local favorite galleries
//! - `history`     : recently visited galleries
//! - `tags`        : cached tags (with a user `status` flag and an
//!                   `online_blacklist` flag)
//! - `downloads`   : in-progress / paused downloads (resumable across restarts)
//! - `local_meta`  : scanned local gallery metadata cache

use std::path::Path;
use std::sync::Arc;

use chrono::Utc;
use parking_lot::Mutex;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

use crate::error::AppResult;
use crate::models::{HistoryEntry, LocalGallery, Tag, TagStatus, TagType};

/// Thread-safe SQLite handle. All access goes through a single `Mutex`; given
/// the access pattern of a desktop client this is plenty.
#[derive(Clone)]
pub struct Database {
    conn: Arc<Mutex<Connection>>,
}

impl std::fmt::Debug for Database {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Database").finish()
    }
}

impl Database {
    pub fn open(app_data: &Path) -> AppResult<Self> {
        std::fs::create_dir_all(app_data).ok();
        let path = app_data.join("nclientv4.db");
        let conn = Connection::open(&path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        migrate(&conn)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    fn with_conn<F, T>(&self, f: F) -> AppResult<T>
    where
        F: FnOnce(&Connection) -> AppResult<T>,
    {
        let conn = self.conn.lock();
        f(&conn)
    }

    // ---------------------------------------------------------------------
    // Favorites
    // ---------------------------------------------------------------------

    pub fn fav_add(&self, id: i64, title: &str, media_id: i64, thumbnail: &str) -> AppResult<()> {
        self.with_conn(|c| {
            c.execute(
                "INSERT OR REPLACE INTO favorites (id, title, media_id, thumbnail, added_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![id, title, media_id, thumbnail, Utc::now().to_rfc3339()],
            )?;
            Ok(())
        })
    }

    pub fn fav_remove(&self, id: i64) -> AppResult<()> {
        self.with_conn(|c| {
            c.execute("DELETE FROM favorites WHERE id = ?1", params![id])?;
            Ok(())
        })
    }

    pub fn fav_is(&self, id: i64) -> AppResult<bool> {
        self.with_conn(|c| {
            let v: i64 = c.query_row(
                "SELECT COUNT(*) FROM favorites WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )?;
            Ok(v > 0)
        })
    }

    pub fn fav_list(&self, limit: u32, offset: u32) -> AppResult<Vec<FavoriteRow>> {
        self.with_conn(|c| {
            let mut stmt = c.prepare(
                "SELECT id, title, media_id, thumbnail, added_at
                 FROM favorites ORDER BY added_at DESC LIMIT ?1 OFFSET ?2",
            )?;
            let rows = stmt
                .query_map(params![limit, offset], |r| {
                    Ok(FavoriteRow {
                        id: r.get(0)?,
                        title: r.get(1)?,
                        media_id: r.get(2)?,
                        thumbnail: r.get(3)?,
                        added_at: r.get(4)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
    }

    // ---------------------------------------------------------------------
    // Read progress
    // ---------------------------------------------------------------------

    /// Record (or advance) the furthest page the user has reached in a gallery.
    /// Only moves the marker forward; re-reading earlier pages never regresses
    /// it. `read` flips to true once `last_page` crosses 50% of `total_pages`.
    pub fn read_progress_upsert(
        &self,
        gallery_id: i64,
        last_page: usize,
        total_pages: usize,
    ) -> AppResult<()> {
        self.with_conn(|c| {
            let now = Utc::now().to_rfc3339();
            let total = total_pages as i64;
            let page = last_page as i64;
            // A gallery counts as "read" once >= 50% of its pages have been
            // viewed. We recompute rather than trust the caller so partial /
            // out-of-order reports still resolve to the right state.
            let read = if total > 0 && page * 2 >= total { 1 } else { 0 };
            c.execute(
                "INSERT INTO read_progress (gallery_id, last_page, total_pages, read, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(gallery_id) DO UPDATE SET
                    last_page   = MAX(excluded.last_page, read_progress.last_page),
                    total_pages = excluded.total_pages,
                    read        = MAX(excluded.read, read_progress.read),
                    updated_at  = excluded.updated_at",
                params![gallery_id, page, total, read, now],
            )?;
            Ok(())
        })
    }

    /// Reset (or remove) a gallery's read progress. Used when the user wants
    /// to mark something as unread again.
    pub fn read_progress_reset(&self, gallery_id: i64) -> AppResult<()> {
        self.with_conn(|c| {
            c.execute(
                "DELETE FROM read_progress WHERE gallery_id = ?1",
                params![gallery_id],
            )?;
            Ok(())
        })
    }

    /// Lookup a single gallery's progress.
    pub fn read_progress_get(&self, gallery_id: i64) -> AppResult<Option<ReadProgressRow>> {
        self.with_conn(|c| {
            let mut stmt = c.prepare(
                "SELECT gallery_id, last_page, total_pages, read, updated_at
                 FROM read_progress WHERE gallery_id = ?1",
            )?;
            let mut rows = stmt.query_map(params![gallery_id], row_to_read_progress)?;
            match rows.next().transpose()? {
                Some(row) => Ok(Some(row)),
                None => Ok(None),
            }
        })
    }

    /// Return the set of gallery IDs the user has finished (>= 50%). Used by
    /// the frontend to badge covers in the online gallery + local library.
    pub fn read_progress_ids(&self) -> AppResult<Vec<i64>> {
        self.with_conn(|c| {
            let mut stmt = c.prepare("SELECT gallery_id FROM read_progress WHERE read = 1")?;
            let rows = stmt
                .query_map([], |r| r.get::<_, i64>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
    }

    // ── local reader resume position ──────────────────────────────────────

    /// Remember the exact page the user stopped at in the local reader.
    /// Unlike `read_progress_upsert` this *overwrites* on every call so the
    /// resume point tracks the user's real position (scrolling back is
    /// remembered), rather than a furthest-reached high-water mark.
    pub fn local_reader_progress_set(
        &self,
        gallery_id: i64,
        page: usize,
        total_pages: usize,
    ) -> AppResult<()> {
        self.with_conn(|c| {
            let now = Utc::now().to_rfc3339();
            c.execute(
                "INSERT INTO local_reader_progress (gallery_id, page, total_pages, updated_at)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(gallery_id) DO UPDATE SET
                    page        = excluded.page,
                    total_pages = excluded.total_pages,
                    updated_at  = excluded.updated_at",
                params![gallery_id, page as i64, total_pages as i64, now],
            )?;
            Ok(())
        })
    }

    /// Fetch the saved resume page for a gallery (1-based), if any.
    pub fn local_reader_progress_get(&self, gallery_id: i64) -> AppResult<Option<usize>> {
        self.with_conn(|c| {
            let mut stmt =
                c.prepare("SELECT page FROM local_reader_progress WHERE gallery_id = ?1")?;
            let mut rows = stmt.query_map(params![gallery_id], |r| r.get::<_, i64>(0))?;
            match rows.next().transpose()? {
                Some(page) => Ok(Some(page.max(1) as usize)),
                None => Ok(None),
            }
        })
    }
}

/// One row of read-progress state. `read` mirrors the SQL boolean (0/1).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadProgressRow {
    pub gallery_id: i64,
    pub last_page: i64,
    pub total_pages: i64,
    pub read: bool,
    pub updated_at: String,
}

fn row_to_read_progress(row: &rusqlite::Row<'_>) -> rusqlite::Result<ReadProgressRow> {
    let read_int: i64 = row.get(3)?;
    Ok(ReadProgressRow {
        gallery_id: row.get(0)?,
        last_page: row.get(1)?,
        total_pages: row.get(2)?,
        read: read_int != 0,
        updated_at: row.get(4)?,
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FavoriteRow {
    pub id: i64,
    pub title: String,
    pub media_id: i64,
    pub thumbnail: String,
    pub added_at: String,
}

/// One gallery's cached AI translations, keyed by the gallery ID.
///
/// `tags` maps a tag ID to `{ name, translated }` and `comments` maps a
/// comment ID to `{ body, translated }`. The source text is stored alongside
/// every translation so the frontend can detect a stale entry (renamed tag or
/// edited comment) without trusting the cache blindly. `config_key` fingerprints
/// the translation settings the entry was produced with (endpoint / model /
/// target language / thinking / proxy) — entries from a different config are
/// treated as misses.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranslationCacheRow {
    pub gallery_id: i64,
    pub config_key: String,
    pub title_source: String,
    pub title_translated: String,
    pub tags: serde_json::Value,
    pub comments: serde_json::Value,
    pub updated_at: String,
}

/// Global database reference, registered by the runtime at startup so the
/// API client (which doesn't carry a `Database` handle) can still read the
/// local tag cache.
static GLOBAL_DB: once_cell::sync::OnceCell<Database> = once_cell::sync::OnceCell::new();

pub fn register_global(db: &Database) {
    let _ = GLOBAL_DB.set(db.clone());
}

fn global() -> Option<&'static Database> {
    GLOBAL_DB.get()
}

/// Free functions used by `api.rs` (read path) without needing a `Database`
/// handle, so the API client doesn't have to clone the DB around.
pub fn tags_get_by_ids(ids: &[i64]) -> AppResult<Vec<Tag>> {
    let Some(db) = global() else {
        return Ok(vec![]);
    };
    if ids.is_empty() {
        return Ok(vec![]);
    }
    db.with_conn(|c| {
        let placeholders = (0..ids.len()).map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!(
            "SELECT id, name, type, count, status, online_blacklist FROM tags WHERE id IN ({})",
            placeholders
        );
        let mut stmt = c.prepare(&sql)?;
        let args: Vec<&dyn rusqlite::ToSql> =
            ids.iter().map(|i| i as &dyn rusqlite::ToSql).collect();
        let rows = stmt
            .query_map(args.as_slice(), row_to_tag)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    })
}

pub fn tags_search(query: &str, limit: usize) -> AppResult<Vec<Tag>> {
    let Some(db) = global() else {
        return Ok(vec![]);
    };
    db.tags_search(query, limit)
}

pub fn tag_upsert(t: &Tag) -> AppResult<()> {
    let Some(db) = global() else {
        return Ok(());
    };
    db.tag_insert_or_update(t)
}

// ---------------------------------------------------------------------------
// Migrations
// ---------------------------------------------------------------------------

fn migrate(conn: &Connection) -> AppResult<()> {
    for stmt in MIGRATIONS {
        conn.execute_batch(stmt)?;
    }
    // Non-fatal: add column for DBs from older versions.
    conn.execute_batch(
        "ALTER TABLE local_meta ADD COLUMN translated_title TEXT NOT NULL DEFAULT '';",
    )
    .ok();
    Ok(())
}

const MIGRATIONS: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS favorites (
        id           INTEGER PRIMARY KEY,
        title        TEXT NOT NULL,
        media_id     INTEGER NOT NULL DEFAULT 0,
        thumbnail    TEXT NOT NULL DEFAULT '',
        added_at     TEXT NOT NULL
    );",
    "CREATE TABLE IF NOT EXISTS history (
        gallery_id   INTEGER PRIMARY KEY,
        title        TEXT NOT NULL,
        media_id     INTEGER NOT NULL DEFAULT 0,
        thumbnail    TEXT NOT NULL DEFAULT '',
        visited_at   TEXT NOT NULL
    );",
    "CREATE TABLE IF NOT EXISTS tags (
        id                INTEGER PRIMARY KEY,
        name              TEXT NOT NULL,
        type              TEXT NOT NULL DEFAULT 'tag',
        count             INTEGER NOT NULL DEFAULT 0,
        status            TEXT NOT NULL DEFAULT 'default',
        online_blacklist  INTEGER NOT NULL DEFAULT 0
    );",
    "CREATE INDEX IF NOT EXISTS idx_tags_name ON tags(name);",
    "CREATE INDEX IF NOT EXISTS idx_tags_status ON tags(status);",
    "CREATE TABLE IF NOT EXISTS downloads (
        id           INTEGER PRIMARY KEY,
        title        TEXT NOT NULL,
        media_id     INTEGER NOT NULL DEFAULT 0,
        thumbnail    TEXT NOT NULL DEFAULT '',
        folder       TEXT NOT NULL,
        total_pages  INTEGER NOT NULL DEFAULT 0,
        done_pages   INTEGER NOT NULL DEFAULT 0,
        status       TEXT NOT NULL DEFAULT 'pending',
        created_at   TEXT NOT NULL,
        updated_at   TEXT NOT NULL
    );",
    "CREATE TABLE IF NOT EXISTS local_meta (
        folder       TEXT PRIMARY KEY,
        gallery_id   INTEGER NOT NULL,
        title        TEXT NOT NULL,
        media_id     INTEGER NOT NULL DEFAULT 0,
        thumbnail    TEXT NOT NULL DEFAULT '',
        num_pages    INTEGER NOT NULL DEFAULT 0,
        page_files   TEXT NOT NULL DEFAULT '[]',
        scanned_at   TEXT NOT NULL,
        translated_title TEXT NOT NULL DEFAULT ''
    );",
    // Read progress: per-gallery furthest page reached + total pages known at
    // the time. `read` is 1 when the user has seen >= 50% of the gallery.
    "CREATE TABLE IF NOT EXISTS read_progress (
        gallery_id   INTEGER PRIMARY KEY,
        last_page    INTEGER NOT NULL DEFAULT 0,
        total_pages  INTEGER NOT NULL DEFAULT 0,
        read         INTEGER NOT NULL DEFAULT 0,
        updated_at   TEXT NOT NULL
    );",
    "CREATE INDEX IF NOT EXISTS idx_read_progress_read ON read_progress(read);",
    // Local reader resume position: the *exact* page the user stopped at, not a
    // furthest-reached high-water mark. Distinct from `read_progress` (which
    // feeds the online reader + "read" cover badge and only ever moves
    // forward) so that scrolling backwards in the local reader is correctly
    // remembered on reopen.
    "CREATE TABLE IF NOT EXISTS local_reader_progress (
        gallery_id INTEGER PRIMARY KEY,
        page       INTEGER NOT NULL DEFAULT 1,
        total_pages INTEGER NOT NULL DEFAULT 0,
        updated_at TEXT NOT NULL
    );",
    // AI translation cache: one row per gallery, keyed by the gallery ID.
    // Holds the translated title plus per-tag / per-comment translations so a
    // repeat visit never spends another AI request on the same gallery. The
    // row count is trimmed to the user-configured limit (`tl_cache_limit`)
    // after every write.
    "CREATE TABLE IF NOT EXISTS translation_cache (
        gallery_id       INTEGER PRIMARY KEY,
        config_key       TEXT NOT NULL DEFAULT '',
        title_source     TEXT NOT NULL DEFAULT '',
        title_translated TEXT NOT NULL DEFAULT '',
        tags             TEXT NOT NULL DEFAULT '{}',
        comments         TEXT NOT NULL DEFAULT '{}',
        updated_at       TEXT NOT NULL
    );",
];

// ---------------------------------------------------------------------------
// Public helpers (used by commands) implemented on the richer Database type
// ---------------------------------------------------------------------------

impl Database {
    pub fn history_add(
        &self,
        id: i64,
        title: &str,
        media_id: i64,
        thumbnail: &str,
    ) -> AppResult<()> {
        self.with_conn(|c| {
            let now = Utc::now().to_rfc3339();
            c.execute(
                "INSERT OR REPLACE INTO history (gallery_id, title, media_id, thumbnail, visited_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![id, title, media_id, thumbnail, now],
            )?;
            // Trim history to max size.
            c.execute(
                "DELETE FROM history WHERE gallery_id NOT IN (
                    SELECT gallery_id FROM history ORDER BY visited_at DESC LIMIT 500
                 );",
                [],
            )?;
            Ok(())
        })
    }

    pub fn history_list(&self, limit: u32) -> AppResult<Vec<HistoryEntry>> {
        self.with_conn(|c| {
            let mut stmt = c.prepare(
                "SELECT gallery_id, title, media_id, thumbnail, visited_at
                 FROM history ORDER BY visited_at DESC LIMIT ?1",
            )?;
            let rows = stmt
                .query_map(params![limit], |r| {
                    let ts: String = r.get(4)?;
                    Ok(HistoryEntry {
                        gallery_id: r.get(0)?,
                        title: r.get(1)?,
                        media_id: r.get(2)?,
                        thumbnail: r.get(3)?,
                        visited_at: chrono::DateTime::parse_from_rfc3339(&ts)
                            .map(|d| d.with_timezone(&Utc))
                            .unwrap_or_else(|_| Utc::now()),
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
    }

    pub fn history_clear(&self) -> AppResult<()> {
        self.with_conn(|c| {
            c.execute("DELETE FROM history", [])?;
            Ok(())
        })
    }

    // Tags ---------------------------------------------------------------

    pub fn tag_insert_or_update(&self, t: &Tag) -> AppResult<()> {
        self.with_conn(|c| {
            c.execute(
                "INSERT INTO tags (id, name, type, count, status, online_blacklist)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(id) DO UPDATE SET
                    name = excluded.name,
                    type = excluded.type,
                    count = excluded.count",
                params![
                    t.id,
                    t.name,
                    t.tag_type.single(),
                    t.count,
                    status_str(t.status),
                    if t.blacklisted { 1 } else { 0 }
                ],
            )?;
            Ok(())
        })
    }

    pub fn tag_set_status(&self, id: i64, status: TagStatus) -> AppResult<()> {
        self.with_conn(|c| {
            c.execute(
                "UPDATE tags SET status = ?1 WHERE id = ?2",
                params![status_str(status), id],
            )?;
            Ok(())
        })
    }

    pub fn tag_set_blacklist(&self, id: i64, blacklisted: bool) -> AppResult<()> {
        self.with_conn(|c| {
            c.execute(
                "UPDATE tags SET online_blacklist = ?1 WHERE id = ?2",
                params![if blacklisted { 1 } else { 0 }, id],
            )?;
            Ok(())
        })
    }

    pub fn replace_blacklist(&self, tags: &[Tag]) -> AppResult<()> {
        self.with_conn(|c| {
            let tx = c.unchecked_transaction()?;
            tx.execute("UPDATE tags SET online_blacklist = 0", [])?;
            for tag in tags {
                tx.execute(
                    "INSERT INTO tags (id, name, type, count, status, online_blacklist)
                     VALUES (?1, ?2, ?3, ?4, 'default', 1)
                     ON CONFLICT(id) DO UPDATE SET
                        name = excluded.name,
                        type = excluded.type,
                        count = excluded.count,
                        online_blacklist = 1",
                    params![tag.id, tag.name, tag.tag_type.single(), tag.count,],
                )?;
            }
            tx.commit()?;
            Ok(())
        })
    }

    pub fn tags_by_type(&self, type_filter: Option<TagType>) -> AppResult<Vec<Tag>> {
        self.with_conn(|c| {
            let mut sql =
                String::from("SELECT id, name, type, count, status, online_blacklist FROM tags");
            let mut args: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
            if let Some(t) = type_filter {
                sql.push_str(" WHERE type = ?1");
                args.push(Box::new(t.single().to_string()));
            }
            sql.push_str(" ORDER BY count DESC LIMIT 2000");
            let mut stmt = c.prepare(&sql)?;
            let arg_refs: Vec<&dyn rusqlite::ToSql> = args.iter().map(|b| b.as_ref()).collect();
            let rows = stmt
                .query_map(arg_refs.as_slice(), row_to_tag)?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
    }

    pub fn tags_all(&self) -> AppResult<Vec<Tag>> {
        self.tags_by_type(None)
    }

    pub fn tags_status(&self, status: TagStatus) -> AppResult<Vec<Tag>> {
        self.with_conn(|c| {
            let mut stmt = c.prepare(
                "SELECT id, name, type, count, status, online_blacklist
                     FROM tags WHERE status = ?1",
            )?;
            let rows = stmt
                .query_map(params![status_str(status)], row_to_tag)?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
    }

    pub fn tags_blacklisted(&self) -> AppResult<Vec<Tag>> {
        self.with_conn(|c| {
            let mut stmt = c.prepare(
                "SELECT id, name, type, count, status, online_blacklist
                 FROM tags WHERE online_blacklist = 1",
            )?;
            let rows = stmt
                .query_map([], row_to_tag)?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
    }

    pub fn tags_search(&self, query: &str, limit: usize) -> AppResult<Vec<Tag>> {
        self.with_conn(|c| {
            let pattern = format!("%{}%", query.to_ascii_lowercase());
            let mut stmt = c.prepare(
                "SELECT id, name, type, count, status, online_blacklist FROM tags
                 WHERE LOWER(name) LIKE ?1
                 ORDER BY count DESC LIMIT ?2",
            )?;
            let rows = stmt
                .query_map(params![pattern, limit as i64], row_to_tag)?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
    }

    // Downloads ----------------------------------------------------------

    pub fn download_upsert(
        &self,
        id: i64,
        title: &str,
        media_id: i64,
        thumbnail: &str,
        folder: &str,
        total_pages: usize,
        done_pages: usize,
        status: &str,
    ) -> AppResult<()> {
        self.with_conn(|c| {
            let now = Utc::now().to_rfc3339();
            c.execute(
                "INSERT INTO downloads (id, title, media_id, thumbnail, folder,
                                        total_pages, done_pages, status,
                                        created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9)
                 ON CONFLICT(id) DO UPDATE SET
                    title = excluded.title,
                    folder = excluded.folder,
                    total_pages = excluded.total_pages,
                    done_pages = excluded.done_pages,
                    status = excluded.status,
                    updated_at = excluded.updated_at",
                params![
                    id,
                    title,
                    media_id,
                    thumbnail,
                    folder,
                    total_pages as i64,
                    done_pages as i64,
                    status,
                    now
                ],
            )?;
            Ok(())
        })
    }

    pub fn download_remove(&self, id: i64) -> AppResult<()> {
        self.with_conn(|c| {
            c.execute("DELETE FROM downloads WHERE id = ?1", params![id])?;
            Ok(())
        })
    }

    pub fn download_set_status(&self, id: i64, status: &str) -> AppResult<()> {
        self.with_conn(|c| {
            c.execute(
                "UPDATE downloads SET status = ?1, updated_at = ?2 WHERE id = ?3",
                params![status, Utc::now().to_rfc3339(), id],
            )?;
            Ok(())
        })
    }

    pub fn downloads_all(&self) -> AppResult<Vec<DownloadRow>> {
        self.with_conn(|c| {
            let mut stmt = c.prepare(
                "SELECT id, title, media_id, thumbnail, folder,
                        total_pages, done_pages, status, updated_at
                 FROM downloads ORDER BY updated_at DESC",
            )?;
            let rows = stmt
                .query_map([], |r| {
                    Ok(DownloadRow {
                        id: r.get(0)?,
                        title: r.get(1)?,
                        media_id: r.get(2)?,
                        thumbnail: r.get(3)?,
                        folder: r.get(4)?,
                        total_pages: r.get(5)?,
                        done_pages: r.get(6)?,
                        status: r.get(7)?,
                        updated_at: r.get(8)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
    }

    // Local library ------------------------------------------------------

    pub fn local_upsert(&self, g: &LocalGallery) -> AppResult<()> {
        self.with_conn(|c| {
            let page_files = serde_json::to_string(&g.page_files).unwrap_or_else(|_| "[]".into());
            c.execute(
                "INSERT INTO local_meta (folder, gallery_id, title, media_id,
                                          thumbnail, num_pages, page_files, scanned_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                 ON CONFLICT(folder) DO UPDATE SET
                    gallery_id = excluded.gallery_id,
                    title = excluded.title,
                    media_id = excluded.media_id,
                    thumbnail = excluded.thumbnail,
                    num_pages = excluded.num_pages,
                    page_files = excluded.page_files,
                    scanned_at = excluded.scanned_at",
                params![
                    g.folder,
                    g.id,
                    g.title,
                    g.media_id,
                    g.thumbnail_path,
                    g.num_pages as i64,
                    page_files,
                    g.scanned_at
                ],
            )?;
            Ok(())
        })
    }

    pub fn local_remove(&self, folder: &str) -> AppResult<()> {
        self.with_conn(|c| {
            c.execute("DELETE FROM local_meta WHERE folder = ?1", params![folder])?;
            Ok(())
        })
    }

    pub fn local_set_translated_title(
        &self,
        gallery_id: i64,
        translated_title: &str,
    ) -> AppResult<()> {
        self.with_conn(|c| {
            c.execute(
                "UPDATE local_meta SET translated_title = ?1 WHERE gallery_id = ?2",
                params![translated_title, gallery_id],
            )?;
            Ok(())
        })
    }

    /// IDs of galleries that exist on disk in the local library (downloaded).
    /// Only rows with a non-zero `gallery_id` count — folders without an id
    /// marker can't be matched against online galleries anyway.
    pub fn local_ids(&self) -> AppResult<Vec<i64>> {
        self.with_conn(|c| {
            let mut stmt = c.prepare(
                "SELECT gallery_id FROM local_meta
                 WHERE gallery_id != 0
                   AND gallery_id NOT IN (SELECT id FROM downloads)",
            )?;
            let rows = stmt
                .query_map([], |r| r.get::<_, i64>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
    }

    pub fn local_get(&self, gallery_id: i64) -> AppResult<Option<LocalGallery>> {
        self.with_conn(|c| {
            let mut stmt = c.prepare(
                "SELECT folder, gallery_id, title, media_id, thumbnail,
                        num_pages, page_files, scanned_at, translated_title
                 FROM local_meta WHERE gallery_id = ?1",
            )?;
            let mut rows = stmt.query_map(params![gallery_id], |r| {
                let page_files_json: String = r.get(6)?;
                let page_files: Vec<String> =
                    serde_json::from_str(&page_files_json).unwrap_or_default();
                Ok(LocalGallery {
                    folder: r.get(0)?,
                    id: r.get(1)?,
                    title: r.get(2)?,
                    media_id: r.get(3)?,
                    thumbnail_path: r.get(4)?,
                    num_pages: r.get::<_, i64>(5)? as usize,
                    page_files,
                    scanned_at: r.get(7)?,
                    translated_title: r.get(8)?,
                })
            })?;
            Ok(rows.next().transpose()?)
        })
    }

    pub fn local_all(&self) -> AppResult<Vec<LocalGallery>> {
        self.with_conn(|c| {
            let mut stmt = c.prepare(
                "SELECT folder, gallery_id, title, media_id, thumbnail,
                        num_pages, page_files, scanned_at, translated_title
                 FROM local_meta ORDER BY title COLLATE NOCASE ASC",
            )?;
            let rows = stmt
                .query_map([], |r| {
                    let page_files_json: String = r.get(6)?;
                    let page_files: Vec<String> =
                        serde_json::from_str(&page_files_json).unwrap_or_default();
                    Ok(LocalGallery {
                        folder: r.get(0)?,
                        id: r.get(1)?,
                        title: r.get(2)?,
                        media_id: r.get(3)?,
                        thumbnail_path: r.get(4)?,
                        num_pages: r.get::<_, i64>(5)? as usize,
                        page_files,
                        scanned_at: r.get(7)?,
                        translated_title: r.get(8)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
    }

    // AI translation cache -------------------------------------------------

    pub fn translation_cache_get(&self, gallery_id: i64) -> AppResult<Option<TranslationCacheRow>> {
        self.with_conn(|c| {
            let mut stmt = c.prepare(
                "SELECT gallery_id, config_key, title_source, title_translated,
                        tags, comments, updated_at
                 FROM translation_cache WHERE gallery_id = ?1",
            )?;
            let mut rows = stmt.query_map(params![gallery_id], |r| row_to_translation_cache(r))?;
            Ok(rows.next().transpose()?)
        })
    }

    /// Insert or refresh one gallery's cache entry (stamping `updated_at`)
    /// and trim the table to `limit` most-recently-updated galleries.
    pub fn translation_cache_upsert(
        &self,
        entry: &TranslationCacheRow,
        limit: u32,
    ) -> AppResult<()> {
        self.with_conn(|c| {
            let tags = serde_json::to_string(&entry.tags).unwrap_or_else(|_| "{}".into());
            let comments = serde_json::to_string(&entry.comments).unwrap_or_else(|_| "{}".into());
            c.execute(
                "INSERT INTO translation_cache (gallery_id, config_key, title_source,
                                                title_translated, tags, comments, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(gallery_id) DO UPDATE SET
                    config_key       = excluded.config_key,
                    title_source     = excluded.title_source,
                    title_translated = excluded.title_translated,
                    tags             = excluded.tags,
                    comments         = excluded.comments,
                    updated_at       = excluded.updated_at",
                params![
                    entry.gallery_id,
                    entry.config_key,
                    entry.title_source,
                    entry.title_translated,
                    tags,
                    comments,
                    Utc::now().to_rfc3339()
                ],
            )?;
            // Enforce the per-gallery cap: keep the newest `limit` rows.
            // A limit of 0 disables caching entirely (the row just written
            // is removed again by the trim).
            if limit == 0 {
                c.execute("DELETE FROM translation_cache", [])?;
            } else {
                c.execute(
                    "DELETE FROM translation_cache WHERE gallery_id NOT IN (
                        SELECT gallery_id FROM translation_cache
                        ORDER BY updated_at DESC, gallery_id DESC LIMIT ?1
                     );",
                    params![limit as i64],
                )?;
            }
            Ok(())
        })
    }

    /// Drop every cached translation. Returns the number of removed rows.
    pub fn translation_cache_clear(&self) -> AppResult<usize> {
        self.with_conn(|c| {
            let removed = c.execute("DELETE FROM translation_cache", [])?;
            Ok(removed)
        })
    }

    /// Number of galleries currently held in the translation cache.
    pub fn translation_cache_count(&self) -> AppResult<i64> {
        self.with_conn(|c| {
            let count: i64 =
                c.query_row("SELECT COUNT(*) FROM translation_cache", [], |r| r.get(0))?;
            Ok(count)
        })
    }
}

fn row_to_translation_cache(r: &rusqlite::Row<'_>) -> rusqlite::Result<TranslationCacheRow> {
    let tags_json: String = r.get(4)?;
    let comments_json: String = r.get(5)?;
    Ok(TranslationCacheRow {
        gallery_id: r.get(0)?,
        config_key: r.get(1)?,
        title_source: r.get(2)?,
        title_translated: r.get(3)?,
        tags: serde_json::from_str(&tags_json).unwrap_or_else(|_| serde_json::json!({})),
        comments: serde_json::from_str(&comments_json).unwrap_or_else(|_| serde_json::json!({})),
        updated_at: r.get(6)?,
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadRow {
    pub id: i64,
    pub title: String,
    pub media_id: i64,
    pub thumbnail: String,
    pub folder: String,
    pub total_pages: usize,
    pub done_pages: usize,
    pub status: String,
    pub updated_at: String,
}

// ---------------------------------------------------------------------------
// Row helpers
// ---------------------------------------------------------------------------

fn row_to_tag(row: &rusqlite::Row<'_>) -> rusqlite::Result<Tag> {
    let type_str: String = row.get(2)?;
    let status_str: String = row.get(4)?;
    Ok(Tag {
        id: row.get(0)?,
        name: row.get(1)?,
        tag_type: TagType::from_name(&type_str),
        count: row.get(3)?,
        status: parse_status(&status_str),
        blacklisted: row.get::<_, i64>(5)? != 0,
    })
}

fn status_str(s: TagStatus) -> &'static str {
    match s {
        TagStatus::Default => "default",
        TagStatus::Accepted => "accepted",
        TagStatus::Avoided => "avoided",
    }
}

fn parse_status(s: &str) -> TagStatus {
    match s {
        "accepted" => TagStatus::Accepted,
        "avoided" => TagStatus::Avoided,
        _ => TagStatus::Default,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn test_db(name: &str) -> Database {
        let dir = std::env::temp_dir().join(format!(
            "nclientv4-db-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        Database::open(&dir).unwrap()
    }

    fn entry(gallery_id: i64, title: &str) -> TranslationCacheRow {
        TranslationCacheRow {
            gallery_id,
            config_key: "cfg".into(),
            title_source: "source".into(),
            title_translated: title.into(),
            tags: json!({ "1": { "name": "tag", "translated": "标签" } }),
            comments: json!({}),
            updated_at: String::new(),
        }
    }

    #[test]
    fn translation_cache_roundtrip() {
        let db = test_db("roundtrip");
        assert_eq!(db.translation_cache_count().unwrap(), 0);
        assert!(db.translation_cache_get(123).unwrap().is_none());

        db.translation_cache_upsert(&entry(123, "译名"), 100)
            .unwrap();
        let stored = db.translation_cache_get(123).unwrap().unwrap();
        assert_eq!(stored.gallery_id, 123);
        assert_eq!(stored.title_source, "source");
        assert_eq!(stored.title_translated, "译名");
        assert_eq!(stored.config_key, "cfg");
        assert_eq!(
            stored.tags.get("1").unwrap().get("translated").unwrap(),
            "标签"
        );
        assert!(!stored.updated_at.is_empty());
        assert_eq!(db.translation_cache_count().unwrap(), 1);
    }

    #[test]
    fn translation_cache_upsert_overwrites_and_trims_to_limit() {
        let db = test_db("trim");
        // Three galleries, then a re-write of the first (refreshes its place).
        db.translation_cache_upsert(&entry(1, "a"), 100).unwrap();
        db.translation_cache_upsert(&entry(2, "b"), 100).unwrap();
        db.translation_cache_upsert(&entry(3, "c"), 100).unwrap();
        db.translation_cache_upsert(&entry(1, "a2"), 100).unwrap();

        // Limit 2 keeps the two most recently updated galleries (1 and 3);
        // gallery 2 is the oldest.
        db.translation_cache_upsert(&entry(4, "d"), 2).unwrap();
        let ids = |db: &Database| -> Vec<i64> {
            (1..=4)
                .filter(|id| db.translation_cache_get(*id).unwrap().is_some())
                .collect()
        };
        assert_eq!(ids(&db), vec![1, 4]);
        assert_eq!(
            db.translation_cache_get(1)
                .unwrap()
                .unwrap()
                .title_translated,
            "a2"
        );

        assert_eq!(db.translation_cache_clear().unwrap(), 2);
        assert_eq!(db.translation_cache_count().unwrap(), 0);
    }
}
