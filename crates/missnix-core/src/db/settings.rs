use rusqlite::params;

use crate::error::CoreResult;
use crate::models::{AppSettings, ArtistDiscoveryMode, ProviderMode};

use super::Database;

impl Database {
    pub fn load_settings(&self) -> CoreResult<AppSettings> {
        let mut s = AppSettings::default();
        s.apply_env_theme_defaults(
            std::env::var("MISSNIX_DEFAULT_DARK_MODE").ok().as_deref(),
            std::env::var("MISSNIX_DEFAULT_ACCENT_HEX").ok().as_deref(),
        );
        let mut stmt = self.conn.prepare("SELECT key, value FROM settings")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        for row in rows {
            let (k, v) = row?;
            match k.as_str() {
                "dark_mode" => s.dark_mode = v == "true",
                "accent_preset" => {
                    if let Ok(n) = v.parse::<u8>() {
                        s.accent_preset = n.min(crate::palette::MAX_ACCENT_PRESET);
                    }
                }
                "custom_accent_hex" => {
                    if let Some(norm) = crate::palette::normalize_hex_color(&v) {
                        s.custom_accent_hex = norm;
                    }
                }
                "grid_density" => {
                    if let Ok(n) = v.parse::<u8>() {
                        s.grid_density = n.min(2);
                    }
                }
                "ui_scale_percent" => {
                    if let Ok(n) = v.parse::<u16>() {
                        s.ui_scale_percent = n.clamp(75, 250);
                    }
                }
                "preview_volume" => {
                    if let Ok(n) = v.parse::<u8>() {
                        s.preview_volume = n.min(100);
                    }
                }
                "lookback_days" => {
                    if let Ok(n) = v.parse::<u32>() {
                        s.lookback_days = n;
                    }
                }
                "include_albums" => s.include_albums = v == "true",
                "include_eps" => s.include_eps = v == "true",
                "include_singles" => s.include_singles = v == "true",
                "hide_owned_locally" => s.hide_owned_locally = v == "true",
                "hide_compilations_and_live" => s.hide_compilations_and_live = v == "true",
                "include_compilation_artists" => s.include_compilation_artists = v == "true",
                "artist_discovery_mode" => {
                    s.artist_discovery_mode = ArtistDiscoveryMode::from_db(&v)
                }
                "split_collaborations" => s.split_collaborations = v == "true",
                "auto_check_interval_hours" => {
                    if let Ok(n) = v.parse::<u32>() {
                        s.auto_check_interval_hours = n;
                    }
                }
                "provider_mode" => s.provider_mode = ProviderMode::from_db(&v),
                _ => {}
            }
        }
        Ok(s)
    }

    pub fn save_settings(&mut self, s: &AppSettings) -> CoreResult<()> {
        let tx = self.conn.transaction()?;
        let normalized_hex = crate::palette::normalize_hex_color(&s.custom_accent_hex)
            .unwrap_or_else(|| crate::palette::DEFAULT_CUSTOM_ACCENT_HEX.to_string());
        let pairs: [(&str, String); 17] = [
            ("dark_mode", s.dark_mode.to_string()),
            (
                "accent_preset",
                s.accent_preset
                    .min(crate::palette::MAX_ACCENT_PRESET)
                    .to_string(),
            ),
            ("custom_accent_hex", normalized_hex),
            ("grid_density", s.grid_density.to_string()),
            (
                "ui_scale_percent",
                s.ui_scale_percent.clamp(75, 250).to_string(),
            ),
            ("preview_volume", s.preview_volume.to_string()),
            ("lookback_days", s.lookback_days.to_string()),
            ("include_albums", s.include_albums.to_string()),
            ("include_eps", s.include_eps.to_string()),
            ("include_singles", s.include_singles.to_string()),
            ("hide_owned_locally", s.hide_owned_locally.to_string()),
            (
                "hide_compilations_and_live",
                s.hide_compilations_and_live.to_string(),
            ),
            (
                "include_compilation_artists",
                s.include_compilation_artists.to_string(),
            ),
            (
                "artist_discovery_mode",
                s.artist_discovery_mode.as_str().to_string(),
            ),
            ("split_collaborations", s.split_collaborations.to_string()),
            (
                "auto_check_interval_hours",
                s.auto_check_interval_hours.to_string(),
            ),
            ("provider_mode", s.provider_mode.as_str().to_string()),
        ];
        for (k, v) in pairs {
            tx.execute(
                "INSERT INTO settings (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![k, v],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
}
