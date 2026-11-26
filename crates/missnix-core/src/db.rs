use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::types::ValueRef;
use rusqlite::{params, params_from_iter, Connection, OptionalExtension, ToSql};

const SCHEMA_VERSION: i32 = 2;

const CANONICAL_DDL: &str = "
    CREATE TABLE IF NOT EXISTS library_folders (
        id              INTEGER PRIMARY KEY AUTOINCREMENT,
        path            TEXT NOT NULL UNIQUE,
        enabled         INTEGER NOT NULL DEFAULT 1,
        track_count     INTEGER NOT NULL DEFAULT 0,
        last_scanned_at TEXT,
        added_at        TEXT NOT NULL
    );

    CREATE TABLE IF NOT EXISTS artists (
        id                INTEGER PRIMARY KEY AUTOINCREMENT,
        name              TEXT NOT NULL,
        normalized_name   TEXT NOT NULL UNIQUE,
        mbid              TEXT,
        deezer_id         INTEGER,
        disambiguation    TEXT,
        avatar_url        TEXT,
        external_url      TEXT,
        status            TEXT NOT NULL DEFAULT 'tracked',
        source            TEXT NOT NULL DEFAULT 'local',
        local_album_count INTEGER NOT NULL DEFAULT 0,
        local_track_count INTEGER NOT NULL DEFAULT 0,
        last_checked_at   TEXT,
        created_at        TEXT NOT NULL,
        updated_at        TEXT NOT NULL
    );

    CREATE TABLE IF NOT EXISTS local_albums (
        id                  INTEGER PRIMARY KEY AUTOINCREMENT,
        artist_id           INTEGER NOT NULL REFERENCES artists(id) ON DELETE CASCADE,
        title               TEXT NOT NULL,
        normalized_title    TEXT NOT NULL,
        release_year        INTEGER,
        mb_release_group_id TEXT,
        mb_album_id         TEXT,
        track_count         INTEGER NOT NULL DEFAULT 0,
        sample_path         TEXT,
        UNIQUE(artist_id, normalized_title)
    );

    CREATE TABLE IF NOT EXISTS releases (
        id                  INTEGER PRIMARY KEY AUTOINCREMENT,
        artist_id           INTEGER NOT NULL REFERENCES artists(id) ON DELETE CASCADE,
        title               TEXT NOT NULL,
        normalized_title    TEXT NOT NULL,
        release_type        TEXT NOT NULL,
        release_date        TEXT NOT NULL DEFAULT '',
        deezer_album_id     INTEGER,
        mb_release_group_id TEXT,
        cover_url           TEXT,
        cover_local_path    TEXT,
        external_url        TEXT,
        musicbrainz_url     TEXT,
        track_count         INTEGER NOT NULL DEFAULT 0,
        explicit            INTEGER NOT NULL DEFAULT 0,
        owned_locally       INTEGER NOT NULL DEFAULT 0,
        state               TEXT NOT NULL DEFAULT 'unseen',
        provider_source     TEXT NOT NULL DEFAULT 'deezer',
        discovered_at       TEXT NOT NULL,
        updated_at          TEXT NOT NULL
    );

    CREATE TABLE IF NOT EXISTS release_tracks (
        id            INTEGER PRIMARY KEY AUTOINCREMENT,
        release_id    INTEGER NOT NULL REFERENCES releases(id) ON DELETE CASCADE,
        disc_number   INTEGER NOT NULL DEFAULT 1,
        track_number  INTEGER NOT NULL,
        title         TEXT NOT NULL,
        duration_secs INTEGER NOT NULL DEFAULT 0,
        preview_url   TEXT,
        artist_credit TEXT,
        UNIQUE(release_id, disc_number, track_number)
    );

    CREATE TABLE IF NOT EXISTS settings (
        key   TEXT PRIMARY KEY,
        value TEXT NOT NULL
    );

    CREATE INDEX IF NOT EXISTS idx_artists_status ON artists(status);
    CREATE INDEX IF NOT EXISTS idx_local_albums_artist ON local_albums(artist_id, normalized_title);
    CREATE INDEX IF NOT EXISTS idx_releases_artist ON releases(artist_id, normalized_title, release_type);
    CREATE INDEX IF NOT EXISTS idx_releases_feed ON releases(state, release_date DESC);
";

use crate::error::{CoreError, CoreResult};
use crate::models::{
    AppSettings, Artist, ArtistDiscoveryMode, ArtistSource, ArtistStatus, FeedQuery, FeedSort,
    LibraryFolder, LibraryStats, LocalAlbum, ProviderMode, Release, ReleaseState, ReleaseTrack,
    ReleaseType,
};
use crate::normalize::{
    normalize_artist_name, normalize_release_title, now_rfc3339_utc, today_iso_utc,
};
use crate::scanner::ScanOutcome;

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

        if is_canonical_schema_valid(&self.conn)? {
            self.conn.execute_batch(CANONICAL_DDL)?;
            self.conn.execute_batch("PRAGMA foreign_keys = ON;")?;
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

    // -------------------------------------------------------------------------
    // Library Folders
    // -------------------------------------------------------------------------

    pub fn add_library_folder(&self, raw_path: &str) -> CoreResult<LibraryFolder> {
        let trimmed = raw_path.trim();
        if trimmed.is_empty() {
            return Err(CoreError::Validation(
                "Folder path cannot be empty".to_string(),
            ));
        }

        let expanded = expand_tilde(trimmed);
        let canonical = std::fs::canonicalize(&expanded)
            .unwrap_or_else(|_| std::path::PathBuf::from(&expanded));
        let path_str = canonical.to_string_lossy().to_string();
        let now = now_rfc3339_utc();

        self.conn.execute(
            "INSERT INTO library_folders (path, enabled, added_at)
             VALUES (?1, 1, ?2)
             ON CONFLICT(path) DO UPDATE SET enabled = 1",
            params![path_str, now],
        )?;

        let folder = self.conn.query_row(
            "SELECT id, path, enabled, track_count, last_scanned_at, added_at
             FROM library_folders WHERE path = ?1",
            params![path_str],
            map_folder_row,
        )?;
        Ok(folder)
    }

    pub fn remove_library_folder(&self, folder_id: i64) -> CoreResult<()> {
        self.conn.execute(
            "DELETE FROM library_folders WHERE id = ?1",
            params![folder_id],
        )?;
        Ok(())
    }

    pub fn list_library_folders(&self) -> CoreResult<Vec<LibraryFolder>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, path, enabled, track_count, last_scanned_at, added_at
             FROM library_folders ORDER BY added_at ASC, id ASC",
        )?;
        let rows = stmt.query_map([], map_folder_row)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(CoreError::from)
    }

    // -------------------------------------------------------------------------
    // Applying Scan Outcome & Reconciling Local Ownership
    // -------------------------------------------------------------------------

    pub fn apply_scan_outcome(&mut self, outcome: &ScanOutcome) -> CoreResult<()> {
        let now = now_rfc3339_utc();
        let tx = self.conn.transaction()?;

        for (folder_path, count) in &outcome.folder_track_counts {
            let p = folder_path.to_string_lossy().to_string();
            tx.execute(
                "UPDATE library_folders
                 SET track_count = ?1, last_scanned_at = ?2
                 WHERE path = ?3",
                params![*count as i64, now, p],
            )?;
        }

        // Reset counts for local artists before re-applying full scan snapshot
        tx.execute(
            "UPDATE artists SET local_album_count = 0, local_track_count = 0 WHERE source = 'local'",
            [],
        )?;
        tx.execute("DELETE FROM local_albums", [])?;

        for record in outcome.artists.values() {
            let album_count = record.albums.len() as i64;
            let track_count = record.track_count as i64;

            tx.execute(
                "INSERT INTO artists (
                    name, normalized_name, mbid, status, source,
                    local_album_count, local_track_count, created_at, updated_at
                 ) VALUES (?1, ?2, ?3, 'tracked', 'local', ?4, ?5, ?6, ?6)
                 ON CONFLICT(normalized_name) DO UPDATE SET
                    mbid = COALESCE(artists.mbid, excluded.mbid),
                    local_album_count = excluded.local_album_count,
                    local_track_count = excluded.local_track_count,
                    updated_at = excluded.updated_at",
                params![
                    record.display_name,
                    record.normalized_name,
                    record.mbid,
                    album_count,
                    track_count,
                    now
                ],
            )?;

            let artist_id: i64 = tx.query_row(
                "SELECT id FROM artists WHERE normalized_name = ?1",
                params![record.normalized_name],
                |r| r.get(0),
            )?;

            for album in record.albums.values() {
                tx.execute(
                    "INSERT INTO local_albums (
                        artist_id, title, normalized_title, release_year,
                        mb_release_group_id, mb_album_id, track_count, sample_path
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                     ON CONFLICT(artist_id, normalized_title) DO UPDATE SET
                        release_year = COALESCE(local_albums.release_year, excluded.release_year),
                        mb_release_group_id = COALESCE(local_albums.mb_release_group_id, excluded.mb_release_group_id),
                        mb_album_id = COALESCE(local_albums.mb_album_id, excluded.mb_album_id),
                        track_count = excluded.track_count,
                        sample_path = excluded.sample_path",
                    params![
                        artist_id,
                        album.title,
                        album.normalized_title,
                        album.release_year,
                        album.mb_release_group_id,
                        album.mb_album_id,
                        album.track_count as i64,
                        album.sample_path
                    ],
                )?;
            }
        }

        // Prune stale `local` + `tracked` artists that no longer match any audio file
        // (for instance, old fake combined collaboration rows like "Alligatoah, Jamal" or
        // removed folders), while preserving manual artists and muted/ignored preferences.
        // Guarded on `folders_scanned > 0 && audio_files_seen > 0` so an unmounted drive
        // never wipes out a user's tracked catalog.
        if outcome.summary.folders_scanned > 0 && outcome.summary.audio_files_seen > 0 {
            tx.execute(
                "DELETE FROM artists
                 WHERE source = 'local'
                   AND status = 'tracked'
                   AND local_track_count = 0",
                [],
            )?;
        }

        // Recompute `owned_locally` across all discovered releases immediately!
        tx.execute(
            "UPDATE releases SET owned_locally = CASE
                WHEN EXISTS (
                    SELECT 1 FROM local_albums la
                    WHERE la.artist_id = releases.artist_id
                      AND (
                          la.normalized_title = releases.normalized_title
                          OR (
                              releases.mb_release_group_id IS NOT NULL
                              AND la.mb_release_group_id = releases.mb_release_group_id
                          )
                      )
                ) THEN 1 ELSE 0 END",
            [],
        )?;

        tx.commit()?;
        Ok(())
    }

    // -------------------------------------------------------------------------
    // Artists Management
    // -------------------------------------------------------------------------

    pub fn list_artists(
        &self,
        search: &str,
        status_filter: Option<ArtistStatus>,
        only_manual: bool,
    ) -> CoreResult<Vec<Artist>> {
        let mut sql = String::from(
            "SELECT
                a.id, a.name, a.normalized_name, a.mbid, a.deezer_id,
                a.disambiguation, a.avatar_url, a.external_url,
                a.status, a.source, a.local_album_count, a.local_track_count,
                (SELECT COUNT(*) FROM releases r WHERE r.artist_id = a.id AND r.state != 'dismissed') AS rel_cnt,
                (SELECT COUNT(*) FROM releases r WHERE r.artist_id = a.id AND r.state = 'unseen' AND r.owned_locally = 0) AS unseen_cnt,
                a.last_checked_at
             FROM artists a
             WHERE 1 = 1",
        );

        let mut bind_vals: Vec<Box<dyn ToSql>> = Vec::new();
        let trimmed = search.trim();
        if !trimmed.is_empty() {
            let norm = normalize_artist_name(trimmed);
            let like_raw = format!("%{}%", escape_sql_like(&trimmed.to_lowercase()));
            let like_norm = format!("%{}%", escape_sql_like(&norm));
            sql.push_str(
                " AND (LOWER(a.name) LIKE ? ESCAPE '\\' OR a.normalized_name LIKE ? ESCAPE '\\')",
            );
            bind_vals.push(Box::new(like_raw));
            bind_vals.push(Box::new(like_norm));
        }

        if let Some(st) = status_filter {
            sql.push_str(" AND a.status = ?");
            bind_vals.push(Box::new(st.as_str().to_string()));
        }

        if only_manual {
            sql.push_str(" AND a.source = 'manual'");
        }

        sql.push_str(" ORDER BY CASE a.status WHEN 'tracked' THEN 0 WHEN 'muted' THEN 1 ELSE 2 END, LOWER(a.name) ASC");

        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(
            params_from_iter(bind_vals.iter().map(|b| b.as_ref())),
            map_artist_row,
        )?;
        rows.collect::<Result<Vec<_>, _>>().map_err(CoreError::from)
    }

    pub fn get_artist(&self, artist_id: i64) -> CoreResult<Option<Artist>> {
        self.conn
            .query_row(
                "SELECT
                    a.id, a.name, a.normalized_name, a.mbid, a.deezer_id,
                    a.disambiguation, a.avatar_url, a.external_url,
                    a.status, a.source, a.local_album_count, a.local_track_count,
                    (SELECT COUNT(*) FROM releases r WHERE r.artist_id = a.id AND r.state != 'dismissed'),
                    (SELECT COUNT(*) FROM releases r WHERE r.artist_id = a.id AND r.state = 'unseen' AND r.owned_locally = 0),
                    a.last_checked_at
                 FROM artists a WHERE a.id = ?1",
                params![artist_id],
                map_artist_row,
            )
            .optional()
            .map_err(CoreError::from)
    }

    pub fn list_tracked_artists(&self) -> CoreResult<Vec<Artist>> {
        self.list_artists("", Some(ArtistStatus::Tracked), false)
    }

    pub fn add_manual_artist(
        &self,
        name: &str,
        mbid: Option<&str>,
        deezer_id: Option<i64>,
        disambiguation: Option<&str>,
        avatar_url: Option<&str>,
        external_url: Option<&str>,
    ) -> CoreResult<Artist> {
        let clean_name = name.trim();
        let norm = normalize_artist_name(clean_name);
        if clean_name.is_empty() || norm.is_empty() {
            return Err(CoreError::Validation(
                "Artist name cannot be empty".to_string(),
            ));
        }

        let now = now_rfc3339_utc();
        let clean_mbid = mbid.map(str::trim).filter(|s| !s.is_empty());
        let clean_disamb = disambiguation.map(str::trim).filter(|s| !s.is_empty());
        let clean_avatar = avatar_url.map(str::trim).filter(|s| !s.is_empty());
        let clean_ext = external_url.map(str::trim).filter(|s| !s.is_empty());

        self.conn.execute(
            "INSERT INTO artists (
                name, normalized_name, mbid, deezer_id, disambiguation,
                avatar_url, external_url, status, source, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'tracked', 'manual', ?8, ?8)
             ON CONFLICT(normalized_name) DO UPDATE SET
                status = 'tracked',
                mbid = COALESCE(?3, artists.mbid),
                deezer_id = COALESCE(?4, artists.deezer_id),
                disambiguation = COALESCE(?5, artists.disambiguation),
                avatar_url = COALESCE(?6, artists.avatar_url),
                external_url = COALESCE(?7, artists.external_url),
                updated_at = ?8",
            params![
                clean_name,
                norm,
                clean_mbid,
                deezer_id,
                clean_disamb,
                clean_avatar,
                clean_ext,
                now
            ],
        )?;

        let id: i64 = self.conn.query_row(
            "SELECT id FROM artists WHERE normalized_name = ?1",
            params![norm],
            |r| r.get(0),
        )?;
        self.get_artist(id)?.ok_or_else(|| {
            CoreError::Validation("Failed to load newly inserted artist".to_string())
        })
    }

    pub fn set_artist_status(&self, artist_id: i64, status: ArtistStatus) -> CoreResult<()> {
        let now = now_rfc3339_utc();
        self.conn.execute(
            "UPDATE artists SET status = ?1, updated_at = ?2 WHERE id = ?3",
            params![status.as_str(), now, artist_id],
        )?;
        Ok(())
    }

    pub fn update_artist_provider_links(
        &self,
        artist_id: i64,
        mbid: Option<&str>,
        deezer_id: Option<i64>,
        disambiguation: Option<&str>,
        avatar_url: Option<&str>,
        external_url: Option<&str>,
    ) -> CoreResult<()> {
        let now = now_rfc3339_utc();
        self.conn.execute(
            "UPDATE artists SET
                mbid = COALESCE(?1, mbid),
                deezer_id = COALESCE(?2, deezer_id),
                disambiguation = COALESCE(?3, disambiguation),
                avatar_url = COALESCE(?4, avatar_url),
                external_url = COALESCE(?5, external_url),
                updated_at = ?6
             WHERE id = ?7",
            params![
                mbid.filter(|s| !s.is_empty()),
                deezer_id,
                disambiguation.filter(|s| !s.is_empty()),
                avatar_url.filter(|s| !s.is_empty()),
                external_url.filter(|s| !s.is_empty()),
                now,
                artist_id
            ],
        )?;
        Ok(())
    }

    /// Explicitly overwrite an artist's linked provider IDs (used when disambiguating homonym artists).
    pub fn override_artist_ids(
        &self,
        artist_id: i64,
        mbid: Option<&str>,
        deezer_id: Option<i64>,
    ) -> CoreResult<()> {
        let now = now_rfc3339_utc();
        self.conn.execute(
            "UPDATE artists SET mbid = ?1, deezer_id = ?2, updated_at = ?3 WHERE id = ?4",
            params![
                mbid.map(str::trim).filter(|s| !s.is_empty()),
                deezer_id.filter(|id| *id > 0),
                now,
                artist_id
            ],
        )?;
        Ok(())
    }

    pub fn delete_artist(&self, artist_id: i64) -> CoreResult<()> {
        self.conn
            .execute("DELETE FROM artists WHERE id = ?1", params![artist_id])?;
        Ok(())
    }

    pub fn mark_artist_checked(&self, artist_id: i64) -> CoreResult<()> {
        let now = now_rfc3339_utc();
        self.conn.execute(
            "UPDATE artists SET last_checked_at = ?1, updated_at = ?1 WHERE id = ?2",
            params![now, artist_id],
        )?;
        Ok(())
    }

    pub fn list_local_albums_for_artist(&self, artist_id: i64) -> CoreResult<Vec<LocalAlbum>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, artist_id, title, normalized_title, release_year,
                    mb_release_group_id, mb_album_id, track_count, sample_path
             FROM local_albums
             WHERE artist_id = ?1
             ORDER BY COALESCE(release_year, 0) DESC, LOWER(title) ASC",
        )?;
        let rows = stmt.query_map(params![artist_id], |r| {
            Ok(LocalAlbum {
                id: r.get(0)?,
                artist_id: r.get(1)?,
                title: r.get(2)?,
                normalized_title: r.get(3)?,
                release_year: r.get(4)?,
                mb_release_group_id: r.get(5)?,
                mb_album_id: r.get(6)?,
                track_count: r.get(7)?,
                sample_path: r.get(8)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(CoreError::from)
    }

    // -------------------------------------------------------------------------
    // Releases & Smart Deduplication Persistence
    // -------------------------------------------------------------------------

    /// Insert or merge a discovered release, preserving any existing user state (`seen` / `dismissed`)
    /// and checking local library ownership automatically.
    /// Returns `(release_id, was_newly_inserted, owned_locally)`.
    pub fn upsert_discovered_release(
        &self,
        input: &DiscoveredReleaseInput,
    ) -> CoreResult<(i64, bool, bool)> {
        let norm_title = normalize_release_title(&input.title);
        if norm_title.is_empty() {
            return Err(CoreError::Validation(
                "Release title normalized to empty".to_string(),
            ));
        }

        let owned_locally = self.is_release_owned_locally(
            input.artist_id,
            &norm_title,
            input.mb_release_group_id.as_deref(),
        )?;

        // Match existing release by Deezer ID, MusicBrainz Release-Group ID, or (normalized_title, release_type)
        let existing: Option<(i64, String)> = self
            .conn
            .query_row(
                "SELECT id, provider_source FROM releases
                 WHERE artist_id = ?1
                   AND (
                       (?2 IS NOT NULL AND deezer_album_id = ?2)
                       OR (?3 IS NOT NULL AND mb_release_group_id = ?3)
                       OR (normalized_title = ?4 AND release_type = ?5)
                   )
                 ORDER BY id ASC LIMIT 1",
                params![
                    input.artist_id,
                    input.deezer_album_id,
                    input.mb_release_group_id,
                    norm_title,
                    input.release_type.as_str()
                ],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;

        let now = now_rfc3339_utc();

        if let Some((existing_id, existing_provider)) = existing {
            let merged_provider = merge_provider_labels(&existing_provider, &input.provider_source);
            self.conn.execute(
                "UPDATE releases SET
                    release_date = CASE
                        WHEN (release_date = '' OR LENGTH(?1) > LENGTH(release_date)) AND ?1 != '' THEN ?1
                        ELSE release_date
                    END,
                    deezer_album_id = COALESCE(?2, deezer_album_id),
                    mb_release_group_id = COALESCE(?3, mb_release_group_id),
                    cover_url = COALESCE(?4, cover_url),
                    external_url = COALESCE(?5, external_url),
                    musicbrainz_url = COALESCE(?6, musicbrainz_url),
                    track_count = MAX(track_count, ?7),
                    explicit = MAX(explicit, ?8),
                    owned_locally = ?9,
                    provider_source = ?10,
                    updated_at = ?11
                 WHERE id = ?12",
                params![
                    input.release_date,
                    input.deezer_album_id,
                    input.mb_release_group_id,
                    input.cover_url,
                    input.external_url,
                    input.musicbrainz_url,
                    input.track_count,
                    i32::from(input.explicit),
                    i32::from(owned_locally),
                    merged_provider,
                    now,
                    existing_id
                ],
            )?;
            Ok((existing_id, false, owned_locally))
        } else {
            self.conn.execute(
                "INSERT INTO releases (
                    artist_id, title, normalized_title, release_type, release_date,
                    deezer_album_id, mb_release_group_id, cover_url, external_url,
                    musicbrainz_url, track_count, explicit, owned_locally,
                    state, provider_source, discovered_at, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, 'unseen', ?14, ?15, ?15)",
                params![
                    input.artist_id,
                    input.title,
                    norm_title,
                    input.release_type.as_str(),
                    input.release_date,
                    input.deezer_album_id,
                    input.mb_release_group_id,
                    input.cover_url,
                    input.external_url,
                    input.musicbrainz_url,
                    input.track_count,
                    i32::from(input.explicit),
                    i32::from(owned_locally),
                    input.provider_source,
                    now
                ],
            )?;
            Ok((self.conn.last_insert_rowid(), true, owned_locally))
        }
    }

    pub fn is_release_owned_locally(
        &self,
        artist_id: i64,
        normalized_title: &str,
        mb_release_group_id: Option<&str>,
    ) -> CoreResult<bool> {
        let found: Option<i32> = self
            .conn
            .query_row(
                "SELECT 1 FROM local_albums
                 WHERE artist_id = ?1
                   AND (
                       normalized_title = ?2
                       OR (?3 IS NOT NULL AND mb_release_group_id = ?3)
                   )
                 LIMIT 1",
                params![artist_id, normalized_title, mb_release_group_id],
                |r| r.get(0),
            )
            .optional()?;
        Ok(found.is_some())
    }

    pub fn query_releases(&self, query: &FeedQuery) -> CoreResult<Vec<Release>> {
        let mut sql = String::from(
            "SELECT
                r.id, r.artist_id, a.name, r.title, r.normalized_title,
                r.release_type, r.release_date, r.deezer_album_id, r.mb_release_group_id,
                r.cover_url, r.cover_local_path, r.external_url, r.musicbrainz_url,
                r.track_count, r.explicit, r.owned_locally, r.state,
                r.provider_source, r.discovered_at
             FROM releases r
             JOIN artists a ON a.id = r.artist_id
             WHERE 1 = 1",
        );

        let mut bind_vals: Vec<Box<dyn ToSql>> = Vec::new();

        // When viewing a specific artist's drops explicitly, allow muted/ignored status;
        // otherwise the global Release Radar shows tracked artists only.
        if query.artist_id.is_none() {
            sql.push_str(" AND a.status = 'tracked'");
        }

        if !query.include_dismissed {
            sql.push_str(" AND r.state != 'dismissed'");
        }
        if query.only_unseen {
            sql.push_str(" AND r.state = 'unseen'");
        }
        if query.only_seen {
            sql.push_str(" AND r.state = 'seen'");
        }
        if query.only_upcoming {
            let today = today_iso_utc();
            sql.push_str(" AND LENGTH(r.release_date) >= 10 AND r.release_date > ?");
            bind_vals.push(Box::new(today));
        }
        if query.hide_owned {
            sql.push_str(" AND r.owned_locally = 0");
        }
        if let Some(aid) = query.artist_id {
            sql.push_str(" AND r.artist_id = ?");
            bind_vals.push(Box::new(aid));
        }

        if let Some(rt) = query.release_type {
            sql.push_str(" AND r.release_type = ?");
            bind_vals.push(Box::new(rt.as_str().to_string()));
        } else {
            let mut allowed_types = Vec::new();
            if query.include_albums {
                allowed_types.push("'album'");
            }
            if query.include_eps {
                allowed_types.push("'ep'");
            }
            if query.include_singles {
                allowed_types.push("'single'");
            }
            if allowed_types.is_empty() {
                return Ok(Vec::new());
            }
            sql.push_str(&format!(
                " AND r.release_type IN ({})",
                allowed_types.join(",")
            ));
        }

        if let Some(cutoff) = query.cutoff_date.as_deref().filter(|c| !c.is_empty()) {
            sql.push_str(" AND (r.release_date = '' OR r.release_date >= ?)");
            bind_vals.push(Box::new(cutoff.to_string()));
        }

        let trimmed = query.search.trim();
        if !trimmed.is_empty() {
            let like_raw = format!("%{}%", escape_sql_like(&trimmed.to_lowercase()));
            let norm_t = format!("%{}%", escape_sql_like(&normalize_release_title(trimmed)));
            sql.push_str(
                " AND (LOWER(r.title) LIKE ? ESCAPE '\\' OR LOWER(a.name) LIKE ? ESCAPE '\\' OR r.normalized_title LIKE ? ESCAPE '\\')",
            );
            bind_vals.push(Box::new(like_raw.clone()));
            bind_vals.push(Box::new(like_raw));
            bind_vals.push(Box::new(norm_t));
        }

        match query.sort.unwrap_or(FeedSort::NewestFirst) {
            FeedSort::NewestFirst => {
                sql.push_str(" ORDER BY CASE WHEN r.release_date = '' THEN 1 ELSE 0 END ASC, r.release_date DESC, r.id DESC");
            }
            FeedSort::UpcomingFirst => {
                let today = today_iso_utc();
                sql.push_str(" ORDER BY CASE WHEN r.release_date > ? THEN 0 ELSE 1 END ASC, r.release_date DESC, r.id DESC");
                bind_vals.push(Box::new(today));
            }
            FeedSort::ArtistAsc => {
                sql.push_str(" ORDER BY LOWER(a.name) ASC, r.release_date DESC");
            }
        }

        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(
            params_from_iter(bind_vals.iter().map(|b| b.as_ref())),
            map_release_row,
        )?;
        rows.collect::<Result<Vec<_>, _>>().map_err(CoreError::from)
    }

    pub fn get_release(&self, release_id: i64) -> CoreResult<Option<Release>> {
        self.conn
            .query_row(
                "SELECT
                    r.id, r.artist_id, a.name, r.title, r.normalized_title,
                    r.release_type, r.release_date, r.deezer_album_id, r.mb_release_group_id,
                    r.cover_url, r.cover_local_path, r.external_url, r.musicbrainz_url,
                    r.track_count, r.explicit, r.owned_locally, r.state,
                    r.provider_source, r.discovered_at
                 FROM releases r
                 JOIN artists a ON a.id = r.artist_id
                 WHERE r.id = ?1",
                params![release_id],
                map_release_row,
            )
            .optional()
            .map_err(CoreError::from)
    }

    pub fn set_release_state(&self, release_id: i64, state: ReleaseState) -> CoreResult<()> {
        let now = now_rfc3339_utc();
        self.conn.execute(
            "UPDATE releases SET state = ?1, updated_at = ?2 WHERE id = ?3",
            params![state.as_str(), now, release_id],
        )?;
        Ok(())
    }

    pub fn mark_all_releases_seen(&self, artist_id: Option<i64>) -> CoreResult<usize> {
        let now = now_rfc3339_utc();
        let updated = if let Some(aid) = artist_id {
            self.conn.execute(
                "UPDATE releases SET state = 'seen', updated_at = ?1
                 WHERE state = 'unseen' AND artist_id = ?2",
                params![now, aid],
            )?
        } else {
            self.conn.execute(
                "UPDATE releases SET state = 'seen', updated_at = ?1
                 WHERE state = 'unseen'
                   AND artist_id IN (SELECT id FROM artists WHERE status = 'tracked')",
                params![now],
            )?
        };
        Ok(updated)
    }

    pub fn set_release_cover_local_path(
        &self,
        release_id: i64,
        local_path: &str,
    ) -> CoreResult<()> {
        self.conn.execute(
            "UPDATE releases SET cover_local_path = ?1 WHERE id = ?2",
            params![local_path, release_id],
        )?;
        Ok(())
    }

    pub fn replace_release_tracks(
        &mut self,
        release_id: i64,
        tracks: &[DiscoveredTrackInput],
    ) -> CoreResult<()> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "DELETE FROM release_tracks WHERE release_id = ?1",
            params![release_id],
        )?;
        for t in tracks {
            tx.execute(
                "INSERT OR REPLACE INTO release_tracks (
                    release_id, disc_number, track_number, title,
                    duration_secs, preview_url, artist_credit
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    release_id,
                    t.disc_number.max(1),
                    t.track_number,
                    t.title,
                    t.duration_secs.max(0),
                    t.preview_url,
                    t.artist_credit
                ],
            )?;
        }
        if !tracks.is_empty() {
            tx.execute(
                "UPDATE releases SET track_count = MAX(track_count, ?1) WHERE id = ?2",
                params![tracks.len() as i32, release_id],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn list_release_tracks(&self, release_id: i64) -> CoreResult<Vec<ReleaseTrack>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, release_id, disc_number, track_number, title, duration_secs, preview_url, artist_credit
             FROM release_tracks
             WHERE release_id = ?1
             ORDER BY disc_number ASC, track_number ASC",
        )?;
        let rows = stmt.query_map(params![release_id], |r| {
            Ok(ReleaseTrack {
                id: r.get(0)?,
                release_id: r.get(1)?,
                disc_number: r.get(2)?,
                track_number: r.get(3)?,
                title: r.get(4)?,
                duration_secs: r.get(5)?,
                preview_url: r.get(6)?,
                artist_credit: r.get(7)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(CoreError::from)
    }

    // -------------------------------------------------------------------------
    // Preferences & Stats
    // -------------------------------------------------------------------------

    pub fn load_settings(&self) -> CoreResult<AppSettings> {
        let mut s = AppSettings::default();
        let mut stmt = self.conn.prepare("SELECT key, value FROM settings")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        for row in rows {
            let (k, v) = row?;
            match k.as_str() {
                "dark_mode" => s.dark_mode = v == "true",
                "accent_preset" => {
                    if let Ok(n) = v.parse::<u8>() {
                        s.accent_preset = n.min(5);
                    }
                }
                "grid_density" => {
                    if let Ok(n) = v.parse::<u8>() {
                        s.grid_density = n.min(2);
                    }
                }
                "ui_scale_percent" => {
                    if let Ok(n) = v.parse::<u16>() {
                        s.ui_scale_percent = n.clamp(75, 250);
                    }
                }
                "preview_volume" => {
                    if let Ok(n) = v.parse::<u8>() {
                        s.preview_volume = n.min(100);
                    }
                }
                "lookback_days" => {
                    if let Ok(n) = v.parse::<u32>() {
                        s.lookback_days = n;
                    }
                }
                "include_albums" => s.include_albums = v == "true",
                "include_eps" => s.include_eps = v == "true",
                "include_singles" => s.include_singles = v == "true",
                "hide_owned_locally" => s.hide_owned_locally = v == "true",
                "hide_compilations_and_live" => s.hide_compilations_and_live = v == "true",
                "include_compilation_artists" => s.include_compilation_artists = v == "true",
                "artist_discovery_mode" => {
                    s.artist_discovery_mode = ArtistDiscoveryMode::from_db(&v)
                }
                "split_collaborations" => s.split_collaborations = v == "true",
                "auto_check_interval_hours" => {
                    if let Ok(n) = v.parse::<u32>() {
                        s.auto_check_interval_hours = n;
                    }
                }
                "provider_mode" => s.provider_mode = ProviderMode::from_db(&v),
                _ => {}
            }
        }
        Ok(s)
    }

    pub fn save_settings(&mut self, s: &AppSettings) -> CoreResult<()> {
        let tx = self.conn.transaction()?;
        let pairs: [(&str, String); 16] = [
            ("dark_mode", s.dark_mode.to_string()),
            ("accent_preset", s.accent_preset.to_string()),
            ("grid_density", s.grid_density.to_string()),
            (
                "ui_scale_percent",
                s.ui_scale_percent.clamp(75, 250).to_string(),
            ),
            ("preview_volume", s.preview_volume.to_string()),
            ("lookback_days", s.lookback_days.to_string()),
            ("include_albums", s.include_albums.to_string()),
            ("include_eps", s.include_eps.to_string()),
            ("include_singles", s.include_singles.to_string()),
            ("hide_owned_locally", s.hide_owned_locally.to_string()),
            (
                "hide_compilations_and_live",
                s.hide_compilations_and_live.to_string(),
            ),
            (
                "include_compilation_artists",
                s.include_compilation_artists.to_string(),
            ),
            (
                "artist_discovery_mode",
                s.artist_discovery_mode.as_str().to_string(),
            ),
            ("split_collaborations", s.split_collaborations.to_string()),
            (
                "auto_check_interval_hours",
                s.auto_check_interval_hours.to_string(),
            ),
            ("provider_mode", s.provider_mode.as_str().to_string()),
        ];
        for (k, v) in pairs {
            tx.execute(
                "INSERT INTO settings (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![k, v],
            )?;
        }
        tx.commit()?;
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

fn map_folder_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<LibraryFolder> {
    Ok(LibraryFolder {
        id: r.get(0)?,
        path: r.get(1)?,
        enabled: r.get::<_, i32>(2)? != 0,
        track_count: r.get(3)?,
        last_scanned_at: r.get(4)?,
        added_at: r.get(5)?,
    })
}

fn map_artist_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Artist> {
    let status_s: String = r.get(8)?;
    let source_s: String = r.get(9)?;
    Ok(Artist {
        id: r.get(0)?,
        name: r.get(1)?,
        normalized_name: r.get(2)?,
        mbid: r.get(3)?,
        deezer_id: r.get(4)?,
        disambiguation: r.get(5)?,
        avatar_url: r.get(6)?,
        external_url: r.get(7)?,
        status: ArtistStatus::from_db(&status_s),
        source: ArtistSource::from_db(&source_s),
        local_album_count: r.get(10)?,
        local_track_count: r.get(11)?,
        discovered_release_count: r.get(12)?,
        unseen_release_count: r.get(13)?,
        last_checked_at: r.get(14)?,
    })
}

fn map_release_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Release> {
    let rt_s: String = r.get(5)?;
    let state_s: String = r.get(16)?;
    Ok(Release {
        id: r.get(0)?,
        artist_id: r.get(1)?,
        artist_name: r.get(2)?,
        title: r.get(3)?,
        normalized_title: r.get(4)?,
        release_type: ReleaseType::from_db(&rt_s),
        release_date: r.get(6)?,
        deezer_album_id: r.get(7)?,
        mb_release_group_id: r.get(8)?,
        cover_url: r.get(9)?,
        cover_local_path: r.get(10)?,
        external_url: r.get(11)?,
        musicbrainz_url: r.get(12)?,
        track_count: r.get(13)?,
        explicit: r.get::<_, i32>(14)? != 0,
        owned_locally: r.get::<_, i32>(15)? != 0,
        state: ReleaseState::from_db(&state_s),
        provider_source: r.get(17)?,
        discovered_at: r.get(18)?,
    })
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
    } else if path == "~" {
        if let Some(home) = dirs::home_dir() {
            return home.to_string_lossy().to_string();
        }
    }
    path.to_string()
}

struct TableSpec {
    name: &'static str,
    required_cols: &'static [&'static str],
    unique_cols: &'static [&'static str],
}

const CANONICAL_SPECS: &[TableSpec] = &[
    TableSpec {
        name: "library_folders",
        required_cols: &[
            "id",
            "path",
            "enabled",
            "track_count",
            "last_scanned_at",
            "added_at",
        ],
        unique_cols: &["path"],
    },
    TableSpec {
        name: "artists",
        required_cols: &[
            "id",
            "name",
            "normalized_name",
            "mbid",
            "deezer_id",
            "disambiguation",
            "avatar_url",
            "external_url",
            "status",
            "source",
            "local_album_count",
            "local_track_count",
            "last_checked_at",
            "created_at",
            "updated_at",
        ],
        unique_cols: &["normalized_name"],
    },
    TableSpec {
        name: "local_albums",
        required_cols: &[
            "id",
            "artist_id",
            "title",
            "normalized_title",
            "release_year",
            "mb_release_group_id",
            "mb_album_id",
            "track_count",
            "sample_path",
        ],
        unique_cols: &["artist_id", "normalized_title"],
    },
    TableSpec {
        name: "releases",
        required_cols: &[
            "id",
            "artist_id",
            "title",
            "normalized_title",
            "release_type",
            "release_date",
            "deezer_album_id",
            "mb_release_group_id",
            "cover_url",
            "cover_local_path",
            "external_url",
            "musicbrainz_url",
            "track_count",
            "explicit",
            "owned_locally",
            "state",
            "provider_source",
            "discovered_at",
            "updated_at",
        ],
        unique_cols: &[],
    },
    TableSpec {
        name: "release_tracks",
        required_cols: &[
            "id",
            "release_id",
            "disc_number",
            "track_number",
            "title",
            "duration_secs",
            "preview_url",
            "artist_credit",
        ],
        unique_cols: &["release_id", "disc_number", "track_number"],
    },
    TableSpec {
        name: "settings",
        required_cols: &["key", "value"],
        unique_cols: &["key"],
    },
];

fn is_canonical_schema_valid(conn: &Connection) -> CoreResult<bool> {
    for spec in CANONICAL_SPECS {
        if !validate_table_spec(conn, spec)? {
            return Ok(false);
        }
    }
    Ok(true)
}

struct ColInfo {
    name: String,
    not_null: bool,
    has_default: bool,
    pk: i32,
}

fn validate_table_spec(conn: &Connection, spec: &TableSpec) -> CoreResult<bool> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info(\"{}\")", spec.name))?;
    let rows = stmt.query_map([], |r| {
        Ok(ColInfo {
            name: r.get::<_, String>(1)?,
            not_null: r.get::<_, i32>(3)? != 0,
            has_default: r.get::<_, Option<String>>(4)?.is_some(),
            pk: r.get::<_, i32>(5)?,
        })
    })?;
    let cols: Vec<ColInfo> = rows.collect::<Result<Vec<_>, _>>()?;
    if cols.is_empty() {
        return Ok(false);
    }

    let present: HashSet<String> = cols.iter().map(|c| c.name.to_ascii_lowercase()).collect();
    for req in spec.required_cols {
        if !present.contains(*req) {
            return Ok(false);
        }
    }

    let required_set: HashSet<&str> = spec.required_cols.iter().copied().collect();
    for col in &cols {
        let lower = col.name.to_ascii_lowercase();
        if !required_set.contains(lower.as_str()) && col.not_null && !col.has_default && col.pk == 0
        {
            return Ok(false);
        }
    }

    if spec.unique_cols.is_empty() {
        Ok(true)
    } else {
        table_has_unique_on(conn, spec.name, &cols, spec.unique_cols)
    }
}

fn table_has_unique_on(
    conn: &Connection,
    table: &str,
    cols: &[ColInfo],
    expected: &[&str],
) -> CoreResult<bool> {
    let mut pk_cols: Vec<(i32, String)> = cols
        .iter()
        .filter(|c| c.pk > 0)
        .map(|c| (c.pk, c.name.to_ascii_lowercase()))
        .collect();
    pk_cols.sort_by_key(|c| c.0);
    if pk_cols.len() == expected.len()
        && pk_cols
            .iter()
            .zip(expected.iter())
            .all(|((_, got), want)| got == want)
    {
        return Ok(true);
    }

    let mut idx_stmt = conn.prepare(&format!("PRAGMA index_list(\"{table}\")"))?;
    let idx_rows = idx_stmt.query_map([], |r| {
        Ok((r.get::<_, String>(1)?, r.get::<_, i32>(2)? != 0))
    })?;
    let indexes: Vec<(String, bool)> = idx_rows.collect::<Result<Vec<_>, _>>()?;

    for (idx_name, is_unique) in indexes {
        if !is_unique {
            continue;
        }
        let escaped = idx_name.replace('"', "\"\"");
        let mut info_stmt = conn.prepare(&format!("PRAGMA index_info(\"{escaped}\")"))?;
        let info_rows = info_stmt.query_map([], |r| r.get::<_, Option<String>>(2))?;
        let idx_cols: Vec<String> = info_rows
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .flatten()
            .map(|s| s.to_ascii_lowercase())
            .collect();
        if idx_cols.len() == expected.len()
            && idx_cols
                .iter()
                .zip(expected.iter())
                .all(|(got, want)| got == want)
        {
            return Ok(true);
        }
    }

    Ok(false)
}

#[derive(Default)]
struct LegacySnapshot {
    folders: Vec<SalvagedFolder>,
    artists: Vec<SalvagedArtist>,
    local_albums: Vec<SalvagedLocalAlbum>,
    releases: Vec<SalvagedRelease>,
    settings: Vec<(String, String)>,
}

struct SalvagedFolder {
    path: String,
    enabled: bool,
    track_count: i64,
    last_scanned_at: Option<String>,
    added_at: String,
}

struct SalvagedArtist {
    name: String,
    normalized_name: String,
    mbid: Option<String>,
    deezer_id: Option<i64>,
    disambiguation: Option<String>,
    avatar_url: Option<String>,
    external_url: Option<String>,
    status: ArtistStatus,
    source: ArtistSource,
    local_album_count: i64,
    local_track_count: i64,
    last_checked_at: Option<String>,
    created_at: String,
}

struct SalvagedLocalAlbum {
    artist_norm: String,
    title: String,
    normalized_title: String,
    release_year: Option<i32>,
    mb_release_group_id: Option<String>,
    mb_album_id: Option<String>,
    track_count: i64,
    sample_path: Option<String>,
}

struct SalvagedRelease {
    artist_norm: String,
    title: String,
    normalized_title: String,
    release_type: ReleaseType,
    release_date: String,
    deezer_album_id: Option<i64>,
    mb_release_group_id: Option<String>,
    cover_url: Option<String>,
    cover_local_path: Option<String>,
    external_url: Option<String>,
    musicbrainz_url: Option<String>,
    track_count: i64,
    explicit: bool,
    owned_locally: bool,
    state: ReleaseState,
    provider_source: String,
    discovered_at: String,
}

fn salvage_legacy_snapshot(conn: &Connection) -> LegacySnapshot {
    let now = now_rfc3339_utc();
    let folders = salvage_folders(conn, &now);
    let (artists, id_to_norm) = salvage_artists(conn, &now);
    let local_albums = salvage_local_albums(conn, &id_to_norm);
    let releases = salvage_releases(conn, &id_to_norm, &now);
    let settings = salvage_settings(conn);
    LegacySnapshot {
        folders,
        artists,
        local_albums,
        releases,
        settings,
    }
}

fn existing_table_columns(
    conn: &Connection,
    candidates: &[&str],
) -> Option<(String, HashMap<String, usize>)> {
    for &table in candidates {
        let escaped = table.replace('"', "\"\"");
        let Ok(mut stmt) = conn.prepare(&format!("PRAGMA table_info(\"{escaped}\")")) else {
            continue;
        };
        let Ok(rows) = stmt.query_map([], |r| r.get::<_, String>(1)) else {
            continue;
        };
        let names: Vec<String> = rows.flatten().collect();
        if !names.is_empty() {
            let map = names
                .into_iter()
                .enumerate()
                .map(|(idx, name)| (name.to_ascii_lowercase(), idx))
                .collect();
            return Some((table.to_string(), map));
        }
    }
    None
}

fn row_string(
    row: &rusqlite::Row<'_>,
    cols: &HashMap<String, usize>,
    keys: &[&str],
) -> Option<String> {
    for key in keys {
        let Some(&idx) = cols.get(*key) else {
            continue;
        };
        let Ok(val) = row.get_ref(idx) else {
            continue;
        };
        let text = match val {
            ValueRef::Null => None,
            ValueRef::Text(bytes) => std::str::from_utf8(bytes).ok().map(str::to_string),
            ValueRef::Integer(i) => Some(i.to_string()),
            ValueRef::Real(f) => Some(f.to_string()),
            ValueRef::Blob(_) => None,
        };
        if let Some(s) = text.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) {
            return Some(s);
        }
    }
    None
}

fn row_i64(row: &rusqlite::Row<'_>, cols: &HashMap<String, usize>, keys: &[&str]) -> Option<i64> {
    for key in keys {
        let Some(&idx) = cols.get(*key) else {
            continue;
        };
        let Ok(val) = row.get_ref(idx) else {
            continue;
        };
        let parsed = match val {
            ValueRef::Integer(i) => Some(i),
            ValueRef::Real(f) => Some(f as i64),
            ValueRef::Text(bytes) => std::str::from_utf8(bytes)
                .ok()
                .and_then(|s| s.trim().parse::<i64>().ok()),
            ValueRef::Null | ValueRef::Blob(_) => None,
        };
        if parsed.is_some() {
            return parsed;
        }
    }
    None
}

fn row_bool(row: &rusqlite::Row<'_>, cols: &HashMap<String, usize>, keys: &[&str]) -> Option<bool> {
    for key in keys {
        let Some(&idx) = cols.get(*key) else {
            continue;
        };
        let Ok(val) = row.get_ref(idx) else {
            continue;
        };
        let parsed = match val {
            ValueRef::Integer(i) => Some(i != 0),
            ValueRef::Text(bytes) => std::str::from_utf8(bytes).ok().and_then(|s| {
                match s.trim().to_ascii_lowercase().as_str() {
                    "1" | "true" | "yes" => Some(true),
                    "0" | "false" | "no" => Some(false),
                    _ => None,
                }
            }),
            ValueRef::Null | ValueRef::Real(_) | ValueRef::Blob(_) => None,
        };
        if parsed.is_some() {
            return parsed;
        }
    }
    None
}

fn salvage_folders(conn: &Connection, now: &str) -> Vec<SalvagedFolder> {
    let Some((table, cols)) =
        existing_table_columns(conn, &["library_folders", "music_folders", "folders"])
    else {
        return Vec::new();
    };
    let escaped = table.replace('"', "\"\"");
    let Ok(mut stmt) = conn.prepare(&format!("SELECT * FROM \"{escaped}\"")) else {
        return Vec::new();
    };
    let Ok(mut rows) = stmt.query([]) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    while let Ok(Some(row)) = rows.next() {
        let Some(path) = row_string(row, &cols, &["path", "folder_path", "directory"]) else {
            continue;
        };
        out.push(SalvagedFolder {
            path,
            enabled: row_bool(row, &cols, &["enabled", "active"]).unwrap_or(true),
            track_count: row_i64(row, &cols, &["track_count"]).unwrap_or(0),
            last_scanned_at: row_string(row, &cols, &["last_scanned_at", "scanned_at"]),
            added_at: row_string(row, &cols, &["added_at", "created_at"])
                .unwrap_or_else(|| now.to_string()),
        });
    }
    out
}

fn salvage_artists(conn: &Connection, now: &str) -> (Vec<SalvagedArtist>, HashMap<i64, String>) {
    let Some((table, cols)) = existing_table_columns(conn, &["artists"]) else {
        return (Vec::new(), HashMap::new());
    };
    let escaped = table.replace('"', "\"\"");
    let Ok(mut stmt) = conn.prepare(&format!("SELECT * FROM \"{escaped}\"")) else {
        return (Vec::new(), HashMap::new());
    };
    let Ok(mut rows) = stmt.query([]) else {
        return (Vec::new(), HashMap::new());
    };
    let mut out = Vec::new();
    let mut id_to_norm = HashMap::new();
    while let Ok(Some(row)) = rows.next() {
        let Some(name) = row_string(row, &cols, &["name", "display_name", "artist_name"]) else {
            continue;
        };
        let normalized_name = row_string(row, &cols, &["normalized_name"])
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| normalize_artist_name(&name));
        if normalized_name.is_empty() {
            continue;
        }
        if let Some(old_id) = row_i64(row, &cols, &["id"]) {
            id_to_norm.insert(old_id, normalized_name.clone());
        }
        let status = if let Some(s) = row_string(row, &cols, &["status"]) {
            ArtistStatus::from_db(&s)
        } else if row_bool(row, &cols, &["ignored", "is_ignored"]).unwrap_or(false) {
            ArtistStatus::Ignored
        } else if row_bool(row, &cols, &["muted", "is_muted"]).unwrap_or(false) {
            ArtistStatus::Muted
        } else {
            ArtistStatus::Tracked
        };
        let source = if let Some(s) = row_string(row, &cols, &["source"]) {
            ArtistSource::from_db(&s)
        } else if row_bool(row, &cols, &["is_manual", "manual"]).unwrap_or(false) {
            ArtistSource::Manual
        } else {
            ArtistSource::Local
        };
        out.push(SalvagedArtist {
            name,
            normalized_name,
            mbid: row_string(row, &cols, &["mbid", "musicbrainz_id"]),
            deezer_id: row_i64(row, &cols, &["deezer_id", "deezer_artist_id"]),
            disambiguation: row_string(row, &cols, &["disambiguation"]),
            avatar_url: row_string(row, &cols, &["avatar_url", "image_url", "picture_url"]),
            external_url: row_string(row, &cols, &["external_url", "url"]),
            status,
            source,
            local_album_count: row_i64(row, &cols, &["local_album_count"]).unwrap_or(0),
            local_track_count: row_i64(row, &cols, &["local_track_count", "track_count"])
                .unwrap_or(0),
            last_checked_at: row_string(row, &cols, &["last_checked_at", "checked_at"]),
            created_at: row_string(row, &cols, &["created_at", "added_at"])
                .unwrap_or_else(|| now.to_string()),
        });
    }
    (out, id_to_norm)
}

fn salvage_local_albums(
    conn: &Connection,
    id_to_norm: &HashMap<i64, String>,
) -> Vec<SalvagedLocalAlbum> {
    let Some((table, cols)) =
        existing_table_columns(conn, &["local_albums", "owned_albums", "albums"])
    else {
        return Vec::new();
    };
    let escaped = table.replace('"', "\"\"");
    let Ok(mut stmt) = conn.prepare(&format!("SELECT * FROM \"{escaped}\"")) else {
        return Vec::new();
    };
    let Ok(mut rows) = stmt.query([]) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    while let Ok(Some(row)) = rows.next() {
        let artist_norm = row_i64(row, &cols, &["artist_id"])
            .and_then(|id| id_to_norm.get(&id).cloned())
            .or_else(|| {
                row_string(row, &cols, &["artist_name", "artist"])
                    .map(|n| normalize_artist_name(&n))
            });
        let Some(artist_norm) = artist_norm.filter(|s| !s.is_empty()) else {
            continue;
        };
        let Some(title) = row_string(row, &cols, &["title", "album_title", "name"]) else {
            continue;
        };
        let normalized_title = row_string(row, &cols, &["normalized_title"])
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| normalize_release_title(&title));
        if normalized_title.is_empty() {
            continue;
        }
        out.push(SalvagedLocalAlbum {
            artist_norm,
            title,
            normalized_title,
            release_year: row_i64(row, &cols, &["release_year", "year"]).map(|y| y as i32),
            mb_release_group_id: row_string(row, &cols, &["mb_release_group_id"]),
            mb_album_id: row_string(row, &cols, &["mb_album_id"]),
            track_count: row_i64(row, &cols, &["track_count"]).unwrap_or(0),
            sample_path: row_string(row, &cols, &["sample_path", "path"]),
        });
    }
    out
}

fn salvage_releases(
    conn: &Connection,
    id_to_norm: &HashMap<i64, String>,
    now: &str,
) -> Vec<SalvagedRelease> {
    let Some((table, cols)) = existing_table_columns(conn, &["releases", "remote_releases"]) else {
        return Vec::new();
    };
    let escaped = table.replace('"', "\"\"");
    let Ok(mut stmt) = conn.prepare(&format!("SELECT * FROM \"{escaped}\"")) else {
        return Vec::new();
    };
    let Ok(mut rows) = stmt.query([]) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    while let Ok(Some(row)) = rows.next() {
        let artist_norm = row_i64(row, &cols, &["artist_id"])
            .and_then(|id| id_to_norm.get(&id).cloned())
            .or_else(|| {
                row_string(row, &cols, &["artist_name", "artist"])
                    .map(|n| normalize_artist_name(&n))
            });
        let Some(artist_norm) = artist_norm.filter(|s| !s.is_empty()) else {
            continue;
        };
        let Some(title) = row_string(row, &cols, &["title", "name"]) else {
            continue;
        };
        let normalized_title = row_string(row, &cols, &["normalized_title"])
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| normalize_release_title(&title));
        if normalized_title.is_empty() {
            continue;
        }
        let release_type = row_string(row, &cols, &["release_type", "kind", "type"])
            .map(|s| ReleaseType::from_db(&s))
            .unwrap_or(ReleaseType::Album);
        let state = if let Some(s) = row_string(row, &cols, &["state", "status"]) {
            ReleaseState::from_db(&s)
        } else if row_bool(row, &cols, &["dismissed", "is_dismissed", "hidden"]).unwrap_or(false) {
            ReleaseState::Dismissed
        } else if row_bool(row, &cols, &["seen", "is_seen", "acknowledged"]).unwrap_or(false) {
            ReleaseState::Seen
        } else {
            ReleaseState::Unseen
        };
        out.push(SalvagedRelease {
            artist_norm,
            title,
            normalized_title,
            release_type,
            release_date: row_string(row, &cols, &["release_date", "first_release_date", "date"])
                .unwrap_or_default(),
            deezer_album_id: row_i64(row, &cols, &["deezer_album_id", "deezer_id"]),
            mb_release_group_id: row_string(
                row,
                &cols,
                &["mb_release_group_id", "release_group_id"],
            ),
            cover_url: row_string(row, &cols, &["cover_url", "artwork_url", "image_url"]),
            cover_local_path: row_string(row, &cols, &["cover_local_path"]),
            external_url: row_string(row, &cols, &["external_url", "url"]),
            musicbrainz_url: row_string(row, &cols, &["musicbrainz_url"]),
            track_count: row_i64(row, &cols, &["track_count"]).unwrap_or(0),
            explicit: row_bool(row, &cols, &["explicit", "is_explicit"]).unwrap_or(false),
            owned_locally: row_bool(row, &cols, &["owned_locally", "is_owned"]).unwrap_or(false),
            state,
            provider_source: row_string(row, &cols, &["provider_source", "provider", "source"])
                .unwrap_or_else(|| "deezer".to_string()),
            discovered_at: row_string(row, &cols, &["discovered_at", "created_at"])
                .unwrap_or_else(|| now.to_string()),
        });
    }
    out
}

fn salvage_settings(conn: &Connection) -> Vec<(String, String)> {
    let Some((table, cols)) = existing_table_columns(conn, &["settings", "app_settings"]) else {
        return Vec::new();
    };
    let escaped = table.replace('"', "\"\"");
    let Ok(mut stmt) = conn.prepare(&format!("SELECT * FROM \"{escaped}\"")) else {
        return Vec::new();
    };
    let Ok(mut rows) = stmt.query([]) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    while let Ok(Some(row)) = rows.next() {
        if let (Some(k), Some(v)) = (
            row_string(row, &cols, &["key", "name"]),
            row_string(row, &cols, &["value", "val"]),
        ) {
            out.push((k, v));
        }
    }
    out
}

fn restore_salvaged_snapshot(
    tx: &rusqlite::Transaction<'_>,
    snapshot: &LegacySnapshot,
) -> CoreResult<()> {
    let now = now_rfc3339_utc();

    for folder in &snapshot.folders {
        tx.execute(
            "INSERT INTO library_folders (path, enabled, track_count, last_scanned_at, added_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(path) DO UPDATE SET
                enabled = excluded.enabled,
                track_count = MAX(library_folders.track_count, excluded.track_count),
                last_scanned_at = COALESCE(excluded.last_scanned_at, library_folders.last_scanned_at)",
            params![
                folder.path,
                folder.enabled as i32,
                folder.track_count,
                folder.last_scanned_at,
                folder.added_at,
            ],
        )?;
    }

    for artist in &snapshot.artists {
        tx.execute(
            "INSERT INTO artists (
                name, normalized_name, mbid, deezer_id, disambiguation,
                avatar_url, external_url, status, source,
                local_album_count, local_track_count, last_checked_at,
                created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
             ON CONFLICT(normalized_name) DO UPDATE SET
                mbid = COALESCE(artists.mbid, excluded.mbid),
                deezer_id = COALESCE(artists.deezer_id, excluded.deezer_id),
                status = CASE WHEN artists.status != 'tracked' THEN artists.status ELSE excluded.status END",
            params![
                artist.name,
                artist.normalized_name,
                artist.mbid,
                artist.deezer_id,
                artist.disambiguation,
                artist.avatar_url,
                artist.external_url,
                artist.status.as_str(),
                artist.source.as_str(),
                artist.local_album_count,
                artist.local_track_count,
                artist.last_checked_at,
                artist.created_at,
                now,
            ],
        )?;
    }

    let norm_to_id: HashMap<String, i64> = {
        let mut stmt = tx.prepare("SELECT normalized_name, id FROM artists")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
        rows.collect::<Result<HashMap<_, _>, _>>()?
    };

    for album in &snapshot.local_albums {
        let Some(&artist_id) = norm_to_id.get(&album.artist_norm) else {
            continue;
        };
        tx.execute(
            "INSERT INTO local_albums (
                artist_id, title, normalized_title, release_year,
                mb_release_group_id, mb_album_id, track_count, sample_path
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(artist_id, normalized_title) DO UPDATE SET
                release_year = COALESCE(local_albums.release_year, excluded.release_year),
                mb_release_group_id = COALESCE(local_albums.mb_release_group_id, excluded.mb_release_group_id),
                track_count = MAX(local_albums.track_count, excluded.track_count)",
            params![
                artist_id,
                album.title,
                album.normalized_title,
                album.release_year,
                album.mb_release_group_id,
                album.mb_album_id,
                album.track_count,
                album.sample_path,
            ],
        )?;
    }

    tx.execute_batch(
        "UPDATE artists SET local_album_count = MAX(
            local_album_count,
            (SELECT COUNT(*) FROM local_albums la WHERE la.artist_id = artists.id)
        );",
    )?;

    for rel in &snapshot.releases {
        let Some(&artist_id) = norm_to_id.get(&rel.artist_norm) else {
            continue;
        };
        let owned_by_album: bool = tx.query_row(
            "SELECT EXISTS(
                SELECT 1 FROM local_albums
                WHERE artist_id = ?1 AND (
                    normalized_title = ?2
                    OR (?3 IS NOT NULL AND mb_release_group_id = ?3)
                )
            )",
            params![artist_id, rel.normalized_title, rel.mb_release_group_id],
            |r| r.get(0),
        )?;
        tx.execute(
            "INSERT INTO releases (
                artist_id, title, normalized_title, release_type, release_date,
                deezer_album_id, mb_release_group_id, cover_url, cover_local_path,
                external_url, musicbrainz_url, track_count, explicit, owned_locally,
                state, provider_source, discovered_at, updated_at
             ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18
             )",
            params![
                artist_id,
                rel.title,
                rel.normalized_title,
                rel.release_type.as_str(),
                rel.release_date,
                rel.deezer_album_id,
                rel.mb_release_group_id,
                rel.cover_url,
                rel.cover_local_path,
                rel.external_url,
                rel.musicbrainz_url,
                rel.track_count,
                rel.explicit as i32,
                (rel.owned_locally || owned_by_album) as i32,
                rel.state.as_str(),
                rel.provider_source,
                rel.discovered_at,
                now,
            ],
        )?;
    }

    for (key, value) in &snapshot.settings {
        tx.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scanner::{DiscoveredAlbumRecord, DiscoveredArtistRecord};

    #[test]
    fn preserves_muted_status_and_deduplicates_owned_and_cross_provider_releases() {
        let mut db = Database::open_in_memory().unwrap();

        let mut albums = HashMap::new();
        albums.insert(
            "discovery".to_string(),
            DiscoveredAlbumRecord {
                title: "Discovery".to_string(),
                normalized_title: "discovery".to_string(),
                release_year: Some(2001),
                mb_release_group_id: Some("11111111-2222-3333-4444-555555555555".to_string()),
                mb_album_id: None,
                track_count: 14,
                sample_path: "/music/Daft Punk/Discovery/01.flac".to_string(),
            },
        );
        let mut outcome = ScanOutcome::default();
        outcome.artists.insert(
            "daft punk".to_string(),
            DiscoveredArtistRecord {
                display_name: "Daft Punk".to_string(),
                normalized_name: "daft punk".to_string(),
                mbid: None,
                track_count: 14,
                albums,
            },
        );
        db.apply_scan_outcome(&outcome).unwrap();

        let artists = db.list_tracked_artists().unwrap();
        assert_eq!(artists.len(), 1);
        let dp_id = artists[0].id;

        // 1. Owned album discovered from Deezer -> automatically flagged owned_locally = true!
        let (_, inserted_owned, owned_flag) = db
            .upsert_discovered_release(&DiscoveredReleaseInput {
                artist_id: dp_id,
                title: "Discovery (Deluxe Edition)".to_string(),
                release_type: ReleaseType::Album,
                release_date: "2001-03-12".to_string(),
                deezer_album_id: Some(302127),
                mb_release_group_id: None,
                cover_url: Some("https://cdn.deezer.com/discovery.jpg".to_string()),
                external_url: Some("https://www.deezer.com/album/302127".to_string()),
                musicbrainz_url: None,
                track_count: 14,
                explicit: false,
                provider_source: "deezer".to_string(),
            })
            .unwrap();
        assert!(inserted_owned);
        assert!(owned_flag);

        // 2. Unowned new EP discovered on Deezer AND then enriched by MusicBrainz -> merges into ONE row!
        let (rel_id_1, ins_1, owned_1) = db
            .upsert_discovered_release(&DiscoveredReleaseInput {
                artist_id: dp_id,
                title: "Horizon Overture - EP".to_string(),
                release_type: ReleaseType::Ep,
                release_date: "2026-10-15".to_string(),
                deezer_album_id: Some(999001),
                mb_release_group_id: None,
                cover_url: Some("https://cdn.deezer.com/horizon.jpg".to_string()),
                external_url: Some("https://www.deezer.com/album/999001".to_string()),
                musicbrainz_url: None,
                track_count: 4,
                explicit: false,
                provider_source: "deezer".to_string(),
            })
            .unwrap();
        assert!(ins_1);
        assert!(!owned_1);

        // Mark it seen, then re-upsert from MusicBrainz -> state stays Seen and provider becomes deezer+musicbrainz
        db.set_release_state(rel_id_1, ReleaseState::Seen).unwrap();
        let (rel_id_2, ins_2, _) = db
            .upsert_discovered_release(&DiscoveredReleaseInput {
                artist_id: dp_id,
                title: "Horizon Overture".to_string(),
                release_type: ReleaseType::Ep,
                release_date: "2026-10-15".to_string(),
                deezer_album_id: None,
                mb_release_group_id: Some("aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee".to_string()),
                cover_url: None,
                external_url: None,
                musicbrainz_url: Some(
                    "https://musicbrainz.org/release-group/aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"
                        .to_string(),
                ),
                track_count: 4,
                explicit: false,
                provider_source: "musicbrainz".to_string(),
            })
            .unwrap();
        assert_eq!(rel_id_1, rel_id_2);
        assert!(!ins_2);

        let merged = db.get_release(rel_id_1).unwrap().unwrap();
        assert_eq!(merged.state, ReleaseState::Seen);
        assert_eq!(merged.provider_source, "deezer+musicbrainz");

        // Mute artist and re-scan -> Muted status is preserved!
        db.set_artist_status(dp_id, ArtistStatus::Muted).unwrap();
        db.apply_scan_outcome(&outcome).unwrap();
        let dp_after = db.get_artist(dp_id).unwrap().unwrap();
        assert_eq!(dp_after.status, ArtistStatus::Muted);
    }

    #[test]
    fn migrates_legacy_database_missing_normalized_columns_and_preserves_data() {
        let dir = std::env::temp_dir().join(format!(
            "missnix_legacy_db_test_{}_{}",
            std::process::id(),
            now_rfc3339_utc().replace([':', '-'], "")
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join("missnix.db");

        // Create a pre-existing database matching the incompatible legacy schema
        // (`local_albums` and `releases` lacking `normalized_title`, `artists` lacking
        // `normalized_name`, boolean `muted` / `seen` columns, plus a foreign table).
        {
            let raw = Connection::open(&db_path).unwrap();
            raw.execute_batch(
                "
                CREATE TABLE library_folders (
                    id INTEGER PRIMARY KEY,
                    path TEXT NOT NULL,
                    added_at TEXT
                );
                CREATE TABLE artists (
                    id INTEGER PRIMARY KEY,
                    name TEXT NOT NULL,
                    mbid TEXT,
                    muted INTEGER NOT NULL DEFAULT 0
                );
                CREATE TABLE local_albums (
                    id INTEGER PRIMARY KEY,
                    artist_id INTEGER NOT NULL REFERENCES artists(id),
                    title TEXT NOT NULL,
                    year INTEGER
                );
                CREATE TABLE releases (
                    id INTEGER PRIMARY KEY,
                    artist_id INTEGER NOT NULL REFERENCES artists(id),
                    title TEXT NOT NULL,
                    release_type TEXT NOT NULL,
                    release_date TEXT NOT NULL,
                    seen INTEGER NOT NULL DEFAULT 0
                );
                CREATE TABLE legacy_sync_log (
                    id INTEGER PRIMARY KEY,
                    artist_id INTEGER NOT NULL REFERENCES artists(id),
                    note TEXT NOT NULL
                );
                CREATE TABLE settings (
                    key TEXT PRIMARY KEY,
                    value TEXT NOT NULL
                );

                INSERT INTO library_folders (id, path, added_at)
                VALUES (1, '/music/library', '2026-01-01T00:00:00Z');

                INSERT INTO artists (id, name, mbid, muted)
                VALUES
                    (10, 'Boards of Canada', '69158f97-4c07-4c4e-baf8-4e4ab1ed666e', 0),
                    (20, 'NoisyCompilationArtist', NULL, 1);

                INSERT INTO local_albums (id, artist_id, title, year)
                VALUES (100, 10, 'Music Has the Right to Children (Remastered)', 1998);

                INSERT INTO releases (id, artist_id, title, release_type, release_date, seen)
                VALUES (200, 10, 'Tomorrow''s Harvest', 'album', '2013-06-05', 1);

                INSERT INTO legacy_sync_log (id, artist_id, note)
                VALUES (1, 10, 'old run');

                INSERT INTO settings (key, value)
                VALUES ('lookback_days', '180');
                ",
            )
            .unwrap();
        }

        // Opening the legacy DB must succeed, create `.bak`, and preserve all user data!
        {
            let db = Database::open(&db_path).unwrap();
            let bak_path = dir.join("missnix.db.bak");
            assert!(bak_path.is_file());

            let folders = db.list_library_folders().unwrap();
            assert_eq!(folders.len(), 1);
            assert_eq!(folders[0].path, "/music/library");

            let tracked = db.list_tracked_artists().unwrap();
            assert_eq!(tracked.len(), 1);
            assert_eq!(tracked[0].name, "Boards of Canada");
            assert_eq!(tracked[0].normalized_name, "boards of canada");
            assert_eq!(tracked[0].local_album_count, 1);

            let muted = db
                .list_artists("", Some(ArtistStatus::Muted), false)
                .unwrap();
            assert_eq!(muted.len(), 1);
            assert_eq!(muted[0].name, "NoisyCompilationArtist");

            let owned = db.list_local_albums_for_artist(tracked[0].id).unwrap();
            assert_eq!(owned.len(), 1);
            assert_eq!(owned[0].normalized_title, "music has the right to children");
            assert_eq!(owned[0].release_year, Some(1998));

            let releases = db
                .query_releases(&FeedQuery {
                    include_albums: true,
                    include_eps: true,
                    include_singles: true,
                    only_unseen: false,
                    hide_owned: false,
                    ..FeedQuery::default()
                })
                .unwrap();
            assert_eq!(releases.len(), 1);
            assert_eq!(releases[0].title, "Tomorrow's Harvest");
            assert_eq!(releases[0].state, ReleaseState::Seen);

            let settings = db.load_settings().unwrap();
            assert_eq!(settings.lookback_days, 180);
        }

        // Re-opening the migrated canonical DB must pass validation cleanly without rebuilding.
        {
            let db_reopened = Database::open(&db_path).unwrap();
            assert_eq!(db_reopened.list_tracked_artists().unwrap().len(), 1);
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn prunes_stale_polluted_local_artists_on_rescan_and_persists_new_settings() {
        let mut db = Database::open_in_memory().unwrap();

        // Seed an initial scan that contained a polluted collaboration row + a muted artist + a manual artist
        let mut initial = ScanOutcome::default();
        initial.summary.folders_scanned = 1;
        initial.summary.audio_files_seen = 3;
        initial.artists.insert(
            "alligatoah".to_string(),
            DiscoveredArtistRecord {
                display_name: "Alligatoah".to_string(),
                normalized_name: "alligatoah".to_string(),
                mbid: None,
                track_count: 10,
                albums: HashMap::new(),
            },
        );
        initial.artists.insert(
            "alligatoah jamal".to_string(),
            DiscoveredArtistRecord {
                display_name: "Alligatoah, Jamal".to_string(),
                normalized_name: "alligatoah jamal".to_string(),
                mbid: None,
                track_count: 1,
                albums: HashMap::new(),
            },
        );
        initial.artists.insert(
            "muted local".to_string(),
            DiscoveredArtistRecord {
                display_name: "Muted Local".to_string(),
                normalized_name: "muted local".to_string(),
                mbid: None,
                track_count: 1,
                albums: HashMap::new(),
            },
        );
        db.apply_scan_outcome(&initial).unwrap();

        let muted = db
            .list_artists("Muted Local", None, false)
            .unwrap()
            .into_iter()
            .next()
            .unwrap();
        db.set_artist_status(muted.id, ArtistStatus::Muted).unwrap();
        db.add_manual_artist("Manual Favorite", None, None, None, None, None)
            .unwrap();

        // Rescan with clean primary artist extraction: "Alligatoah, Jamal" is no longer present
        let mut clean_scan = ScanOutcome::default();
        clean_scan.summary.folders_scanned = 1;
        clean_scan.summary.audio_files_seen = 11;
        clean_scan.artists.insert(
            "alligatoah".to_string(),
            DiscoveredArtistRecord {
                display_name: "Alligatoah".to_string(),
                normalized_name: "alligatoah".to_string(),
                mbid: None,
                track_count: 11,
                albums: HashMap::new(),
            },
        );
        db.apply_scan_outcome(&clean_scan).unwrap();

        let all = db.list_artists("", None, false).unwrap();
        let names: Vec<&str> = all.iter().map(|a| a.name.as_str()).collect();
        assert!(names.contains(&"Alligatoah"));
        assert!(names.contains(&"Manual Favorite"));
        assert!(names.contains(&"Muted Local"));
        assert!(!names.contains(&"Alligatoah, Jamal"));

        // Persist and reload customization & scan settings (including HiDPI UI scale)
        let custom = AppSettings {
            dark_mode: false,
            accent_preset: 3,
            grid_density: 2,
            ui_scale_percent: 150,
            preview_volume: 65,
            artist_discovery_mode: ArtistDiscoveryMode::FolderOnly,
            split_collaborations: false,
            ..AppSettings::default()
        };
        db.save_settings(&custom).unwrap();
        let loaded = db.load_settings().unwrap();
        assert!(!loaded.dark_mode);
        assert_eq!(loaded.accent_preset, 3);
        assert_eq!(loaded.grid_density, 2);
        assert_eq!(loaded.ui_scale_percent, 150);
        assert_eq!(loaded.preview_volume, 65);
        assert_eq!(
            loaded.artist_discovery_mode,
            ArtistDiscoveryMode::FolderOnly
        );
        assert!(!loaded.split_collaborations);

        // Literal `%` and `_` in search queries do not act as SQLite wildcards
        assert_eq!(escape_sql_like("99.9%_test\\x"), "99.9\\%\\_test\\\\x");
        assert!(db.list_artists("%", None, false).unwrap().is_empty());
    }
}
