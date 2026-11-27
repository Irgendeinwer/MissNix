mod artists;
mod folders;
mod releases;
mod salvage;
mod schema;
mod settings;
#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::Connection;

use crate::error::{CoreError, CoreResult};
use crate::models::{LibraryStats, ReleaseType};

use self::salvage::{restore_salvaged_snapshot, salvage_legacy_snapshot};
use self::schema::{
    CANONICAL_DDL, SCHEMA_VERSION, is_canonical_schema_valid, is_partial_canonical_schema_valid,
};

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
        self.conn.busy_timeout(Duration::from_millis(5000))?;
        self.conn.execute_batch(
            "
            PRAGMA journal_mode = WAL;
            PRAGMA synchronous = NORMAL;
            ",
        )?;

        let existing_tables: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
            [],
            |r| r.get(0),
        )?;

        if existing_tables == 0 {
            self.conn.execute_batch(CANONICAL_DDL)?;
            self.conn.execute_batch(&format!(
                "PRAGMA user_version = {SCHEMA_VERSION}; PRAGMA foreign_keys = ON;"
            ))?;
            return Ok(());
        }

        if is_canonical_schema_valid(&self.conn)? || is_partial_canonical_schema_valid(&self.conn)?
        {
            self.conn.execute_batch(CANONICAL_DDL)?;
            self.conn.execute_batch(&format!(
                "PRAGMA user_version = {SCHEMA_VERSION}; PRAGMA foreign_keys = ON;"
            ))?;
            return Ok(());
        }

        self.salvage_and_rebuild_schema(db_path)?;
        Ok(())
    }

    fn salvage_and_rebuild_schema(&mut self, db_path: Option<&Path>) -> CoreResult<()> {
        let _ = self.conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);");
        if let Some(path) = db_path.filter(|p| p.is_file()) {
            let mut bak_os = path.as_os_str().to_os_string();
            bak_os.push(".bak");
            let bak_path = PathBuf::from(bak_os);
            if !bak_path.exists() {
                let _ = std::fs::copy(path, bak_path);
            }
        }

        let snapshot = salvage_legacy_snapshot(&self.conn);

        self.conn.execute_batch("PRAGMA foreign_keys = OFF;")?;

        let objects: Vec<(String, String)> = {
            let mut stmt = self.conn.prepare(
                "SELECT type, name FROM sqlite_master
                 WHERE type IN ('table', 'view') AND name NOT LIKE 'sqlite_%'",
            )?;
            let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.collect::<Result<Vec<_>, _>>()?
        };

        let tx = self.conn.transaction()?;
        for (kind, name) in objects {
            let kw = if kind.eq_ignore_ascii_case("view") {
                "VIEW"
            } else {
                "TABLE"
            };
            let escaped = name.replace('"', "\"\"");
            tx.execute_batch(&format!("DROP {kw} IF EXISTS \"{escaped}\";"))?;
        }

        tx.execute_batch(CANONICAL_DDL)?;
        restore_salvaged_snapshot(&tx, &snapshot)?;
        tx.commit()?;

        self.conn.execute_batch(&format!(
            "PRAGMA user_version = {SCHEMA_VERSION}; PRAGMA foreign_keys = ON;"
        ))?;
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
