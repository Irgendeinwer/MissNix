use rusqlite::params;

use crate::error::{CoreError, CoreResult};
use crate::models::LibraryFolder;
use crate::normalize::now_rfc3339_utc;
use crate::scanner::ScanOutcome;

use super::{Database, expand_tilde};

impl Database {
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
