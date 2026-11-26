pub mod db;
pub mod error;
pub mod models;
pub mod normalize;
pub mod providers;
pub mod scanner;
pub mod sync;

pub use db::{Database, DiscoveredReleaseInput, DiscoveredTrackInput};
pub use error::{CoreError, CoreResult};
pub use models::{
    AppSettings, Artist, ArtistDiscoveryMode, ArtistSource, ArtistStatus, FeedQuery, FeedSort,
    LibraryFolder, LibraryStats, LocalAlbum, ProgressEvent, ProviderMode, Release, ReleaseState,
    ReleaseTrack, ReleaseType, ScanSummary, SyncSummary,
};
pub use providers::ArtistSearchCandidate;
pub use scanner::{
    ScanOptions, ScanOutcome, scan_library_folders, scan_library_folders_with_options,
};
pub use sync::{AppPaths, SyncEngine, is_deezer_preview_url_expired};
