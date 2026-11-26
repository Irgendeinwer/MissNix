use missnix_core::normalize::{days_ago_iso_utc, today_iso_utc};
use missnix_core::{
    Artist, ArtistSource, ArtistStatus, FeedQuery, FeedSort, Release, ReleaseState, ReleaseTrack,
    ReleaseType,
};
use slint::{Image, ModelRc, VecModel};

use crate::{
    AppWindow, ArtistRowData, FolderRowData, LocalAlbumRowData, ReleaseCardData, TrackRowData,
};

use super::{ImageJob, UiState};

pub(super) fn refresh_folders_and_stats(win: &AppWindow, state: &UiState) {
    let today = today_iso_utc();
    if let Ok(folders) = state.db.list_library_folders() {
        let rows: Vec<FolderRowData> = folders
            .into_iter()
            .map(|f| {
                let scanned = f
                    .last_scanned_at
                    .as_deref()
                    .map(|ts| format!("Scanned {}", compact_timestamp(ts, &today)))
                    .unwrap_or_else(|| "Not yet scanned".to_string());
                FolderRowData {
                    id: f.id as i32,
                    path: f.path.into(),
                    stats_label: format!(
                        "{} audio {} · {}",
                        f.track_count,
                        pluralize(f.track_count, "file", "files"),
                        scanned
                    )
                    .into(),
                }
            })
            .collect();
        win.set_library_folders(ModelRc::new(VecModel::from(rows)));
    }

    if let Ok(stats) = state.db.library_stats() {
        win.set_unseen_badge_count(stats.unseen_releases as i32);
        win.set_library_summary_text(
            format!(
                "{} tracked {} · {} local {}",
                stats.tracked_artists,
                pluralize(stats.tracked_artists as i64, "artist", "artists"),
                stats.local_albums,
                pluralize(stats.local_albums as i64, "album", "albums"),
            )
            .into(),
        );
    }
}

pub(super) fn refresh_feed(win: &AppWindow, state: &mut UiState) {
    let status_idx = win.get_feed_status_index();
    let release_type = match win.get_feed_type_index() {
        1 => Some(ReleaseType::Album),
        2 => Some(ReleaseType::Ep),
        3 => Some(ReleaseType::Single),
        _ => None,
    };
    let cutoff_date =
        (state.settings.lookback_days > 0).then(|| days_ago_iso_utc(state.settings.lookback_days));
    let artist_id = (win.get_filtered_artist_id() > 0).then(|| win.get_filtered_artist_id() as i64);

    let query = FeedQuery {
        search: win.get_feed_search_text().to_string(),
        release_type,
        include_albums: state.settings.include_albums,
        include_eps: state.settings.include_eps,
        include_singles: state.settings.include_singles,
        only_unseen: status_idx == 1,
        only_upcoming: status_idx == 2,
        only_seen: false,
        hide_owned: win.get_feed_hide_owned(),
        include_dismissed: false,
        artist_id,
        cutoff_date,
        sort: Some(FeedSort::from_index(win.get_feed_sort_index())),
    };

    let Ok(releases) = state.db.query_releases(&query) else {
        return;
    };

    let mut cards = Vec::with_capacity(releases.len());
    for rel in releases {
        let cached_img = state.decoded_covers.get(&rel.id).cloned();
        if cached_img.is_none()
            && rel.cover_url.is_some()
            && !state.inflight_covers.contains(&rel.id)
        {
            state.inflight_covers.insert(rel.id);
            let _ = state.image_tx.send(ImageJob::ReleaseCover(rel.clone()));
        }
        cards.push(to_release_card(&rel, cached_img.as_ref()));
    }

    state.release_model.set_vec(cards);
    refresh_folders_and_stats(win, state);
}

pub(super) fn refresh_artists(win: &AppWindow, state: &mut UiState) {
    let filter_idx = win.get_artist_filter_index();
    let status_filter = match filter_idx {
        1 => Some(ArtistStatus::Tracked),
        2 => Some(ArtistStatus::Muted),
        3 => Some(ArtistStatus::Ignored),
        _ => None,
    };
    let only_manual = filter_idx == 4;
    let search = win.get_artist_search_text();
    let today = today_iso_utc();

    if let Ok(list) = state
        .db
        .list_artists(search.as_str(), status_filter, only_manual)
    {
        let mut rows = Vec::with_capacity(list.len());
        for art in list {
            let cached_avatar = state.decoded_avatars.get(&art.id).cloned();
            if cached_avatar.is_none()
                && art.avatar_url.is_some()
                && !state.inflight_avatars.contains(&art.id)
            {
                state.inflight_avatars.insert(art.id);
                let _ = state.image_tx.send(ImageJob::ArtistAvatar(art.clone()));
            }
            rows.push(to_artist_row(&art, cached_avatar.as_ref(), &today));
        }
        state.artist_model.set_vec(rows);
    }
}

pub(super) fn select_artist_detail(win: &AppWindow, state: &mut UiState, artist_id: i64) {
    if artist_id <= 0 {
        win.set_selected_artist_id(0);
        if win.get_inspector_mode() == 2 {
            win.set_inspector_mode(0);
        }
        return;
    }
    let Ok(Some(artist)) = state.db.get_artist(artist_id) else {
        win.set_selected_artist_id(0);
        if win.get_inspector_mode() == 2 {
            win.set_inspector_mode(0);
        }
        return;
    };

    let cached_avatar = state.decoded_avatars.get(&artist.id).cloned();
    if cached_avatar.is_none()
        && artist.avatar_url.is_some()
        && !state.inflight_avatars.contains(&artist.id)
    {
        state.inflight_avatars.insert(artist.id);
        let _ = state.image_tx.send(ImageJob::ArtistAvatar(artist.clone()));
    }

    let today = today_iso_utc();
    let row = to_artist_row(&artist, cached_avatar.as_ref(), &today);
    win.set_edit_artist_mbid(row.mbid.clone());
    win.set_edit_artist_deezer_id(row.deezer_id.clone());
    win.set_selected_artist(row);
    win.set_selected_artist_id(artist_id as i32);
    win.set_inspector_mode(2);

    let albums = state
        .db
        .list_local_albums_for_artist(artist_id)
        .unwrap_or_default();
    let album_rows: Vec<LocalAlbumRowData> = albums
        .into_iter()
        .map(|a| LocalAlbumRowData {
            title: a.title.into(),
            year_label: a
                .release_year
                .map(|y| y.to_string())
                .unwrap_or_default()
                .into(),
            track_count_label: format!(
                "{} local {}",
                a.track_count,
                pluralize(a.track_count, "track", "tracks")
            )
            .into(),
            sample_path: a.sample_path.unwrap_or_default().into(),
        })
        .collect();
    win.set_selected_artist_local_albums(ModelRc::new(VecModel::from(album_rows)));
}

pub(super) fn to_release_card(rel: &Release, cover_img: Option<&Image>) -> ReleaseCardData {
    let today = today_iso_utc();
    let provider_label = match rel.provider_source.as_str() {
        "deezer+musicbrainz" => "DEEZER · MB",
        "musicbrainz" => "MUSICBRAINZ",
        _ => "DEEZER",
    };

    ReleaseCardData {
        id: rel.id as i32,
        artist_id: rel.artist_id as i32,
        title: rel.title.clone().into(),
        artist_name: rel.artist_name.clone().into(),
        release_type: rel.release_type.badge_label().into(),
        release_date: rel.release_date.clone().into(),
        cover: cover_img.cloned().unwrap_or_default(),
        has_cover: cover_img.is_some(),
        monogram: make_monogram(&rel.artist_name).into(),
        accent_hue: (rel.artist_id.unsigned_abs() % 6) as i32,
        unseen: rel.state == ReleaseState::Unseen,
        upcoming: rel.is_upcoming(&today),
        owned_locally: rel.owned_locally,
        explicit: rel.explicit,
        provider_label: provider_label.into(),
        external_url: rel.external_url.clone().unwrap_or_default().into(),
        musicbrainz_url: rel.musicbrainz_url.clone().unwrap_or_default().into(),
        track_count: rel.track_count,
    }
}

pub(super) fn to_artist_row(
    art: &Artist,
    avatar_img: Option<&Image>,
    today_iso: &str,
) -> ArtistRowData {
    let source_label = match art.source {
        ArtistSource::Manual => {
            if art.local_track_count > 0 {
                format!(
                    "{} {} ({} {})",
                    art.local_album_count,
                    pluralize(art.local_album_count, "album", "albums"),
                    art.local_track_count,
                    pluralize(art.local_track_count, "track", "tracks"),
                )
            } else {
                "Manual radar".to_string()
            }
        }
        ArtistSource::Local => {
            if art.local_album_count > 0 {
                format!(
                    "{} local {} · {} {}",
                    art.local_album_count,
                    pluralize(art.local_album_count, "album", "albums"),
                    art.local_track_count,
                    pluralize(art.local_track_count, "track", "tracks"),
                )
            } else {
                format!(
                    "{} local {}",
                    art.local_track_count,
                    pluralize(art.local_track_count, "track", "tracks"),
                )
            }
        }
    };

    let last_checked = art
        .last_checked_at
        .as_deref()
        .map(|ts| compact_timestamp(ts, today_iso))
        .unwrap_or_else(|| "Unchecked".to_string());

    ArtistRowData {
        id: art.id as i32,
        name: art.name.clone().into(),
        monogram: make_monogram(&art.name).into(),
        accent_hue: (art.id.unsigned_abs() % 6) as i32,
        avatar: avatar_img.cloned().unwrap_or_default(),
        has_avatar: avatar_img.is_some(),
        disambiguation: art.disambiguation.clone().unwrap_or_default().into(),
        source_label: source_label.into(),
        status: art.status.as_str().into(),
        is_manual: art.source == ArtistSource::Manual && art.local_track_count == 0,
        unseen_count: art.unseen_release_count as i32,
        release_count: art.discovered_release_count as i32,
        last_checked: last_checked.into(),
        mbid: art.mbid.clone().unwrap_or_default().into(),
        deezer_id: art
            .deezer_id
            .map(|id| id.to_string())
            .unwrap_or_default()
            .into(),
        external_url: art.external_url.clone().unwrap_or_default().into(),
    }
}

pub(super) fn tracks_to_model(tracks: &[ReleaseTrack]) -> ModelRc<TrackRowData> {
    let multi_disc = tracks.iter().any(|t| t.disc_number > 1);
    let rows: Vec<TrackRowData> = tracks
        .iter()
        .map(|t| {
            let number_label = if multi_disc {
                format!("{}-{:02}", t.disc_number, t.track_number)
            } else {
                format!("{:02}", t.track_number)
            };
            let preview = t.preview_url.clone().unwrap_or_default();
            TrackRowData {
                disc_number: t.disc_number,
                track_number: t.track_number,
                number_label: number_label.into(),
                title: t.title.clone().into(),
                duration: t.formatted_duration().into(),
                has_preview: !preview.is_empty(),
                preview_url: preview.into(),
            }
        })
        .collect();
    ModelRc::new(VecModel::from(rows))
}

pub(super) fn make_monogram(name: &str) -> String {
    let mut initials = String::new();
    for word in name
        .split_whitespace()
        .filter(|w| !w.eq_ignore_ascii_case("the"))
    {
        if let Some(ch) = word.chars().find(|c| c.is_alphanumeric()) {
            for up in ch.to_uppercase() {
                initials.push(up);
            }
            if initials.chars().count() >= 2 {
                break;
            }
        }
    }
    if initials.is_empty() {
        "♪".to_string()
    } else {
        initials
    }
}

pub(super) fn pluralize<'a>(count: i64, singular: &'a str, plural: &'a str) -> &'a str {
    if count == 1 { singular } else { plural }
}

pub(super) fn compact_timestamp(rfc3339: &str, today_iso: &str) -> String {
    if rfc3339.len() >= 16 {
        let date_part = &rfc3339[..10];
        let time_part = &rfc3339[11..16];
        if date_part == today_iso {
            format!("Today {time_part}")
        } else {
            date_part.to_string()
        }
    } else {
        rfc3339.to_string()
    }
}
