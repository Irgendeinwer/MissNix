use rusqlite::{
    OptionalExtension, ToSql, Transaction, TransactionBehavior, params, params_from_iter,
};

use crate::error::{CoreError, CoreResult};
use crate::models::{Artist, ArtistSource, ArtistStatus, LocalAlbum};
use crate::normalize::{normalize_artist_name, now_rfc3339_utc};

use super::{Database, escape_sql_like};

impl Database {
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
            if norm.is_empty() {
                sql.push_str(" AND LOWER(a.name) LIKE ? ESCAPE '\\'");
                bind_vals.push(Box::new(like_raw));
            } else {
                let like_norm = format!("%{}%", escape_sql_like(&norm));
                sql.push_str(
                    " AND (LOWER(a.name) LIKE ? ESCAPE '\\' OR a.normalized_name LIKE ? ESCAPE '\\')",
                );
                bind_vals.push(Box::new(like_raw));
                bind_vals.push(Box::new(like_norm));
            }
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

        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let existing: Option<(Option<String>, Option<i64>)> = tx
            .query_row(
                "SELECT mbid, deezer_id FROM artists WHERE normalized_name = ?1",
                params![norm],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if let Some((old_mbid, old_deezer)) = existing {
            let same_mb = clean_mbid.is_some() && clean_mbid == old_mbid.as_deref();
            let same_dz = deezer_id.is_some() && deezer_id == old_deezer;
            let has_old_id = old_mbid.is_some() || old_deezer.is_some();
            let has_new_id = clean_mbid.is_some() || deezer_id.is_some();
            let conflicting_mb = clean_mbid.is_some() && old_mbid.is_some() && !same_mb;
            let conflicting_dz = deezer_id.is_some() && old_deezer.is_some() && !same_dz;
            if conflicting_mb
                || conflicting_dz
                || (has_old_id && has_new_id && !same_mb && !same_dz)
            {
                return Err(CoreError::Validation(format!(
                    "An artist named '{clean_name}' already has different provider IDs. \
                     Edit that artist's IDs explicitly instead of merging by name."
                )));
            }
        }
        tx.execute(
            "INSERT INTO artists (
                name, normalized_name, mbid, deezer_id, disambiguation,
                avatar_url, external_url, status, source, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'tracked', 'manual', ?8, ?8)
             ON CONFLICT(normalized_name) DO UPDATE SET
                status = 'tracked',
                source = 'manual',
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

        let id: i64 = tx.query_row(
            "SELECT id FROM artists WHERE normalized_name = ?1",
            params![norm],
            |r| r.get(0),
        )?;
        tx.commit()?;
        self.get_artist(id)?.ok_or_else(|| {
            CoreError::Validation("Failed to load newly inserted artist".to_string())
        })
    }

    pub fn set_artist_status(&self, artist_id: i64, status: ArtistStatus) -> CoreResult<()> {
        let now = now_rfc3339_utc();
        let updated = self.conn.execute(
            "UPDATE artists SET
                source = CASE WHEN ?1 = 'tracked' AND source = 'local' AND local_track_count = 0
                              THEN 'manual' ELSE source END,
                status = ?1, updated_at = ?2 WHERE id = ?3",
            params![status.as_str(), now, artist_id],
        )?;
        if updated == 0 {
            return Err(CoreError::Validation("Artist no longer exists".to_string()));
        }
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
