mod tags;
#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use lofty::config::{ParseOptions, ParsingMode};
use walkdir::WalkDir;

use crate::error::{CoreError, CoreResult};
use crate::models::{AppSettings, ArtistDiscoveryMode, ScanSummary};
use crate::normalize::{
    collapse_whitespace, extract_primary_artist, fold_diacritics_and_symbols,
    is_various_or_generic_artist, normalize_artist_name, normalize_release_title,
};

pub use self::tags::is_valid_uuid;
use self::tags::{
    ExtractedTrackMeta, FolderHint, extract_tags_from_file, infer_from_folder_hierarchy,
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

#[derive(Debug, Clone, Copy)]
pub struct ScanOptions {
    pub discovery_mode: ArtistDiscoveryMode,
    pub split_collaborations: bool,
    pub include_compilation_artists: bool,
}

impl Default for ScanOptions {
    fn default() -> Self {
        Self {
            discovery_mode: ArtistDiscoveryMode::AlbumArtistOrFolder,
            split_collaborations: true,
            include_compilation_artists: false,
        }
    }
}

impl ScanOptions {
    pub fn from_settings(settings: &AppSettings) -> Self {
        Self {
            discovery_mode: settings.artist_discovery_mode,
            split_collaborations: settings.split_collaborations,
            include_compilation_artists: settings.include_compilation_artists,
        }
    }
}

#[derive(Debug, Default)]
pub struct ScanOutcome {
    pub summary: ScanSummary,
    pub folder_track_counts: HashMap<PathBuf, usize>,
    pub artists: HashMap<String, DiscoveredArtistRecord>,
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

/// Scan one or more music library folders with default `AlbumArtistOrFolder` + collaboration-splitting rules.
pub fn scan_library_folders<F>(
    folders: &[PathBuf],
    include_compilation_artists: bool,
    cancel_flag: Option<&AtomicBool>,
    on_progress: F,
) -> CoreResult<ScanOutcome>
where
    F: FnMut(&str, usize),
{
    let options = ScanOptions {
        include_compilation_artists,
        ..ScanOptions::default()
    };
    scan_library_folders_with_options(folders, &options, cancel_flag, on_progress)
}

/// Scan one or more music library folders using configurable artist discovery mode and
/// multi-artist collaboration handling.
pub fn scan_library_folders_with_options<F>(
    folders: &[PathBuf],
    options: &ScanOptions,
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

            let meta = extract_tags_from_file(file_path, parse_options);

            if meta.is_compilation && !options.include_compilation_artists {
                outcome.summary.skipped_compilations += 1;
                continue;
            }

            let folder_hint = infer_from_folder_hierarchy(root, file_path);

            let Some(resolved) = resolve_artist_identity(&meta, &folder_hint, options) else {
                if meta.is_compilation
                    || meta
                        .album_artist
                        .as_deref()
                        .is_some_and(is_various_or_generic_artist)
                    || folder_hint
                        .artist
                        .as_deref()
                        .is_some_and(is_various_or_generic_artist)
                {
                    outcome.summary.skipped_compilations += 1;
                }
                continue;
            };

            let display_artist = collapse_whitespace(&resolved.artist_name);
            let norm_artist = normalize_artist_name(&display_artist);
            if norm_artist.is_empty() {
                continue;
            }

            if resolved.used_tag_artist {
                outcome.summary.tags_extracted += 1;
            } else {
                outcome.summary.folder_fallbacks += 1;
            }

            let album_title = meta
                .album
                .or(folder_hint.album)
                .map(|a| collapse_whitespace(&a))
                .filter(|a| !a.is_empty());
            let release_year = meta.year.or(folder_hint.year);

            let artist_entry = outcome
                .artists
                .entry(norm_artist.clone())
                .or_insert_with(|| DiscoveredArtistRecord {
                    display_name: display_artist.clone(),
                    normalized_name: norm_artist,
                    mbid: resolved.mb_artist_id.clone(),
                    track_count: 0,
                    albums: HashMap::new(),
                });

            artist_entry.track_count += 1;
            if artist_entry.mbid.is_none() && resolved.mb_artist_id.is_some() {
                artist_entry.mbid = resolved.mb_artist_id;
            }

            if let Some(album_title) = album_title {
                let norm_album = normalize_release_title(&album_title);
                if !norm_album.is_empty() {
                    let album_entry = artist_entry
                        .albums
                        .entry(norm_album.clone())
                        .or_insert_with(|| DiscoveredAlbumRecord {
                            title: album_title,
                            normalized_title: norm_album,
                            release_year,
                            mb_release_group_id: meta.mb_release_group_id.clone(),
                            mb_album_id: meta.mb_album_id.clone(),
                            track_count: 0,
                            sample_path: file_path.to_string_lossy().to_string(),
                        });
                    album_entry.track_count += 1;
                    if album_entry.release_year.is_none() && release_year.is_some() {
                        album_entry.release_year = release_year;
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

struct ResolvedArtist {
    artist_name: String,
    mb_artist_id: Option<String>,
    used_tag_artist: bool,
}

fn resolve_artist_identity(
    meta: &ExtractedTrackMeta,
    folder_hint: &FolderHint,
    options: &ScanOptions,
) -> Option<ResolvedArtist> {
    let valid_folder_artist = folder_hint
        .artist
        .as_deref()
        .filter(|a| !is_various_or_generic_artist(a) && !is_generic_bucket_folder(a));

    let clean_album_artist = meta
        .album_artist
        .as_deref()
        .filter(|a| !is_various_or_generic_artist(a))
        .map(|aa| extract_primary_artist(aa, valid_folder_artist, options.split_collaborations))
        .filter(|a| !a.is_empty() && !is_various_or_generic_artist(a));

    let clean_track_artist = meta
        .track_artist
        .as_deref()
        .filter(|a| !is_various_or_generic_artist(a))
        .map(|ta| extract_primary_artist(ta, valid_folder_artist, options.split_collaborations))
        .filter(|a| !a.is_empty() && !is_various_or_generic_artist(a));

    let clean_folder_artist = valid_folder_artist
        .map(|fa| extract_primary_artist(fa, None, options.split_collaborations))
        .filter(|a| {
            !a.is_empty() && !is_various_or_generic_artist(a) && !is_generic_bucket_folder(a)
        });

    match options.discovery_mode {
        ArtistDiscoveryMode::FolderOnly => {
            let fa = clean_folder_artist?;
            let fa_norm = fold_diacritics_and_symbols(&fa);
            // Attach embedded MBID only if the embedded artist tag actually matches this folder artist
            let mbid = if clean_album_artist
                .as_deref()
                .is_some_and(|aa| fold_diacritics_and_symbols(aa) == fa_norm)
            {
                meta.mb_album_artist_id
                    .clone()
                    .or_else(|| meta.mb_track_artist_id.clone())
            } else if clean_track_artist
                .as_deref()
                .is_some_and(|ta| fold_diacritics_and_symbols(ta) == fa_norm)
            {
                meta.mb_track_artist_id.clone()
            } else {
                None
            };
            Some(ResolvedArtist {
                artist_name: fa,
                mb_artist_id: mbid,
                used_tag_artist: false,
            })
        }
        ArtistDiscoveryMode::AlbumArtistOrFolder => {
            // 1. Explicit `ALBUMARTIST` tag (cleaned of multi-artist delimiters)
            if let Some(aa) = clean_album_artist {
                let mbid = safe_tag_mbid(
                    meta.album_artist.as_deref(),
                    &aa,
                    meta.mb_album_artist_id
                        .as_deref()
                        .or(meta.mb_track_artist_id.as_deref()),
                );
                return Some(ResolvedArtist {
                    artist_name: aa,
                    mb_artist_id: mbid,
                    used_tag_artist: true,
                });
            }

            // 2. Structured `<Artist>/<Album>/track` or `<Artist - Album>/track` folder hierarchy
            //    wins over messy per-track collaboration `ARTIST` tags!
            if folder_hint.structured_album_hierarchy
                && let Some(fa) = clean_folder_artist
            {
                let fa_norm = fold_diacritics_and_symbols(&fa);
                let mbid = clean_track_artist
                    .as_deref()
                    .filter(|ta| fold_diacritics_and_symbols(ta) == fa_norm)
                    .and_then(|ta| {
                        safe_tag_mbid(
                            meta.track_artist.as_deref(),
                            ta,
                            meta.mb_track_artist_id.as_deref(),
                        )
                    });
                return Some(ResolvedArtist {
                    artist_name: fa,
                    mb_artist_id: mbid,
                    used_tag_artist: false,
                });
            }

            // 3. Cleaned primary artist from track `ARTIST` tag (for flat/bucket folders)
            if let Some(ta) = clean_track_artist {
                let mbid = safe_tag_mbid(
                    meta.track_artist.as_deref(),
                    &ta,
                    meta.mb_track_artist_id.as_deref(),
                );
                return Some(ResolvedArtist {
                    artist_name: ta,
                    mb_artist_id: mbid,
                    used_tag_artist: true,
                });
            }

            // 4. Fallback to single-level folder / filename artist
            clean_folder_artist.map(|fa| ResolvedArtist {
                artist_name: fa,
                mb_artist_id: None,
                used_tag_artist: false,
            })
        }
        ArtistDiscoveryMode::AllTagsAndFolder => {
            if let Some(aa) = clean_album_artist {
                let mbid = safe_tag_mbid(
                    meta.album_artist.as_deref(),
                    &aa,
                    meta.mb_album_artist_id
                        .as_deref()
                        .or(meta.mb_track_artist_id.as_deref()),
                );
                return Some(ResolvedArtist {
                    artist_name: aa,
                    mb_artist_id: mbid,
                    used_tag_artist: true,
                });
            }
            if let Some(ta) = clean_track_artist {
                let mbid = safe_tag_mbid(
                    meta.track_artist.as_deref(),
                    &ta,
                    meta.mb_track_artist_id.as_deref(),
                );
                return Some(ResolvedArtist {
                    artist_name: ta,
                    mb_artist_id: mbid,
                    used_tag_artist: true,
                });
            }
            clean_folder_artist.map(|fa| ResolvedArtist {
                artist_name: fa,
                mb_artist_id: None,
                used_tag_artist: false,
            })
        }
    }
}

/// Only keep the extracted first UUID when `chosen_artist` corresponds to the lead artist
/// of `raw_tag` (prevents attaching Artist A's UUID to Artist B when folder hint selects Artist B).
fn safe_tag_mbid(raw_tag: Option<&str>, chosen_artist: &str, mbid: Option<&str>) -> Option<String> {
    let uuid = mbid?;
    let Some(raw) = raw_tag else {
        return Some(uuid.to_string());
    };
    let raw_norm = fold_diacritics_and_symbols(raw);
    let chosen_norm = fold_diacritics_and_symbols(chosen_artist);
    if !chosen_norm.is_empty() && raw_norm.starts_with(&chosen_norm) {
        Some(uuid.to_string())
    } else {
        None
    }
}
