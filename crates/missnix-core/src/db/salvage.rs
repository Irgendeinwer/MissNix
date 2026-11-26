use std::collections::HashMap;

use rusqlite::types::ValueRef;
use rusqlite::{Connection, OptionalExtension, params};

use crate::error::CoreResult;
use crate::models::{ArtistSource, ArtistStatus, ReleaseState, ReleaseType};
use crate::normalize::{normalize_artist_name, normalize_release_title, now_rfc3339_utc};

#[derive(Default)]
pub(super) struct LegacySnapshot {
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

pub(super) fn salvage_legacy_snapshot(conn: &Connection) -> LegacySnapshot {
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

pub(super) fn restore_salvaged_snapshot(
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
