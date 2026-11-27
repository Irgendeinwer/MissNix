use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::Instant;

use missnix_core::{
    AppPaths, Artist, CoreError, Database, ProgressEvent, ScanOutcome, SyncEngine,
    is_deezer_preview_url_expired, scan_library_folders_with_options,
};
use slint::{
    ComponentHandle, Image, Model, ModelRc, Rgba8Pixel, SharedPixelBuffer, VecModel, Weak,
};

use crate::{AppWindow, TrackRowData};

use super::hydrate::{pluralize, to_release_card, tracks_to_model};
use super::{ImageJob, UiState, WeakUiState};

/// Changes made during a scan or sync must be reconciled after that task ends.
pub(super) fn request_library_scan(weak: &Weak<AppWindow>, state: &Rc<RefCell<UiState>>) {
    let Some(win) = weak.upgrade() else { return };
    if win.get_is_busy() {
        win.set_scan_pending(true);
        win.set_status_is_warning(false);
        win.set_status_banner_text("Library changed; a rescan is queued.".into());
    } else {
        start_library_scan(weak, state);
    }
}

fn start_queued_scan(win: &AppWindow) {
    if win.get_scan_pending() {
        win.set_scan_pending(false);
        win.invoke_scan_library();
    }
}

pub(super) fn start_library_scan(weak: &Weak<AppWindow>, state: &Rc<RefCell<UiState>>) {
    let Some(win) = weak.upgrade() else { return };
    if win.get_is_busy() {
        return;
    }

    let (snapshot, db_path, cancel_flag) = {
        let mut guard = state.borrow_mut();
        let snapshot = match guard.db.prepare_library_scan() {
            Ok(snapshot) => snapshot,
            Err(err) => {
                win.set_status_is_warning(true);
                win.set_status_banner_text(format!("Cannot prepare scan: {err}").into());
                return;
            }
        };
        guard.cancel_flag = Arc::new(AtomicBool::new(false));
        (
            snapshot,
            guard.paths.db_path.clone(),
            guard.cancel_flag.clone(),
        )
    };

    // Removing the final folder is an intentional empty library, not a scan of
    // an inaccessible mount. Revision checking still prevents old workers from
    // restoring the removed folder's snapshot.
    if snapshot.folders.is_empty() {
        let result = state.borrow_mut().db.apply_scan_outcome_checked(
            &ScanOutcome::default(),
            &snapshot,
            false,
            None,
        );
        match result {
            Ok(()) => {
                win.set_status_is_warning(false);
                win.set_status_banner_text(
                    "No music folders selected; previous artist history was retained.".into(),
                );
            }
            Err(err) => {
                win.set_status_is_warning(true);
                win.set_status_banner_text(format!("Scan error: {err}").into());
            }
        }
        win.invoke_feed_filters_changed();
        win.invoke_artist_filters_changed();
        win.invoke_navigate(win.get_active_view());
        return;
    }

    win.set_is_busy(true);
    win.set_progress_indeterminate(true);
    win.set_progress_fraction(0.0);
    win.set_busy_label("Scanning music library...".into());

    let weak_prog = weak.clone();
    let weak_done = weak.clone();

    thread::spawn(move || {
        let scan_res = scan_library_folders_with_options(
            &snapshot.folders,
            &snapshot.options,
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
            worker_db.apply_scan_outcome_checked(&outcome, &snapshot, false, Some(&cancel_flag))?;
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
            start_queued_scan(&win);
        });
    });
}

pub(super) fn start_release_sync(
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
        if only_artist_id.is_none() {
            guard.last_auto_check = Instant::now();
        }
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
        let total_artists = artists.len();
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
                    let not_attempted =
                        total_artists.saturating_sub(sum.artists_checked + sum.artists_failed);
                    let incomplete =
                        sum.artists_failed > 0 || sum.artists_partial > 0 || not_attempted > 0;
                    win.set_status_is_warning(incomplete);
                    let mut message = format!(
                        "Checked {} {} · {} new {}",
                        sum.artists_checked,
                        pluralize(sum.artists_checked as i64, "artist", "artists"),
                        sum.new_releases_added,
                        pluralize(sum.new_releases_added as i64, "drop", "drops"),
                    );
                    if incomplete {
                        message.push_str(&format!(
                            " · {} partial, {} failed, {} not checked",
                            sum.artists_partial, sum.artists_failed, not_attempted
                        ));
                        if let Some(error) = sum.last_error {
                            message.push_str(&format!(". {error}"));
                        }
                    }
                    win.set_status_banner_text(message.into());
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
            start_queued_scan(&win);
        });
    });
}

pub(super) fn open_release_inspector(
    win: &AppWindow,
    state: &Rc<RefCell<UiState>>,
    release_id: i64,
) {
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
            if let Ok(tracks) = fetched
                && !tracks.is_empty()
            {
                win.set_selected_release_tracks(tracks_to_model(&tracks));
            }
        });
    });
}

pub(super) fn spawn_image_worker(
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
                        clear_failed_image_job(release.id, true);
                        continue;
                    };
                    let Some((w, h, rgba)) = decode_thumbnail_rgba(&local_path, 300) else {
                        let _ = std::fs::remove_file(&local_path);
                        clear_failed_image_job(release.id, true);
                        continue;
                    };

                    let rel_id = release.id;
                    let weak_win = weak.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        let buf = SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(&rgba, w, h);
                        let slint_img = Image::from_rgba8(buf);

                        UI_STATE_SLOT.with(|slot| {
                            let Some(rc_st) =
                                slot.borrow().as_ref().and_then(std::rc::Weak::upgrade)
                            else {
                                return;
                            };
                            let mut guard = rc_st.borrow_mut();
                            guard.inflight_covers.remove(&rel_id);
                            guard.decoded_covers.insert(rel_id, slint_img.clone());

                            let row_count = guard.release_model.row_count();
                            for idx in 0..row_count {
                                if let Some(mut row) = guard.release_model.row_data(idx)
                                    && row.id == rel_id as i32
                                {
                                    row.cover = slint_img.clone();
                                    row.has_cover = true;
                                    guard.release_model.set_row_data(idx, row);
                                    break;
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
                        clear_failed_image_job(artist.id, false);
                        continue;
                    };
                    let Some((w, h, rgba)) = decode_thumbnail_rgba(&local_path, 128) else {
                        let _ = std::fs::remove_file(&local_path);
                        clear_failed_image_job(artist.id, false);
                        continue;
                    };

                    let art_id = artist.id;
                    let weak_win = weak.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        let buf = SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(&rgba, w, h);
                        let slint_img = Image::from_rgba8(buf);

                        UI_STATE_SLOT.with(|slot| {
                            let Some(rc_st) =
                                slot.borrow().as_ref().and_then(std::rc::Weak::upgrade)
                            else {
                                return;
                            };
                            let mut guard = rc_st.borrow_mut();
                            guard.inflight_avatars.remove(&art_id);
                            guard.decoded_avatars.insert(art_id, slint_img.clone());

                            let row_count = guard.artist_model.row_count();
                            for idx in 0..row_count {
                                if let Some(mut row) = guard.artist_model.row_data(idx)
                                    && row.id == art_id as i32
                                {
                                    row.avatar = slint_img.clone();
                                    row.has_avatar = true;
                                    guard.artist_model.set_row_data(idx, row);
                                    break;
                                }
                            }

                            if let Some(win) = weak_win.upgrade()
                                && win.get_selected_artist_id() == art_id as i32
                            {
                                let mut sel = win.get_selected_artist();
                                sel.avatar = slint_img;
                                sel.has_avatar = true;
                                win.set_selected_artist(sel);
                            }
                        });
                    });
                }
            }
        }
    });
}

fn clear_failed_image_job(id: i64, is_cover: bool) {
    let _ = slint::invoke_from_event_loop(move || {
        UI_STATE_SLOT.with(|slot| {
            if let Some(state) = slot.borrow().as_ref().and_then(std::rc::Weak::upgrade) {
                let mut guard = state.borrow_mut();
                if is_cover {
                    guard.inflight_covers.remove(&id);
                } else {
                    guard.inflight_avatars.remove(&id);
                }
            }
        });
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
