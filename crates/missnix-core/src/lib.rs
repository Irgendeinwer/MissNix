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
    AppSettings, Artist, ArtistSource, ArtistStatus, FeedQuery, FeedSort, LibraryFolder,
    LibraryStats, LocalAlbum, ProgressEvent, ProviderMode, Release, ReleaseState, ReleaseTrack,
    ReleaseType, ScanSummary, SyncSummary,
};
pub use providers::ArtistSearchCandidate;
pub use scanner::{scan_library_folders, ScanOutcome};
pub use sync::{AppPaths, SyncEngine};
