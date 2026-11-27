use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use rusqlite::{Connection, Transaction, TransactionBehavior, params};

use crate::error::{CoreError, CoreResult};
use crate::models::LibraryFolder;
use crate::normalize::now_rfc3339_utc;
use crate::scanner::{ScanOptions, ScanOutcome};

use super::settings::{advance_scan_revision, load_settings_from_conn, scan_revision};
use super::{Database, expand_tilde};

/// A consistent view of the configured folders and scan preferences. Apply the
/// resulting scan with this same snapshot to reject stale concurrent workers.
#[derive(Debug, Clone)]
pub struct LibraryScanSnapshot {
    pub folders: Vec<PathBuf>,
    pub options: ScanOptions,
    folder_ids: Vec<(i64, String)>,
    revision: i64,
}

fn enabled_folders(conn: &Connection) -> CoreResult<Vec<(i64, String)>> {
    let mut stmt =
        conn.prepare("SELECT id, path FROM library_folders WHERE enabled = 1 ORDER BY id")?;
    let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
    rows.collect::<Result<Vec<_>, _>>().map_err(CoreError::from)
}

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

        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let changed = tx.execute(
            "INSERT INTO library_folders (path, enabled, added_at)
             VALUES (?1, 1, ?2)
             ON CONFLICT(path) DO UPDATE SET enabled = 1 WHERE enabled != 1",
            params![path_str, now],
        )?;
        if changed > 0 {
            advance_scan_revision(&tx)?;
        }
        let folder = tx.query_row(
            "SELECT id, path, enabled, track_count, last_scanned_at, added_at
             FROM library_folders WHERE path = ?1",
            params![path_str],
            map_folder_row,
        )?;
        tx.commit()?;
        Ok(folder)
    }

    pub fn remove_library_folder(&self, folder_id: i64) -> CoreResult<()> {
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        if tx.execute(
            "DELETE FROM library_folders WHERE id = ?1",
            params![folder_id],
        )? > 0
        {
            advance_scan_revision(&tx)?;
        }
        tx.commit()?;
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

    pub fn prepare_library_scan(&self) -> CoreResult<LibraryScanSnapshot> {
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Deferred)?;
        let folder_ids = enabled_folders(&tx)?;
        let folders = folder_ids
            .iter()
            .map(|(_, path)| PathBuf::from(path))
            .collect();
        let options = ScanOptions::from_settings(&load_settings_from_conn(&tx)?);
        let revision = scan_revision(&tx)?;
        tx.commit()?;
        Ok(LibraryScanSnapshot {
            folders,
            options,
            folder_ids,
            revision,
        })
    }

    /// Apply an outcome against the current configuration. Callers which scan the
    /// filesystem should use `prepare_library_scan` and `apply_scan_outcome_checked`
    /// instead, so changes made while the scan runs cannot be overwritten.
    pub fn apply_scan_outcome(&mut self, outcome: &ScanOutcome) -> CoreResult<()> {
        let snapshot = self.prepare_library_scan()?;
        self.apply_scan_outcome_checked(outcome, &snapshot, false, None)
    }

    /// Apply a full, current scan atomically. An unexpectedly empty previously indexed
    /// folder is rejected unless the user explicitly confirms it was emptied.
    pub fn apply_scan_outcome_checked(
        &mut self,
        outcome: &ScanOutcome,
        snapshot: &LibraryScanSnapshot,
        allow_empty_folders: bool,
        cancel_flag: Option<&AtomicBool>,
    ) -> CoreResult<()> {
        let now = now_rfc3339_utc();
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if cancel_flag.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
            return Err(CoreError::Cancelled);
        }
        if scan_revision(&tx)? != snapshot.revision
            || enabled_folders(&tx)? != snapshot.folder_ids
            || ScanOptions::from_settings(&load_settings_from_conn(&tx)?) != snapshot.options
        {
            return Err(CoreError::Validation(
                "Library folders or scan settings changed while scanning; scan again.".to_string(),
            ));
        }
        if outcome.folder_track_counts.len() != snapshot.folders.len()
            || snapshot
                .folders
                .iter()
                .any(|path| !outcome.folder_track_counts.contains_key(path))
        {
            return Err(CoreError::Validation(
                "Incomplete library scan; previous library data was not changed.".to_string(),
            ));
        }
        if !allow_empty_folders {
            for path in &snapshot.folders {
                let previous_count: i64 = tx.query_row(
                    "SELECT track_count FROM library_folders WHERE path = ?1 AND enabled = 1",
                    params![path.to_string_lossy().to_string()],
                    |r| r.get(0),
                )?;
                if previous_count > 0 && outcome.folder_track_counts[path] == 0 {
                    return Err(CoreError::Validation(format!(
                        "{} previously contained audio files but is now empty. \
                         Reconnect the drive and scan again, or remove and re-add the folder \
                         to confirm it was intentionally emptied. Library data was not changed.",
                        path.display()
                    )));
                }
            }
        }

        for (folder_path, count) in &outcome.folder_track_counts {
            let p = folder_path.to_string_lossy().to_string();
            tx.execute(
                "UPDATE library_folders
                 SET track_count = ?1, last_scanned_at = ?2
                 WHERE path = ?3",
                params![*count as i64, now, p],
            )?;
        }

        // Manual artists can also have local files, so reset counts for every artist.
        tx.execute(
            "UPDATE artists SET local_album_count = 0, local_track_count = 0",
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
                    status = CASE WHEN artists.status = 'missing' THEN 'tracked' ELSE artists.status END,
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

        // Never cascade-delete release history, tracklists, or user state just
        // because a local artist disappeared. Retire it from the radar instead;
        // it will resume tracking if a later scan finds it again.
        tx.execute(
            "UPDATE artists SET status = 'missing'
             WHERE source = 'local' AND status = 'tracked' AND local_track_count = 0",
            [],
        )?;

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

        advance_scan_revision(&tx)?;
        if cancel_flag.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
            return Err(CoreError::Cancelled);
        }
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
