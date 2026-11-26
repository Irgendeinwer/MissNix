use std::time::{SystemTime, UNIX_EPOCH};

/// Fold common Latin diacritics and typographic punctuation into ASCII equivalents
/// for resilient artist and album matching across local tags, Deezer, and MusicBrainz.
pub fn fold_diacritics_and_symbols(input: &str) -> String {
    let mut out = String::with_capacity(input.len() + 4);
    for ch in input.chars() {
        match ch {
            'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ă' | 'ą' | 'À' | 'Á' | 'Â' | 'Ã' | 'Ä'
            | 'Å' | 'Ā' | 'Ă' | 'Ą' => out.push('a'),
            'æ' | 'Æ' => out.push_str("ae"),
            'ç' | 'ć' | 'č' | 'Ç' | 'Ć' | 'Č' => out.push('c'),
            'ď' | 'đ' | 'Ď' | 'Đ' | 'ð' | 'Ð' => out.push('d'),
            'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ĕ' | 'ė' | 'ę' | 'ě' | 'È' | 'É' | 'Ê' | 'Ë' | 'Ē'
            | 'Ĕ' | 'Ė' | 'Ę' | 'Ě' => out.push('e'),
            'ğ' | 'Ğ' => out.push('g'),
            'ì' | 'í' | 'î' | 'ï' | 'ī' | 'į' | 'ı' | 'Ì' | 'Í' | 'Î' | 'Ï' | 'Ī' | 'Į' | 'İ' => {
                out.push('i')
            }
            'ł' | 'Ł' => out.push('l'),
            'ñ' | 'ń' | 'ň' | 'Ñ' | 'Ń' | 'Ň' => out.push('n'),
            'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' | 'ő' | 'Ò' | 'Ó' | 'Ô' | 'Õ' | 'Ö' | 'Ø'
            | 'Ō' | 'Ő' => out.push('o'),
            'œ' | 'Œ' => out.push_str("oe"),
            'ř' | 'Ř' => out.push('r'),
            'ś' | 'š' | 'ş' | 'Ś' | 'Š' | 'Ş' => out.push('s'),
            'ß' => out.push_str("ss"),
            'ť' | 'ţ' | 'Ť' | 'Ţ' | 'þ' | 'Þ' => out.push('t'),
            'ù' | 'ú' | 'û' | 'ü' | 'ū' | 'ů' | 'ű' | 'Ù' | 'Ú' | 'Û' | 'Ü' | 'Ū' | 'Ů' | 'Ű' => {
                out.push('u')
            }
            'ý' | 'ÿ' | 'Ý' | 'Ÿ' => out.push('y'),
            'ź' | 'ż' | 'ž' | 'Ź' | 'Ż' | 'Ž' => out.push('z'),
            '&' | '+' => out.push_str(" and "),
            '’' | '‘' | '`' | '´' => {}
            '\'' => {}
            c if c.is_alphanumeric() => {
                for lc in c.to_lowercase() {
                    out.push(lc);
                }
            }
            _ => out.push(' '),
        }
    }
    collapse_whitespace(&out)
}

pub fn collapse_whitespace(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for word in input.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
    }
    out
}

/// Normalize an artist name for identity deduplication.
pub fn normalize_artist_name(name: &str) -> String {
    let primary = strip_featured_artists(name);
    fold_diacritics_and_symbols(&primary)
}

/// Strip trailing featured-artist credits (`feat.`, `ft.`, `featuring`) when extracting
/// primary artist identity from track-level artist tags.
pub fn strip_featured_artists(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    // Remove parenthesized/bracketed feat clauses first, e.g. "Artist (feat. Someone)"
    let mut cleaned = strip_matching_brackets(trimmed, is_feat_clause);

    // Strip unbracketed " feat. ...", " ft. ...", " featuring ..."
    let lower = cleaned.to_lowercase();
    for marker in [" feat. ", " feat ", " ft. ", " ft ", " featuring "] {
        if let Some(idx) = lower.find(marker) {
            cleaned.truncate(idx);
            break;
        }
    }

    collapse_whitespace(cleaned.trim_end_matches(['-', ',', ';', '/']))
}

/// Detect generic compilation / placeholder artist tags that shouldn't become tracked artists.
pub fn is_various_or_generic_artist(name: &str) -> bool {
    let norm = fold_diacritics_and_symbols(name);
    matches!(
        norm.as_str(),
        "" | "various artists"
            | "various"
            | "va"
            | "v a"
            | "unknown"
            | "unknown artist"
            | "soundtrack"
            | "original soundtrack"
            | "original motion picture soundtrack"
            | "ost"
            | "none"
    )
}

/// Normalize an album/EP/single title by stripping edition, remaster, region,
/// format, and featured-credit suffixes so local albums match streaming editions.
pub fn normalize_release_title(title: &str) -> String {
    let mut working = title.trim().to_string();

    // 1. Remove bracketed/parenthesized edition or feat clauses
    working = strip_matching_brackets(&working, is_edition_or_feat_clause);

    // 2. Remove trailing " - Edition" / " - EP" / " - Single" / " - 2024 Remaster" suffixes
    loop {
        let trimmed = working.trim();
        let Some((left, right)) = split_last_dash_suffix(trimmed) else {
            break;
        };
        if is_edition_or_format_suffix(right) && !left.trim().is_empty() {
            working = left.trim().to_string();
        } else {
            break;
        }
    }

    fold_diacritics_and_symbols(&working)
}

/// Heuristic filter for live albums, greatest-hits compilations, karaoke, or tributes.
pub fn is_noisy_release_title(title: &str) -> bool {
    let lower = title.to_lowercase();
    let norm = fold_diacritics_and_symbols(title);

    for phrase in [
        "greatest hits",
        "best of ",
        "the best of",
        "anthology",
        "the essential ",
        "complete studio",
        "karaoke",
        "tribute to ",
        "in the style of",
        "commentary",
        "track by track",
        "instrumentals",
        "instrumental version",
    ] {
        if norm.contains(phrase) {
            return true;
        }
    }

    // Live markers in parentheses/brackets or " - Live..."
    for marker in [
        "(live)",
        "(live ",
        "[live]",
        "[live ",
        "- live",
        "live at ",
        "live from ",
        "live in ",
        "unplugged",
        "bbc sessions",
    ] {
        if lower.contains(marker) {
            return true;
        }
    }

    false
}

/// Strip folder decoration like `[FLAC]`, `(2021)`, `[24-96]` when inferring album title from folder names.
pub fn clean_folder_album_name(folder: &str) -> (String, Option<i32>) {
    let mut extracted_year: Option<i32> = None;
    let mut s = folder.trim().to_string();

    // Check leading year prefix like "2001 - Discovery" or "[2001] Discovery" or "(2001) Discovery"
    let leading_re_candidates = s.trim_start_matches(['[', '(']);
    if let Some(yr) = leading_re_candidates
        .get(..4)
        .and_then(|p| p.parse::<i32>().ok())
        .filter(|y| (1950..=2100).contains(y))
    {
        extracted_year = Some(yr);
        let rest = leading_re_candidates[4..].trim_start_matches([']', ')', ' ', '-', '_', '.']);
        if !rest.is_empty() {
            s = rest.to_string();
        }
    }

    let cleaned = strip_matching_brackets(&s, |clause| {
        let c = fold_diacritics_and_symbols(clause);
        if let Some(yr) = (c.len() == 4)
            .then(|| c.parse::<i32>().ok())
            .flatten()
            .filter(|y| (1950..=2100).contains(y))
        {
            if extracted_year.is_none() {
                extracted_year = Some(yr);
            }
            return true;
        }
        for token in [
            "flac", "mp3", "320", "v0", "cbr", "vbr", "ogg", "opus", "aac", "m4a", "wav", "alac",
            "24bit", "16bit", "web", "cd", "vinyl", "lossless", "hi res", "hires",
        ] {
            if c == token
                || c.starts_with(&format!("{token} "))
                || c.ends_with(&format!(" {token}"))
            {
                return true;
            }
        }
        false
    });

    (collapse_whitespace(&cleaned), extracted_year)
}

pub fn is_disc_subfolder(folder_name: &str) -> bool {
    let norm = fold_diacritics_and_symbols(folder_name);
    for prefix in ["cd", "disc", "disk", "side"] {
        if let Some(rest) = norm.strip_prefix(prefix) {
            let rest = rest.trim();
            if !rest.is_empty()
                && rest
                    .chars()
                    .all(|c| c.is_ascii_digit() || matches!(c, 'a' | 'b'))
            {
                return true;
            }
        }
    }
    false
}

fn split_last_dash_suffix(s: &str) -> Option<(&str, &str)> {
    for sep in [" - ", " – ", " — ", ": "] {
        if let Some(pos) = s.rfind(sep) {
            return Some((&s[..pos], &s[pos + sep.len()..]));
        }
    }
    None
}

fn strip_matching_brackets<F>(input: &str, mut predicate: F) -> String
where
    F: FnMut(&str) -> bool,
{
    let mut out = String::with_capacity(input.len());
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let open = chars[i];
        let close = match open {
            '(' => Some(')'),
            '[' => Some(']'),
            '{' => Some('}'),
            _ => None,
        };
        if let Some(close_ch) = close {
            let mut depth = 1usize;
            let mut j = i + 1;
            while j < chars.len() && depth > 0 {
                if chars[j] == open {
                    depth += 1;
                } else if chars[j] == close_ch {
                    depth -= 1;
                }
                j += 1;
            }
            if depth == 0 {
                let inner: String = chars[i + 1..j - 1].iter().collect();
                if predicate(&inner) {
                    i = j;
                    continue;
                }
            }
        }
        out.push(open);
        i += 1;
    }
    collapse_whitespace(&out)
}

fn is_feat_clause(clause: &str) -> bool {
    let c = clause.trim().to_lowercase();
    c.starts_with("feat.")
        || c.starts_with("feat ")
        || c.starts_with("ft.")
        || c.starts_with("ft ")
        || c.starts_with("featuring ")
        || c.starts_with("with ")
}

fn is_edition_or_feat_clause(clause: &str) -> bool {
    if is_feat_clause(clause) {
        return true;
    }
    is_edition_or_format_suffix(clause)
}

fn is_edition_or_format_suffix(clause: &str) -> bool {
    let norm = fold_diacritics_and_symbols(clause);
    if norm.is_empty() {
        return true;
    }
    if matches!(norm.as_str(), "ep" | "single" | "explicit" | "clean") {
        return true;
    }
    for kw in [
        "deluxe",
        "remaster",
        "remastered",
        "edition",
        "anniversary",
        "bonus track",
        "bonus tracks",
        "expanded",
        "reissue",
        "collector",
        "special version",
        "deluxe version",
        "album version",
        "single version",
        "radio edit",
        "explicit version",
        "clean version",
        "tour edition",
        "japanese",
        "international version",
    ] {
        if norm.contains(kw) {
            return true;
        }
    }
    false
}

/// Compute current UTC date (`YYYY-MM-DD`).
pub fn today_iso_utc() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    epoch_secs_to_iso_date(secs)
}

/// Compute current UTC timestamp (`YYYY-MM-DDTHH:MM:SSZ`).
pub fn now_rfc3339_utc() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let hh = rem / 3600;
    let mm = (rem % 3600) / 60;
    let ss = rem % 60;
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}Z")
}

/// Compute UTC date `days_back` days before today (`YYYY-MM-DD`).
pub fn days_ago_iso_utc(days_back: u32) -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let shifted = secs - (days_back as i64) * 86_400;
    epoch_secs_to_iso_date(shifted)
}

pub fn epoch_secs_to_iso_date(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Howard Hinnant's proleptic Gregorian `civil_from_days` algorithm.
fn civil_from_days(days_since_unix_epoch: i64) -> (i32, u32, u32) {
    let z = days_since_unix_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };
    (year as i32, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_artist_diacritics_and_feat_clauses() {
        assert_eq!(normalize_artist_name("Sigur Rós"), "sigur ros");
        assert_eq!(normalize_artist_name("Björk"), "bjork");
        assert_eq!(
            normalize_artist_name("Florence + The Machine"),
            "florence and the machine"
        );
        assert_eq!(
            strip_featured_artists("Gorillaz feat. De La Soul"),
            "Gorillaz"
        );
        assert_eq!(
            strip_featured_artists("Daft Punk (feat. Pharrell Williams)"),
            "Daft Punk"
        );
    }

    #[test]
    fn normalizes_deluxe_and_remastered_release_titles() {
        assert_eq!(
            normalize_release_title("Random Access Memories (10th Anniversary Edition)"),
            normalize_release_title("Random Access Memories")
        );
        assert_eq!(
            normalize_release_title("Discovery [2021 Remaster] - Deluxe Edition"),
            "discovery"
        );
        assert_eq!(normalize_release_title("Windowlicker - EP"), "windowlicker");
        assert_eq!(
            normalize_release_title("Midnight City - Single"),
            "midnight city"
        );
        // Distinct non-edition parenthetical title stays intact
        assert_eq!(
            normalize_release_title("Blue (Da Ba Dee)"),
            "blue da ba dee"
        );
    }

    #[test]
    fn detects_various_artists_and_noisy_releases() {
        assert!(is_various_or_generic_artist("Various Artists"));
        assert!(is_various_or_generic_artist("V.A."));
        assert!(is_various_or_generic_artist("Original Soundtrack"));
        assert!(!is_various_or_generic_artist("Boards of Canada"));

        assert!(is_noisy_release_title("Greatest Hits (Remastered)"));
        assert!(is_noisy_release_title("Alive 2007 (Live at Bercy)"));
        assert!(!is_noisy_release_title("Music Has the Right to Children"));
    }

    #[test]
    fn cleans_folder_names_and_disc_subfolders() {
        let (title, year) =
            clean_folder_album_name("1998 - Music Has the Right to Children [FLAC]");
        assert_eq!(title, "Music Has the Right to Children");
        assert_eq!(year, Some(1998));
        assert!(is_disc_subfolder("CD 1"));
        assert!(is_disc_subfolder("Disc 2"));
        assert!(!is_disc_subfolder("Discovery"));
    }

    #[test]
    fn formats_epoch_dates_accurately() {
        assert_eq!(epoch_secs_to_iso_date(0), "1970-01-01");
        // 2026-11-26 00:00:00 UTC = 20783 days * 86400 = 1795651200
        assert_eq!(epoch_secs_to_iso_date(1_795_651_200), "2026-11-26");
    }
}
