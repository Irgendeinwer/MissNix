use std::collections::HashSet;

use rusqlite::Connection;

use crate::error::CoreResult;

pub(super) const SCHEMA_VERSION: i32 = 2;

pub(super) const CANONICAL_DDL: &str = "
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

pub(super) fn is_canonical_schema_valid(conn: &Connection) -> CoreResult<bool> {
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
