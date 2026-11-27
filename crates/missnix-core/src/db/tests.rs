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
fn refuses_legacy_schema_without_modifying_any_data() {
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

    // Incompatible databases must not be guessed at and rebuilt in place.
    let error = match Database::open(&db_path) {
        Ok(_) => panic!("incompatible database unexpectedly opened"),
        Err(err) => err,
    };
    assert!(error.to_string().contains("No migration was performed"));
    let raw = Connection::open(&db_path).unwrap();
    let note: String = raw
        .query_row("SELECT note FROM legacy_sync_log WHERE id = 1", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(note, "old run");
    let seen: i64 = raw
        .query_row("SELECT seen FROM releases WHERE id = 200", [], |r| r.get(0))
        .unwrap();
    assert_eq!(seen, 1);
    let name: String = raw
        .query_row("SELECT name FROM artists WHERE id = 10", [], |r| r.get(0))
        .unwrap();
    assert_eq!(name, "Boards of Canada");
    let value: String = raw
        .query_row(
            "SELECT value FROM settings WHERE key = 'lookback_days'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(value, "180");
    drop(raw);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn retires_stale_local_artists_without_losing_history_and_persists_settings() {
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
    assert!(names.contains(&"Alligatoah, Jamal"));
    assert_eq!(
        all.iter()
            .find(|a| a.name == "Alligatoah, Jamal")
            .unwrap()
            .status,
        ArtistStatus::Missing
    );

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
    use crate::scanner::scan_library_folders_with_options;

    let dir = unique_test_dir("partial_schema_unmounted");
    let db_path = dir.join("missnix.db");

    // A canonical settings table can be pre-seeded before the first launch.
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

    let music = dir.join("music");
    let album = music.join("Boards of Canada/Geogaddi");
    std::fs::create_dir_all(&album).unwrap();
    std::fs::write(album.join("01.flac"), b"placeholder").unwrap();
    db.add_library_folder(music.to_str().unwrap()).unwrap();
    let snapshot = db.prepare_library_scan().unwrap();
    let populated =
        scan_library_folders_with_options(&snapshot.folders, &snapshot.options, None, |_, _| {})
            .unwrap();
    db.apply_scan_outcome_checked(&populated, &snapshot, false, None)
        .unwrap();
    assert_eq!(db.library_stats().unwrap().local_albums, 1);
    let artist = db
        .list_artists("Boards of Canada", None, false)
        .unwrap()
        .remove(0);
    let (release_id, _, owned) = db
        .upsert_discovered_release(&test_release(artist.id, "Geogaddi", "deezer"))
        .unwrap();
    assert!(owned);

    // A missing mount cannot produce a complete scan, and a synthetic empty
    // outcome cannot be applied against the configured folder snapshot either.
    std::fs::remove_dir_all(&music).unwrap();
    let snapshot = db.prepare_library_scan().unwrap();
    assert!(
        scan_library_folders_with_options(&snapshot.folders, &snapshot.options, None, |_, _| {})
            .is_err()
    );
    assert!(
        db.apply_scan_outcome_checked(&ScanOutcome::default(), &snapshot, false, None)
            .is_err()
    );

    // An empty mount point is also rejected unless the user confirms the change.
    std::fs::create_dir_all(&music).unwrap();
    let empty =
        scan_library_folders_with_options(&snapshot.folders, &snapshot.options, None, |_, _| {})
            .unwrap();
    assert!(
        db.apply_scan_outcome_checked(&empty, &snapshot, false, None)
            .is_err()
    );
    assert_eq!(db.library_stats().unwrap().local_albums, 1);
    assert!(db.get_release(release_id).unwrap().unwrap().owned_locally);

    let report = crate::models::HeadlessReport {
        db_path: db_path.to_string_lossy().to_string(),
        status: Some(db.library_stats().unwrap()),
        ..crate::models::HeadlessReport::default()
    };
    let json = report.to_json_pretty().unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed["status"]["local_albums"], 1);
    assert!(parsed.get("scan").is_none());
    assert!(parsed.get("check").is_none());

    db.apply_scan_outcome_checked(&empty, &snapshot, true, None)
        .unwrap();
    assert_eq!(db.library_stats().unwrap().local_albums, 0);
    assert_eq!(
        db.get_artist(artist.id).unwrap().unwrap().status,
        ArtistStatus::Missing
    );
    assert!(!db.get_release(release_id).unwrap().unwrap().owned_locally);
    drop(db);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn theme_env_defaults_apply_before_saved_settings() {
    const CHILD: &str = "MISSNIX_THEME_DEFAULTS_TEST_CHILD";
    if std::env::var_os(CHILD).is_some() {
        let mut db = Database::open_in_memory().unwrap();
        let defaults = db.load_settings().unwrap();
        assert!(!defaults.dark_mode);
        assert_eq!(defaults.accent_preset, crate::palette::CUSTOM_ACCENT_PRESET);
        assert_eq!(defaults.custom_accent_hex, "#8A8079");

        let mut saved = defaults;
        saved.dark_mode = true;
        saved.accent_preset = 2;
        saved.custom_accent_hex = "#112233".to_string();
        db.save_settings(&saved).unwrap();
        let loaded = db.load_settings().unwrap();
        assert!(loaded.dark_mode);
        assert_eq!(loaded.accent_preset, 2);
        assert_eq!(loaded.custom_accent_hex, "#112233");
        return;
    }

    // Environment changes are confined to a child process so the other tests
    // can safely run in parallel without sharing mutable process-wide state.
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "db::tests::theme_env_defaults_apply_before_saved_settings",
            "--nocapture",
        ])
        .env(CHILD, "1")
        .env("MISSNIX_DEFAULT_DARK_MODE", "false")
        .env("MISSNIX_DEFAULT_ACCENT_HEX", "#8a8079")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "theme defaults subprocess failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
}

#[test]
fn refuses_unsafe_partial_databases_without_modifying_them() {
    let dir = unique_test_dir("partial_schema_refusal");
    for (filename, extra_sql) in [
        (
            "extra.db",
            "CREATE TABLE notes (text TEXT); INSERT INTO notes VALUES ('keep');",
        ),
        ("future.db", "PRAGMA user_version = 999;"),
        ("other-app.db", "PRAGMA application_id = 123456;"),
    ] {
        let path = dir.join(filename);
        let raw = Connection::open(&path).unwrap();
        raw.execute_batch(
            "CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             INSERT INTO settings VALUES ('important', 'keep');",
        )
        .unwrap();
        raw.execute_batch(extra_sql).unwrap();
        drop(raw);

        assert!(Database::open(&path).is_err(), "accepted {filename}");
        let raw = Connection::open(&path).unwrap();
        let value: String = raw
            .query_row(
                "SELECT value FROM settings WHERE key = 'important'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(value, "keep");
        let artists_tables: i64 = raw
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'artists'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(artists_tables, 0, "modified {filename}");
        if filename == "extra.db" {
            let note: String = raw
                .query_row("SELECT text FROM notes", [], |r| r.get(0))
                .unwrap();
            assert_eq!(note, "keep");
        }
    }
    std::fs::remove_dir_all(dir).unwrap();
}

fn unique_test_dir(label: &str) -> std::path::PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "missnix_db_{label}_{}_{}",
        std::process::id(),
        nanos
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn test_release(artist_id: i64, title: &str, source: &str) -> DiscoveredReleaseInput {
    DiscoveredReleaseInput {
        artist_id,
        title: title.to_string(),
        release_type: ReleaseType::Album,
        release_date: "2026-01-01".to_string(),
        deezer_album_id: None,
        mb_release_group_id: None,
        cover_url: None,
        external_url: None,
        musicbrainz_url: None,
        track_count: 1,
        explicit: false,
        provider_source: source.to_string(),
    }
}

#[test]
fn refuses_unrelated_and_newer_databases_without_destroying_wal_data() {
    let dir = unique_test_dir("unknown");
    let path = dir.join("other.db");
    let raw = Connection::open(&path).unwrap();
    raw.execute_batch(
        "PRAGMA journal_mode=WAL;
         CREATE TABLE important_notes(id INTEGER PRIMARY KEY, note TEXT NOT NULL);
         INSERT INTO important_notes VALUES(42, 'keep this');
         PRAGMA user_version=91;",
    )
    .unwrap();
    let err = match Database::open(&path) {
        Ok(_) => panic!("unrelated database unexpectedly opened"),
        Err(err) => err,
    };
    assert!(err.to_string().contains("No migration was performed"));
    let note: String = raw
        .query_row("SELECT note FROM important_notes WHERE id=42", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(note, "keep this");
    let version: i32 = raw
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(version, 91);
    drop(raw);

    let future = dir.join("future.db");
    {
        let db = Database::open(&future).unwrap();
        db.add_manual_artist("Preserve Me", None, None, None, None, None)
            .unwrap();
    }
    let raw = Connection::open(&future).unwrap();
    raw.execute_batch("PRAGMA user_version=999;").unwrap();
    drop(raw);
    assert!(Database::open(&future).is_err());
    let raw = Connection::open(&future).unwrap();
    let name: String = raw
        .query_row("SELECT name FROM artists", [], |r| r.get(0))
        .unwrap();
    assert_eq!(name, "Preserve Me");
    drop(raw);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn empty_mount_and_stale_scans_do_not_erase_library_history() {
    use crate::scanner::scan_library_folders_with_options;

    let dir = unique_test_dir("scan_safety");
    let first = dir.join("first");
    let second = dir.join("second");
    for (root, artist) in [(&first, "Alpha"), (&second, "Beta")] {
        let album = root.join(artist).join("Album");
        std::fs::create_dir_all(&album).unwrap();
        std::fs::write(album.join("01.flac"), b"placeholder").unwrap();
    }
    let mut db = Database::open(&dir.join("library.db")).unwrap();
    db.add_library_folder(first.to_str().unwrap()).unwrap();
    let second_folder = db.add_library_folder(second.to_str().unwrap()).unwrap();
    let snapshot = db.prepare_library_scan().unwrap();
    let outcome =
        scan_library_folders_with_options(&snapshot.folders, &snapshot.options, None, |_, _| {})
            .unwrap();
    db.apply_scan_outcome_checked(&outcome, &snapshot, false, None)
        .unwrap();
    let beta = db.list_artists("Beta", None, false).unwrap().remove(0);
    let (release_id, _, owned) = db
        .upsert_discovered_release(&test_release(beta.id, "Album", "deezer"))
        .unwrap();
    assert!(owned);
    db.set_release_state(release_id, ReleaseState::Dismissed)
        .unwrap();
    db.replace_release_tracks(
        release_id,
        &[DiscoveredTrackInput {
            disc_number: 1,
            track_number: 1,
            title: "Keep me".to_string(),
            duration_secs: 30,
            preview_url: None,
            artist_credit: None,
        }],
    )
    .unwrap();

    // Simulate an unmounted drive whose mount point still exists as an empty directory.
    std::fs::remove_dir_all(&second).unwrap();
    std::fs::create_dir_all(&second).unwrap();
    let snapshot = db.prepare_library_scan().unwrap();
    let empty =
        scan_library_folders_with_options(&snapshot.folders, &snapshot.options, None, |_, _| {})
            .unwrap();
    let error = db
        .apply_scan_outcome_checked(&empty, &snapshot, false, None)
        .unwrap_err();
    assert!(error.to_string().contains("previously contained audio"));
    assert_eq!(
        db.get_artist(beta.id).unwrap().unwrap().local_track_count,
        1
    );
    assert!(db.get_release(release_id).unwrap().unwrap().owned_locally);
    assert_eq!(
        db.list_library_folders()
            .unwrap()
            .iter()
            .find(|f| f.id == second_folder.id)
            .unwrap()
            .track_count,
        1
    );
    assert_eq!(db.list_release_tracks(release_id).unwrap().len(), 1);

    // Explicit confirmation may accept an intentionally emptied folder, but never
    // deletes the artist's release state or cached tracklist.
    db.apply_scan_outcome_checked(&empty, &snapshot, true, None)
        .unwrap();
    assert_eq!(
        db.get_artist(beta.id).unwrap().unwrap().status,
        ArtistStatus::Missing
    );
    assert_eq!(
        db.get_release(release_id).unwrap().unwrap().state,
        ReleaseState::Dismissed
    );
    assert_eq!(db.list_release_tracks(release_id).unwrap().len(), 1);

    // When files reappear, the same artist and release IDs/history return.
    let album = second.join("Beta/Album");
    std::fs::create_dir_all(&album).unwrap();
    std::fs::write(album.join("01.flac"), b"placeholder").unwrap();
    let snapshot = db.prepare_library_scan().unwrap();
    let restored =
        scan_library_folders_with_options(&snapshot.folders, &snapshot.options, None, |_, _| {})
            .unwrap();
    db.apply_scan_outcome_checked(&restored, &snapshot, false, None)
        .unwrap();
    assert_eq!(
        db.get_artist(beta.id).unwrap().unwrap().status,
        ArtistStatus::Tracked
    );
    assert!(db.get_release(release_id).unwrap().unwrap().owned_locally);
    assert_eq!(
        db.get_release(release_id).unwrap().unwrap().state,
        ReleaseState::Dismissed
    );
    drop(db);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn scan_revision_rejects_changed_folders_options_and_out_of_order_results() {
    use crate::scanner::scan_library_folders_with_options;
    use std::sync::atomic::AtomicBool;

    let dir = unique_test_dir("revision");
    let music = dir.join("music");
    std::fs::create_dir_all(music.join("Example/Album")).unwrap();
    std::fs::write(music.join("Example/Album/01.flac"), b"placeholder").unwrap();
    let db_path = dir.join("library.db");
    let mut db = Database::open(&db_path).unwrap();
    let folder = db.add_library_folder(music.to_str().unwrap()).unwrap();
    let mut other = Database::open(&db_path).unwrap();

    let old = db.prepare_library_scan().unwrap();
    let outcome =
        scan_library_folders_with_options(&old.folders, &old.options, None, |_, _| {}).unwrap();
    let mut settings = other.load_settings().unwrap();
    settings.split_collaborations = !settings.split_collaborations;
    other.save_settings(&settings).unwrap();
    assert!(
        db.apply_scan_outcome_checked(&outcome, &old, false, None)
            .is_err()
    );
    assert!(db.list_artists("", None, false).unwrap().is_empty());

    let first = db.prepare_library_scan().unwrap();
    let second = other.prepare_library_scan().unwrap();
    let outcome =
        scan_library_folders_with_options(&first.folders, &first.options, None, |_, _| {}).unwrap();
    let cancelled = AtomicBool::new(true);
    assert!(matches!(
        db.apply_scan_outcome_checked(&outcome, &first, false, Some(&cancelled)),
        Err(CoreError::Cancelled)
    ));
    db.apply_scan_outcome_checked(&outcome, &first, false, None)
        .unwrap();
    assert!(
        other
            .apply_scan_outcome_checked(&outcome, &second, false, None)
            .is_err()
    );

    // Remove the last folder, reconcile deliberately empty, then ensure a worker
    // with the old snapshot cannot put the removed library back.
    let stale = db.prepare_library_scan().unwrap();
    db.remove_library_folder(folder.id).unwrap();
    let empty = db.prepare_library_scan().unwrap();
    db.apply_scan_outcome_checked(&ScanOutcome::default(), &empty, false, None)
        .unwrap();
    assert!(
        other
            .apply_scan_outcome_checked(&outcome, &stale, false, None)
            .is_err()
    );
    let artist = db.list_artists("Example", None, false).unwrap().remove(0);
    assert_eq!(artist.status, ArtistStatus::Missing);
    assert_eq!(artist.local_track_count, 0);

    // Explicitly tracking a missing artist converts it into a manual favorite.
    db.set_artist_status(artist.id, ArtistStatus::Tracked)
        .unwrap();
    db.apply_scan_outcome(&ScanOutcome::default()).unwrap();
    let tracked = db.get_artist(artist.id).unwrap().unwrap();
    assert_eq!(tracked.status, ArtistStatus::Tracked);
    assert_eq!(tracked.source, crate::models::ArtistSource::Manual);
    drop(other);
    drop(db);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn release_updates_keep_ownership_and_prefer_deezer_cover() {
    let mut db = Database::open_in_memory().unwrap();
    let artist = db
        .add_manual_artist("Example", None, None, None, None, None)
        .unwrap();
    let mut albums = HashMap::new();
    albums.insert(
        "local name".to_string(),
        DiscoveredAlbumRecord {
            title: "Local Name".to_string(),
            normalized_title: "local name".to_string(),
            release_year: None,
            mb_release_group_id: Some("group-1".to_string()),
            mb_album_id: None,
            track_count: 1,
            sample_path: "/music/file.flac".to_string(),
        },
    );
    let mut outcome = ScanOutcome::default();
    outcome.artists.insert(
        "example".to_string(),
        DiscoveredArtistRecord {
            display_name: "Example".to_string(),
            normalized_name: "example".to_string(),
            mbid: None,
            track_count: 1,
            albums,
        },
    );
    db.apply_scan_outcome(&outcome).unwrap();
    let mut mb = test_release(artist.id, "Different Name", "musicbrainz");
    mb.mb_release_group_id = Some("group-1".to_string());
    mb.cover_url = Some("https://coverartarchive.org/speculative".to_string());
    let (id, _, owned) = db.upsert_discovered_release(&mb).unwrap();
    assert!(owned);
    let mut dz = test_release(artist.id, "Different Name", "deezer");
    dz.deezer_album_id = Some(99);
    dz.cover_url = Some("https://cdn.deezer.com/real.jpg".to_string());
    assert!(db.upsert_discovered_release(&dz).unwrap().2);
    assert!(db.upsert_discovered_release(&mb).unwrap().2);
    let release = db.get_release(id).unwrap().unwrap();
    assert!(release.owned_locally);
    assert_eq!(
        release.cover_url.as_deref(),
        Some("https://cdn.deezer.com/real.jpg")
    );
}

#[test]
fn concurrent_database_opens_initialize_and_reopen_safely() {
    use std::sync::{Arc, Barrier};

    let dir = unique_test_dir("concurrent_open");
    let existing_path = dir.join("existing.db");
    let existing = Database::open(&existing_path).unwrap();
    existing
        .add_manual_artist("Persisted Artist", None, None, None, None, None)
        .unwrap();

    // Exercise both simultaneous first-time WAL setup and simultaneous schema
    // validation/migration on an existing database. Wait before the fallible open,
    // so a failure cannot strand the other worker on a barrier.
    for round in 0..32 {
        let fresh_path = dir.join(format!("fresh-{round}.db"));
        for (path, expected_artists) in [(&fresh_path, 0), (&existing_path, 1)] {
            let barrier = Arc::new(Barrier::new(2));
            let mut joins = Vec::new();
            for _ in 0..2 {
                let path = path.clone();
                let barrier = barrier.clone();
                joins.push(std::thread::spawn(move || {
                    barrier.wait();
                    Database::open(&path)?.library_stats()
                }));
            }
            for join in joins {
                let stats = join.join().unwrap().unwrap();
                assert_eq!(stats.total_artists, expected_artists);
            }
        }
    }
    assert_eq!(existing.library_stats().unwrap().total_artists, 1);
    drop(existing);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn wal_switch_retry_is_bounded_when_another_connection_holds_a_lock() {
    let dir = unique_test_dir("wal_busy");
    let path = dir.join("busy.db");
    let blocker = Connection::open(&path).unwrap();
    let waiting = Connection::open(&path).unwrap();
    waiting.busy_timeout(Duration::ZERO).unwrap();
    blocker
        .execute_batch("CREATE TABLE marker (value TEXT); BEGIN EXCLUSIVE;")
        .unwrap();

    let err = enable_wal_with_retry(&waiting, Duration::from_millis(25)).unwrap_err();
    assert!(matches!(
        err,
        CoreError::Database(ref e) if e.sqlite_error_code() == Some(ErrorCode::DatabaseBusy)
    ));
    blocker.execute_batch("ROLLBACK;").unwrap();
    enable_wal_with_retry(&waiting, Duration::from_millis(25)).unwrap();
    drop(waiting);
    drop(blocker);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn concurrent_release_upserts_do_not_create_duplicate_rows() {
    use std::sync::{Arc, Barrier};
    let dir = unique_test_dir("concurrent_upsert");
    let db_path = dir.join("library.db");
    let db = Database::open(&db_path).unwrap();
    let artist = db
        .add_manual_artist("Example", None, None, None, None, None)
        .unwrap();
    // Open both connections before spawning: if an open fails, no worker is left
    // waiting forever at the upsert barrier.
    let connections = (0..2)
        .map(|_| Database::open(&db_path).unwrap())
        .collect::<Vec<_>>();
    let barrier = Arc::new(Barrier::new(2));
    let mut joins = Vec::new();
    for worker_db in connections {
        let barrier = barrier.clone();
        let input = test_release(artist.id, "Same Release", "deezer");
        joins.push(std::thread::spawn(move || {
            barrier.wait();
            worker_db.upsert_discovered_release(&input).unwrap()
        }));
    }
    let first = joins.remove(0).join().unwrap();
    let second = joins.remove(0).join().unwrap();
    assert_eq!(first.0, second.0);
    assert_ne!(first.1, second.1);
    let count: i64 = db
        .conn
        .query_row("SELECT COUNT(*) FROM releases", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 1);
    drop(db);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn mark_filtered_seen_does_not_touch_other_releases() {
    let mut db = Database::open_in_memory().unwrap();
    let artist = db
        .add_manual_artist("Example", None, None, None, None, None)
        .unwrap();
    let visible = db
        .upsert_discovered_release(&test_release(artist.id, "Visible", "deezer"))
        .unwrap()
        .0;
    let hidden = db
        .upsert_discovered_release(&test_release(artist.id, "Hidden", "deezer"))
        .unwrap()
        .0;
    let selected = db
        .query_releases(&FeedQuery {
            search: "Visible".to_string(),
            include_albums: true,
            ..FeedQuery::default()
        })
        .unwrap();
    assert_eq!(selected.len(), 1);
    let ids: Vec<i64> = selected.iter().map(|release| release.id).collect();
    assert_eq!(db.mark_releases_seen(&ids).unwrap(), 1);
    assert_eq!(
        db.get_release(visible).unwrap().unwrap().state,
        ReleaseState::Seen
    );
    assert_eq!(
        db.get_release(hidden).unwrap().unwrap().state,
        ReleaseState::Unseen
    );
}

#[test]
fn manual_search_does_not_overwrite_a_different_same_name_identity() {
    let db = Database::open_in_memory().unwrap();
    let artist = db
        .add_manual_artist("Same", None, Some(123), None, None, None)
        .unwrap();
    assert!(
        db.add_manual_artist("Same", None, Some(456), None, None, None)
            .is_err()
    );
    assert!(
        db.add_manual_artist(
            "Same",
            Some("11111111-1111-1111-1111-111111111111"),
            None,
            None,
            None,
            None
        )
        .is_err()
    );
    assert_eq!(
        db.get_artist(artist.id).unwrap().unwrap().deezer_id,
        Some(123)
    );
    assert_eq!(db.get_artist(artist.id).unwrap().unwrap().mbid, None);
    assert_eq!(
        db.add_manual_artist("Same", None, Some(123), None, None, None)
            .unwrap()
            .id,
        artist.id
    );
}

#[test]
fn stale_provider_checks_cannot_overwrite_ids_or_add_wrong_releases() {
    let db = Database::open_in_memory().unwrap();
    let stale = db
        .add_manual_artist("Example", None, Some(1), None, None, None)
        .unwrap();
    db.override_artist_ids(stale.id, None, Some(2)).unwrap();
    assert!(
        db.update_artist_provider_links(&stale, None, Some(1), None, None, None)
            .is_err()
    );
    let release = test_release(stale.id, "Wrong Artist's Album", "deezer");
    assert!(
        db.upsert_discovered_release_if_ids_match(&release, None, Some(1))
            .is_err()
    );
    assert!(
        db.mark_artist_checked_if_ids_match(stale.id, None, Some(1))
            .is_err()
    );
    let current = db.get_artist(stale.id).unwrap().unwrap();
    assert_eq!(current.deezer_id, Some(2));
    assert_eq!(current.last_checked_at, None);
    assert_eq!(current.discovered_release_count, 0);
}
