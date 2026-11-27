pub mod deezer;
pub mod musicbrainz;

use serde::{Deserialize, Serialize};

pub const USER_AGENT: &str = "MissNix/1.1.0 ( https://github.com/Irgendeinwer/MissNix )";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtistSearchCandidate {
    pub name: String,
    pub deezer_id: Option<i64>,
    pub mbid: Option<String>,
    pub disambiguation: Option<String>,
    pub avatar_url: Option<String>,
    pub external_url: Option<String>,
    pub fan_count: Option<u64>,
    pub source: &'static str,
}

pub fn url_encode_query(input: &str) -> String {
    let mut out = String::with_capacity(input.len() * 2);
    for b in input.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            b' ' => out.push_str("%20"),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}
