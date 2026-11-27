use std::thread;

use missnix_core::{ArtistStatus, scanner::is_valid_uuid};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::{AppWindow, SearchCandidateRowData};

use super::hydrate::{
    refresh_artists, refresh_feed, refresh_folders_and_stats, select_artist_detail,
};
use super::workers::start_release_sync;
use super::{CANDIDATE_STASH, SharedUiState, UiState};

pub(super) fn wire_artist_callbacks(app: &AppWindow, state: &SharedUiState) {
    // -------------------------------------------------------------------------
    // Tracked Artists Callbacks
    // -------------------------------------------------------------------------
    {
        let weak = app.as_weak();
        let st = state.clone();
        app.on_artist_filters_changed(move || {
            let Some(win) = weak.upgrade() else { return };
            refresh_artists(&win, &mut st.borrow_mut());
        });
    }

    {
        let weak = app.as_weak();
        let st = state.clone();
        app.on_select_artist(move |artist_id| {
            let Some(win) = weak.upgrade() else { return };
            let mut guard = st.borrow_mut();
            select_artist_detail(&win, &mut guard, artist_id as i64);
        });
    }

    {
        let weak = app.as_weak();
        let st = state.clone();
        app.on_check_single_artist(move |artist_id| {
            start_release_sync(&weak, &st, Some(artist_id as i64));
        });
    }

    {
        let weak = app.as_weak();
        let st = state.clone();
        app.on_view_artist_releases(move |artist_id| {
            let Some(win) = weak.upgrade() else { return };
            let mut guard = st.borrow_mut();
            if let Ok(Some(artist)) = guard.db.get_artist(artist_id as i64) {
                win.set_filtered_artist_id(artist_id);
                win.set_filtered_artist_name(artist.name.into());
                win.set_filtered_artist_status(artist.status.as_str().into());
                win.set_feed_status_index(0);
                win.set_active_view(0);
                refresh_feed(&win, &mut guard);
            }
        });
    }

    {
        let weak = app.as_weak();
        let st = state.clone();
        app.on_toggle_artist_mute(move |artist_id| {
            let Some(win) = weak.upgrade() else { return };
            let mut guard = st.borrow_mut();
            if let Ok(Some(art)) = guard.db.get_artist(artist_id as i64) {
                let next = if art.status == ArtistStatus::Muted {
                    ArtistStatus::Tracked
                } else {
                    ArtistStatus::Muted
                };
                if let Err(err) = guard.db.set_artist_status(art.id, next) {
                    win.set_status_is_warning(true);
                    win.set_status_banner_text(
                        format!("Could not change artist status: {err}").into(),
                    );
                    return;
                }
                win.set_status_is_warning(false);
                win.set_status_banner_text(
                    if next == ArtistStatus::Muted {
                        format!("Muted '{}' in Release Radar", art.name)
                    } else {
                        format!("Unmuted '{}'", art.name)
                    }
                    .into(),
                );
                sync_artist_status_badges(&win, &mut guard, artist_id, next);
                refresh_folders_and_stats(&win, &guard);
                refresh_artists(&win, &mut guard);
                refresh_feed(&win, &mut guard);
            }
        });
    }

    {
        let weak = app.as_weak();
        let st = state.clone();
        app.on_toggle_artist_ignore(move |artist_id| {
            let Some(win) = weak.upgrade() else { return };
            let mut guard = st.borrow_mut();
            if let Ok(Some(art)) = guard.db.get_artist(artist_id as i64) {
                let next = if matches!(art.status, ArtistStatus::Ignored | ArtistStatus::Missing) {
                    ArtistStatus::Tracked
                } else {
                    ArtistStatus::Ignored
                };
                if let Err(err) = guard.db.set_artist_status(art.id, next) {
                    win.set_status_is_warning(true);
                    win.set_status_banner_text(
                        format!("Could not change artist status: {err}").into(),
                    );
                    return;
                }
                win.set_status_is_warning(false);
                win.set_status_banner_text(
                    if next == ArtistStatus::Ignored {
                        format!("Hidden '{}' from Release Radar", art.name)
                    } else {
                        format!("Restored '{}' to Release Radar", art.name)
                    }
                    .into(),
                );
                sync_artist_status_badges(&win, &mut guard, artist_id, next);
                refresh_folders_and_stats(&win, &guard);
                refresh_artists(&win, &mut guard);
                refresh_feed(&win, &mut guard);
            }
        });
    }

    {
        let weak = app.as_weak();
        let st = state.clone();
        app.on_delete_artist(move |artist_id| {
            let Some(win) = weak.upgrade() else { return };
            let mut guard = st.borrow_mut();
            if let Err(err) = guard.db.delete_artist(artist_id as i64) {
                win.set_status_is_warning(true);
                win.set_status_banner_text(format!("Could not delete artist: {err}").into());
                return;
            }
            if win.get_selected_artist_id() == artist_id {
                win.set_selected_artist_id(0);
                if win.get_inspector_mode() == 2 {
                    win.set_inspector_mode(0);
                }
            }
            refresh_folders_and_stats(&win, &guard);
            refresh_artists(&win, &mut guard);
            refresh_feed(&win, &mut guard);
        });
    }

    {
        let weak = app.as_weak();
        let st = state.clone();
        app.on_save_artist_provider_ids(move |artist_id, mbid_s, dz_s| {
            let Some(win) = weak.upgrade() else { return };
            let dz_trimmed = dz_s.trim();
            let parsed_dz = if dz_trimmed.is_empty() {
                None
            } else {
                match dz_trimmed.parse::<i64>() {
                    Ok(v) if v > 0 => Some(v),
                    _ => {
                        win.set_status_is_warning(true);
                        win.set_status_banner_text(
                            "Deezer Artist ID must be a positive number".into(),
                        );
                        return;
                    }
                }
            };

            let mbid_trimmed = mbid_s.trim();
            let mbid_opt = if mbid_trimmed.is_empty() {
                None
            } else if !is_valid_uuid(mbid_trimmed) {
                win.set_status_is_warning(true);
                win.set_status_banner_text("MusicBrainz MBID must be a 36-char UUID".into());
                return;
            } else {
                Some(mbid_trimmed)
            };

            let mut guard = st.borrow_mut();
            if let Err(e) = guard
                .db
                .override_artist_ids(artist_id as i64, mbid_opt, parsed_dz)
            {
                win.set_status_is_warning(true);
                win.set_status_banner_text(format!("Failed to save IDs: {e}").into());
                return;
            }
            win.set_status_is_warning(false);
            win.set_status_banner_text("Saved artist provider IDs".into());
            refresh_artists(&win, &mut guard);
            select_artist_detail(&win, &mut guard, artist_id as i64);
        });
    }

    {
        let weak = app.as_weak();
        let st = state.clone();
        app.on_search_external_artist(move |query| {
            let q = query.trim().to_string();
            if q.is_empty() {
                return;
            }
            let Some(win) = weak.upgrade() else { return };
            win.set_searching_external_artists(true);
            let request_id = win.get_search_request_id().wrapping_add(1);
            win.set_search_request_id(request_id);
            CANDIDATE_STASH.with(|stash| stash.borrow_mut().clear());
            win.set_artist_search_candidates(ModelRc::new(
                VecModel::<SearchCandidateRowData>::default(),
            ));

            let (engine, mode) = {
                let guard = st.borrow();
                (guard.engine.clone(), guard.settings.provider_mode)
            };

            let weak_done = weak.clone();
            thread::spawn(move || {
                let res = engine.search_external_artists(&q, mode);
                let _ = slint::invoke_from_event_loop(move || {
                    let Some(win) = weak_done.upgrade() else {
                        return;
                    };
                    if win.get_search_request_id() != request_id {
                        return;
                    }
                    win.set_searching_external_artists(false);
                    if win.get_manual_artist_query().trim() != q {
                        return;
                    }
                    match res {
                        Ok(result) => {
                            if !result.warnings.is_empty() {
                                win.set_status_is_warning(true);
                                win.set_status_banner_text(
                                    format!(
                                        "Some artist searches failed: {}",
                                        result.warnings.join("; ")
                                    )
                                    .into(),
                                );
                            }
                            CANDIDATE_STASH.with(|stash| {
                                *stash.borrow_mut() = result.candidates.clone();
                            });
                            let rows: Vec<SearchCandidateRowData> = result
                                .candidates
                                .iter()
                                .enumerate()
                                .map(|(idx, c)| SearchCandidateRowData {
                                    index: idx as i32,
                                    name: c.name.clone().into(),
                                    subtitle: c.disambiguation.clone().unwrap_or_default().into(),
                                    source_badge: c.source.to_ascii_uppercase().into(),
                                })
                                .collect();
                            win.set_artist_search_candidates(ModelRc::new(VecModel::from(rows)));
                        }
                        Err(err) => {
                            win.set_status_is_warning(true);
                            win.set_status_banner_text(
                                format!("Artist search failed: {err}").into(),
                            );
                        }
                    }
                });
            });
        });
    }

    {
        let weak = app.as_weak();
        let st = state.clone();
        app.on_add_artist_by_name(move |name| {
            let Some(win) = weak.upgrade() else { return };
            let artist_id = {
                let mut guard = st.borrow_mut();
                match guard
                    .db
                    .add_manual_artist(name.as_str(), None, None, None, None, None)
                {
                    Ok(art) => {
                        win.set_manual_artist_query(SharedString::default());
                        win.set_artist_search_candidates(ModelRc::new(VecModel::<
                            SearchCandidateRowData,
                        >::default(
                        )));
                        win.set_status_is_warning(false);
                        win.set_status_banner_text(
                            format!("Added '{}' to tracked artists", art.name).into(),
                        );
                        refresh_folders_and_stats(&win, &guard);
                        refresh_artists(&win, &mut guard);
                        Some(art.id)
                    }
                    Err(err) => {
                        win.set_status_is_warning(true);
                        win.set_status_banner_text(err.to_string().into());
                        None
                    }
                }
            };
            if let Some(id) = artist_id {
                start_release_sync(&weak, &st, Some(id));
            }
        });
    }

    {
        let weak = app.as_weak();
        let st = state.clone();
        app.on_add_search_candidate(move |idx| {
            let Some(win) = weak.upgrade() else { return };
            let chosen = CANDIDATE_STASH.with(|stash| stash.borrow().get(idx as usize).cloned());
            let Some(cand) = chosen else { return };

            let artist_id = {
                let mut guard = st.borrow_mut();
                match guard.db.add_manual_artist(
                    &cand.name,
                    cand.mbid.as_deref(),
                    cand.deezer_id,
                    cand.disambiguation.as_deref(),
                    cand.avatar_url.as_deref(),
                    cand.external_url.as_deref(),
                ) {
                    Ok(art) => {
                        win.set_manual_artist_query(SharedString::default());
                        win.set_artist_search_candidates(ModelRc::new(VecModel::<
                            SearchCandidateRowData,
                        >::default(
                        )));
                        win.set_show_add_artist_bar(false);
                        win.set_status_is_warning(false);
                        win.set_status_banner_text(format!("Tracking '{}'", art.name).into());
                        refresh_folders_and_stats(&win, &guard);
                        refresh_artists(&win, &mut guard);
                        Some(art.id)
                    }
                    Err(err) => {
                        win.set_status_is_warning(true);
                        win.set_status_banner_text(err.to_string().into());
                        None
                    }
                }
            };
            if let Some(id) = artist_id {
                start_release_sync(&weak, &st, Some(id));
            }
        });
    }
}

pub(super) fn sync_artist_status_badges(
    win: &AppWindow,
    state: &mut UiState,
    artist_id: i32,
    next_status: ArtistStatus,
) {
    let status_str: SharedString = next_status.as_str().into();
    if win.get_selected_release().artist_id == artist_id {
        win.set_selected_release_artist_status(status_str.clone());
    }
    if win.get_filtered_artist_id() == artist_id {
        win.set_filtered_artist_status(status_str);
    }
    if win.get_selected_artist_id() == artist_id {
        select_artist_detail(win, state, artist_id as i64);
    }
}
