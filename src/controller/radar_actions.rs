use std::sync::atomic::Ordering;

use missnix_core::ReleaseState;
use slint::{ComponentHandle, SharedString};

use crate::AppWindow;
use crate::audio_player::PreviewRequest;

use super::SharedUiState;
use super::hydrate::{
    make_monogram, pluralize, refresh_artists, refresh_feed, refresh_folders_and_stats,
    select_artist_detail, to_release_card,
};
use super::workers::{open_release_inspector, start_library_scan, start_release_sync};

pub(super) fn wire_radar_and_preview_callbacks(app: &AppWindow, state: &SharedUiState) {
    // -------------------------------------------------------------------------
    // Navigation & Global Task Callbacks (0=Radar, 1=Artists, 2=Settings)
    // -------------------------------------------------------------------------
    {
        let weak = app.as_weak();
        let st = state.clone();
        app.on_navigate(move |view_idx| {
            let Some(win) = weak.upgrade() else { return };
            win.set_active_view(view_idx);
            if view_idx == 1 && win.get_inspector_mode() == 1 {
                win.set_inspector_mode(0);
                win.set_selected_release_id(0);
            }
            let mut guard = st.borrow_mut();
            match view_idx {
                0 => refresh_feed(&win, &mut guard),
                1 => refresh_artists(&win, &mut guard),
                2 => refresh_folders_and_stats(&win, &guard),
                _ => {}
            }
        });
    }

    {
        let st = state.clone();
        app.on_cancel_background_task(move || {
            st.borrow().cancel_flag.store(true, Ordering::Relaxed);
        });
    }

    {
        let weak = app.as_weak();
        let st = state.clone();
        app.on_scan_library(move || {
            start_library_scan(&weak, &st);
        });
    }

    {
        let weak = app.as_weak();
        let st = state.clone();
        app.on_check_all_releases(move || {
            start_release_sync(&weak, &st, None);
        });
    }

    // -------------------------------------------------------------------------
    // Native In-App 30s Audio Preview Callbacks
    // -------------------------------------------------------------------------
    {
        let weak = app.as_weak();
        let st = state.clone();
        app.on_play_track_preview(move |rel_id, disc_num, track_num| {
            let Some(win) = weak.upgrade() else { return };
            let guard = st.borrow();
            let Ok(Some(rel)) = guard.db.get_release(rel_id as i64) else {
                return;
            };
            let tracks = guard
                .db
                .list_release_tracks(rel_id as i64)
                .unwrap_or_default();
            let Some(trk) = tracks
                .into_iter()
                .find(|t| t.disc_number == disc_num && t.track_number == track_num)
            else {
                return;
            };
            let Some(preview_url) = trk.preview_url.filter(|u| !u.is_empty()) else {
                win.set_status_is_warning(true);
                win.set_status_banner_text("No 30s preview available for this track".into());
                return;
            };

            if let Some(cov) = guard.decoded_covers.get(&rel.id) {
                win.set_preview_cover(cov.clone());
                win.set_preview_has_cover(true);
            } else {
                win.set_preview_has_cover(false);
            }
            win.set_preview_monogram(make_monogram(&rel.artist_name).into());
            win.set_preview_accent_hue((rel.artist_id.unsigned_abs() % 6) as i32);

            guard.audio.play(PreviewRequest {
                release_id: rel.id,
                disc_number: disc_num,
                track_number: track_num,
                track_title: trk.title,
                artist_name: rel.artist_name,
                release_title: rel.title,
                preview_url,
            });
        });
    }

    {
        let st = state.clone();
        app.on_toggle_preview_pause(move || {
            st.borrow().audio.toggle_pause();
        });
    }

    {
        let st = state.clone();
        app.on_seek_preview(move |frac| {
            st.borrow().audio.seek(frac);
        });
    }

    {
        let st = state.clone();
        app.on_set_preview_volume(move |pct| {
            let clamped = pct.clamp(0, 100) as u8;
            let mut guard = st.borrow_mut();
            guard.audio.set_volume(clamped);
            guard.settings.preview_volume = clamped;
            let s = guard.settings.clone();
            let _ = guard.db.save_settings(&s);
        });
    }

    {
        let st = state.clone();
        app.on_stop_preview(move || {
            st.borrow().audio.stop();
        });
    }

    // -------------------------------------------------------------------------
    // Unified Release Radar & Right Inspector Callbacks
    // -------------------------------------------------------------------------
    {
        let weak = app.as_weak();
        let st = state.clone();
        app.on_feed_filters_changed(move || {
            let Some(win) = weak.upgrade() else { return };
            refresh_feed(&win, &mut st.borrow_mut());
        });
    }

    {
        let weak = app.as_weak();
        let st = state.clone();
        app.on_clear_artist_feed_filter(move || {
            let Some(win) = weak.upgrade() else { return };
            win.set_filtered_artist_id(0);
            win.set_filtered_artist_name(SharedString::default());
            win.set_filtered_artist_status("tracked".into());
            refresh_feed(&win, &mut st.borrow_mut());
        });
    }

    {
        let weak = app.as_weak();
        let st = state.clone();
        app.on_select_release(move |rel_id| {
            let Some(win) = weak.upgrade() else { return };
            if win.get_inspector_mode() == 1
                && win.get_selected_release_id() == rel_id
                && win.get_active_view() == 0
            {
                win.set_inspector_mode(0);
                win.set_selected_release_id(0);
                win.set_loading_tracks(false);
                return;
            }
            open_release_inspector(&win, &st, rel_id as i64);
        });
    }

    {
        let weak = app.as_weak();
        app.on_close_inspector(move || {
            if let Some(win) = weak.upgrade() {
                win.set_inspector_mode(0);
                win.set_selected_release_id(0);
                win.set_selected_artist_id(0);
                win.set_loading_tracks(false);
            }
        });
    }

    {
        let weak = app.as_weak();
        let st = state.clone();
        app.on_toggle_release_seen(move |rel_id| {
            let Some(win) = weak.upgrade() else { return };
            let mut guard = st.borrow_mut();
            if let Ok(Some(rel)) = guard.db.get_release(rel_id as i64) {
                let next_state = if rel.state == ReleaseState::Unseen {
                    ReleaseState::Seen
                } else {
                    ReleaseState::Unseen
                };
                let _ = guard.db.set_release_state(rel.id, next_state);
                refresh_folders_and_stats(&win, &guard);
                refresh_feed(&win, &mut guard);
                if win.get_selected_release_id() == rel_id
                    && let Ok(Some(updated)) = guard.db.get_release(rel.id)
                {
                    let card = to_release_card(&updated, guard.decoded_covers.get(&updated.id));
                    win.set_selected_release(card);
                }
            }
        });
    }

    {
        let weak = app.as_weak();
        let st = state.clone();
        app.on_dismiss_release(move |rel_id| {
            let Some(win) = weak.upgrade() else { return };
            let mut guard = st.borrow_mut();
            let _ = guard
                .db
                .set_release_state(rel_id as i64, ReleaseState::Dismissed);
            if win.get_inspector_mode() == 1 && win.get_selected_release_id() == rel_id {
                win.set_inspector_mode(0);
                win.set_selected_release_id(0);
            }
            refresh_folders_and_stats(&win, &guard);
            refresh_feed(&win, &mut guard);
        });
    }

    {
        let weak = app.as_weak();
        let st = state.clone();
        app.on_mark_all_seen(move || {
            let Some(win) = weak.upgrade() else { return };
            let mut guard = st.borrow_mut();
            let artist_filter =
                (win.get_filtered_artist_id() > 0).then(|| win.get_filtered_artist_id() as i64);
            if let Ok(count) = guard.db.mark_all_releases_seen(artist_filter) {
                win.set_status_is_warning(false);
                win.set_status_banner_text(
                    format!(
                        "Marked {} {} as seen",
                        count,
                        pluralize(count as i64, "release", "releases")
                    )
                    .into(),
                );
            }
            refresh_folders_and_stats(&win, &guard);
            refresh_feed(&win, &mut guard);
        });
    }

    {
        let weak = app.as_weak();
        app.on_open_external_url(move |url| {
            let u = url.trim();
            if (u.starts_with("https://") || u.starts_with("http://"))
                && let Err(err) = open::that_detached(u)
                && let Some(win) = weak.upgrade()
            {
                win.set_status_is_warning(true);
                win.set_status_banner_text(format!("Could not open browser: {err}").into());
            }
        });
    }

    // Clicking an artist directly in Release Radar opens the full-width Artist Inspector
    // right beside Release Radar (preserving scroll position!) with Mute & Hide actions ready.
    {
        let weak = app.as_weak();
        let st = state.clone();
        app.on_inspect_artist(move |artist_id| {
            let Some(win) = weak.upgrade() else { return };
            if win.get_inspector_mode() == 2 && win.get_selected_artist_id() == artist_id {
                win.set_inspector_mode(0);
                win.set_selected_artist_id(0);
                return;
            }
            let mut guard = st.borrow_mut();
            select_artist_detail(&win, &mut guard, artist_id as i64);
        });
    }
}
