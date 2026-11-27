use rusqlite::{
    Connection, OptionalExtension, ToSql, Transaction, TransactionBehavior, params,
    params_from_iter,
};

use crate::error::{CoreError, CoreResult};
use crate::models::{FeedQuery, FeedSort, Release, ReleaseState, ReleaseTrack, ReleaseType};
use crate::normalize::{normalize_release_title, now_rfc3339_utc, today_iso_utc};

use super::{
    Database, DiscoveredReleaseInput, DiscoveredTrackInput, escape_sql_like, merge_provider_labels,
};

impl Database {
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

        // Serialize the match and update/insert across GUI and CLI connections.
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;

        // Match existing release by Deezer ID, MusicBrainz Release-Group ID, or (normalized_title, release_type)
        let existing: Option<(i64, String, String, Option<String>)> = tx
            .query_row(
                "SELECT id, provider_source, normalized_title, mb_release_group_id FROM releases
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
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?;

        // Use the persisted release identity as well as the incoming provider's
        // identity. Deezer updates normally omit a previously known MB group ID.
        let owned_locally = release_owned_locally(
            &tx,
            input.artist_id,
            existing
                .as_ref()
                .map_or(norm_title.as_str(), |row| row.2.as_str()),
            input
                .mb_release_group_id
                .as_deref()
                .or_else(|| existing.as_ref().and_then(|row| row.3.as_deref())),
        )?;
        let now = now_rfc3339_utc();

        let (release_id, inserted) = if let Some((existing_id, existing_provider, _, _)) = existing
        {
            let merged_provider = merge_provider_labels(&existing_provider, &input.provider_source);
            tx.execute(
                "UPDATE releases SET
                    release_date = CASE
                        WHEN (release_date = '' OR LENGTH(?1) > LENGTH(release_date)) AND ?1 != '' THEN ?1
                        ELSE release_date
                    END,
                    deezer_album_id = COALESCE(?2, deezer_album_id),
                    mb_release_group_id = COALESCE(?3, mb_release_group_id),
                    cover_local_path = CASE
                        WHEN ?13 = 'deezer' AND ?4 IS NOT NULL AND COALESCE(cover_url, '') != ?4
                        THEN NULL ELSE cover_local_path END,
                    cover_url = CASE WHEN ?13 = 'deezer' AND ?4 IS NOT NULL THEN ?4
                                     ELSE COALESCE(cover_url, ?4) END,
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
                    existing_id,
                    input.provider_source
                ],
            )?;
            (existing_id, false)
        } else {
            tx.execute(
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
            (tx.last_insert_rowid(), true)
        };
        tx.commit()?;
        Ok((release_id, inserted, owned_locally))
    }

    pub fn is_release_owned_locally(
        &self,
        artist_id: i64,
        normalized_title: &str,
        mb_release_group_id: Option<&str>,
    ) -> CoreResult<bool> {
        release_owned_locally(&self.conn, artist_id, normalized_title, mb_release_group_id)
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
            let norm = normalize_release_title(trimmed);
            if norm.is_empty() {
                sql.push_str(
                    " AND (LOWER(r.title) LIKE ? ESCAPE '\\' OR LOWER(a.name) LIKE ? ESCAPE '\\')",
                );
                bind_vals.push(Box::new(like_raw.clone()));
                bind_vals.push(Box::new(like_raw));
            } else {
                let norm_t = format!("%{}%", escape_sql_like(&norm));
                sql.push_str(
                    " AND (LOWER(r.title) LIKE ? ESCAPE '\\' OR LOWER(a.name) LIKE ? ESCAPE '\\' OR r.normalized_title LIKE ? ESCAPE '\\')",
                );
                bind_vals.push(Box::new(like_raw.clone()));
                bind_vals.push(Box::new(like_raw));
                bind_vals.push(Box::new(norm_t));
            }
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
        let updated = self.conn.execute(
            "UPDATE releases SET state = ?1, updated_at = ?2 WHERE id = ?3",
            params![state.as_str(), now, release_id],
        )?;
        if updated == 0 {
            return Err(CoreError::Validation(
                "Release no longer exists".to_string(),
            ));
        }
        Ok(())
    }

    /// Mark only the release IDs selected by the current feed filters. Already
    /// seen/dismissed or concurrently deleted rows are left unchanged.
    pub fn mark_releases_seen(&mut self, release_ids: &[i64]) -> CoreResult<usize> {
        let tx = self.conn.transaction()?;
        let now = now_rfc3339_utc();
        let mut updated = 0;
        for id in release_ids {
            updated += tx.execute(
                "UPDATE releases SET state = 'seen', updated_at = ?1
                 WHERE id = ?2 AND state = 'unseen'",
                params![now, id],
            )?;
        }
        tx.commit()?;
        Ok(updated)
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
}

fn release_owned_locally(
    conn: &Connection,
    artist_id: i64,
    normalized_title: &str,
    mb_release_group_id: Option<&str>,
) -> CoreResult<bool> {
    let found: Option<i32> = conn
        .query_row(
            "SELECT 1 FROM local_albums
             WHERE artist_id = ?1
               AND (normalized_title = ?2 OR (?3 IS NOT NULL AND mb_release_group_id = ?3))
             LIMIT 1",
            params![artist_id, normalized_title, mb_release_group_id],
            |r| r.get(0),
        )
        .optional()?;
    Ok(found.is_some())
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
