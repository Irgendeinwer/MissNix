use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde::Deserialize;

use crate::db::{DiscoveredReleaseInput, DiscoveredTrackInput};
use crate::error::{CoreError, CoreResult};
use crate::models::ReleaseType;
use crate::normalize::{is_noisy_release_title, normalize_artist_name, normalize_release_title};
use crate::providers::{url_encode_query, ArtistSearchCandidate, USER_AGENT};

const DEFAULT_MB_BASE: &str = "https://musicbrainz.org/ws/2";
const DEFAULT_CAA_BASE: &str = "https://coverartarchive.org";

#[derive(Clone)]
pub struct MusicBrainzClient {
    agent: ureq::Agent,
    mb_base_url: String,
    caa_base_url: String,
    min_interval: Duration,
    last_request: Arc<Mutex<Option<Instant>>>,
}

#[derive(Debug, Clone)]
pub struct MbArtistMetadata {
    pub mbid: String,
    pub name: String,
    pub disambiguation: Option<String>,
    pub linked_deezer_id: Option<i64>,
    pub external_url: String,
}

#[derive(Debug, Deserialize)]
struct MbArtistSearchResponse {
    #[serde(default)]
    artists: Vec<MbArtistSearchItem>,
}

#[derive(Debug, Deserialize)]
struct MbArtistSearchItem {
    id: String,
    name: String,
    #[serde(default)]
    score: Option<u32>,
    #[serde(default)]
    country: Option<String>,
    #[serde(default, rename = "type")]
    artist_type: Option<String>,
    #[serde(default)]
    disambiguation: Option<String>,
}

#[derive(Debug, Deserialize)]
struct MbArtistDetailDto {
    id: String,
    name: String,
    #[serde(default)]
    country: Option<String>,
    #[serde(default, rename = "type")]
    artist_type: Option<String>,
    #[serde(default)]
    disambiguation: Option<String>,
    #[serde(default)]
    relations: Vec<MbRelationDto>,
}

#[derive(Debug, Deserialize)]
struct MbRelationDto {
    #[serde(default)]
    url: Option<MbUrlResourceDto>,
}

#[derive(Debug, Deserialize)]
struct MbUrlResourceDto {
    resource: String,
}

#[derive(Debug, Deserialize)]
struct MbReleaseGroupPageDto {
    #[serde(default, rename = "release-groups")]
    release_groups: Vec<MbReleaseGroupDto>,
    #[serde(default, rename = "release-group-count")]
    total_count: usize,
}

#[derive(Debug, Deserialize)]
struct MbReleaseGroupDto {
    id: String,
    title: String,
    #[serde(default, rename = "primary-type")]
    primary_type: Option<String>,
    #[serde(default, rename = "secondary-types")]
    secondary_types: Vec<String>,
    #[serde(default, rename = "first-release-date")]
    first_release_date: Option<String>,
}

#[derive(Debug, Deserialize)]
struct MbReleaseListResponse {
    #[serde(default)]
    releases: Vec<MbReleaseDetailDto>,
}

#[derive(Debug, Deserialize)]
struct MbReleaseDetailDto {
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    media: Vec<MbMediumDto>,
}

#[derive(Debug, Deserialize)]
struct MbMediumDto {
    #[serde(default)]
    position: Option<i32>,
    #[serde(default)]
    tracks: Vec<MbTrackDto>,
}

#[derive(Debug, Deserialize)]
struct MbTrackDto {
    #[serde(default)]
    position: Option<i32>,
    title: String,
    #[serde(default)]
    length: Option<i64>,
}

impl Default for MusicBrainzClient {
    fn default() -> Self {
        Self::new()
    }
}

impl MusicBrainzClient {
    pub fn new() -> Self {
        Self::with_base_urls(
            DEFAULT_MB_BASE,
            DEFAULT_CAA_BASE,
            Duration::from_millis(1100),
        )
    }

    pub fn with_base_urls(mb_base: &str, caa_base: &str, min_interval: Duration) -> Self {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_connect(Some(Duration::from_secs(8)))
            .timeout_recv_response(Some(Duration::from_secs(18)))
            .timeout_recv_body(Some(Duration::from_secs(20)))
            .user_agent(USER_AGENT)
            .build()
            .into();
        Self {
            agent,
            mb_base_url: mb_base.trim_end_matches('/').to_string(),
            caa_base_url: caa_base.trim_end_matches('/').to_string(),
            min_interval,
            last_request: Arc::new(Mutex::new(None)),
        }
    }

    pub fn lookup_artist_by_mbid(&self, mbid: &str) -> CoreResult<MbArtistMetadata> {
        let url = format!(
            "{}/artist/{}?inc=url-rels&fmt=json",
            self.mb_base_url,
            url_encode_query(mbid.trim())
        );
        let dto: MbArtistDetailDto = self.get_json_polite(&url)?;
        let linked_deezer_id = dto
            .relations
            .iter()
            .filter_map(|r| r.url.as_ref())
            .find_map(|u| parse_deezer_artist_id_from_url(&u.resource));

        let disamb = format_mb_disambiguation(
            dto.artist_type.as_deref(),
            dto.country.as_deref(),
            dto.disambiguation.as_deref(),
        );

        Ok(MbArtistMetadata {
            external_url: format!("https://musicbrainz.org/artist/{}", dto.id),
            mbid: dto.id,
            name: dto.name,
            disambiguation: disamb,
            linked_deezer_id,
        })
    }

    pub fn search_artists(
        &self,
        query: &str,
        limit: usize,
    ) -> CoreResult<Vec<ArtistSearchCandidate>> {
        let trimmed = query.trim().replace('"', "");
        if trimmed.is_empty() {
            return Ok(Vec::new());
        }
        let lucene = format!("artist:\"{trimmed}\"");
        let url = format!(
            "{}/artist?query={}&limit={}&fmt=json",
            self.mb_base_url,
            url_encode_query(&lucene),
            limit.clamp(1, 20)
        );
        let resp: MbArtistSearchResponse = self.get_json_polite(&url)?;
        Ok(resp
            .artists
            .into_iter()
            .map(|a| {
                let disamb = format_mb_disambiguation(
                    a.artist_type.as_deref(),
                    a.country.as_deref(),
                    a.disambiguation.as_deref(),
                );
                let _ = a.score;
                ArtistSearchCandidate {
                    external_url: Some(format!("https://musicbrainz.org/artist/{}", a.id)),
                    name: a.name,
                    deezer_id: None,
                    mbid: Some(a.id),
                    disambiguation: disamb,
                    avatar_url: None,
                    fan_count: None,
                    source: "musicbrainz",
                }
            })
            .collect())
    }

    pub fn resolve_artist_by_name(
        &self,
        artist_name: &str,
    ) -> CoreResult<Option<ArtistSearchCandidate>> {
        let target_norm = normalize_artist_name(artist_name);
        if target_norm.is_empty() {
            return Ok(None);
        }
        let candidates = self.search_artists(artist_name, 8)?;
        Ok(candidates
            .into_iter()
            .find(|c| normalize_artist_name(&c.name) == target_norm))
    }

    /// Fetch official Album / EP / Single release-groups for an artist MBID.
    pub fn fetch_artist_release_groups(
        &self,
        artist_id: i64,
        mbid: &str,
        hide_compilations_and_live: bool,
    ) -> CoreResult<Vec<DiscoveredReleaseInput>> {
        let mut out = Vec::new();
        let mut offset = 0usize;
        let limit = 100usize;

        for _page in 0..3 {
            let url = format!(
                "{}/release-group?artist={}&type=album%7Cep%7Csingle&limit={}&offset={}&fmt=json",
                self.mb_base_url,
                url_encode_query(mbid.trim()),
                limit,
                offset
            );
            let page_dto: MbReleaseGroupPageDto = self.get_json_polite(&url)?;
            let fetched_len = page_dto.release_groups.len();

            for rg in page_dto.release_groups {
                if hide_compilations_and_live && has_noisy_secondary_type(&rg.secondary_types) {
                    continue;
                }
                if hide_compilations_and_live && is_noisy_release_title(&rg.title) {
                    continue;
                }

                let release_type = match rg
                    .primary_type
                    .as_deref()
                    .unwrap_or("Album")
                    .to_ascii_lowercase()
                    .as_str()
                {
                    "ep" => ReleaseType::Ep,
                    "single" => ReleaseType::Single,
                    "album" => ReleaseType::Album,
                    _ => continue,
                };

                if normalize_release_title(&rg.title).is_empty() {
                    continue;
                }

                let release_date = rg.first_release_date.unwrap_or_default();
                let cover_url = Some(format!(
                    "{}/release-group/{}/front-500",
                    self.caa_base_url, rg.id
                ));
                let mb_url = Some(format!("https://musicbrainz.org/release-group/{}", rg.id));

                out.push(DiscoveredReleaseInput {
                    artist_id,
                    title: rg.title,
                    release_type,
                    release_date,
                    deezer_album_id: None,
                    mb_release_group_id: Some(rg.id),
                    cover_url,
                    external_url: None,
                    musicbrainz_url: mb_url,
                    track_count: 0,
                    explicit: false,
                    provider_source: "musicbrainz".to_string(),
                });
            }

            offset += fetched_len;
            if fetched_len < limit || offset >= page_dto.total_count {
                break;
            }
        }

        out.sort_by(|a, b| b.release_date.cmp(&a.release_date));
        Ok(out)
    }

    /// Fetch tracklist for a MusicBrainz release-group when Deezer tracklist isn't available.
    pub fn fetch_release_group_tracks(
        &self,
        mb_release_group_id: &str,
    ) -> CoreResult<Vec<DiscoveredTrackInput>> {
        let url = format!(
            "{}/release?release-group={}&inc=recordings&limit=5&fmt=json",
            self.mb_base_url,
            url_encode_query(mb_release_group_id.trim())
        );
        let resp: MbReleaseListResponse = self.get_json_polite(&url)?;

        // Prefer an "Official" release with the richest tracklist
        let chosen = resp.releases.into_iter().max_by_key(|rel| {
            let official_bonus = if rel
                .status
                .as_deref()
                .is_some_and(|s| s.eq_ignore_ascii_case("official"))
            {
                1000usize
            } else {
                0usize
            };
            let track_total: usize = rel.media.iter().map(|m| m.tracks.len()).sum();
            official_bonus + track_total
        });

        let Some(release) = chosen else {
            return Ok(Vec::new());
        };

        let mut tracks = Vec::new();
        for (m_idx, medium) in release.media.into_iter().enumerate() {
            let disc = medium.position.unwrap_or((m_idx + 1) as i32).max(1);
            for (t_idx, tr) in medium.tracks.into_iter().enumerate() {
                let duration_secs = tr.length.map(|ms| (ms / 1000) as i32).unwrap_or(0).max(0);
                tracks.push(DiscoveredTrackInput {
                    disc_number: disc,
                    track_number: tr.position.unwrap_or((t_idx + 1) as i32),
                    title: tr.title,
                    duration_secs,
                    preview_url: None,
                    artist_credit: None,
                });
            }
        }
        Ok(tracks)
    }

    fn enforce_polite_rate_limit(&self) {
        if let Ok(mut guard) = self.last_request.lock() {
            if let Some(prev) = *guard {
                let elapsed = prev.elapsed();
                if elapsed < self.min_interval {
                    thread::sleep(self.min_interval - elapsed);
                }
            }
            *guard = Some(Instant::now());
        }
    }

    fn get_json_polite<T: serde::de::DeserializeOwned>(&self, url: &str) -> CoreResult<T> {
        let mut backoff = Duration::from_millis(1200);
        for attempt in 0..3 {
            self.enforce_polite_rate_limit();
            let result = self
                .agent
                .get(url)
                .header("Accept", "application/json")
                .call();
            match result {
                Ok(mut resp) => {
                    return resp.body_mut().read_json().map_err(|e| {
                        CoreError::InvalidResponse {
                            provider: "musicbrainz",
                            message: e.to_string(),
                        }
                    });
                }
                Err(ureq::Error::StatusCode(503 | 429)) if attempt < 2 => {
                    thread::sleep(backoff);
                    backoff *= 2;
                }
                Err(e) => {
                    return Err(CoreError::Network {
                        provider: "musicbrainz",
                        message: e.to_string(),
                    });
                }
            }
        }
        Err(CoreError::RateLimited {
            provider: "musicbrainz",
        })
    }
}

fn has_noisy_secondary_type(secondary_types: &[String]) -> bool {
    for st in secondary_types {
        let lower = st.to_ascii_lowercase();
        if matches!(
            lower.as_str(),
            "compilation"
                | "live"
                | "soundtrack"
                | "remix"
                | "dj-mix"
                | "mixtape/street"
                | "spokenword"
                | "interview"
                | "audio drama"
                | "demo"
        ) {
            return true;
        }
    }
    false
}

fn parse_deezer_artist_id_from_url(url: &str) -> Option<i64> {
    let lower = url.to_ascii_lowercase();
    let idx = lower.find("deezer.com/")?;
    let tail = &lower[idx + "deezer.com/".len()..];
    // Matches `deezer.com/artist/27` or `deezer.com/us/artist/27`
    let parts: Vec<&str> = tail.split('/').filter(|s| !s.is_empty()).collect();
    for window in parts.windows(2) {
        if window[0] == "artist" {
            let digits: String = window[1]
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect();
            if let Some(id) = digits.parse::<i64>().ok().filter(|&id| id > 0) {
                return Some(id);
            }
        }
    }
    None
}

fn format_mb_disambiguation(
    artist_type: Option<&str>,
    country: Option<&str>,
    disambiguation: Option<&str>,
) -> Option<String> {
    let mut parts: Vec<&str> = Vec::new();
    if let Some(d) = disambiguation.map(str::trim).filter(|s| !s.is_empty()) {
        parts.push(d);
    } else {
        if let Some(t) = artist_type.map(str::trim).filter(|s| !s.is_empty()) {
            parts.push(t);
        }
        if let Some(c) = country.map(str::trim).filter(|s| !s.is_empty()) {
            parts.push(c);
        }
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" · "))
    }
}
