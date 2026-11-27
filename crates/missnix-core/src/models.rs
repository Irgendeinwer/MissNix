use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtistStatus {
    Tracked,
    Muted,
    Ignored,
    /// A locally discovered artist whose files are no longer in the scanned library.
    /// Its releases and user state are retained and tracking resumes if it reappears.
    Missing,
}

impl ArtistStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Tracked => "tracked",
            Self::Muted => "muted",
            Self::Ignored => "ignored",
            Self::Missing => "missing",
        }
    }

    pub fn from_db(value: &str) -> Self {
        match value {
            "muted" => Self::Muted,
            "ignored" => Self::Ignored,
            "missing" => Self::Missing,
            _ => Self::Tracked,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Tracked => "Tracked",
            Self::Muted => "Muted",
            Self::Ignored => "Ignored",
            Self::Missing => "Not in library",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtistSource {
    Local,
    Manual,
}

impl ArtistSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Manual => "manual",
        }
    }

    pub fn from_db(value: &str) -> Self {
        match value {
            "manual" => Self::Manual,
            _ => Self::Local,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReleaseType {
    Album,
    Ep,
    Single,
}

impl ReleaseType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Album => "album",
            Self::Ep => "ep",
            Self::Single => "single",
        }
    }

    pub fn from_db(value: &str) -> Self {
        match value.to_ascii_lowercase().as_str() {
            "ep" => Self::Ep,
            "single" => Self::Single,
            _ => Self::Album,
        }
    }

    pub fn badge_label(self) -> &'static str {
        match self {
            Self::Album => "ALBUM",
            Self::Ep => "EP",
            Self::Single => "SINGLE",
        }
    }

    pub fn rank(self) -> u8 {
        match self {
            Self::Album => 3,
            Self::Ep => 2,
            Self::Single => 1,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReleaseState {
    Unseen,
    Seen,
    Dismissed,
}

impl ReleaseState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unseen => "unseen",
            Self::Seen => "seen",
            Self::Dismissed => "dismissed",
        }
    }

    pub fn from_db(value: &str) -> Self {
        match value {
            "seen" => Self::Seen,
            "dismissed" => Self::Dismissed,
            _ => Self::Unseen,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderMode {
    DeezerAndMusicBrainz,
    DeezerOnly,
    MusicBrainzOnly,
}

impl ProviderMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DeezerAndMusicBrainz => "deezer_and_musicbrainz",
            Self::DeezerOnly => "deezer_only",
            Self::MusicBrainzOnly => "musicbrainz_only",
        }
    }

    pub fn from_db(value: &str) -> Self {
        match value {
            "deezer_only" => Self::DeezerOnly,
            "musicbrainz_only" => Self::MusicBrainzOnly,
            _ => Self::DeezerAndMusicBrainz,
        }
    }

    pub fn index(self) -> i32 {
        match self {
            Self::DeezerAndMusicBrainz => 0,
            Self::DeezerOnly => 1,
            Self::MusicBrainzOnly => 2,
        }
    }

    pub fn from_index(index: i32) -> Self {
        match index {
            1 => Self::DeezerOnly,
            2 => Self::MusicBrainzOnly,
            _ => Self::DeezerAndMusicBrainz,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtistDiscoveryMode {
    /// Prefer `ALBUMARTIST`, then structured `<Artist>/<Album>` folder names, then cleaned track `ARTIST`.
    AlbumArtistOrFolder,
    /// Infer artists strictly from `<MusicRoot>/<Artist>/<Album>` directory names.
    FolderOnly,
    /// Use `ALBUMARTIST`, then cleaned track `ARTIST`, then folder fallback.
    AllTagsAndFolder,
}

impl ArtistDiscoveryMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AlbumArtistOrFolder => "album_artist_or_folder",
            Self::FolderOnly => "folder_only",
            Self::AllTagsAndFolder => "all_tags_and_folder",
        }
    }

    pub fn from_db(value: &str) -> Self {
        match value {
            "folder_only" => Self::FolderOnly,
            "all_tags_and_folder" => Self::AllTagsAndFolder,
            _ => Self::AlbumArtistOrFolder,
        }
    }

    pub fn index(self) -> i32 {
        match self {
            Self::AlbumArtistOrFolder => 0,
            Self::FolderOnly => 1,
            Self::AllTagsAndFolder => 2,
        }
    }

    pub fn from_index(index: i32) -> Self {
        match index {
            1 => Self::FolderOnly,
            2 => Self::AllTagsAndFolder,
            _ => Self::AlbumArtistOrFolder,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FeedSort {
    NewestFirst,
    UpcomingFirst,
    ArtistAsc,
}

impl FeedSort {
    pub fn from_index(index: i32) -> Self {
        match index {
            1 => Self::UpcomingFirst,
            2 => Self::ArtistAsc,
            _ => Self::NewestFirst,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LibraryFolder {
    pub id: i64,
    pub path: String,
    pub enabled: bool,
    pub track_count: i64,
    pub last_scanned_at: Option<String>,
    pub added_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Artist {
    pub id: i64,
    pub name: String,
    pub normalized_name: String,
    pub mbid: Option<String>,
    pub deezer_id: Option<i64>,
    pub disambiguation: Option<String>,
    pub avatar_url: Option<String>,
    pub external_url: Option<String>,
    pub status: ArtistStatus,
    pub source: ArtistSource,
    pub local_album_count: i64,
    pub local_track_count: i64,
    pub discovered_release_count: i64,
    pub unseen_release_count: i64,
    pub last_checked_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalAlbum {
    pub id: i64,
    pub artist_id: i64,
    pub title: String,
    pub normalized_title: String,
    pub release_year: Option<i32>,
    pub mb_release_group_id: Option<String>,
    pub mb_album_id: Option<String>,
    pub track_count: i64,
    pub sample_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Release {
    pub id: i64,
    pub artist_id: i64,
    pub artist_name: String,
    pub title: String,
    pub normalized_title: String,
    pub release_type: ReleaseType,
    /// ISO-8601 date (`YYYY-MM-DD`, `YYYY-MM`, or `YYYY`), empty if unknown.
    pub release_date: String,
    pub deezer_album_id: Option<i64>,
    pub mb_release_group_id: Option<String>,
    pub cover_url: Option<String>,
    pub cover_local_path: Option<String>,
    pub external_url: Option<String>,
    pub musicbrainz_url: Option<String>,
    pub track_count: i32,
    pub explicit: bool,
    pub owned_locally: bool,
    pub state: ReleaseState,
    pub provider_source: String,
    pub discovered_at: String,
}

impl Release {
    pub fn is_upcoming(&self, today_iso: &str) -> bool {
        self.release_date.len() >= 10 && self.release_date.as_str() > today_iso
    }

    pub fn best_external_url(&self) -> Option<&str> {
        self.external_url
            .as_deref()
            .filter(|s| !s.is_empty())
            .or_else(|| self.musicbrainz_url.as_deref().filter(|s| !s.is_empty()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseTrack {
    pub id: i64,
    pub release_id: i64,
    pub disc_number: i32,
    pub track_number: i32,
    pub title: String,
    pub duration_secs: i32,
    pub preview_url: Option<String>,
    pub artist_credit: Option<String>,
}

impl ReleaseTrack {
    pub fn formatted_duration(&self) -> String {
        if self.duration_secs <= 0 {
            return "--:--".to_string();
        }
        let mins = self.duration_secs / 60;
        let secs = self.duration_secs % 60;
        format!("{mins}:{secs:02}")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSettings {
    pub dark_mode: bool,
    /// `0..=6` built-in Material 3 tonal accent presets (`0` = Iris Violet), `7` = custom `#RRGGBB`.
    pub accent_preset: u8,
    /// User-defined `#RRGGBB` seed color used when `accent_preset == 7`.
    pub custom_accent_hex: String,
    /// `0` = Compact, `1` = Comfortable (default), `2` = Spacious.
    pub grid_density: u8,
    /// UI scale percentage (`75..=250`, default `100`) for HiDPI screens without WM scaling.
    pub ui_scale_percent: u16,
    /// `0..=100` in-app 30s preview audio volume percentage.
    pub preview_volume: u8,
    /// `0` means all time; otherwise number of days to look back (`90`, `180`, `365`, `730`).
    pub lookback_days: u32,
    pub include_albums: bool,
    pub include_eps: bool,
    pub include_singles: bool,
    pub hide_owned_locally: bool,
    pub hide_compilations_and_live: bool,
    pub include_compilation_artists: bool,
    pub artist_discovery_mode: ArtistDiscoveryMode,
    pub split_collaborations: bool,
    /// `0` means manual check only; otherwise hours (`6`, `12`, `24`).
    pub auto_check_interval_hours: u32,
    pub provider_mode: ProviderMode,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            dark_mode: true,
            accent_preset: 0,
            custom_accent_hex: crate::palette::DEFAULT_CUSTOM_ACCENT_HEX.to_string(),
            grid_density: 1,
            ui_scale_percent: 100,
            preview_volume: 80,
            lookback_days: 365,
            include_albums: true,
            include_eps: true,
            include_singles: true,
            hide_owned_locally: true,
            hide_compilations_and_live: true,
            include_compilation_artists: false,
            artist_discovery_mode: ArtistDiscoveryMode::AlbumArtistOrFolder,
            split_collaborations: true,
            auto_check_interval_hours: 12,
            provider_mode: ProviderMode::DeezerAndMusicBrainz,
        }
    }
}

impl AppSettings {
    /// Apply optional first-run theme defaults from `MISSNIX_DEFAULT_DARK_MODE`
    /// (`true`/`false`/`1`/`0`) and `MISSNIX_DEFAULT_ACCENT_HEX` (`#RRGGBB`) before
    /// persisted SQLite `settings` rows are loaded on top.
    pub fn apply_env_theme_defaults(
        &mut self,
        dark_mode_env: Option<&str>,
        accent_hex_env: Option<&str>,
    ) {
        if let Some(raw) = dark_mode_env.map(str::trim) {
            if raw.eq_ignore_ascii_case("true") || raw == "1" {
                self.dark_mode = true;
            } else if raw.eq_ignore_ascii_case("false") || raw == "0" {
                self.dark_mode = false;
            }
        }
        if let Some(norm) = accent_hex_env.and_then(crate::palette::normalize_hex_color) {
            self.accent_preset = crate::palette::CUSTOM_ACCENT_PRESET;
            self.custom_accent_hex = norm;
        }
    }

    pub fn lookback_index(&self) -> i32 {
        match self.lookback_days {
            90 => 0,
            180 => 1,
            365 => 2,
            730 => 3,
            0 => 4,
            _ => 2,
        }
    }

    pub fn lookback_days_from_index(idx: i32) -> u32 {
        match idx {
            0 => 90,
            1 => 180,
            2 => 365,
            3 => 730,
            4 => 0,
            _ => 365,
        }
    }

    pub fn interval_index(&self) -> i32 {
        match self.auto_check_interval_hours {
            0 => 0,
            6 => 1,
            12 => 2,
            24 => 3,
            _ => 2,
        }
    }

    pub fn interval_hours_from_index(idx: i32) -> u32 {
        match idx {
            0 => 0,
            1 => 6,
            2 => 12,
            3 => 24,
            _ => 12,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct FeedQuery {
    pub search: String,
    pub release_type: Option<ReleaseType>,
    pub include_albums: bool,
    pub include_eps: bool,
    pub include_singles: bool,
    pub only_unseen: bool,
    pub only_upcoming: bool,
    pub only_seen: bool,
    pub hide_owned: bool,
    pub include_dismissed: bool,
    pub artist_id: Option<i64>,
    /// Minimum `YYYY-MM-DD` release date; `None` means no cutoff.
    pub cutoff_date: Option<String>,
    pub sort: Option<FeedSort>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ScanSummary {
    pub folders_scanned: usize,
    pub audio_files_seen: usize,
    pub tags_extracted: usize,
    pub folder_fallbacks: usize,
    pub skipped_compilations: usize,
    pub artists_discovered: usize,
    pub local_albums_indexed: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SyncSummary {
    /// Artists with at least one provider checked; `artists_partial` is a subset.
    pub artists_checked: usize,
    pub artists_partial: usize,
    pub artists_failed: usize,
    pub new_releases_added: usize,
    pub releases_updated: usize,
    pub owned_deduplicated: usize,
    pub offline_or_rate_limited: bool,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LibraryStats {
    pub total_artists: usize,
    pub tracked_artists: usize,
    pub muted_artists: usize,
    pub ignored_artists: usize,
    pub missing_artists: usize,
    pub local_albums: usize,
    pub total_releases: usize,
    pub unseen_releases: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HeadlessReport {
    pub db_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scan: Option<ScanSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub check: Option<SyncSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<LibraryStats>,
}

impl HeadlessReport {
    pub fn to_json_pretty(&self) -> Result<String, String> {
        serde_json::to_string_pretty(self).map_err(|e| e.to_string())
    }
}

#[derive(Debug, Clone)]
pub enum ProgressEvent {
    ScanStarted {
        total_folders: usize,
    },
    ScanProgress {
        folder: String,
        files_scanned: usize,
    },
    ScanFinished(ScanSummary),
    SyncStarted {
        total_artists: usize,
    },
    SyncArtist {
        current: usize,
        total: usize,
        artist_name: String,
    },
    CoverFetched {
        release_id: i64,
        local_path: String,
    },
    SyncFinished(SyncSummary),
    Error(String),
}
