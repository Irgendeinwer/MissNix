use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use lofty::config::{ParseOptions, ParsingMode};
use lofty::file::TaggedFileExt;
use lofty::probe::Probe;
use lofty::tag::{Accessor, ItemKey, Tag};
use walkdir::WalkDir;

use crate::error::{CoreError, CoreResult};
use crate::models::ScanSummary;
use crate::normalize::{
    clean_folder_album_name, collapse_whitespace, is_disc_subfolder, is_various_or_generic_artist,
    normalize_artist_name, normalize_release_title, strip_featured_artists,
};

const AUDIO_EXTENSIONS: &[&str] = &[
    "flac", "mp3", "ogg", "oga", "opus", "m4a", "mp4", "aac", "wav", "aif", "aiff", "wv", "ape",
    "mpc",
];

#[derive(Debug, Clone)]
pub struct DiscoveredAlbumRecord {
    pub title: String,
    pub normalized_title: String,
    pub release_year: Option<i32>,
    pub mb_release_group_id: Option<String>,
    pub mb_album_id: Option<String>,
    pub track_count: usize,
    pub sample_path: String,
}

#[derive(Debug, Clone)]
pub struct DiscoveredArtistRecord {
    pub display_name: String,
    pub normalized_name: String,
    pub mbid: Option<String>,
    pub track_count: usize,
    pub albums: HashMap<String, DiscoveredAlbumRecord>,
}

#[derive(Debug, Default)]
pub struct ScanOutcome {
    pub summary: ScanSummary,
    pub folder_track_counts: HashMap<PathBuf, usize>,
    pub artists: HashMap<String, DiscoveredArtistRecord>,
}

#[derive(Debug, Default)]
struct ExtractedTrackMeta {
    artist: Option<String>,
    album: Option<String>,
    year: Option<i32>,
    is_compilation: bool,
    mb_artist_id: Option<String>,
    mb_release_group_id: Option<String>,
    mb_album_id: Option<String>,
    had_tag_artist: bool,
}

pub fn is_supported_audio_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|ext| {
            let lower = ext.to_ascii_lowercase();
            AUDIO_EXTENSIONS.contains(&lower.as_str())
        })
        .unwrap_or(false)
}

/// Scan one or more music library folders, extracting artist & album identity from
/// embedded audio tags (`lofty`) with intelligent folder-structure fallback.
pub fn scan_library_folders<F>(
    folders: &[PathBuf],
    include_compilation_artists: bool,
    cancel_flag: Option<&AtomicBool>,
    mut on_progress: F,
) -> CoreResult<ScanOutcome>
where
    F: FnMut(&str, usize),
{
    let mut outcome = ScanOutcome::default();
    let parse_options = ParseOptions::new()
        .read_properties(false)
        .read_cover_art(false)
        .parsing_mode(ParsingMode::Relaxed);

    for root in folders {
        if cancel_flag.is_some_and(|c| c.load(Ordering::Relaxed)) {
            return Err(CoreError::Cancelled);
        }
        if !root.exists() {
            outcome.folder_track_counts.insert(root.clone(), 0);
            continue;
        }

        outcome.summary.folders_scanned += 1;
        let root_display = root.to_string_lossy().to_string();
        let mut folder_tracks = 0usize;

        for entry in WalkDir::new(root)
            .follow_links(true)
            .into_iter()
            .filter_map(Result::ok)
        {
            if cancel_flag.is_some_and(|c| c.load(Ordering::Relaxed)) {
                return Err(CoreError::Cancelled);
            }
            if !entry.file_type().is_file() {
                continue;
            }
            let file_path = entry.path();
            if !is_supported_audio_file(file_path) {
                continue;
            }

            outcome.summary.audio_files_seen += 1;
            folder_tracks += 1;

            if outcome.summary.audio_files_seen % 50 == 0 {
                on_progress(&root_display, outcome.summary.audio_files_seen);
            }

            let mut meta =
                extract_tags_from_file(file_path, parse_options, include_compilation_artists);

            if meta.is_compilation && !include_compilation_artists {
                outcome.summary.skipped_compilations += 1;
                continue;
            }

            let folder_hint = infer_from_folder_hierarchy(root, file_path);

            let used_tag_artist = meta.had_tag_artist;
            if meta.artist.is_none() {
                meta.artist = folder_hint.artist;
            }
            if meta.album.is_none() {
                meta.album = folder_hint.album;
            }
            if meta.year.is_none() {
                meta.year = folder_hint.year;
            }

            let Some(raw_artist) = meta.artist else {
                continue;
            };
            if is_various_or_generic_artist(&raw_artist) {
                outcome.summary.skipped_compilations += 1;
                continue;
            }

            let norm_artist = normalize_artist_name(&raw_artist);
            if norm_artist.is_empty() {
                continue;
            }

            if used_tag_artist {
                outcome.summary.tags_extracted += 1;
            } else {
                outcome.summary.folder_fallbacks += 1;
            }

            let display_artist = collapse_whitespace(&strip_featured_artists(&raw_artist));
            let artist_entry = outcome
                .artists
                .entry(norm_artist.clone())
                .or_insert_with(|| DiscoveredArtistRecord {
                    display_name: display_artist.clone(),
                    normalized_name: norm_artist,
                    mbid: meta.mb_artist_id.clone(),
                    track_count: 0,
                    albums: HashMap::new(),
                });

            artist_entry.track_count += 1;
            if artist_entry.mbid.is_none() && meta.mb_artist_id.is_some() {
                artist_entry.mbid = meta.mb_artist_id.clone();
            }

            if let Some(album_title) = meta
                .album
                .map(|a| collapse_whitespace(&a))
                .filter(|a| !a.is_empty())
            {
                let norm_album = normalize_release_title(&album_title);
                if !norm_album.is_empty() {
                    let album_entry = artist_entry
                        .albums
                        .entry(norm_album.clone())
                        .or_insert_with(|| DiscoveredAlbumRecord {
                            title: album_title,
                            normalized_title: norm_album,
                            release_year: meta.year,
                            mb_release_group_id: meta.mb_release_group_id.clone(),
                            mb_album_id: meta.mb_album_id.clone(),
                            track_count: 0,
                            sample_path: file_path.to_string_lossy().to_string(),
                        });
                    album_entry.track_count += 1;
                    if album_entry.release_year.is_none() && meta.year.is_some() {
                        album_entry.release_year = meta.year;
                    }
                    if album_entry.mb_release_group_id.is_none()
                        && meta.mb_release_group_id.is_some()
                    {
                        album_entry.mb_release_group_id = meta.mb_release_group_id;
                    }
                    if album_entry.mb_album_id.is_none() && meta.mb_album_id.is_some() {
                        album_entry.mb_album_id = meta.mb_album_id;
                    }
                }
            }
        }

        outcome
            .folder_track_counts
            .insert(root.clone(), folder_tracks);
        on_progress(&root_display, outcome.summary.audio_files_seen);
    }

    outcome.summary.artists_discovered = outcome.artists.len();
    outcome.summary.local_albums_indexed = outcome.artists.values().map(|a| a.albums.len()).sum();
    Ok(outcome)
}

fn extract_tags_from_file(
    path: &Path,
    options: ParseOptions,
    include_compilation_artists: bool,
) -> ExtractedTrackMeta {
    let mut out = ExtractedTrackMeta::default();

    let tagged = Probe::open(path)
        .ok()
        .and_then(|p| p.options(options).guess_file_type().ok())
        .and_then(|p| p.read().ok());

    let Some(tagged_file) = tagged else {
        return out;
    };

    let mut ordered_tags: Vec<&Tag> = Vec::new();
    if let Some(primary) = tagged_file.primary_tag() {
        ordered_tags.push(primary);
    }
    for t in tagged_file.tags() {
        if !ordered_tags
            .iter()
            .any(|existing| std::ptr::eq(*existing, t))
        {
            ordered_tags.push(t);
        }
    }

    let mut album_artist: Option<String> = None;
    let mut track_artist: Option<String> = None;
    let mut mb_album_artist_id: Option<String> = None;
    let mut mb_track_artist_id: Option<String> = None;

    for tag in ordered_tags {
        if album_artist.is_none() {
            album_artist = first_non_empty_item(tag, ItemKey::AlbumArtist)
                .or_else(|| first_non_empty_item(tag, ItemKey::AlbumArtists));
        }
        if track_artist.is_none() {
            track_artist = tag
                .artist()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .or_else(|| first_non_empty_item(tag, ItemKey::TrackArtist));
        }
        if out.album.is_none() {
            out.album = tag
                .album()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .or_else(|| first_non_empty_item(tag, ItemKey::AlbumTitle));
        }
        if out.year.is_none() {
            out.year = tag
                .date()
                .map(|ts| ts.year as i32)
                .filter(|y| (1900..=2100).contains(y))
                .or_else(|| parse_year_from_tag(tag));
        }
        if !out.is_compilation
            && first_non_empty_item(tag, ItemKey::FlagCompilation).is_some_and(|comp| {
                matches!(
                    comp.trim().to_ascii_lowercase().as_str(),
                    "1" | "true" | "yes"
                )
            })
        {
            out.is_compilation = true;
        }
        if mb_album_artist_id.is_none() {
            mb_album_artist_id = first_non_empty_item(tag, ItemKey::MusicBrainzReleaseArtistId)
                .and_then(extract_uuid);
        }
        if mb_track_artist_id.is_none() {
            mb_track_artist_id =
                first_non_empty_item(tag, ItemKey::MusicBrainzArtistId).and_then(extract_uuid);
        }
        if out.mb_release_group_id.is_none() {
            out.mb_release_group_id = first_non_empty_item(tag, ItemKey::MusicBrainzReleaseGroupId)
                .and_then(extract_uuid);
        }
        if out.mb_album_id.is_none() {
            out.mb_album_id =
                first_non_empty_item(tag, ItemKey::MusicBrainzReleaseId).and_then(extract_uuid);
        }
    }

    if album_artist
        .as_deref()
        .is_some_and(is_various_or_generic_artist)
    {
        out.is_compilation = true;
        if include_compilation_artists {
            if let Some(stripped) = track_artist
                .as_deref()
                .map(strip_featured_artists)
                .filter(|s| !s.is_empty() && !is_various_or_generic_artist(s))
            {
                out.artist = Some(stripped);
                out.mb_artist_id = mb_track_artist_id;
                out.had_tag_artist = true;
            }
        }
        return out;
    }

    // If FlagCompilation=1 with no distinct AlbumArtist, treat as compilation
    if out.is_compilation && album_artist.is_none() && !include_compilation_artists {
        return out;
    }

    // If FlagCompilation=1 BUT a real, specific AlbumArtist is set (e.g. an artist's own Greatest Hits),
    // we still discover the artist!
    if out.is_compilation && album_artist.is_some() {
        out.is_compilation = false;
    }

    if let Some(aa) = album_artist {
        let cleaned = strip_featured_artists(&aa);
        if !cleaned.is_empty() {
            out.artist = Some(cleaned);
            out.mb_artist_id = mb_album_artist_id.or(mb_track_artist_id);
            out.had_tag_artist = true;
        }
    } else if let Some(ta) = track_artist {
        let cleaned = strip_featured_artists(&ta);
        if !cleaned.is_empty() {
            out.artist = Some(cleaned);
            out.mb_artist_id = mb_track_artist_id;
            out.had_tag_artist = true;
        }
    }

    out
}

fn first_non_empty_item(tag: &Tag, key: ItemKey) -> Option<String> {
    tag.get_string(key)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn parse_year_from_tag(tag: &Tag) -> Option<i32> {
    for key in [
        ItemKey::OriginalReleaseDate,
        ItemKey::ReleaseDate,
        ItemKey::RecordingDate,
        ItemKey::Year,
    ] {
        if let Some(yr) = first_non_empty_item(tag, key)
            .and_then(|val| val.get(..4)?.parse::<i32>().ok())
            .filter(|y| (1900..=2100).contains(y))
        {
            return Some(yr);
        }
    }
    None
}

fn extract_uuid(raw: String) -> Option<String> {
    for part in raw.split([';', '/', ',', ' ']) {
        let p = part.trim().to_ascii_lowercase();
        if is_valid_uuid(&p) {
            return Some(p);
        }
    }
    None
}

pub fn is_valid_uuid(s: &str) -> bool {
    if s.len() != 36 {
        return false;
    }
    for (idx, ch) in s.chars().enumerate() {
        if matches!(idx, 8 | 13 | 18 | 23) {
            if ch != '-' {
                return false;
            }
        } else if !ch.is_ascii_hexdigit() {
            return false;
        }
    }
    true
}

#[derive(Debug, Default)]
struct FolderHint {
    artist: Option<String>,
    album: Option<String>,
    year: Option<i32>,
}

fn infer_from_folder_hierarchy(root: &Path, file_path: &Path) -> FolderHint {
    let Ok(rel) = file_path.strip_prefix(root) else {
        return FolderHint::default();
    };
    let mut dirs: Vec<String> = rel
        .parent()
        .map(|p| {
            p.components()
                .filter_map(|c| c.as_os_str().to_str().map(|s| s.trim().to_string()))
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default();

    // Pop trailing disc subfolders like "CD1" or "Disc 2"
    while let Some(last) = dirs.last() {
        if is_disc_subfolder(last) {
            dirs.pop();
        } else {
            break;
        }
    }

    match dirs.as_slice() {
        [] => {
            // Try "Artist - Track.ext" from filename stem if directly inside root
            if let Some(stem) = file_path.file_stem().and_then(|s| s.to_str()) {
                if let Some((left, _right)) = stem.split_once(" - ") {
                    let left = left.trim();
                    // Ignore numeric track prefixes like "01 - Track"
                    if !left.is_empty() && !left.chars().all(|c| c.is_ascii_digit()) {
                        return FolderHint {
                            artist: Some(left.to_string()),
                            album: None,
                            year: None,
                        };
                    }
                }
            }
            FolderHint::default()
        }
        [single_dir] => {
            // Could be "Artist - Album" or just "Artist"
            if let Some((artist_part, album_part)) = single_dir.split_once(" - ") {
                let artist_clean = artist_part.trim();
                let (album_clean, year) = clean_folder_album_name(album_part);
                if !artist_clean.is_empty()
                    && !artist_clean.chars().all(|c| c.is_ascii_digit())
                    && !album_clean.is_empty()
                {
                    return FolderHint {
                        artist: Some(artist_clean.to_string()),
                        album: Some(album_clean),
                        year,
                    };
                }
            }
            FolderHint {
                artist: Some(single_dir.clone()),
                album: None,
                year: None,
            }
        }
        many => {
            // Typical ".../Artist/Album" (use the last two non-disc components)
            let album_raw = &many[many.len() - 1];
            let artist_raw = &many[many.len() - 2];
            let (album_clean, year) = clean_folder_album_name(album_raw);
            FolderHint {
                artist: Some(artist_raw.clone()),
                album: if album_clean.is_empty() {
                    None
                } else {
                    Some(album_clean)
                },
                year,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lofty::config::WriteOptions;
    use lofty::tag::{ItemValue, TagExt, TagItem, TagType};
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
}
