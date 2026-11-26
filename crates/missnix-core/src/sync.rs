use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::db::Database;
use crate::error::{CoreError, CoreResult};
use crate::models::{
    AppSettings, Artist, ProgressEvent, ProviderMode, Release, ReleaseTrack, SyncSummary,
};
use crate::normalize::{days_ago_iso_utc, normalize_artist_name};
use crate::providers::deezer::DeezerClient;
use crate::providers::musicbrainz::MusicBrainzClient;
use crate::providers::ArtistSearchCandidate;

#[derive(Debug, Clone)]
pub struct AppPaths {
    pub db_path: PathBuf,
    pub cover_cache_dir: PathBuf,
}

impl AppPaths {
    pub fn resolve() -> Self {
        let db_path = if let Ok(custom) = std::env::var("MISSNIX_DB_PATH") {
            PathBuf::from(custom)
        } else {
            dirs::data_local_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("missnix")
                .join("missnix.db")
        };

        let cover_cache_dir = if let Ok(custom_cache) = std::env::var("MISSNIX_CACHE_DIR") {
            PathBuf::from(custom_cache).join("covers")
        } else {
            dirs::cache_dir()
                .unwrap_or_else(|| PathBuf::from(".cache"))
                .join("missnix")
                .join("covers")
        };

        Self {
            db_path,
            cover_cache_dir,
        }
    }
}

#[derive(Clone)]
pub struct SyncEngine {
    pub deezer: DeezerClient,
    pub musicbrainz: MusicBrainzClient,
    pub cover_cache_dir: PathBuf,
}

impl SyncEngine {
    pub fn new(cover_cache_dir: PathBuf) -> Self {
        Self {
            deezer: DeezerClient::new(),
            musicbrainz: MusicBrainzClient::new(),
            cover_cache_dir,
        }
    }

    pub fn with_clients(
        deezer: DeezerClient,
        musicbrainz: MusicBrainzClient,
        cover_cache_dir: PathBuf,
    ) -> Self {
        Self {
            deezer,
            musicbrainz,
            cover_cache_dir,
        }
    }

    /// Synchronize releases for a slice of tracked artists against Deezer and/or MusicBrainz.
    pub fn sync_artists<F>(
        &self,
        db: &mut Database,
        artists: &[Artist],
        settings: &AppSettings,
        cancel_flag: Option<&AtomicBool>,
        mut on_event: F,
    ) -> CoreResult<SyncSummary>
    where
        F: FnMut(ProgressEvent),
    {
        let mut summary = SyncSummary::default();
        let total = artists.len();
        on_event(ProgressEvent::SyncStarted {
            total_artists: total,
        });

        // Keep at least 2 years of releases in SQLite when lookback is bounded so switching
        // lookback filter in Preferences (90d / 180d / 1y / 2y) works instantaneously.
        let ingest_cutoff = if settings.lookback_days == 0 {
            None
        } else {
            Some(days_ago_iso_utc(settings.lookback_days.max(730)))
        };

        let mut consecutive_network_errors = 0usize;

        for (idx, artist) in artists.iter().enumerate() {
            if cancel_flag.is_some_and(|c| c.load(Ordering::Relaxed)) {
                return Err(CoreError::Cancelled);
            }

            on_event(ProgressEvent::SyncArtist {
                current: idx + 1,
                total,
                artist_name: artist.name.clone(),
            });

            match self.sync_single_artist(db, artist, settings, ingest_cutoff.as_deref()) {
                Ok(artist_stats) => {
                    consecutive_network_errors = 0;
                    summary.artists_checked += 1;
                    summary.new_releases_added += artist_stats.new_releases_added;
                    summary.releases_updated += artist_stats.releases_updated;
                    summary.owned_deduplicated += artist_stats.owned_deduplicated;
                }
                Err(CoreError::Cancelled) => return Err(CoreError::Cancelled),
                Err(err) => {
                    summary.artists_failed += 1;
                    summary.last_error = Some(format!("{}: {}", artist.name, err));
                    if matches!(
                        err,
                        CoreError::Network { .. } | CoreError::RateLimited { .. }
                    ) {
                        summary.offline_or_rate_limited = true;
                        consecutive_network_errors += 1;
                        // Polite circuit-breaker: if 3 artists in a row fail with network/rate-limit
                        // errors, stop hammering unreachable services.
                        if consecutive_network_errors >= 3 {
                            break;
                        }
                    }
                }
            }
        }

        on_event(ProgressEvent::SyncFinished(summary.clone()));
        Ok(summary)
    }

    fn sync_single_artist(
        &self,
        db: &mut Database,
        artist: &Artist,
        settings: &AppSettings,
        ingest_cutoff: Option<&str>,
    ) -> CoreResult<SyncSummary> {
        let mut stats = SyncSummary::default();
        let local_albums: HashSet<String> = db
            .list_local_albums_for_artist(artist.id)?
            .into_iter()
            .map(|a| a.normalized_title)
            .collect();

        let mut mbid = artist.mbid.clone();
        let mut deezer_id = artist.deezer_id;
        let mut disambiguation = artist.disambiguation.clone();
        let mut avatar_url = artist.avatar_url.clone();
        let mut external_url = artist.external_url.clone();
        let mut links_changed = false;

        let use_deezer = settings.provider_mode != ProviderMode::MusicBrainzOnly;
        let use_mb = settings.provider_mode != ProviderMode::DeezerOnly;

        // 1. If we already have a MusicBrainz Artist ID (e.g. from local FLAC tags!) and still
        //    lack a Deezer ID or disambiguation, query MusicBrainz `url-rels` for exact linking!
        if use_mb && (deezer_id.is_none() || disambiguation.is_none()) {
            if let Some(ref existing_mbid) = mbid {
                if let Ok(mb_meta) = self.musicbrainz.lookup_artist_by_mbid(existing_mbid) {
                    if deezer_id.is_none() && mb_meta.linked_deezer_id.is_some() {
                        deezer_id = mb_meta.linked_deezer_id;
                        links_changed = true;
                    }
                    if disambiguation.is_none() && mb_meta.disambiguation.is_some() {
                        disambiguation = mb_meta.disambiguation;
                        links_changed = true;
                    }
                    if external_url.is_none() {
                        external_url = Some(mb_meta.external_url);
                        links_changed = true;
                    }
                }
            }
        }

        // 2. Resolve Deezer artist using smart local-discography disambiguation when needed
        if use_deezer && deezer_id.is_none() {
            match self
                .deezer
                .resolve_artist_smart(&artist.name, &local_albums)
            {
                Ok(Some(cand)) => {
                    deezer_id = cand.deezer_id;
                    if avatar_url.is_none() {
                        avatar_url = cand.avatar_url;
                    }
                    if external_url.is_none() {
                        external_url = cand.external_url;
                    }
                    if disambiguation.is_none() {
                        disambiguation = cand.disambiguation;
                    }
                    links_changed = true;
                }
                Ok(None) => {}
                Err(e) if !use_mb => return Err(e),
                Err(_) => {}
            }
        }

        // 3. Resolve MusicBrainz artist if needed (MusicBrainzOnly, or Deezer had no match,
        //    or in combined mode when MBID is not yet linked)
        if use_mb && mbid.is_none() {
            match self.musicbrainz.resolve_artist_by_name(&artist.name) {
                Ok(Some(cand)) => {
                    mbid = cand.mbid;
                    if cand.disambiguation.is_some() {
                        disambiguation = cand.disambiguation;
                    }
                    if external_url.is_none() {
                        external_url = cand.external_url;
                    }
                    links_changed = true;
                }
                Ok(None) => {}
                Err(e) if !use_deezer && deezer_id.is_none() => return Err(e),
                Err(_) => {}
            }
        }

        if links_changed {
            db.update_artist_provider_links(
                artist.id,
                mbid.as_deref(),
                deezer_id,
                disambiguation.as_deref(),
                avatar_url.as_deref(),
                external_url.as_deref(),
            )?;
        }

        let mut any_provider_succeeded = false;
        let mut last_err: Option<CoreError> = None;

        // 4. Fetch Deezer releases
        if use_deezer {
            if let Some(dz_id) = deezer_id {
                match self.deezer.fetch_artist_releases(
                    artist.id,
                    dz_id,
                    settings.hide_compilations_and_live,
                ) {
                    Ok(items) => {
                        any_provider_succeeded = true;
                        for rel in items {
                            if ingest_cutoff.is_some_and(|cutoff| {
                                !rel.release_date.is_empty() && rel.release_date.as_str() < cutoff
                            }) {
                                continue;
                            }
                            let (_, inserted, owned) = db.upsert_discovered_release(&rel)?;
                            if inserted {
                                stats.new_releases_added += 1;
                            } else {
                                stats.releases_updated += 1;
                            }
                            if owned {
                                stats.owned_deduplicated += 1;
                            }
                        }
                    }
                    Err(e) => {
                        last_err = Some(e);
                    }
                }
            }
        }

        // 5. Fetch MusicBrainz release-groups (captures upcoming announced releases & enriches MBIDs!)
        if use_mb {
            if let Some(ref artist_mbid) = mbid {
                match self.musicbrainz.fetch_artist_release_groups(
                    artist.id,
                    artist_mbid,
                    settings.hide_compilations_and_live,
                ) {
                    Ok(items) => {
                        any_provider_succeeded = true;
                        for rel in items {
                            if ingest_cutoff.is_some_and(|cutoff| {
                                !rel.release_date.is_empty() && rel.release_date.as_str() < cutoff
                            }) {
                                continue;
                            }
                            let (_, inserted, owned) = db.upsert_discovered_release(&rel)?;
                            if inserted {
                                stats.new_releases_added += 1;
                            } else {
                                stats.releases_updated += 1;
                            }
                            if owned {
                                stats.owned_deduplicated += 1;
                            }
                        }
                    }
                    Err(e) => {
                        last_err = Some(e);
                    }
                }
            }
        }

        if !any_provider_succeeded {
            if let Some(err) = last_err {
                return Err(err);
            }
        }

        db.mark_artist_checked(artist.id)?;
        Ok(stats)
    }

    /// Download and cache a release's cover image to disk if not already cached.
    pub fn ensure_release_cover_cached(
        &self,
        db: &Database,
        release: &Release,
    ) -> CoreResult<Option<PathBuf>> {
        if let Some(ref existing) = release.cover_local_path {
            let p = PathBuf::from(existing);
            if p.is_file() {
                return Ok(Some(p));
            }
        }

        let Some(ref url) = release.cover_url else {
            return Ok(None);
        };
        if url.is_empty() {
            return Ok(None);
        }

        fs::create_dir_all(&self.cover_cache_dir).map_err(|source| CoreError::Io {
            path: self.cover_cache_dir.clone(),
            source,
        })?;

        let bytes = self.deezer.download_bytes(url)?;
        let Some(ext) = detect_image_extension(&bytes) else {
            return Ok(None);
        };

        let dest = self
            .cover_cache_dir
            .join(format!("release_{}.{}", release.id, ext));
        fs::write(&dest, &bytes).map_err(|source| CoreError::Io {
            path: dest.clone(),
            source,
        })?;

        let dest_str = dest.to_string_lossy().to_string();
        db.set_release_cover_local_path(release.id, &dest_str)?;
        Ok(Some(dest))
    }

    /// Return cached tracks for `release`, or fetch from Deezer / MusicBrainz and persist them.
    pub fn fetch_and_cache_release_tracks(
        &self,
        db: &mut Database,
        release: &Release,
    ) -> CoreResult<Vec<ReleaseTrack>> {
        let cached = db.list_release_tracks(release.id)?;
        if !cached.is_empty() {
            return Ok(cached);
        }

        if let Some(dz_album_id) = release.deezer_album_id {
            if let Ok(tracks) = self.deezer.fetch_album_tracks(dz_album_id) {
                if !tracks.is_empty() {
                    db.replace_release_tracks(release.id, &tracks)?;
                    return db.list_release_tracks(release.id);
                }
            }
        }

        if let Some(ref rg_id) = release.mb_release_group_id {
            if let Ok(tracks) = self.musicbrainz.fetch_release_group_tracks(rg_id) {
                if !tracks.is_empty() {
                    db.replace_release_tracks(release.id, &tracks)?;
                    return db.list_release_tracks(release.id);
                }
            }
        }

        Ok(Vec::new())
    }

    /// Search Deezer and MusicBrainz for manual artist addition, merging matching entries.
    pub fn search_external_artists(
        &self,
        query: &str,
        mode: ProviderMode,
    ) -> CoreResult<Vec<ArtistSearchCandidate>> {
        let mut results: Vec<ArtistSearchCandidate> = Vec::new();

        if mode != ProviderMode::MusicBrainzOnly {
            if let Ok(dz) = self.deezer.search_artists(query, 8) {
                results.extend(dz);
            }
        }

        if mode != ProviderMode::DeezerOnly {
            if let Ok(mb) = self.musicbrainz.search_artists(query, 6) {
                for m in mb {
                    let m_norm = normalize_artist_name(&m.name);
                    if let Some(existing) = results
                        .iter_mut()
                        .find(|r| r.mbid.is_none() && normalize_artist_name(&r.name) == m_norm)
                    {
                        existing.mbid = m.mbid;
                        if m.disambiguation.is_some() {
                            existing.disambiguation = m.disambiguation;
                        }
                        existing.source = "deezer+musicbrainz";
                    } else {
                        results.push(m);
                    }
                }
            }
        }

        Ok(results)
    }
}

pub fn detect_image_extension(bytes: &[u8]) -> Option<&'static str> {
    if bytes.len() >= 3 && bytes[0..3] == [0xFF, 0xD8, 0xFF] {
        Some("jpg")
    } else if bytes.len() >= 8 && bytes[0..8] == [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A] {
        Some("png")
    } else if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("webp")
    } else {
        None
    }
}

pub fn check_cache_dir_exists(path: &Path) -> bool {
    path.is_dir()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{FeedQuery, ReleaseType};
    use crate::scanner::{DiscoveredAlbumRecord, DiscoveredArtistRecord, ScanOutcome};
    use std::collections::HashMap;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;
    use std::time::Duration;

    fn spawn_mock_http_server(routes: HashMap<String, String>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        listener.set_nonblocking(false).unwrap();

        thread::spawn(move || {
            for stream in listener.incoming().take(20) {
                let Ok(mut stream) = stream else { break };
                let mut buf = [0u8; 4096];
                let n = stream.read(&mut buf).unwrap_or(0);
                if n == 0 {
                    continue;
                }
                let req = String::from_utf8_lossy(&buf[..n]);
                let first_line = req.lines().next().unwrap_or("");
                let path = first_line.split_whitespace().nth(1).unwrap_or("/");

                let (status, body) = if let Some(resp_body) = routes.get(path) {
                    ("200 OK", resp_body.as_str())
                } else {
                    (
                        "404 Not Found",
                        "{\"error\":{\"code\":800,\"message\":\"not found\"}}",
                    )
                };

                let http = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(http.as_bytes());
            }
        });

        format!("http://{addr}")
    }

    #[test]
    fn disambiguates_homonym_artists_via_local_albums_and_merges_deezer_and_mb() {
        let mut routes = HashMap::new();

        // Two artists named "Air" on Deezer:
        // #100 has MORE fans (500k) but wrong discography ("Cloud Pop")
        // #200 has 420k fans and HAS "Moon Safari" (which the user owns locally!)
        routes.insert(
            "/search/artist?q=Air&limit=10".to_string(),
            r#"{"data":[
                {"id":100,"name":"Air","nb_fan":500000,"link":"https://www.deezer.com/artist/100"},
                {"id":200,"name":"Air","nb_fan":420000,"link":"https://www.deezer.com/artist/200"}
            ]}"#
            .to_string(),
        );
        routes.insert(
            "/artist/100/albums?limit=100".to_string(),
            r#"{"data":[{"id":1001,"title":"Cloud Pop","release_date":"2026-05-01","record_type":"album","nb_tracks":9}]}"#.to_string(),
        );
        routes.insert(
            "/artist/200/albums?limit=100".to_string(),
            r#"{"data":[
                {"id":2001,"title":"Moon Safari (25th Anniversary Edition)","release_date":"2026-03-01","record_type":"album","nb_tracks":10},
                {"id":2002,"title":"Solaris - EP","release_date":"2026-10-20","record_type":"ep","nb_tracks":4,"explicit_lyrics":false},
                {"id":2003,"title":"Solaris (Explicit) - EP","release_date":"2026-10-20","record_type":"ep","nb_tracks":5,"explicit_lyrics":true},
                {"id":2004,"title":"Best Of Air","release_date":"2026-01-10","record_type":"compile","nb_tracks":18}
            ]}"#
            .to_string(),
        );

        // MusicBrainz search + release-groups (includes an upcoming 2027 album!)
        routes.insert(
            "/ws/2/artist?query=artist%3A%22Air%22&limit=8&fmt=json".to_string(),
            r#"{"artists":[{"id":"cb67438a-7f50-4f2b-a6f1-2bb2729fd538","name":"Air","country":"FR","type":"Group","disambiguation":"French electronic duo"}]}"#.to_string(),
        );
        routes.insert(
            "/ws/2/release-group?artist=cb67438a-7f50-4f2b-a6f1-2bb2729fd538&type=album%7Cep%7Csingle&limit=100&offset=0&fmt=json".to_string(),
            r#"{"release-group-count":2,"release-groups":[
                {"id":"11111111-aaaa-bbbb-cccc-000000000001","title":"Solaris","primary-type":"EP","secondary-types":[],"first-release-date":"2026-10-20"},
                {"id":"11111111-aaaa-bbbb-cccc-000000000002","title":"Blue Horizon","primary-type":"Album","secondary-types":[],"first-release-date":"2027-02-14"},
                {"id":"11111111-aaaa-bbbb-cccc-000000000003","title":"Live in Paris","primary-type":"Album","secondary-types":["Live"],"first-release-date":"2026-09-01"}
            ]}"#.to_string(),
        );

        let base = spawn_mock_http_server(routes);
        let deezer = DeezerClient::with_base_url(&base, Duration::from_millis(1));
        let mb = MusicBrainzClient::with_base_urls(
            &format!("{base}/ws/2"),
            &base,
            Duration::from_millis(1),
        );
        let engine =
            SyncEngine::with_clients(deezer, mb, std::env::temp_dir().join("missnix_test_covers"));

        let mut db = Database::open_in_memory().unwrap();
        let mut albums = HashMap::new();
        albums.insert(
            "moon safari".to_string(),
            DiscoveredAlbumRecord {
                title: "Moon Safari".to_string(),
                normalized_title: "moon safari".to_string(),
                release_year: Some(1998),
                mb_release_group_id: None,
                mb_album_id: None,
                track_count: 10,
                sample_path: "/music/Air/Moon Safari/01.flac".to_string(),
            },
        );
        let mut scan = ScanOutcome::default();
        scan.artists.insert(
            "air".to_string(),
            DiscoveredArtistRecord {
                display_name: "Air".to_string(),
                normalized_name: "air".to_string(),
                mbid: None,
                track_count: 10,
                albums,
            },
        );
        db.apply_scan_outcome(&scan).unwrap();

        let settings = AppSettings {
            lookback_days: 0,
            ..Default::default()
        };
        let tracked = db.list_tracked_artists().unwrap();
        let summary = engine
            .sync_artists(&mut db, &tracked, &settings, None, |_| {})
            .unwrap();
        assert_eq!(summary.artists_checked, 1);

        // Verify homonym disambiguation picked Deezer artist #200 (who has Moon Safari!)
        let air = db
            .list_tracked_artists()
            .unwrap()
            .into_iter()
            .next()
            .unwrap();
        assert_eq!(air.deezer_id, Some(200));
        assert_eq!(
            air.mbid.as_deref(),
            Some("cb67438a-7f50-4f2b-a6f1-2bb2729fd538")
        );
        assert_eq!(air.disambiguation.as_deref(), Some("French electronic duo"));

        // Query unowned feed: Moon Safari (25th Anniversary) is hidden as owned_locally,
        // Best Of Air & Live in Paris were filtered as compilations/live,
        // Solaris Explicit + Clean + MB merged into ONE EP with 5 tracks,
        // and Blue Horizon (2027-02-14) upcoming album is present!
        let feed = db
            .query_releases(&FeedQuery {
                include_albums: true,
                include_eps: true,
                include_singles: true,
                only_unseen: true,
                hide_owned: true,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(feed.len(), 2);
        assert_eq!(feed[0].title, "Blue Horizon");
        assert_eq!(feed[0].release_type, ReleaseType::Album);
        assert!(feed[0].is_upcoming("2026-11-26"));

        assert_eq!(feed[1].normalized_title, "solaris");
        assert_eq!(feed[1].release_type, ReleaseType::Ep);
        assert_eq!(feed[1].track_count, 5);
        assert_eq!(feed[1].provider_source, "deezer+musicbrainz");
    }
}
