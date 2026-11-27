use rusqlite::{
    Connection, OptionalExtension, ToSql, Transaction, TransactionBehavior, params,
    params_from_iter,
};

use crate::error::{CoreError, CoreResult};
use crate::models::{FeedQuery, FeedSort, Release, ReleaseState, ReleaseTrack, ReleaseType};
use crate::normalize::{
    fold_diacritics_and_symbols, normalize_release_title, now_rfc3339_utc, today_iso_utc,
};

use super::{
    Database, DiscoveredReleaseInput, DiscoveredTrackInput, escape_sql_like, merge_provider_labels,
    release_match::{ReleaseIdentity, choose_release_match},
};

impl Database {
    /// Insert or merge a discovered release, preserving any existing user state (`seen` / `dismissed`)
    /// and checking local library ownership automatically.
    /// Returns `(release_id, was_newly_inserted, owned_locally)`.
    pub fn upsert_discovered_release(
        &self,
        input: &DiscoveredReleaseInput,
    ) -> CoreResult<(i64, bool, bool)> {
        self.upsert_discovered_release_inner(input, None, true)
    }

    pub(crate) fn upsert_discovered_release_if_ids_match(
        &self,
        input: &DiscoveredReleaseInput,
        mbid: Option<&str>,
        deezer_id: Option<i64>,
        allow_cross_type: bool,
    ) -> CoreResult<(i64, bool, bool)> {
        self.upsert_discovered_release_inner(input, Some((mbid, deezer_id)), allow_cross_type)
    }

    fn upsert_discovered_release_inner(
        &self,
        input: &DiscoveredReleaseInput,
        expected_ids: Option<(Option<&str>, Option<i64>)>,
        allow_cross_type: bool,
    ) -> CoreResult<(i64, bool, bool)> {
        let norm_title = normalize_release_title(&input.title);
        if norm_title.is_empty() {
            return Err(CoreError::Validation(
                "Release title normalized to empty".to_string(),
            ));
        }

        // Serialize the match and update/insert across GUI and CLI connections.
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        if let Some((mbid, deezer_id)) = expected_ids {
            let matches: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM artists WHERE id = ?1 AND mbid IS ?2 AND deezer_id IS ?3)",
                params![input.artist_id, mbid, deezer_id],
                |r| r.get(0),
            )?;
            if !matches {
                return Err(CoreError::Validation(
                    "Artist provider IDs changed during the check; please retry.".to_string(),
                ));
            }
        }

        let incoming = ReleaseIdentity {
            id: 0,
            title: input.title.clone(),
            normalized_title: norm_title.clone(),
            release_type: input.release_type.as_str().to_string(),
            release_date: input.release_date.clone(),
            deezer_album_id: input.deezer_album_id,
            mb_release_group_id: input.mb_release_group_id.clone(),
        };
        // Include competing same-title rows and rows connected to an exact ID.
        // Do not pick an arbitrary first title match when several are plausible.
        let candidates = {
            let mut stmt = tx.prepare(
                "WITH direct AS (
                    SELECT id, normalized_title, deezer_album_id, mb_release_group_id
                    FROM releases WHERE artist_id = ?1 AND
                        ((?3 IS NOT NULL AND deezer_album_id = ?3)
                         OR (?4 IS NOT NULL AND mb_release_group_id = ?4))
                 )
                 SELECT id, title, normalized_title, release_type, release_date,
                        deezer_album_id, mb_release_group_id
                 FROM releases WHERE artist_id = ?1 AND (
                    normalized_title = ?2 OR normalized_title IN (SELECT normalized_title FROM direct)
                    OR id IN (SELECT id FROM direct)
                    OR deezer_album_id IN (SELECT deezer_album_id FROM direct)
                    OR mb_release_group_id IN (SELECT mb_release_group_id FROM direct)
                 ) ORDER BY id",
            )?;
            let rows = stmt.query_map(
                params![
                    input.artist_id,
                    norm_title,
                    input.deezer_album_id,
                    input.mb_release_group_id
                ],
                |row| {
                    Ok(ReleaseIdentity {
                        id: row.get(0)?,
                        title: row.get(1)?,
                        normalized_title: row.get(2)?,
                        release_type: row.get(3)?,
                        release_date: row.get(4)?,
                        deezer_album_id: row.get(5)?,
                        mb_release_group_id: row.get(6)?,
                    })
                },
            )?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        let (selected, duplicate) = choose_release_match(&incoming, &candidates, allow_cross_type);
        if selected.is_some_and(|index| candidates[index].conflicts_with(&incoming)) {
            return Err(CoreError::Validation(
                "Conflicting provider IDs for an existing release; no release was changed."
                    .to_string(),
            ));
        }
        let selected_id = if let Some(index) = selected {
            let id = candidates[index].id;
            if let Some(other) =
                duplicate.filter(|&other| !candidates[other].conflicts_with(&incoming))
            {
                consolidate_release_pair(&tx, id, candidates[other].id)?.unwrap_or(id)
            } else {
                id
            }
        } else {
            0
        };
        let existing: Option<(i64, String, String, Option<String>)> = if selected_id > 0 {
            Some(tx.query_row(
                "SELECT id, provider_source, normalized_title, mb_release_group_id
                 FROM releases WHERE id = ?1",
                params![selected_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )?)
        } else {
            None
        };

        // Consider both the incoming and retained identity. If only the incoming
        // spelling matches a local album, retain it so later scans keep ownership.
        let mb_group = input
            .mb_release_group_id
            .as_deref()
            .or_else(|| existing.as_ref().and_then(|row| row.3.as_deref()));
        let existing_owned = match existing.as_ref() {
            Some(row) => release_owned_locally(&tx, input.artist_id, &row.2, mb_group)?,
            None => false,
        };
        let incoming_owned = release_owned_locally(&tx, input.artist_id, &norm_title, mb_group)?;
        let owned_locally = existing_owned || incoming_owned;
        let promote_incoming_title = existing.is_some() && !existing_owned && incoming_owned;
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
                    deezer_album_id = COALESCE(deezer_album_id, ?2),
                    mb_release_group_id = COALESCE(mb_release_group_id, ?3),
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
                    updated_at = ?11,
                    release_type = CASE WHEN ?13 = 'musicbrainz' THEN ?14 ELSE release_type END,
                    title = CASE WHEN ?15 THEN ?16 ELSE title END,
                    normalized_title = CASE WHEN ?15 THEN ?17 ELSE normalized_title END
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
                    input.provider_source,
                    input.release_type.as_str(),
                    promote_incoming_title,
                    input.title,
                    norm_title
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
        replace_tracks_in_transaction(&tx, release_id, tracks)?;
        tx.commit()?;
        Ok(())
    }

    /// Do not let an in-flight provider response overwrite tracks transferred
    /// by a concurrent duplicate merge (or by another track refresh).
    pub(crate) fn replace_release_tracks_if_unchanged(
        &mut self,
        release: &Release,
        previous: &[ReleaseTrack],
        tracks: &[DiscoveredTrackInput],
    ) -> CoreResult<bool> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let same_identity: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM releases WHERE id = ?1 AND artist_id = ?2
                 AND deezer_album_id IS ?3 AND mb_release_group_id IS ?4)",
            params![
                release.id,
                release.artist_id,
                release.deezer_album_id,
                release.mb_release_group_id
            ],
            |row| row.get(0),
        )?;
        if !same_identity || load_release_tracks(&tx, release.id)? != previous {
            return Ok(false);
        }
        replace_tracks_in_transaction(&tx, release.id, tracks)?;
        tx.commit()?;
        Ok(true)
    }

    pub fn list_release_tracks(&self, release_id: i64) -> CoreResult<Vec<ReleaseTrack>> {
        load_release_tracks(&self.conn, release_id)
    }
}

fn replace_tracks_in_transaction(
    tx: &Transaction<'_>,
    release_id: i64,
    tracks: &[DiscoveredTrackInput],
) -> CoreResult<()> {
    tx.execute(
        "DELETE FROM release_tracks WHERE release_id = ?1",
        params![release_id],
    )?;
    for track in tracks {
        tx.execute(
            "INSERT OR REPLACE INTO release_tracks (
                release_id, disc_number, track_number, title,
                duration_secs, preview_url, artist_credit
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                release_id,
                track.disc_number.max(1),
                track.track_number,
                track.title,
                track.duration_secs.max(0),
                track.preview_url,
                track.artist_credit
            ],
        )?;
    }
    if !tracks.is_empty() {
        tx.execute(
            "UPDATE releases SET track_count = MAX(track_count, ?1) WHERE id = ?2",
            params![tracks.len() as i32, release_id],
        )?;
    }
    Ok(())
}

fn load_release_tracks(conn: &Connection, release_id: i64) -> CoreResult<Vec<ReleaseTrack>> {
    let mut stmt = conn.prepare(
        "SELECT id, release_id, disc_number, track_number, title, duration_secs, preview_url, artist_credit
         FROM release_tracks WHERE release_id = ?1
         ORDER BY disc_number ASC, track_number ASC",
    )?;
    let rows = stmt.query_map(params![release_id], |row| {
        Ok(ReleaseTrack {
            id: row.get(0)?,
            release_id: row.get(1)?,
            disc_number: row.get(2)?,
            track_number: row.get(3)?,
            title: row.get(4)?,
            duration_secs: row.get(5)?,
            preview_url: row.get(6)?,
            artist_credit: row.get(7)?,
        })
    })?;
    rows.collect::<Result<Vec<_>, _>>().map_err(CoreError::from)
}

#[derive(Debug)]
struct StoredRelease {
    identity: ReleaseIdentity,
    artist_id: i64,
    cover_url: Option<String>,
    cover_local_path: Option<String>,
    external_url: Option<String>,
    musicbrainz_url: Option<String>,
    track_count: i32,
    explicit: bool,
    state: String,
    provider_source: String,
    discovered_at: String,
}

fn stored_release(conn: &Connection, id: i64) -> CoreResult<StoredRelease> {
    conn.query_row(
        "SELECT id, artist_id, title, normalized_title, release_type, release_date,
                deezer_album_id, mb_release_group_id, cover_url, cover_local_path,
                external_url, musicbrainz_url, track_count, explicit, state,
                provider_source, discovered_at FROM releases WHERE id = ?1",
        params![id],
        |row| {
            Ok(StoredRelease {
                identity: ReleaseIdentity {
                    id: row.get(0)?,
                    title: row.get(2)?,
                    normalized_title: row.get(3)?,
                    release_type: row.get(4)?,
                    release_date: row.get(5)?,
                    deezer_album_id: row.get(6)?,
                    mb_release_group_id: row.get(7)?,
                },
                artist_id: row.get(1)?,
                cover_url: row.get(8)?,
                cover_local_path: row.get(9)?,
                external_url: row.get(10)?,
                musicbrainz_url: row.get(11)?,
                track_count: row.get(12)?,
                explicit: row.get::<_, i32>(13)? != 0,
                state: row.get(14)?,
                provider_source: row.get(15)?,
                discovered_at: row.get(16)?,
            })
        },
    )
    .map_err(CoreError::from)
}

fn state_priority(state: &str) -> Option<u8> {
    match state {
        "unseen" => Some(0),
        "seen" => Some(1),
        "dismissed" => Some(2),
        _ => None,
    }
}

/// Consolidate an already-matched pair atomically. The older row ID survives;
/// conflicting track titles or unknown states leave both rows untouched.
fn consolidate_release_pair(
    tx: &Transaction<'_>,
    first_id: i64,
    second_id: i64,
) -> CoreResult<Option<i64>> {
    let survivor_id = first_id.min(second_id);
    let donor_id = first_id.max(second_id);
    let survivor = stored_release(tx, survivor_id)?;
    let donor = stored_release(tx, donor_id)?;
    if survivor.artist_id != donor.artist_id || survivor.identity.conflicts_with(&donor.identity) {
        return Ok(None);
    }
    let (Some(survivor_state), Some(donor_state)) = (
        state_priority(&survivor.state),
        state_priority(&donor.state),
    ) else {
        return Ok(None);
    };

    // Mismatching titles, artist credits or substantially different lengths at
    // the same position can mean different recordings or editions. In that
    // case there is no safe union, even if the release titles happen to match.
    let tracks_compatible = {
        let mut stmt = tx.prepare(
            "SELECT a.title, b.title, a.duration_secs, b.duration_secs,
                    a.artist_credit, b.artist_credit
             FROM release_tracks a JOIN release_tracks b
             ON a.disc_number = b.disc_number AND a.track_number = b.track_number
             WHERE a.release_id = ?1 AND b.release_id = ?2",
        )?;
        let pairs = stmt.query_map(params![survivor_id, donor_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i32>(2)?,
                row.get::<_, i32>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<String>>(5)?,
            ))
        })?;
        let mut compatible = true;
        for pair in pairs {
            let (left, right, left_duration, right_duration, left_credit, right_credit) = pair?;
            let title_conflict = !left.trim().is_empty()
                && !right.trim().is_empty()
                && fold_diacritics_and_symbols(&left) != fold_diacritics_and_symbols(&right);
            let duration_conflict = left_duration > 0
                && right_duration > 0
                && left_duration.abs_diff(right_duration) > 10;
            let credit_conflict = matches!(
                (left_credit.as_deref(), right_credit.as_deref()),
                (Some(a), Some(b)) if !a.trim().is_empty() && !b.trim().is_empty()
                    && fold_diacritics_and_symbols(a) != fold_diacritics_and_symbols(b)
            );
            if title_conflict || duration_conflict || credit_conflict {
                compatible = false;
                break;
            }
        }
        compatible
    };
    if !tracks_compatible {
        return Ok(None);
    }

    let mb_group = survivor
        .identity
        .mb_release_group_id
        .as_deref()
        .or(donor.identity.mb_release_group_id.as_deref());
    let survivor_owned = release_owned_locally(
        tx,
        survivor.artist_id,
        &survivor.identity.normalized_title,
        mb_group,
    )?;
    let donor_owned = release_owned_locally(
        tx,
        donor.artist_id,
        &donor.identity.normalized_title,
        mb_group,
    )?;
    let title_from = if !survivor_owned && donor_owned {
        &donor.identity
    } else {
        &survivor.identity
    };
    let release_type = if survivor.identity.mb_release_group_id.is_none()
        && donor.identity.mb_release_group_id.is_some()
    {
        &donor.identity.release_type
    } else {
        &survivor.identity.release_type
    };
    let release_date = if !donor.identity.release_date.is_empty()
        && donor.identity.release_date.len() > survivor.identity.release_date.len()
    {
        &donor.identity.release_date
    } else {
        &survivor.identity.release_date
    };

    // Prefer Deezer's cover when present, and carry its matching cached path.
    let donor_has_better_cover = donor
        .cover_url
        .as_deref()
        .is_some_and(|url| !url.is_empty())
        && (survivor.cover_url.as_deref().is_none_or(str::is_empty)
            || (survivor.identity.deezer_album_id.is_none()
                && donor.identity.deezer_album_id.is_some()));
    let (cover, alternate_cover) = if donor_has_better_cover {
        (&donor, &survivor)
    } else {
        (&survivor, &donor)
    };
    let cover_path = cover.cover_local_path.as_deref().or_else(|| {
        (cover.cover_url == alternate_cover.cover_url)
            .then_some(alternate_cover.cover_local_path.as_deref())
            .flatten()
    });
    let external_url = if survivor.identity.deezer_album_id.is_none()
        && donor.identity.deezer_album_id.is_some()
    {
        donor
            .external_url
            .as_deref()
            .or(survivor.external_url.as_deref())
    } else {
        survivor
            .external_url
            .as_deref()
            .or(donor.external_url.as_deref())
    };
    let state = if donor_state > survivor_state {
        &donor.state
    } else {
        &survivor.state
    };
    let discovered_at = survivor.discovered_at.as_str().min(&donor.discovered_at);
    let provider = merge_provider_labels(&survivor.provider_source, &donor.provider_source);

    tx.execute(
        "UPDATE releases SET title = ?1, normalized_title = ?2, release_type = ?3,
            release_date = ?4, deezer_album_id = ?5, mb_release_group_id = ?6,
            cover_url = ?7, cover_local_path = ?8, external_url = ?9, musicbrainz_url = ?10,
            track_count = ?11, explicit = ?12, owned_locally = ?13, state = ?14,
            provider_source = ?15, discovered_at = ?16, updated_at = ?17
         WHERE id = ?18",
        params![
            title_from.title,
            title_from.normalized_title,
            release_type,
            release_date,
            survivor
                .identity
                .deezer_album_id
                .or(donor.identity.deezer_album_id),
            mb_group,
            cover.cover_url,
            cover_path,
            external_url,
            survivor
                .musicbrainz_url
                .as_deref()
                .or(donor.musicbrainz_url.as_deref()),
            survivor.track_count.max(donor.track_count),
            i32::from(survivor.explicit || donor.explicit),
            i32::from(survivor_owned || donor_owned),
            state,
            provider,
            discovered_at,
            now_rfc3339_utc(),
            survivor_id
        ],
    )?;

    // Enrich overlapping positions, then move all remaining track rows without
    // changing their IDs. The earlier compatibility check prevents mismatched
    // tracklists from being blended together.
    tx.execute(
        "UPDATE release_tracks AS dst SET
            title = CASE WHEN dst.title = '' THEN COALESCE((
                SELECT src.title FROM release_tracks src WHERE src.release_id = ?2
                  AND src.disc_number = dst.disc_number AND src.track_number = dst.track_number
            ), dst.title) ELSE dst.title END,
            duration_secs = MAX(dst.duration_secs, COALESCE((
                SELECT src.duration_secs FROM release_tracks src WHERE src.release_id = ?2
                  AND src.disc_number = dst.disc_number AND src.track_number = dst.track_number
            ), 0)),
            preview_url = COALESCE(NULLIF(dst.preview_url, ''), (
                SELECT NULLIF(src.preview_url, '') FROM release_tracks src WHERE src.release_id = ?2
                  AND src.disc_number = dst.disc_number AND src.track_number = dst.track_number
            )),
            artist_credit = COALESCE(NULLIF(dst.artist_credit, ''), (
                SELECT NULLIF(src.artist_credit, '') FROM release_tracks src WHERE src.release_id = ?2
                  AND src.disc_number = dst.disc_number AND src.track_number = dst.track_number
            ))
         WHERE dst.release_id = ?1",
        params![survivor_id, donor_id],
    )?;
    tx.execute(
        "UPDATE release_tracks SET release_id = ?1 WHERE release_id = ?2
         AND NOT EXISTS (SELECT 1 FROM release_tracks dst WHERE dst.release_id = ?1
             AND dst.disc_number = release_tracks.disc_number
             AND dst.track_number = release_tracks.track_number)",
        params![survivor_id, donor_id],
    )?;
    tx.execute(
        "DELETE FROM release_tracks WHERE release_id = ?1",
        params![donor_id],
    )?;
    tx.execute("DELETE FROM releases WHERE id = ?1", params![donor_id])?;
    Ok(Some(survivor_id))
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
