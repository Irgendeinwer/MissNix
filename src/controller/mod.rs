mod artist_actions;
mod hydrate;
mod radar_actions;
mod settings_actions;
mod workers;

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use missnix_core::{
    AppPaths, AppSettings, Artist, ArtistSearchCandidate, Database, Release, SyncEngine,
};
use slint::{ComponentHandle, Image, ModelRc, Timer, TimerMode, VecModel};

use crate::audio_player::{AudioPreviewHandle, PreviewUiUpdate};
use crate::{AppWindow, ArtistRowData, ReleaseCardData};

use self::artist_actions::wire_artist_callbacks;
use self::hydrate::{refresh_artists, refresh_feed, refresh_folders_and_stats};
use self::radar_actions::wire_radar_and_preview_callbacks;
use self::settings_actions::{
    apply_settings_to_window, apply_ui_scale_to_window, wire_settings_callbacks,
};
use self::workers::{spawn_image_worker, start_release_sync};

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
    pub applied_scale_factor: Option<f32>,
    pub last_auto_check: Instant,
}

pub type SharedUiState = Rc<RefCell<UiState>>;
pub(super) type WeakUiState = std::rc::Weak<RefCell<UiState>>;

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
        applied_scale_factor: None,
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

    // Keep persisted UI scale synchronized once winit/Wayland maps & configures the surface.
    // winit's `ensure_window()` and Wayland's post-map `fractional_scale` / `preferred_buffer_scale`
    // events reset `window().scale_factor()` after startup, so we watch for window map and
    // native monitor DPI changes and re-apply `base_scale * (ui_scale_percent / 100)`.
    let startup_scale_timer = Timer::default();
    {
        let weak = app.as_weak();
        let st = state.clone();
        let env_has_slint_scale = std::env::var_os("SLINT_SCALE_FACTOR").is_some();
        let allow_custom_scale = !env_has_slint_scale || cli_scale_override.is_some();
        startup_scale_timer.start(TimerMode::Repeated, Duration::from_millis(80), move || {
            if !allow_custom_scale {
                return;
            }
            let Some(win) = weak.upgrade() else { return };
            let phys = win.window().size();
            if phys.width == 0 || phys.height == 0 {
                return;
            }
            let current_sf = {
                let sf = win.window().scale_factor();
                if sf > 0.0 { sf } else { 1.0 }
            };
            let mut guard = st.borrow_mut();
            let pct = guard.settings.ui_scale_percent.clamp(75, 250);

            match guard.applied_scale_factor {
                None => {
                    guard.base_scale_factor = Some(current_sf);
                    if pct != 100 {
                        apply_ui_scale_to_window(&win, &mut guard, pct);
                    } else {
                        guard.applied_scale_factor = Some(current_sf);
                    }
                }
                Some(applied) if (current_sf - applied).abs() > 0.005 => {
                    guard.base_scale_factor = Some(current_sf);
                    if pct != 100 {
                        apply_ui_scale_to_window(&win, &mut guard, pct);
                    } else {
                        guard.applied_scale_factor = Some(current_sf);
                    }
                }
                _ => {}
            }
        });
    }

    wire_radar_and_preview_callbacks(app, &state);
    wire_artist_callbacks(app, &state);
    wire_settings_callbacks(app, &state);

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
    pub(super) static CANDIDATE_STASH: RefCell<Vec<ArtistSearchCandidate>> = const { RefCell::new(Vec::new()) };
}
