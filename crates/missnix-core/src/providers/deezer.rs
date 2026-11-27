use std::collections::{HashMap, HashSet};
use std::thread;
use std::time::{Duration, Instant};

use serde::Deserialize;

use crate::db::{DiscoveredReleaseInput, DiscoveredTrackInput};
use crate::error::{CoreError, CoreResult};
use crate::models::ReleaseType;
use crate::normalize::{is_noisy_release_title, normalize_artist_name, normalize_release_title};
use crate::providers::{ArtistSearchCandidate, USER_AGENT, url_encode_query};

const DEFAULT_DEEZER_BASE: &str = "https://api.deezer.com";

#[derive(Clone)]
pub struct DeezerClient {
    agent: ureq::Agent,
    base_url: String,
    min_interval: Duration,
}

#[derive(Debug, Deserialize)]
#[serde(bound(deserialize = "T: serde::de::DeserializeOwned"))]
struct DeezerEnvelope<T> {
    #[serde(default)]
    data: Vec<T>,
    #[serde(default)]
    next: Option<String>,
    #[serde(default)]
    error: Option<DeezerApiError>,
}

#[derive(Debug, Clone, Deserialize)]
struct DeezerApiError {
    #[serde(default)]
    code: i32,
    #[serde(default)]
    message: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DeezerArtistDto {
    pub id: i64,
    pub name: String,
    #[serde(default)]
    pub link: Option<String>,
    #[serde(default)]
    pub picture_medium: Option<String>,
    #[serde(default)]
    pub nb_fan: Option<u64>,
    #[serde(default)]
    error: Option<DeezerApiError>,
}

#[derive(Debug, Clone, Deserialize)]
struct DeezerAlbumSummaryDto {
    id: i64,
    title: String,
    #[serde(default)]
    link: Option<String>,
    #[serde(default)]
    cover_big: Option<String>,
    #[serde(default)]
    cover_medium: Option<String>,
    #[serde(default)]
    release_date: Option<String>,
    #[serde(default)]
    record_type: Option<String>,
    #[serde(default)]
    explicit_lyrics: bool,
    #[serde(default)]
    nb_tracks: Option<i32>,
}

#[derive(Debug, Deserialize)]
struct DeezerAlbumDetailDto {
    #[serde(default)]
    nb_tracks: Option<i32>,
    #[serde(default)]
    tracks: Option<DeezerTracksPage>,
    #[serde(default)]
    error: Option<DeezerApiError>,
}

#[derive(Debug, Deserialize)]
struct DeezerTracksPage {
    #[serde(default)]
    data: Vec<DeezerTrackDto>,
}

#[derive(Debug, Deserialize)]
struct DeezerTrackDto {
    title: String,
    #[serde(default)]
    duration: i32,
    #[serde(default)]
    track_position: Option<i32>,
    #[serde(default)]
    disk_number: Option<i32>,
    #[serde(default)]
    preview: Option<String>,
    #[serde(default)]
    artist: Option<DeezerTrackArtistDto>,
}

#[derive(Debug, Deserialize)]
struct DeezerTrackArtistDto {
    name: String,
}

impl Default for DeezerClient {
    fn default() -> Self {
        Self::new()
    }
}

impl DeezerClient {
    pub fn new() -> Self {
        Self::with_base_url(DEFAULT_DEEZER_BASE, Duration::from_millis(120))
    }

    pub fn with_base_url(base_url: &str, min_interval: Duration) -> Self {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_connect(Some(Duration::from_secs(8)))
            .timeout_recv_response(Some(Duration::from_secs(15)))
            .timeout_recv_body(Some(Duration::from_secs(20)))
            .user_agent(USER_AGENT)
            .build()
            .into();
        Self {
            agent,
            base_url: base_url.trim_end_matches('/').to_string(),
            min_interval,
        }
    }

    /// Search Deezer for candidate artists matching `query`.
    pub fn search_artists(
        &self,
        query: &str,
        limit: usize,
    ) -> CoreResult<Vec<ArtistSearchCandidate>> {
        let encoded = url_encode_query(query.trim());
        let url = format!(
            "{}/search/artist?q={}&limit={}",
            self.base_url,
            encoded,
            limit.clamp(1, 25)
        );
        let env: DeezerEnvelope<DeezerArtistDto> = self.get_json_with_retry(&url)?;
        if let Some(err) = env.error {
            if err.code == 4 {
                return Err(CoreError::RateLimited { provider: "deezer" });
            }
            return Err(CoreError::InvalidResponse {
                provider: "deezer",
                message: err.message,
            });
        }

        Ok(env
            .data
            .into_iter()
            .map(|a| ArtistSearchCandidate {
                name: a.name,
                deezer_id: Some(a.id),
                mbid: None,
                disambiguation: a
                    .nb_fan
                    .map(|f| format!("{} fans on Deezer", format_compact_num(f))),
                avatar_url: a.picture_medium.filter(|u| !u.is_empty()),
                external_url: a
                    .link
                    .filter(|u| !u.is_empty())
                    .or_else(|| Some(format!("https://www.deezer.com/artist/{}", a.id))),
                fan_count: a.nb_fan,
                source: "deezer",
            })
            .collect())
    }

    pub fn get_artist(&self, deezer_id: i64) -> CoreResult<ArtistSearchCandidate> {
        let url = format!("{}/artist/{deezer_id}", self.base_url);
        let dto: DeezerArtistDto = self.get_json_with_retry(&url)?;
        if let Some(err) = dto.error {
            return Err(CoreError::InvalidResponse {
                provider: "deezer",
                message: err.message,
            });
        }
        Ok(ArtistSearchCandidate {
            name: dto.name,
            deezer_id: Some(dto.id),
            mbid: None,
            disambiguation: dto
                .nb_fan
                .map(|f| format!("{} fans on Deezer", format_compact_num(f))),
            avatar_url: dto.picture_medium.filter(|u| !u.is_empty()),
            external_url: dto
                .link
                .filter(|u| !u.is_empty())
                .or_else(|| Some(format!("https://www.deezer.com/artist/{}", dto.id))),
            fan_count: dto.nb_fan,
            source: "deezer",
        })
    }

    /// Resolve the best Deezer artist for `artist_name`, disambiguating homonym artists
    /// by checking how many of the user's known `local_normalized_albums` appear in each
    /// exact-match candidate's discography!
    pub fn resolve_artist_smart(
        &self,
        artist_name: &str,
        local_normalized_albums: &HashSet<String>,
    ) -> CoreResult<Option<ArtistSearchCandidate>> {
        let target_norm = normalize_artist_name(artist_name);
        if target_norm.is_empty() {
            return Ok(None);
        }

        let candidates = self.search_artists(artist_name, 10)?;
        let exact: Vec<ArtistSearchCandidate> = candidates
            .into_iter()
            .filter(|c| normalize_artist_name(&c.name) == target_norm)
            .collect();

        if exact.len() <= 1 {
            return Ok(exact.into_iter().next());
        }
        // A popularity tie-breaker can associate the entire discography of the
        // wrong person. Only resolve a homonym when local albums distinguish it.
        if local_normalized_albums.is_empty() || exact.len() > 4 {
            return Ok(None);
        }

        let mut best: Option<(usize, ArtistSearchCandidate)> = None;
        let mut tied = false;
        for cand in exact {
            let Some(dz_id) = cand.deezer_id else {
                return Ok(None);
            };
            let albums = self.fetch_raw_artist_albums(dz_id, 1)?;
            let overlap = albums
                .iter()
                .map(|album| normalize_release_title(&album.title))
                .filter(|title| !title.is_empty() && local_normalized_albums.contains(title))
                .collect::<HashSet<_>>()
                .len();
            match &best {
                None => {
                    best = Some((overlap, cand));
                    tied = false;
                }
                Some((best_overlap, _)) if overlap > *best_overlap => {
                    best = Some((overlap, cand));
                    tied = false;
                }
                Some((best_overlap, _)) if overlap == *best_overlap => tied = true,
                _ => {}
            }
        }
        Ok(match best {
            Some((overlap, candidate)) if overlap > 0 && !tied => Some(candidate),
            _ => None,
        })
    }

    /// Fetch and deduplicate an artist's releases on Deezer (`album`, `ep`, `single`).
    pub fn fetch_artist_releases(
        &self,
        artist_id: i64,
        deezer_artist_id: i64,
        hide_compilations_and_live: bool,
    ) -> CoreResult<Vec<DiscoveredReleaseInput>> {
        let raw_albums = self.fetch_raw_artist_albums(deezer_artist_id, 3)?;
        let mut deduped: HashMap<(String, ReleaseType), DiscoveredReleaseInput> = HashMap::new();

        for item in raw_albums {
            let rec = item
                .record_type
                .as_deref()
                .unwrap_or("album")
                .to_ascii_lowercase();

            if rec == "compile" && hide_compilations_and_live {
                continue;
            }
            if hide_compilations_and_live && is_noisy_release_title(&item.title) {
                continue;
            }

            let release_type = match rec.as_str() {
                "ep" => ReleaseType::Ep,
                "single" => ReleaseType::Single,
                "album" | "compile" => ReleaseType::Album,
                _ => continue,
            };

            let norm_title = normalize_release_title(&item.title);
            if norm_title.is_empty() {
                continue;
            }

            let release_date = item
                .release_date
                .filter(|d| d != "0000-00-00")
                .unwrap_or_default();
            let cover_url = item
                .cover_big
                .filter(|u| !u.is_empty())
                .or_else(|| item.cover_medium.filter(|u| !u.is_empty()));
            let external_url = item
                .link
                .filter(|u| !u.is_empty())
                .or_else(|| Some(format!("https://www.deezer.com/album/{}", item.id)));

            let candidate = DiscoveredReleaseInput {
                artist_id,
                title: clean_display_title(&item.title),
                release_type,
                release_date,
                deezer_album_id: Some(item.id),
                mb_release_group_id: None,
                cover_url,
                external_url,
                musicbrainz_url: None,
                track_count: item.nb_tracks.unwrap_or(0),
                explicit: item.explicit_lyrics,
                provider_source: "deezer".to_string(),
            };

            // Collapse Explicit/Clean/Deluxe duplicate editions of the same (normalized_title, release_type)
            let key = (norm_title, release_type);
            match deduped.get_mut(&key) {
                None => {
                    deduped.insert(key, candidate);
                }
                Some(existing) => {
                    // Prefer earlier valid release date, explicit over censored clean edition, and higher track count
                    if !candidate.release_date.is_empty()
                        && (existing.release_date.is_empty()
                            || candidate.release_date < existing.release_date)
                    {
                        existing.release_date = candidate.release_date.clone();
                    }
                    if (candidate.explicit && !existing.explicit)
                        || candidate.track_count > existing.track_count
                    {
                        existing.deezer_album_id = candidate.deezer_album_id;
                        existing.external_url = candidate.external_url;
                        existing.cover_url =
                            candidate.cover_url.or_else(|| existing.cover_url.take());
                        existing.track_count = existing.track_count.max(candidate.track_count);
                        existing.explicit |= candidate.explicit;
                    }
                }
            }
        }

        let mut list: Vec<DiscoveredReleaseInput> = deduped.into_values().collect();
        list.sort_by(|a, b| b.release_date.cmp(&a.release_date));
        Ok(list)
    }

    /// Fetch full tracklist & 30s preview URLs for a Deezer album.
    pub fn fetch_album_tracks(
        &self,
        deezer_album_id: i64,
    ) -> CoreResult<Vec<DiscoveredTrackInput>> {
        let url = format!("{}/album/{deezer_album_id}", self.base_url);
        let detail: DeezerAlbumDetailDto = self.get_json_with_retry(&url)?;
        if let Some(err) = detail.error {
            return Err(CoreError::InvalidResponse {
                provider: "deezer",
                message: err.message,
            });
        }
        let raw_tracks = detail.tracks.map(|t| t.data).unwrap_or_default();
        let mut out = Vec::with_capacity(raw_tracks.len());
        for (idx, tr) in raw_tracks.into_iter().enumerate() {
            out.push(DiscoveredTrackInput {
                disc_number: tr.disk_number.unwrap_or(1).max(1),
                track_number: tr.track_position.unwrap_or((idx + 1) as i32),
                title: tr.title,
                duration_secs: tr.duration.max(0),
                preview_url: tr.preview.filter(|p| !p.is_empty()),
                artist_credit: tr.artist.map(|a| a.name).filter(|n| !n.is_empty()),
            });
        }
        let _ = detail.nb_tracks;
        Ok(out)
    }

    /// Download cover image or preview bytes (up to 8 MiB) from a URL.
    pub fn download_bytes(&self, url: &str) -> CoreResult<Vec<u8>> {
        let mut resp = self.agent.get(url).call().map_err(|e| CoreError::Network {
            provider: "deezer",
            message: e.to_string(),
        })?;
        let buf = resp
            .body_mut()
            .with_config()
            .limit(8 * 1024 * 1024)
            .read_to_vec()
            .map_err(|e| CoreError::InvalidResponse {
                provider: "deezer",
                message: e.to_string(),
            })?;
        Ok(buf)
    }

    fn fetch_raw_artist_albums(
        &self,
        deezer_artist_id: i64,
        max_pages: usize,
    ) -> CoreResult<Vec<DeezerAlbumSummaryDto>> {
        let mut out = Vec::new();
        let mut next_url = Some(format!(
            "{}/artist/{deezer_artist_id}/albums?limit=100",
            self.base_url
        ));
        let mut page = 0usize;

        while let Some(url) = next_url.take() {
            if page >= max_pages {
                break;
            }
            page += 1;
            let env: DeezerEnvelope<DeezerAlbumSummaryDto> = self.get_json_with_retry(&url)?;
            if let Some(err) = env.error {
                if err.code == 4 {
                    return Err(CoreError::RateLimited { provider: "deezer" });
                }
                return Err(CoreError::InvalidResponse {
                    provider: "deezer",
                    message: err.message,
                });
            }
            out.extend(env.data);
            next_url = env.next.filter(|s| !s.is_empty());
        }

        Ok(out)
    }

    fn get_json_with_retry<T: serde::de::DeserializeOwned>(&self, url: &str) -> CoreResult<T> {
        let mut backoff = Duration::from_millis(450);
        for attempt in 0..3 {
            let started = Instant::now();
            let result = self
                .agent
                .get(url)
                .header("Accept", "application/json")
                .call();
            match result {
                Ok(mut resp) => {
                    let body: serde_json::Value =
                        resp.body_mut()
                            .read_json()
                            .map_err(|e| CoreError::InvalidResponse {
                                provider: "deezer",
                                message: e.to_string(),
                            })?;

                    // Deezer returns HTTP 200 with `{"error": {"code": 4, ...}}` when quota-limited
                    if body
                        .get("error")
                        .and_then(|e| e.get("code"))
                        .and_then(|c| c.as_i64())
                        == Some(4)
                        && attempt < 2
                    {
                        thread::sleep(backoff);
                        backoff *= 2;
                        continue;
                    }

                    let parsed: T = serde_json::from_value(body)?;
                    let elapsed = started.elapsed();
                    if elapsed < self.min_interval {
                        thread::sleep(self.min_interval - elapsed);
                    }
                    return Ok(parsed);
                }
                Err(ureq::Error::StatusCode(429 | 503)) if attempt < 2 => {
                    thread::sleep(backoff);
                    backoff *= 2;
                }
                Err(e) => {
                    return Err(CoreError::Network {
                        provider: "deezer",
                        message: e.to_string(),
                    });
                }
            }
        }
        Err(CoreError::RateLimited { provider: "deezer" })
    }
}

fn clean_display_title(raw: &str) -> String {
    let trimmed = raw.trim();
    for suffix in [" - Single", " - EP"] {
        if let Some(stripped) = trimmed
            .strip_suffix(suffix)
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            return stripped.to_string();
        }
    }
    trimmed.to_string()
}

fn format_compact_num(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", (n as f64) / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}K", (n as f64) / 1_000.0)
    } else {
        n.to_string()
    }
}
