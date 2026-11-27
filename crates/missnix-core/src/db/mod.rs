mod artists;
mod folders;
mod release_match;
mod releases;
mod schema;
mod settings;
#[cfg(test)]
mod tests;

use std::path::Path;
use std::time::{Duration, Instant};

use rusqlite::{Connection, ErrorCode, TransactionBehavior};

use crate::error::{CoreError, CoreResult};
use crate::models::{LibraryStats, ReleaseType};

pub use self::folders::LibraryScanSnapshot;
use self::schema::{
    CANONICAL_DDL, SCHEMA_VERSION, is_canonical_schema_valid, is_partial_canonical_schema_valid,
};

fn validate_compatible_schema(conn: &Connection, db_path: Option<&Path>) -> CoreResult<()> {
    let version: i32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    let application_id: i64 = conn.query_row("PRAGMA application_id", [], |r| r.get(0))?;
    let existing_objects: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE name NOT LIKE 'sqlite_%'",
        [],
        |r| r.get(0),
    )?;
    let empty = existing_objects == 0 && version == 0 && application_id == 0;
    // A deliberately pre-seeded partial schema has no application ID. Do not
    // complete a lookalike settings table belonging to a different application.
    let compatible = empty
        || is_canonical_schema_valid(conn)?
        || (application_id == 0 && is_partial_canonical_schema_valid(conn)?);
    if !(0..=SCHEMA_VERSION).contains(&version) || !compatible {
        let database = db_path
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "in-memory database".to_string());
        return Err(CoreError::Validation(format!(
            "Unrecognized or newer database schema in {database}. No migration was performed; \
             use a compatible database or restore a backup."
        )));
    }
    Ok(())
}

fn enable_wal_with_retry(conn: &Connection, timeout: Duration) -> CoreResult<()> {
    let started = Instant::now();
    loop {
        match conn.execute_batch("PRAGMA journal_mode = WAL;") {
            Ok(()) => return Ok(()),
            // SQLite can bypass its busy handler when two connections change journal
            // mode at once. Retrying outside a transaction lets the other opener finish.
            Err(err)
                if err.sqlite_error_code() == Some(ErrorCode::DatabaseBusy)
                    && started.elapsed() < timeout =>
            {
                let remaining = timeout.saturating_sub(started.elapsed());
                std::thread::sleep(Duration::from_millis(10).min(remaining));
            }
            Err(err) => return Err(err.into()),
        }
    }
}

#[derive(Debug, Clone)]
pub struct DiscoveredReleaseInput {
    pub artist_id: i64,
    pub title: String,
    pub release_type: ReleaseType,
    pub release_date: String,
    pub deezer_album_id: Option<i64>,
    pub mb_release_group_id: Option<String>,
    pub cover_url: Option<String>,
    pub external_url: Option<String>,
    pub musicbrainz_url: Option<String>,
    pub track_count: i32,
    pub explicit: bool,
    pub provider_source: String,
}

#[derive(Debug, Clone)]
pub struct DiscoveredTrackInput {
    pub disc_number: i32,
    pub track_number: i32,
    pub title: String,
    pub duration_secs: i32,
    pub preview_url: Option<String>,
    pub artist_credit: Option<String>,
}

pub struct Database {
    conn: Connection,
}

impl Database {
    pub fn open(path: &Path) -> CoreResult<Self> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent).map_err(|source| CoreError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        let conn = Connection::open(path)?;
        let mut db = Self { conn };
        db.configure_and_migrate(Some(path))?;
        Ok(db)
    }

    pub fn open_in_memory() -> CoreResult<Self> {
        let conn = Connection::open_in_memory()?;
        let mut db = Self { conn };
        db.configure_and_migrate(None)?;
        Ok(db)
    }

    fn configure_and_migrate(&mut self, db_path: Option<&Path>) -> CoreResult<()> {
        const BUSY_TIMEOUT: Duration = Duration::from_secs(5);
        self.conn.busy_timeout(BUSY_TIMEOUT)?;

        // Validate before changing journal mode or writing any schema. A path supplied via
        // --db may be an unrelated SQLite database, and a newer/legacy schema is not safe
        // to rebuild by guessing which tables or columns contain user data.
        validate_compatible_schema(&self.conn, db_path)?;
        enable_wal_with_retry(&self.conn, BUSY_TIMEOUT)?;
        self.conn.execute_batch("PRAGMA synchronous = NORMAL;")?;

        // Grab the write lock before reading the schema in the transaction. A deferred
        // transaction can acquire an old WAL snapshot, then fail immediately with BUSY
        // when another connection updates user_version before this one upgrades to write.
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        // Another connection may have initialized or changed the database since the
        // first validation, so check again while holding the migration lock.
        validate_compatible_schema(&tx, db_path)?;
        tx.execute_batch(CANONICAL_DDL)?;
        tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        tx.commit()?;
        self.conn.execute_batch("PRAGMA foreign_keys = ON;")?;
        Ok(())
    }

    pub fn library_stats(&self) -> CoreResult<LibraryStats> {
        let total_artists: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM artists", [], |r| r.get(0))?;
        let tracked_artists: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM artists WHERE status = 'tracked'",
            [],
            |r| r.get(0),
        )?;
        let muted_artists: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM artists WHERE status = 'muted'",
            [],
            |r| r.get(0),
        )?;
        let ignored_artists: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM artists WHERE status = 'ignored'",
            [],
            |r| r.get(0),
        )?;
        let missing_artists: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM artists WHERE status = 'missing'",
            [],
            |r| r.get(0),
        )?;
        let local_albums: i64 =
            self.conn
                .query_row("SELECT COUNT(*) FROM local_albums", [], |r| r.get(0))?;
        let total_releases: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM releases r JOIN artists a ON a.id = r.artist_id
             WHERE a.status = 'tracked' AND r.state != 'dismissed'",
            [],
            |r| r.get(0),
        )?;
        let unseen_releases: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM releases r JOIN artists a ON a.id = r.artist_id
             WHERE a.status = 'tracked' AND r.state = 'unseen' AND r.owned_locally = 0",
            [],
            |r| r.get(0),
        )?;

        Ok(LibraryStats {
            total_artists: total_artists as usize,
            tracked_artists: tracked_artists as usize,
            muted_artists: muted_artists as usize,
            ignored_artists: ignored_artists as usize,
            missing_artists: missing_artists as usize,
            local_albums: local_albums as usize,
            total_releases: total_releases as usize,
            unseen_releases: unseen_releases as usize,
        })
    }
}

fn escape_sql_like(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        if matches!(ch, '\\' | '%' | '_') {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

fn merge_provider_labels(existing: &str, incoming: &str) -> String {
    if existing == incoming || existing == "deezer+musicbrainz" {
        return existing.to_string();
    }
    if (existing.contains("deezer") && incoming.contains("musicbrainz"))
        || (existing.contains("musicbrainz") && incoming.contains("deezer"))
    {
        return "deezer+musicbrainz".to_string();
    }
    incoming.to_string()
}

pub fn expand_tilde(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest).to_string_lossy().to_string();
        }
    } else if path == "~"
        && let Some(home) = dirs::home_dir()
    {
        return home.to_string_lossy().to_string();
    }
    path.to_string()
}
