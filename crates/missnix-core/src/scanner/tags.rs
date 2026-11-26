use std::path::Path;

use lofty::config::ParseOptions;
use lofty::file::TaggedFileExt;
use lofty::probe::Probe;
use lofty::tag::{Accessor, ItemKey, Tag};

use crate::normalize::{clean_folder_album_name, is_disc_subfolder, is_various_or_generic_artist};

#[derive(Debug, Default)]
pub(super) struct ExtractedTrackMeta {
    pub(super) album_artist: Option<String>,
    pub(super) track_artist: Option<String>,
    pub(super) album: Option<String>,
    pub(super) year: Option<i32>,
    pub(super) is_compilation: bool,
    pub(super) mb_album_artist_id: Option<String>,
    pub(super) mb_track_artist_id: Option<String>,
    pub(super) mb_release_group_id: Option<String>,
    pub(super) mb_album_id: Option<String>,
}

pub(super) fn extract_tags_from_file(path: &Path, options: ParseOptions) -> ExtractedTrackMeta {
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
        out.album_artist = None;
        out.track_artist = track_artist;
        out.mb_track_artist_id = mb_track_artist_id;
        return out;
    }

    // If FlagCompilation=1 BUT a real, specific AlbumArtist is set (e.g. an artist's own Greatest Hits),
    // we still discover the artist!
    if out.is_compilation && album_artist.is_some() {
        out.is_compilation = false;
    }

    out.album_artist = album_artist;
    out.track_artist = track_artist;
    out.mb_album_artist_id = mb_album_artist_id;
    out.mb_track_artist_id = mb_track_artist_id;
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
pub(super) struct FolderHint {
    pub(super) artist: Option<String>,
    pub(super) album: Option<String>,
    pub(super) year: Option<i32>,
    /// True when path follows `<Artist>/<Album>/track` or `<Artist - Album>/track`
    /// rather than a loose dump directory.
    structured_album_hierarchy: bool,
}

pub(super) fn infer_from_folder_hierarchy(root: &Path, file_path: &Path) -> FolderHint {
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
            if let Some(stem) = file_path.file_stem().and_then(|s| s.to_str())
                && let Some((left, _right)) = stem.split_once(" - ")
            {
                let left = left.trim();
                // Ignore numeric track prefixes like "01 - Track"
                if !left.is_empty() && !left.chars().all(|c| c.is_ascii_digit()) {
                    return FolderHint {
                        artist: Some(left.to_string()),
                        album: None,
                        year: None,
                        structured_album_hierarchy: false,
                    };
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
                        structured_album_hierarchy: true,
                    };
                }
            }
            FolderHint {
                artist: Some(single_dir.clone()),
                album: None,
                year: None,
                structured_album_hierarchy: false,
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
                structured_album_hierarchy: true,
            }
        }
    }
}
