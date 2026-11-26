use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::{Duration, Instant};

use missnix_core::normalize::{days_ago_iso_utc, today_iso_utc};
use missnix_core::{
    is_deezer_preview_url_expired, scan_library_folders_with_options, AppPaths, AppSettings,
    Artist, ArtistDiscoveryMode, ArtistSearchCandidate, ArtistSource, ArtistStatus, CoreError,
    Database, FeedQuery, FeedSort, ProgressEvent, ProviderMode, Release, ReleaseState,
    ReleaseTrack, ReleaseType, ScanOptions, ScanOutcome, SyncEngine,
};
use slint::platform::WindowEvent;
use slint::{
    ComponentHandle, Image, Model, ModelRc, Rgba8Pixel, SharedPixelBuffer, SharedString, Timer,
    TimerMode, VecModel, Weak,
};

use crate::audio_player::{AudioPreviewHandle, PreviewRequest, PreviewUiUpdate};
use crate::folder_picker::pick_music_folder;
use crate::{
    AppWindow, ArtistRowData, FolderRowData, LocalAlbumRowData, ReleaseCardData,
    SearchCandidateRowData, TrackRowData,
};

pub enum ImageJob {
    ReleaseCover(Release),
    ArtistAvatar(Artist),
}

pub struct UiState {
    pub paths: AppPaths,
    pub db: Database,
    pub engine: SyncEngine,
    pub settings: AppSettings,
    pub audio: AudioPreviewHandle,
    pub cancel_flag: Arc<AtomicBool>,
    pub image_tx: mpsc::Sender<ImageJob>,
    pub decoded_covers: HashMap<i64, Image>,
    pub inflight_covers: HashSet<i64>,
    pub decoded_avatars: HashMap<i64, Image>,
    pub inflight_avatars: HashSet<i64>,
    pub release_model: Rc<VecModel<ReleaseCardData>>,
    pub artist_model: Rc<VecModel<ArtistRowData>>,
    pub base_scale_factor: Option<f32>,
    pub last_auto_check: Instant,
}

pub type SharedUiState = Rc<RefCell<UiState>>;
type WeakUiState = std::rc::Weak<RefCell<UiState>>;

pub fn wire_controller(
    app: &AppWindow,
    paths: AppPaths,
    cli_scale_override: Option<u16>,
) -> Result<(SharedUiState, (Timer, Timer)), String> {
    let mut db = Database::open(&paths.db_path).map_err(|e| e.to_string())?;
    let mut settings = db.load_settings().map_err(|e| e.to_string())?;
    if let Some(pct) = cli_scale_override {
        settings.ui_scale_percent = pct.clamp(75, 250);
        let _ = db.save_settings(&settings);
    }

    let engine = SyncEngine::new(paths.cover_cache_dir.clone());

    let (image_tx, image_rx) = mpsc::channel::<ImageJob>();
    let release_model = Rc::new(VecModel::<ReleaseCardData>::default());
    let artist_model = Rc::new(VecModel::<ArtistRowData>::default());
    app.set_releases(ModelRc::from(release_model.clone()));
    app.set_artists(ModelRc::from(artist_model.clone()));

    let weak_audio = app.as_weak();
    let audio = AudioPreviewHandle::spawn(
        paths.clone(),
        engine.clone(),
        settings.preview_volume,
        move |upd: PreviewUiUpdate| {
            let w = weak_audio.clone();
            let _ = slint::invoke_from_event_loop(move || {
                let Some(win) = w.upgrade() else { return };
                win.set_preview_active(upd.active);
                win.set_preview_playing(upd.playing);
                win.set_preview_buffering(upd.buffering);
                win.set_preview_release_id(upd.release_id);
                win.set_preview_disc_number(upd.disc_number);
                win.set_preview_track_number(upd.track_number);
                win.set_preview_track_title(upd.track_title.into());
                win.set_preview_artist_name(upd.artist_name.into());
                win.set_preview_release_title(upd.release_title.into());
                win.set_preview_progress(upd.progress);
                win.set_preview_elapsed_label(upd.elapsed_label.into());
                win.set_preview_duration_label(upd.duration_label.into());
                win.set_preview_level(upd.level);

                if let Some(err) = upd.error {
                    win.set_status_is_warning(true);
                    win.set_status_banner_text(err.into());
                }
            });
        },
    );

    let state = Rc::new(RefCell::new(UiState {
        paths: paths.clone(),
        db,
        engine: engine.clone(),
        settings: settings.clone(),
        audio,
        cancel_flag: Arc::new(AtomicBool::new(false)),
        image_tx,
        decoded_covers: HashMap::new(),
        inflight_covers: HashSet::new(),
        decoded_avatars: HashMap::new(),
        inflight_avatars: HashSet::new(),
        release_model,
        artist_model,
        base_scale_factor: None,
        last_auto_check: Instant::now(),
    }));

    // Apply persisted preferences to UI controls & Material 3 theme
    apply_settings_to_window(app, &settings);

    // Initial fast synchronous hydrate from SQLite
    refresh_folders_and_stats(app, &state.borrow());
    refresh_feed(app, &mut state.borrow_mut());
    refresh_artists(app, &mut state.borrow_mut());

    // Start persistent background cover-art & artist-avatar downloader/decoder worker
    spawn_image_worker(paths, engine, image_rx, app.as_weak(), &state);

    // Capture native monitor scale factor once winit maps the window, then apply persisted UI scale
    let startup_scale_timer = Timer::default();
    {
        let weak = app.as_weak();
        let st = state.clone();
        let env_has_slint_scale = std::env::var_os("SLINT_SCALE_FACTOR").is_some();
        startup_scale_timer.start(
            TimerMode::SingleShot,
            Duration::from_millis(25),
            move || {
                let Some(win) = weak.upgrade() else { return };
                let mut guard = st.borrow_mut();
                let native_sf = win.window().scale_factor();
                if guard.base_scale_factor.is_none() {
                    guard.base_scale_factor = Some(if native_sf > 0.0 { native_sf } else { 1.0 });
                }
                let pct = guard.settings.ui_scale_percent;
                if pct != 100 && (!env_has_slint_scale || cli_scale_override.is_some()) {
                    apply_ui_scale_to_window(&win, &mut guard, pct);
                }
            },
        );
    }

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
                if win.get_selected_release_id() == rel_id {
                    if let Ok(Some(updated)) = guard.db.get_release(rel.id) {
                        let card = to_release_card(&updated, guard.decoded_covers.get(&updated.id));
                        win.set_selected_release(card);
                    }
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
            if u.starts_with("https://") || u.starts_with("http://") {
                if let Err(err) = open::that_detached(u) {
                    if let Some(win) = weak.upgrade() {
                        win.set_status_is_warning(true);
                        win.set_status_banner_text(format!("Could not open browser: {err}").into());
                    }
                }
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
                let _ = guard.db.set_artist_status(art.id, next);
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
                let next = if art.status == ArtistStatus::Ignored {
                    ArtistStatus::Tracked
                } else {
                    ArtistStatus::Ignored
                };
                let _ = guard.db.set_artist_status(art.id, next);
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
            let _ = guard.db.delete_artist(artist_id as i64);
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
            } else if mbid_trimmed.len() != 36
                || mbid_trimmed.chars().filter(|&c| c == '-').count() != 4
            {
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
                    win.set_searching_external_artists(false);
                    match res {
                        Ok(cands) => {
                            CANDIDATE_STASH.with(|stash| {
                                *stash.borrow_mut() = cands.clone();
                            });
                            let rows: Vec<SearchCandidateRowData> = cands
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

    // -------------------------------------------------------------------------
    // Library Folders, UI Scale & Preferences Callbacks
    // -------------------------------------------------------------------------
    {
        let weak = app.as_weak();
        let st = state.clone();
        app.on_add_folder_path(move |raw_path| {
            let Some(win) = weak.upgrade() else { return };
            let added_ok = {
                let guard = st.borrow();
                match guard.db.add_library_folder(raw_path.as_str()) {
                    Ok(folder) => {
                        win.set_new_folder_path(SharedString::default());
                        win.set_status_is_warning(false);
                        win.set_status_banner_text(format!("Added folder {}", folder.path).into());
                        refresh_folders_and_stats(&win, &guard);
                        true
                    }
                    Err(err) => {
                        win.set_status_is_warning(true);
                        win.set_status_banner_text(err.to_string().into());
                        false
                    }
                }
            };
            if added_ok {
                start_library_scan(&weak, &st);
            }
        });
    }

    {
        let weak = app.as_weak();
        app.on_browse_for_folder(move || {
            let weak_done = weak.clone();
            thread::spawn(move || {
                let res = pick_music_folder();
                let _ = slint::invoke_from_event_loop(move || {
                    let Some(win) = weak_done.upgrade() else {
                        return;
                    };
                    match res {
                        Ok(Some(path)) => {
                            let s: SharedString = path.to_string_lossy().to_string().into();
                            win.set_new_folder_path(s.clone());
                            win.invoke_add_folder_path(s);
                        }
                        Ok(None) => {}
                        Err(msg) => {
                            win.set_status_is_warning(true);
                            win.set_status_banner_text(msg.into());
                        }
                    }
                });
            });
        });
    }

    {
        let weak = app.as_weak();
        let st = state.clone();
        app.on_remove_folder(move |folder_id| {
            let Some(win) = weak.upgrade() else { return };
            let has_remaining = {
                let mut guard = st.borrow_mut();
                let _ = guard.db.remove_library_folder(folder_id as i64);
                let remaining = guard
                    .db
                    .list_library_folders()
                    .map(|v| v.iter().any(|f| f.enabled))
                    .unwrap_or(false);
                if !remaining {
                    let _ = guard.db.apply_scan_outcome(&ScanOutcome::default());
                    refresh_artists(&win, &mut guard);
                    refresh_feed(&win, &mut guard);
                }
                refresh_folders_and_stats(&win, &guard);
                remaining
            };
            if has_remaining && !win.get_is_busy() {
                start_library_scan(&weak, &st);
            }
        });
    }

    {
        let weak = app.as_weak();
        let st = state.clone();
        app.on_preferences_changed(move || {
            let Some(win) = weak.upgrade() else { return };
            let mut guard = st.borrow_mut();
            let prev_vol = guard.settings.preview_volume;
            let mut updated = read_settings_from_window(&win);
            updated.preview_volume = prev_vol;
            win.set_feed_hide_owned(updated.hide_owned_locally);
            let _ = guard.db.save_settings(&updated);
            guard.settings = updated;
            refresh_feed(&win, &mut guard);
        });
    }

    {
        let weak = app.as_weak();
        let st = state.clone();
        app.on_scan_preferences_changed(move || {
            let Some(win) = weak.upgrade() else { return };
            let should_rescan = {
                let mut guard = st.borrow_mut();
                let prev_vol = guard.settings.preview_volume;
                let mut updated = read_settings_from_window(&win);
                updated.preview_volume = prev_vol;
                let _ = guard.db.save_settings(&updated);
                guard.settings = updated;
                !win.get_is_busy()
                    && guard
                        .db
                        .list_library_folders()
                        .map(|f| f.iter().any(|x| x.enabled))
                        .unwrap_or(false)
            };
            if should_rescan {
                start_library_scan(&weak, &st);
            }
        });
    }

    {
        let weak = app.as_weak();
        let st = state.clone();
        app.on_theme_toggled(move |dark| {
            let Some(win) = weak.upgrade() else { return };
            let mut guard = st.borrow_mut();
            guard.settings.dark_mode = dark;
            let accent = guard.settings.accent_preset as i32;
            let s = guard.settings.clone();
            let _ = guard.db.save_settings(&s);
            win.invoke_apply_theme(dark, accent);
        });
    }

    {
        let weak = app.as_weak();
        let st = state.clone();
        app.on_ui_scale_changed(move |pct| {
            let Some(win) = weak.upgrade() else { return };
            let clamped = (pct.clamp(75, 250)) as u16;
            let mut guard = st.borrow_mut();
            guard.settings.ui_scale_percent = clamped;
            let s = guard.settings.clone();
            let _ = guard.db.save_settings(&s);
            apply_ui_scale_to_window(&win, &mut guard, clamped);
            win.set_status_is_warning(false);
            win.set_status_banner_text(format!("Interface scale set to {clamped}%").into());
        });
    }

    // -------------------------------------------------------------------------
    // Periodic Background Schedule Timer (checks every 60s whether interval elapsed)
    // -------------------------------------------------------------------------
    let schedule_timer = Timer::default();
    {
        let weak = app.as_weak();
        let st = state.clone();
        schedule_timer.start(TimerMode::Repeated, Duration::from_secs(60), move || {
            let Some(win) = weak.upgrade() else { return };
            if win.get_is_busy() {
                return;
            }
            let should_trigger = {
                let mut guard = st.borrow_mut();
                let hours = guard.settings.auto_check_interval_hours;
                if hours == 0 {
                    false
                } else {
                    let interval = Duration::from_secs(u64::from(hours) * 3600);
                    if guard.last_auto_check.elapsed() >= interval {
                        guard.last_auto_check = Instant::now();
                        true
                    } else {
                        false
                    }
                }
            };
            if should_trigger {
                start_release_sync(&weak, &st, None);
            }
        });
    }

    Ok((state, (startup_scale_timer, schedule_timer)))
}

thread_local! {
    static CANDIDATE_STASH: RefCell<Vec<ArtistSearchCandidate>> = const { RefCell::new(Vec::new()) };
}

fn apply_ui_scale_to_window(win: &AppWindow, state: &mut UiState, scale_pct: u16) {
    let clamped = scale_pct.clamp(75, 250);
    win.set_pref_ui_scale_pct(i32::from(clamped));

    let base = *state.base_scale_factor.get_or_insert_with(|| {
        let sf = win.window().scale_factor();
        if sf > 0.0 {
            sf
        } else {
            1.0
        }
    });

    let effective = (base * (f32::from(clamped) / 100.0)).clamp(0.5, 4.0);
    let phys = win.window().size();
    win.window()
        .dispatch_event(WindowEvent::ScaleFactorChanged {
            scale_factor: effective,
        });
    if phys.width > 0 && phys.height > 0 {
        win.window().dispatch_event(WindowEvent::Resized {
            size: phys.to_logical(effective),
        });
    }
    win.window().request_redraw();
}

fn sync_artist_status_badges(
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

// =============================================================================
// Background Worker Spawners
// =============================================================================

fn start_library_scan(weak: &Weak<AppWindow>, state: &Rc<RefCell<UiState>>) {
    let Some(win) = weak.upgrade() else { return };
    if win.get_is_busy() {
        return;
    }

    let (folders, db_path, scan_options, cancel_flag) = {
        let mut guard = state.borrow_mut();
        let Ok(list) = guard.db.list_library_folders() else {
            return;
        };
        let enabled: Vec<_> = list
            .into_iter()
            .filter(|f| f.enabled)
            .map(|f| std::path::PathBuf::from(f.path))
            .collect();
        if enabled.is_empty() {
            win.set_status_is_warning(true);
            win.set_status_banner_text("Add at least one music folder first".into());
            win.set_active_view(2);
            return;
        }
        guard.cancel_flag = Arc::new(AtomicBool::new(false));
        (
            enabled,
            guard.paths.db_path.clone(),
            ScanOptions::from_settings(&guard.settings),
            guard.cancel_flag.clone(),
        )
    };

    win.set_is_busy(true);
    win.set_progress_indeterminate(true);
    win.set_progress_fraction(0.0);
    win.set_busy_label("Scanning music library...".into());

    let weak_prog = weak.clone();
    let weak_done = weak.clone();

    thread::spawn(move || {
        let scan_res = scan_library_folders_with_options(
            &folders,
            &scan_options,
            Some(&cancel_flag),
            |folder_name, count| {
                let label = format!("Scanned {count} files · {folder_name}");
                let w = weak_prog.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(win) = w.upgrade() {
                        win.set_busy_label(label.into());
                    }
                });
            },
        );

        let final_res = scan_res.and_then(|outcome| {
            let mut worker_db = Database::open(&db_path)?;
            worker_db.apply_scan_outcome(&outcome)?;
            Ok(outcome.summary)
        });

        let _ = slint::invoke_from_event_loop(move || {
            let Some(win) = weak_done.upgrade() else {
                return;
            };
            win.set_is_busy(false);
            win.set_progress_indeterminate(false);
            match final_res {
                Ok(sum) => {
                    win.set_status_is_warning(false);
                    win.set_status_banner_text(
                        format!(
                            "Discovered {} {} ({} {}) from {} {}",
                            sum.artists_discovered,
                            pluralize(sum.artists_discovered as i64, "artist", "artists"),
                            sum.local_albums_indexed,
                            pluralize(sum.local_albums_indexed as i64, "album", "albums"),
                            sum.audio_files_seen,
                            pluralize(sum.audio_files_seen as i64, "file", "files"),
                        )
                        .into(),
                    );
                }
                Err(CoreError::Cancelled) => {
                    win.set_status_is_warning(false);
                    win.set_status_banner_text("Library scan cancelled".into());
                }
                Err(err) => {
                    win.set_status_is_warning(true);
                    win.set_status_banner_text(format!("Scan error: {err}").into());
                }
            }
            win.invoke_feed_filters_changed();
            win.invoke_artist_filters_changed();
            win.invoke_navigate(win.get_active_view());
        });
    });
}

fn start_release_sync(
    weak: &Weak<AppWindow>,
    state: &Rc<RefCell<UiState>>,
    only_artist_id: Option<i64>,
) {
    let Some(win) = weak.upgrade() else { return };
    if win.get_is_busy() {
        return;
    }

    let (artists, settings, db_path, engine, cancel_flag) = {
        let mut guard = state.borrow_mut();
        let target_artists: Vec<Artist> = if let Some(aid) = only_artist_id {
            guard
                .db
                .get_artist(aid)
                .ok()
                .flatten()
                .into_iter()
                .collect()
        } else {
            guard.db.list_tracked_artists().unwrap_or_default()
        };

        if target_artists.is_empty() {
            win.set_status_is_warning(true);
            win.set_status_banner_text(
                "No tracked artists yet — scan your music folders first".into(),
            );
            return;
        }

        guard.cancel_flag = Arc::new(AtomicBool::new(false));
        guard.last_auto_check = Instant::now();
        (
            target_artists,
            guard.settings.clone(),
            guard.paths.db_path.clone(),
            guard.engine.clone(),
            guard.cancel_flag.clone(),
        )
    };

    win.set_is_busy(true);
    win.set_progress_indeterminate(false);
    win.set_progress_fraction(0.0);
    win.set_busy_label(
        format!(
            "Checking {} {}...",
            artists.len(),
            pluralize(artists.len() as i64, "artist", "artists")
        )
        .into(),
    );

    let weak_prog = weak.clone();
    let weak_done = weak.clone();

    thread::spawn(move || {
        let result = Database::open(&db_path).and_then(|mut worker_db| {
            engine.sync_artists(
                &mut worker_db,
                &artists,
                &settings,
                Some(&cancel_flag),
                |ev| {
                    if let ProgressEvent::SyncArtist {
                        current,
                        total,
                        artist_name,
                    } = ev
                    {
                        let frac = if total > 0 {
                            (current as f32) / (total as f32)
                        } else {
                            0.0
                        };
                        let lbl = format!("({current}/{total}) {artist_name}");
                        let refresh_live = current > 1 && current % 8 == 1;
                        let w = weak_prog.clone();
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(win) = w.upgrade() {
                                win.set_progress_fraction(frac);
                                win.set_busy_label(lbl.into());
                                if refresh_live {
                                    win.invoke_feed_filters_changed();
                                    win.invoke_artist_filters_changed();
                                }
                            }
                        });
                    }
                },
            )
        });

        let _ = slint::invoke_from_event_loop(move || {
            let Some(win) = weak_done.upgrade() else {
                return;
            };
            win.set_is_busy(false);
            win.set_progress_fraction(0.0);
            match result {
                Ok(sum) => {
                    if sum.offline_or_rate_limited && sum.artists_checked == 0 {
                        win.set_status_is_warning(true);
                        win.set_status_banner_text(
                            sum.last_error
                                .unwrap_or_else(|| {
                                    "Offline or rate-limited by provider".to_string()
                                })
                                .into(),
                        );
                    } else {
                        win.set_status_is_warning(sum.offline_or_rate_limited);
                        win.set_status_banner_text(
                            format!(
                                "Checked {} {} · {} new {}",
                                sum.artists_checked,
                                pluralize(sum.artists_checked as i64, "artist", "artists"),
                                sum.new_releases_added,
                                pluralize(sum.new_releases_added as i64, "drop", "drops"),
                            )
                            .into(),
                        );
                    }
                }
                Err(CoreError::Cancelled) => {
                    win.set_status_is_warning(false);
                    win.set_status_banner_text("Release check cancelled".into());
                }
                Err(err) => {
                    win.set_status_is_warning(true);
                    win.set_status_banner_text(format!("Sync error: {err}").into());
                }
            }
            win.invoke_feed_filters_changed();
            win.invoke_artist_filters_changed();
            win.invoke_navigate(win.get_active_view());
        });
    });
}

fn open_release_inspector(win: &AppWindow, state: &Rc<RefCell<UiState>>, release_id: i64) {
    let (release, cached_tracks, engine, db_path) = {
        let guard = state.borrow();
        let Ok(Some(rel)) = guard.db.get_release(release_id) else {
            return;
        };
        let artist_status = guard
            .db
            .get_artist(rel.artist_id)
            .ok()
            .flatten()
            .map(|a| a.status.as_str())
            .unwrap_or("tracked");

        let card = to_release_card(&rel, guard.decoded_covers.get(&rel.id));
        win.set_selected_release_artist_status(artist_status.into());
        win.set_selected_release(card);
        win.set_selected_release_id(release_id as i32);
        win.set_inspector_mode(1);

        let cached = guard.db.list_release_tracks(release_id).unwrap_or_default();
        (
            rel,
            cached,
            guard.engine.clone(),
            guard.paths.db_path.clone(),
        )
    };

    let has_expired_preview = cached_tracks.iter().any(|t| {
        t.preview_url
            .as_deref()
            .is_some_and(is_deezer_preview_url_expired)
    });

    if !cached_tracks.is_empty() {
        win.set_loading_tracks(false);
        win.set_selected_release_tracks(tracks_to_model(&cached_tracks));
        if !has_expired_preview {
            return;
        }
    } else {
        win.set_selected_release_tracks(ModelRc::new(VecModel::<TrackRowData>::default()));
        win.set_loading_tracks(true);
    }

    let weak = win.as_weak();
    thread::spawn(move || {
        let fetched = Database::open(&db_path).and_then(|mut worker_db| {
            engine.fetch_and_cache_release_tracks(&mut worker_db, &release)
        });

        let _ = slint::invoke_from_event_loop(move || {
            let Some(win) = weak.upgrade() else { return };
            if win.get_selected_release_id() != release.id as i32 {
                return;
            }
            win.set_loading_tracks(false);
            if let Ok(tracks) = fetched {
                if !tracks.is_empty() {
                    win.set_selected_release_tracks(tracks_to_model(&tracks));
                }
            }
        });
    });
}

fn spawn_image_worker(
    paths: AppPaths,
    engine: SyncEngine,
    rx: mpsc::Receiver<ImageJob>,
    weak: Weak<AppWindow>,
    state: &Rc<RefCell<UiState>>,
) {
    let st_ptr = Rc::downgrade(state);
    UI_STATE_SLOT.with(|slot| {
        *slot.borrow_mut() = Some(st_ptr);
    });

    thread::spawn(move || {
        let Ok(worker_db) = Database::open(&paths.db_path) else {
            return;
        };
        while let Ok(job) = rx.recv() {
            match job {
                ImageJob::ReleaseCover(release) => {
                    let Ok(Some(local_path)) =
                        engine.ensure_release_cover_cached(&worker_db, &release)
                    else {
                        continue;
                    };
                    let Some((w, h, rgba)) = decode_thumbnail_rgba(&local_path, 300) else {
                        continue;
                    };

                    let rel_id = release.id;
                    let weak_win = weak.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        let buf = SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(&rgba, w, h);
                        let slint_img = Image::from_rgba8(buf);

                        UI_STATE_SLOT.with(|slot| {
                            let Some(rc_st) = slot.borrow().as_ref().and_then(std::rc::Weak::upgrade)
                            else {
                                return;
                            };
                            let mut guard = rc_st.borrow_mut();
                            guard.inflight_covers.remove(&rel_id);
                            guard.decoded_covers.insert(rel_id, slint_img.clone());

                            let row_count = guard.release_model.row_count();
                            for idx in 0..row_count {
                                if let Some(mut row) = guard.release_model.row_data(idx) {
                                    if row.id == rel_id as i32 {
                                        row.cover = slint_img.clone();
                                        row.has_cover = true;
                                        guard.release_model.set_row_data(idx, row);
                                        break;
                                    }
                                }
                            }

                            if let Some(win) = weak_win.upgrade() {
                                if win.get_selected_release_id() == rel_id as i32 {
                                    let mut sel = win.get_selected_release();
                                    sel.cover = slint_img.clone();
                                    sel.has_cover = true;
                                    win.set_selected_release(sel);
                                }
                                if win.get_preview_release_id() == rel_id as i32 {
                                    win.set_preview_cover(slint_img);
                                    win.set_preview_has_cover(true);
                                }
                            }
                        });
                    });
                }
                ImageJob::ArtistAvatar(artist) => {
                    let Ok(Some(local_path)) =
                        engine.ensure_artist_avatar_cached(&paths.avatar_cache_dir, &artist)
                    else {
                        continue;
                    };
                    let Some((w, h, rgba)) = decode_thumbnail_rgba(&local_path, 128) else {
                        continue;
                    };

                    let art_id = artist.id;
                    let weak_win = weak.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        let buf = SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(&rgba, w, h);
                        let slint_img = Image::from_rgba8(buf);

                        UI_STATE_SLOT.with(|slot| {
                            let Some(rc_st) = slot.borrow().as_ref().and_then(std::rc::Weak::upgrade)
                            else {
                                return;
                            };
                            let mut guard = rc_st.borrow_mut();
                            guard.inflight_avatars.remove(&art_id);
                            guard.decoded_avatars.insert(art_id, slint_img.clone());

                            let row_count = guard.artist_model.row_count();
                            for idx in 0..row_count {
                                if let Some(mut row) = guard.artist_model.row_data(idx) {
                                    if row.id == art_id as i32 {
                                        row.avatar = slint_img.clone();
                                        row.has_avatar = true;
                                        guard.artist_model.set_row_data(idx, row);
                                        break;
                                    }
                                }
                            }

                            if let Some(win) = weak_win.upgrade() {
                                if win.get_selected_artist_id() == art_id as i32 {
                                    let mut sel = win.get_selected_artist();
                                    sel.avatar = slint_img;
                                    sel.has_avatar = true;
                                    win.set_selected_artist(sel);
                                }
                            }
                        });
                    });
                }
            }
        }
    });
}

thread_local! {
    static UI_STATE_SLOT: RefCell<Option<WeakUiState>> = const { RefCell::new(None) };
}

fn decode_thumbnail_rgba(path: &Path, max_dim: u32) -> Option<(u32, u32, Vec<u8>)> {
    let dyn_img = image::open(path).ok()?;
    let thumb = if dyn_img.width() > max_dim || dyn_img.height() > max_dim {
        dyn_img.thumbnail(max_dim, max_dim)
    } else {
        dyn_img
    };
    let rgba = thumb.to_rgba8();
    let (w, h) = rgba.dimensions();
    Some((w, h, rgba.into_raw()))
}

// =============================================================================
// UI Hydration & Formatting Helpers
// =============================================================================

fn apply_settings_to_window(win: &AppWindow, s: &AppSettings) {
    win.set_pref_dark_mode(s.dark_mode);
    win.set_pref_accent_preset(i32::from(s.accent_preset));
    win.set_pref_grid_density(i32::from(s.grid_density));
    win.set_pref_ui_scale_pct(i32::from(s.ui_scale_percent));
    win.set_preview_volume_pct(i32::from(s.preview_volume));
    win.set_pref_discovery_mode(s.artist_discovery_mode.index());
    win.set_pref_split_collaborations(s.split_collaborations);
    win.set_pref_include_albums(s.include_albums);
    win.set_pref_include_eps(s.include_eps);
    win.set_pref_include_singles(s.include_singles);
    win.set_pref_hide_owned(s.hide_owned_locally);
    win.set_feed_hide_owned(s.hide_owned_locally);
    win.set_pref_hide_noise(s.hide_compilations_and_live);
    win.set_pref_include_compilations(s.include_compilation_artists);
    win.set_pref_lookback_index(s.lookback_index());
    win.set_pref_provider_index(s.provider_mode.index());
    win.set_pref_schedule_index(s.interval_index());
    win.invoke_apply_theme(s.dark_mode, i32::from(s.accent_preset));
}

fn read_settings_from_window(win: &AppWindow) -> AppSettings {
    AppSettings {
        dark_mode: win.get_pref_dark_mode(),
        accent_preset: win.get_pref_accent_preset().clamp(0, 5) as u8,
        grid_density: win.get_pref_grid_density().clamp(0, 2) as u8,
        ui_scale_percent: win.get_pref_ui_scale_pct().clamp(75, 250) as u16,
        preview_volume: win.get_preview_volume_pct().clamp(0, 100) as u8,
        lookback_days: AppSettings::lookback_days_from_index(win.get_pref_lookback_index()),
        include_albums: win.get_pref_include_albums(),
        include_eps: win.get_pref_include_eps(),
        include_singles: win.get_pref_include_singles(),
        hide_owned_locally: win.get_pref_hide_owned(),
        hide_compilations_and_live: win.get_pref_hide_noise(),
        include_compilation_artists: win.get_pref_include_compilations(),
        artist_discovery_mode: ArtistDiscoveryMode::from_index(win.get_pref_discovery_mode()),
        split_collaborations: win.get_pref_split_collaborations(),
        auto_check_interval_hours: AppSettings::interval_hours_from_index(
            win.get_pref_schedule_index(),
        ),
        provider_mode: ProviderMode::from_index(win.get_pref_provider_index()),
    }
}

fn refresh_folders_and_stats(win: &AppWindow, state: &UiState) {
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

fn refresh_feed(win: &AppWindow, state: &mut UiState) {
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

fn refresh_artists(win: &AppWindow, state: &mut UiState) {
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

fn select_artist_detail(win: &AppWindow, state: &mut UiState, artist_id: i64) {
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

fn to_release_card(rel: &Release, cover_img: Option<&Image>) -> ReleaseCardData {
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

fn to_artist_row(art: &Artist, avatar_img: Option<&Image>, today_iso: &str) -> ArtistRowData {
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

fn tracks_to_model(tracks: &[ReleaseTrack]) -> ModelRc<TrackRowData> {
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

fn make_monogram(name: &str) -> String {
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

fn pluralize<'a>(count: i64, singular: &'a str, plural: &'a str) -> &'a str {
    if count == 1 {
        singular
    } else {
        plural
    }
}

fn compact_timestamp(rfc3339: &str, today_iso: &str) -> String {
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
