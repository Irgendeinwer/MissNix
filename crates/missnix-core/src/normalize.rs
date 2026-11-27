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
    let first_null = raw.split('\0').next().unwrap_or(raw).trim();
    if first_null.is_empty() {
        return String::new();
    }

    // Remove parenthesized/bracketed feat clauses first, e.g. "Artist (feat. Someone)"
    let mut cleaned = strip_matching_brackets(first_null, is_feat_clause);

    // The markers are ASCII. ASCII-only lowercasing preserves the byte offsets of
    // the original UTF-8 string, unlike Unicode lowercasing (e.g. ẞ or İ).
    let lower = cleaned.to_ascii_lowercase();
    for marker in [
        " feat. ",
        " feat ",
        " ft. ",
        " ft ",
        " featuring ",
        " w/ ",
        " pres. ",
        " presents ",
    ] {
        if let Some(idx) = lower.find(marker) {
            cleaned.truncate(idx);
            break;
        }
    }

    collapse_whitespace(cleaned.trim_end_matches(['-', ',', ';', '/']))
}

/// Extract the primary artist from a tag or folder value that may contain multi-artist
/// collaboration delimiters (`;`, `\0`, ` / `, `\\`, `feat.`, `vs.`, or `, `), while
/// preserving genuine comma-containing band names (`Tyler, The Creator`, `Earth, Wind & Fire`,
/// `Crosby, Stills & Nash`, `Black Country, New Road`, `AC/DC`) and honoring any matching
/// folder artist hint (`folder_artist_hint`).
pub fn extract_primary_artist(
    raw: &str,
    folder_artist_hint: Option<&str>,
    split_collaborations: bool,
) -> String {
    let base = strip_featured_artists(raw);
    if base.is_empty() {
        return String::new();
    }
    if !split_collaborations {
        return base;
    }

    let hint_clean = folder_artist_hint.map(strip_featured_artists).filter(|h| {
        !h.is_empty() && !is_various_or_generic_artist(h) && !is_generic_bucket_folder(h)
    });
    let hint_norm = hint_clean.as_deref().map(fold_diacritics_and_symbols);

    // If the parent folder is literally named the exact same full string (e.g. `/Music/Tyler, The Creator/`
    // or `/Music/Black Country, New Road/`), preserve it intact unless it uses hard `;` tag separators.
    if !base.contains(';')
        && !base.contains('\\')
        && let Some(ref hn) = hint_norm
        && !hn.is_empty()
        && fold_diacritics_and_symbols(&base) == *hn
    {
        return base;
    }

    // 1. Hard multi-artist tag delimiters: `;`, `\`, ` // `, ` / ` (spaced so `AC/DC` stays intact), ` vs. `, ` vs `
    let hard_parts = split_hard_artist_delimiters(&base);
    if hard_parts.len() > 1 {
        if let Some(ref hn) = hint_norm
            && let Some(matched) = hard_parts
                .iter()
                .find(|p| fold_diacritics_and_symbols(p) == *hn)
        {
            return (*matched).clone();
        }
        if let Some(first) = hard_parts
            .into_iter()
            .find(|p| !p.is_empty() && !is_various_or_generic_artist(p))
        {
            // Also check comma delimiters inside the first segment if any
            return extract_comma_primary(&first, hint_norm.as_deref());
        }
    }

    // 2. Comma-space collaboration credits (e.g. `"Alligatoah, Jamal"`, `"Alexander Marcus, 257ers"`)
    extract_comma_primary(&base, hint_norm.as_deref())
}

fn split_hard_artist_delimiters(input: &str) -> Vec<String> {
    let mut normalized_seps = input.replace([';', '\\', '\0'], "\x1f");
    for sep in [" // ", " / ", " vs. ", " vs ", " VS. ", " VS "] {
        normalized_seps = normalized_seps.replace(sep, "\x1f");
    }
    normalized_seps
        .split('\x1f')
        .map(strip_featured_artists)
        .filter(|s| !s.is_empty())
        .collect()
}

fn extract_comma_primary(input: &str, hint_norm: Option<&str>) -> String {
    if !input.contains(", ") || is_preserved_comma_artist_name(input) {
        return collapse_whitespace(input);
    }

    let parts: Vec<String> = input
        .split(", ")
        .map(strip_featured_artists)
        .filter(|s| !s.is_empty())
        .collect();

    if parts.len() <= 1 {
        return collapse_whitespace(input);
    }

    if let Some(hn) = hint_norm
        && let Some(matched) = parts.iter().find(|p| fold_diacritics_and_symbols(p) == hn)
    {
        return matched.clone();
    }

    parts
        .into_iter()
        .find(|p| !is_various_or_generic_artist(p))
        .unwrap_or_else(|| collapse_whitespace(input))
}

/// Returns `true` for real artist/band names that legitimately contain `", "` so they
/// are never split into fake partial names.
pub fn is_preserved_comma_artist_name(name: &str) -> bool {
    let trimmed = collapse_whitespace(name);
    let lower = trimmed.to_lowercase();

    // Multi-member ensembles ending with `&` or `and`, e.g. "Earth, Wind & Fire",
    // "Crosby, Stills & Nash", "Emerson, Lake & Palmer", "Peter, Paul and Mary".
    if lower.contains(" & ") || lower.contains(" and ") {
        return true;
    }

    // "<Name>, The <Epithet>" such as "Tyler, The Creator" or "Chance, The Rapper"
    if let Some((left, right)) = lower.split_once(", ") {
        let left_words = left.split_whitespace().count();
        let right_words = right.split_whitespace().count();
        if !right.contains(", ")
            && (right.starts_with("the ")
                || right == "jr."
                || right == "jr"
                || right == "sr."
                || right == "ii"
                || right == "iii")
            && left_words <= 2
            && right_words <= 3
        {
            return true;
        }
    }

    let folded = fold_diacritics_and_symbols(&trimmed);
    matches!(
        folded.as_str(),
        "black country new road"
            | "now now"
            | "hey nothing"
            | "oh sleeper"
            | "goodbye june"
            | "hello meteor"
            | "mewithoutyou"
            | "dexys midnight runners"
    )
}

/// Detect generic non-artist bucket folder names (`Downloads`, `Singles`, `Unsorted`, `OST`, etc.)
/// so folder-based discovery does not treat top-level dump directories as artist names.
pub fn is_generic_bucket_folder(name: &str) -> bool {
    let norm = fold_diacritics_and_symbols(name);
    matches!(
        norm.as_str(),
        "" | "music"
            | "my music"
            | "audio"
            | "downloads"
            | "download"
            | "singles"
            | "single"
            | "albums"
            | "album"
            | "tracks"
            | "unsorted"
            | "misc"
            | "miscellaneous"
            | "compilations"
            | "compilation"
            | "soundtracks"
            | "soundtrack"
            | "ost"
            | "messy dump"
            | "incoming"
            | "new"
            | "temp"
            | "tmp"
            | "flac"
            | "mp3"
            | "lossless"
            | "rips"
    ) || is_various_or_generic_artist(name)
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
    fn strips_features_without_corrupting_unicode_byte_offsets() {
        assert_eq!(strip_featured_artists("GROẞ feat. Guest"), "GROẞ");
        assert_eq!(strip_featured_artists("STRAẞE feat. Guest"), "STRAẞE");
        assert_eq!(strip_featured_artists("İ FEAT. Guest"), "İ");
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

    #[test]
    fn extracts_primary_artist_from_multi_artist_delimiters_without_breaking_bands() {
        // Real-world semicolon & comma collaboration tags
        assert_eq!(
            extract_primary_artist("Alottadogs; GLITTERMUTT", None, true),
            "Alottadogs"
        );
        assert_eq!(
            extract_primary_artist("Alottadogs; GLITTERMUTT", Some("GLITTERMUTT"), true),
            "GLITTERMUTT"
        );
        assert_eq!(
            extract_primary_artist("Alligatoah, Jamal", None, true),
            "Alligatoah"
        );
        assert_eq!(
            extract_primary_artist("Alligatoah, Dima Richman", Some("Alligatoah"), true),
            "Alligatoah"
        );
        assert_eq!(
            extract_primary_artist("Alexander Marcus, 257ers", None, true),
            "Alexander Marcus"
        );
        assert_eq!(
            extract_primary_artist("Kanye West / Jay-Z", None, true),
            "Kanye West"
        );

        // Genuine band names with slashes or commas must stay intact
        assert_eq!(extract_primary_artist("AC/DC", None, true), "AC/DC");
        assert_eq!(
            extract_primary_artist("Tyler, The Creator", None, true),
            "Tyler, The Creator"
        );
        assert_eq!(
            extract_primary_artist("Earth, Wind & Fire", None, true),
            "Earth, Wind & Fire"
        );
        assert_eq!(
            extract_primary_artist("Crosby, Stills & Nash", None, true),
            "Crosby, Stills & Nash"
        );
        assert_eq!(
            extract_primary_artist("Black Country, New Road", None, true),
            "Black Country, New Road"
        );
        assert_eq!(
            extract_primary_artist("10,000 Maniacs", None, true),
            "10,000 Maniacs"
        );
    }
}
