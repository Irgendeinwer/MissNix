use std::thread;

use missnix_core::{
    AppSettings, ArtistDiscoveryMode, CUSTOM_ACCENT_PRESET, DEFAULT_CUSTOM_ACCENT_HEX,
    MAX_ACCENT_PRESET, ProviderMode, ScanOutcome, normalize_hex_color, resolve_theme_palette,
};
use slint::platform::WindowEvent;
use slint::{Color, ComponentHandle, SharedString};

use crate::folder_picker::pick_music_folder;
use crate::{AppWindow, Theme};

use super::hydrate::{refresh_artists, refresh_feed, refresh_folders_and_stats};
use super::workers::start_library_scan;
use super::{SharedUiState, UiState};

pub(super) fn wire_settings_callbacks(app: &AppWindow, state: &SharedUiState) {
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
            let prev_hex = guard.settings.custom_accent_hex.clone();
            let mut updated = read_settings_from_window(&win);
            updated.preview_volume = prev_vol;
            if normalize_hex_color(&updated.custom_accent_hex).is_none() {
                updated.custom_accent_hex = prev_hex;
            }
            apply_theme_to_window(&win, &updated);
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
            let s = guard.settings.clone();
            let _ = guard.db.save_settings(&s);
            apply_theme_to_window(&win, &s);
        });
    }

    {
        let weak = app.as_weak();
        let st = state.clone();
        app.on_custom_accent_changed(move |raw_hex, finalize| {
            let Some(win) = weak.upgrade() else { return };
            if let Some(norm) = normalize_hex_color(&raw_hex) {
                let mut guard = st.borrow_mut();
                guard.settings.custom_accent_hex = norm.clone();
                guard.settings.accent_preset = CUSTOM_ACCENT_PRESET;
                let s = guard.settings.clone();
                let _ = guard.db.save_settings(&s);
                if finalize {
                    win.set_pref_custom_accent_hex(norm.clone().into());
                    win.set_status_is_warning(false);
                    win.set_status_banner_text(
                        format!("Applied custom Material 3 palette ({norm})").into(),
                    );
                }
                apply_theme_to_window(&win, &s);
            } else if finalize {
                win.set_status_is_warning(true);
                win.set_status_banner_text(
                    "Enter a valid 3- or 6-digit hex color such as #EA6C34 or #38BDF8.".into(),
                );
            }
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
}

pub(super) fn apply_ui_scale_to_window(win: &AppWindow, state: &mut UiState, scale_pct: u16) {
    let clamped = scale_pct.clamp(75, 250);
    win.set_pref_ui_scale_pct(i32::from(clamped));

    let base = *state.base_scale_factor.get_or_insert_with(|| {
        let sf = win.window().scale_factor();
        if sf > 0.0 { sf } else { 1.0 }
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

fn rgb(c: [u8; 3]) -> Color {
    Color::from_rgb_u8(c[0], c[1], c[2])
}

pub(super) fn apply_theme_to_window(win: &AppWindow, s: &AppSettings) {
    win.invoke_apply_theme(s.dark_mode, i32::from(s.accent_preset));
    let p = resolve_theme_palette(s.accent_preset, &s.custom_accent_hex, s.dark_mode);
    let theme = win.global::<Theme>();
    theme.set_dark_mode(s.dark_mode);
    theme.set_accent_preset(i32::from(s.accent_preset));
    theme.set_custom_swatch(rgb(p.custom_swatch));

    theme.set_surface(rgb(p.surface));
    theme.set_surface_rail(rgb(p.surface_rail));
    theme.set_surface_container_low(rgb(p.surface_container_low));
    theme.set_surface_container(rgb(p.surface_container));
    theme.set_surface_container_high(rgb(p.surface_container_high));
    theme.set_surface_container_highest(rgb(p.surface_container_highest));

    theme.set_on_surface(rgb(p.on_surface));
    theme.set_on_surface_variant(rgb(p.on_surface_variant));
    theme.set_on_surface_muted(rgb(p.on_surface_muted));

    theme.set_outline(rgb(p.outline));
    theme.set_outline_variant(rgb(p.outline_variant));

    theme.set_primary(rgb(p.primary));
    theme.set_on_primary(rgb(p.on_primary));
    theme.set_primary_container(rgb(p.primary_container));
    theme.set_on_primary_container(rgb(p.on_primary_container));

    theme.set_secondary_container(rgb(p.secondary_container));
    theme.set_on_secondary_container(rgb(p.on_secondary_container));
}

pub(super) fn apply_settings_to_window(win: &AppWindow, s: &AppSettings) {
    win.set_pref_dark_mode(s.dark_mode);
    win.set_pref_accent_preset(i32::from(s.accent_preset));
    win.set_pref_custom_accent_hex(s.custom_accent_hex.clone().into());
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
    apply_theme_to_window(win, s);
}

pub(super) fn read_settings_from_window(win: &AppWindow) -> AppSettings {
    let custom_hex = normalize_hex_color(&win.get_pref_custom_accent_hex())
        .unwrap_or_else(|| DEFAULT_CUSTOM_ACCENT_HEX.to_string());
    AppSettings {
        dark_mode: win.get_pref_dark_mode(),
        accent_preset: win
            .get_pref_accent_preset()
            .clamp(0, i32::from(MAX_ACCENT_PRESET)) as u8,
        custom_accent_hex: custom_hex,
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
