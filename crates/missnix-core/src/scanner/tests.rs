use super::*;
use lofty::config::WriteOptions;
use lofty::tag::{ItemKey, ItemValue, Tag, TagExt, TagItem, TagType};
use std::fs;

fn unique_temp_dir(label: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("missnix_scan_{label}_{nanos}"));
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// Build a minimal valid MPEG1 Layer3 frame buffer and attach an ID3v2 tag via `lofty`.
fn write_tagged_mp3(path: &Path, items: &[(ItemKey, &str)]) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    // Valid silent MPEG-1 Layer III 128kbps 44.1kHz stereo frames (417 bytes per frame)
    let mut frame = vec![0u8; 417];
    frame[0] = 0xFF;
    frame[1] = 0xFB;
    frame[2] = 0x90;
    frame[3] = 0x64;
    let mut mp3_bytes = Vec::with_capacity(417 * 4);
    for _ in 0..4 {
        mp3_bytes.extend_from_slice(&frame);
    }
    fs::write(path, &mp3_bytes).unwrap();

    let mut tag = Tag::new(TagType::Id3v2);
    for (key, val) in items {
        tag.insert(TagItem::new(*key, ItemValue::Text((*val).to_string())));
    }
    tag.save_to_path(path, WriteOptions::default()).unwrap();
}

#[test]
fn extracts_tags_mbids_and_skips_various_artists_compilations() {
    let root = unique_temp_dir("tags");

    // 1. Properly tagged album with AlbumArtist + feat. track + MusicBrainz IDs
    write_tagged_mp3(
        &root.join("messy_dump/track01.mp3"),
        &[
            (ItemKey::AlbumArtist, "Daft Punk"),
            (ItemKey::TrackArtist, "Daft Punk feat. Pharrell Williams"),
            (ItemKey::AlbumTitle, "Random Access Memories"),
            (ItemKey::RecordingDate, "2013-05-17"),
            (
                ItemKey::MusicBrainzReleaseArtistId,
                "056e4f3e-d505-4dad-8ec1-d04f521cbb56",
            ),
            (
                ItemKey::MusicBrainzReleaseGroupId,
                "aa997ea0-2936-40bd-886d-3af8a0e06470",
            ),
        ],
    );

    // 2. Various Artists soundtrack compilation -> skipped by default
    write_tagged_mp3(
        &root.join("OST/03_one_off.mp3"),
        &[
            (ItemKey::AlbumArtist, "Various Artists"),
            (ItemKey::TrackArtist, "One Off Cameo"),
            (ItemKey::AlbumTitle, "Hackers OST"),
            (ItemKey::FlagCompilation, "1"),
        ],
    );

    // 3. Untagged raw audio file inside structured folders with CD subfolder
    let untagged = root
        .join("Boards of Canada")
        .join("1998 - Music Has the Right to Children [FLAC]")
        .join("CD1")
        .join("01 - Wildlife Analysis.flac");
    fs::create_dir_all(untagged.parent().unwrap()).unwrap();
    fs::write(&untagged, b"not_even_valid_flac_frames").unwrap();

    let outcome =
        scan_library_folders(std::slice::from_ref(&root), false, None, |_, _| {}).unwrap();
    assert_eq!(outcome.summary.audio_files_seen, 3);
    assert_eq!(outcome.summary.tags_extracted, 1);
    assert_eq!(outcome.summary.folder_fallbacks, 1);
    assert_eq!(outcome.summary.skipped_compilations, 1);
    assert_eq!(outcome.artists.len(), 2);

    let dp = outcome
        .artists
        .get("daft punk")
        .expect("Daft Punk discovered");
    assert_eq!(dp.display_name, "Daft Punk");
    assert_eq!(
        dp.mbid.as_deref(),
        Some("056e4f3e-d505-4dad-8ec1-d04f521cbb56")
    );
    let ram = dp.albums.get("random access memories").unwrap();
    assert_eq!(ram.release_year, Some(2013));
    assert_eq!(
        ram.mb_release_group_id.as_deref(),
        Some("aa997ea0-2936-40bd-886d-3af8a0e06470")
    );

    let boc = outcome
        .artists
        .get("boards of canada")
        .expect("Boards of Canada discovered from folder fallback");
    let mhtrtc = boc
        .albums
        .get("music has the right to children")
        .expect("Album inferred from folder");
    assert_eq!(mhtrtc.release_year, Some(1998));

    let _ = fs::remove_dir_all(&root);
}

#[test]
fn prevents_multi_artist_tag_pollution_and_supports_folder_only_mode() {
    let root = unique_temp_dir("collab");

    // 1. Structured folder `<Artist>/<Album>/track` with comma-joined TrackArtist credits
    write_tagged_mp3(
        &root.join("Alligatoah/Schlaftabletten, Rotwein V/01_collab.mp3"),
        &[
            (ItemKey::TrackArtist, "Alligatoah, Jamal"),
            (ItemKey::AlbumTitle, "Schlaftabletten, Rotwein V"),
        ],
    );
    write_tagged_mp3(
        &root.join("Alligatoah/Schlaftabletten, Rotwein V/02_collab.mp3"),
        &[
            (ItemKey::TrackArtist, "Alligatoah, Dima Richman"),
            (ItemKey::AlbumTitle, "Schlaftabletten, Rotwein V"),
        ],
    );

    // 2. Semicolon-delimited AlbumArtist tag (`Alottadogs; GLITTERMUTT`)
    write_tagged_mp3(
        &root.join("Singles/split_ep.mp3"),
        &[
            (ItemKey::AlbumArtist, "Alottadogs; GLITTERMUTT"),
            (ItemKey::TrackArtist, "Alottadogs; GLITTERMUTT"),
            (ItemKey::AlbumTitle, "Split Single"),
        ],
    );

    // 3. Comma-delimited TrackArtist in a loose bucket folder (`Alexander Marcus, 257ers`)
    write_tagged_mp3(
        &root.join("Downloads/party.mp3"),
        &[
            (ItemKey::TrackArtist, "Alexander Marcus, 257ers"),
            (ItemKey::AlbumTitle, "Pharao"),
        ],
    );

    // 4. Genuine comma-containing artist (`Tyler, The Creator`) must NOT be split
    write_tagged_mp3(
        &root.join("Tyler, The Creator/IGOR/01_igor.mp3"),
        &[
            (ItemKey::AlbumArtist, "Tyler, The Creator"),
            (ItemKey::TrackArtist, "Tyler, The Creator"),
            (ItemKey::AlbumTitle, "IGOR"),
        ],
    );

    let default_outcome = scan_library_folders_with_options(
        std::slice::from_ref(&root),
        &ScanOptions::default(),
        None,
        |_, _| {},
    )
    .unwrap();

    assert!(default_outcome.artists.contains_key("alligatoah"));
    assert!(!default_outcome.artists.contains_key("alligatoah jamal"));
    assert!(
        !default_outcome
            .artists
            .contains_key("alligatoah dima richman")
    );
    assert_eq!(default_outcome.artists["alligatoah"].track_count, 2);

    assert!(default_outcome.artists.contains_key("alottadogs"));
    assert!(
        !default_outcome
            .artists
            .contains_key("alottadogs glittermutt")
    );

    assert!(default_outcome.artists.contains_key("alexander marcus"));
    assert!(
        !default_outcome
            .artists
            .contains_key("alexander marcus 257ers")
    );

    assert!(default_outcome.artists.contains_key("tyler the creator"));
    assert_eq!(
        default_outcome.artists["tyler the creator"].display_name,
        "Tyler, The Creator"
    );

    // Now test strict `FolderOnly` mode: only structured non-bucket folders (`Alligatoah` and
    // `Tyler, The Creator`) are discovered, ignoring `Singles` and `Downloads` bucket folders.
    let folder_only_outcome = scan_library_folders_with_options(
        std::slice::from_ref(&root),
        &ScanOptions {
            discovery_mode: ArtistDiscoveryMode::FolderOnly,
            split_collaborations: true,
            include_compilation_artists: false,
        },
        None,
        |_, _| {},
    )
    .unwrap();

    assert_eq!(folder_only_outcome.artists.len(), 2);
    assert!(folder_only_outcome.artists.contains_key("alligatoah"));
    assert!(
        folder_only_outcome
            .artists
            .contains_key("tyler the creator")
    );

    let _ = fs::remove_dir_all(&root);
}
