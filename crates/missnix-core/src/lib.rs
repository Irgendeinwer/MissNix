pub mod db;
pub mod error;
pub mod models;
pub mod normalize;
pub mod palette;
pub mod providers;
pub mod scanner;
pub mod sync;

pub use db::{Database, DiscoveredReleaseInput, DiscoveredTrackInput};
pub use error::{CoreError, CoreResult};
pub use models::{
    AppSettings, Artist, ArtistDiscoveryMode, ArtistSource, ArtistStatus, FeedQuery, FeedSort,
    HeadlessReport, LibraryFolder, LibraryStats, LocalAlbum, ProgressEvent, ProviderMode, Release,
    ReleaseState, ReleaseTrack, ReleaseType, ScanSummary, SyncSummary,
};
pub use palette::{
    CUSTOM_ACCENT_PRESET, DEFAULT_CUSTOM_ACCENT_HEX, MAX_ACCENT_PRESET, ThemePalette,
    format_hex_color, generate_m3_palette, normalize_hex_color, parse_hex_color, preset_seed_rgb,
    resolve_theme_palette,
};
pub use providers::ArtistSearchCandidate;
pub use scanner::{
    ScanOptions, ScanOutcome, scan_library_folders, scan_library_folders_with_options,
};
pub use sync::{AppPaths, SyncEngine, is_deezer_preview_url_expired};
