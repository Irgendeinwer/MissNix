use std::collections::HashMap;

use crate::models::{AppSettings, ArtistDiscoveryMode, ArtistStatus, FeedQuery, ReleaseState};
use crate::normalize::now_rfc3339_utc;
use crate::scanner::{DiscoveredAlbumRecord, DiscoveredArtistRecord, ScanOutcome};

use super::*;

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
        accent_preset: 7,
        custom_accent_hex: "#ea6c34".to_string(),
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
    assert_eq!(loaded.accent_preset, 7);
    assert_eq!(loaded.custom_accent_hex, "#EA6C34");
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

#[test]
fn preserves_local_albums_on_unmounted_scan_and_completes_partial_settings_schema() {
    let dir = std::env::temp_dir().join(format!(
        "missnix_partial_schema_test_{}_{}",
        std::process::id(),
        now_rfc3339_utc().replace([':', '-'], "")
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let db_path = dir.join("missnix.db");

    // Pre-seed only the canonical `settings` table before MissNix ever opens the DB.
    {
        let raw = Connection::open(&db_path).unwrap();
        raw.execute_batch(
            "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             INSERT INTO settings (key, value) VALUES ('custom_accent_hex', '#E9B143');",
        )
        .unwrap();
    }

    let mut db = Database::open(&db_path).unwrap();
    assert!(!dir.join("missnix.db.bak").exists());
    assert_eq!(db.load_settings().unwrap().custom_accent_hex, "#E9B143");

    // Verify env theme defaults apply when unset and normalize `#RRGGBB` to preset 7.
    let mut env_defaults = AppSettings::default();
    env_defaults.apply_env_theme_defaults(Some("false"), Some("#e9b143"));
    assert!(!env_defaults.dark_mode);
    assert_eq!(
        env_defaults.accent_preset,
        crate::palette::CUSTOM_ACCENT_PRESET
    );
    assert_eq!(env_defaults.custom_accent_hex, "#E9B143");

    // Seed a local album, then simulate an unmounted/unsynced folder scan (`folders_scanned = 0`).
    let mut albums = HashMap::new();
    albums.insert(
        "geogaddi".to_string(),
        DiscoveredAlbumRecord {
            title: "Geogaddi".to_string(),
            normalized_title: "geogaddi".to_string(),
            release_year: Some(2002),
            mb_release_group_id: None,
            mb_album_id: None,
            track_count: 23,
            sample_path: "/music/Boards of Canada/Geogaddi/01.flac".to_string(),
        },
    );
    let mut populated = ScanOutcome::default();
    populated.summary.folders_scanned = 1;
    populated.summary.audio_files_seen = 23;
    populated.artists.insert(
        "boards of canada".to_string(),
        DiscoveredArtistRecord {
            display_name: "Boards of Canada".to_string(),
            normalized_name: "boards of canada".to_string(),
            mbid: None,
            track_count: 23,
            albums,
        },
    );
    db.apply_scan_outcome(&populated).unwrap();
    assert_eq!(db.library_stats().unwrap().local_albums, 1);

    let unmounted = ScanOutcome::default();
    db.apply_scan_outcome(&unmounted).unwrap();
    assert_eq!(db.library_stats().unwrap().local_albums, 1);

    let _ = std::fs::remove_dir_all(&dir);
}
