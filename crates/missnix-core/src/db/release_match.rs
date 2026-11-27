//! Conservative matching of provider releases. Provider IDs are authoritative;
//! title/date matching is only a fallback and must not choose between candidates.

use crate::normalize::fold_diacritics_and_symbols;

#[derive(Debug, Clone)]
pub(super) struct ReleaseIdentity {
    pub id: i64,
    pub title: String,
    pub normalized_title: String,
    pub release_type: String,
    pub release_date: String,
    pub deezer_album_id: Option<i64>,
    pub mb_release_group_id: Option<String>,
}

impl ReleaseIdentity {
    fn shares_id_with(&self, other: &Self) -> bool {
        self.deezer_album_id.is_some() && self.deezer_album_id == other.deezer_album_id
            || self.mb_release_group_id.is_some()
                && self.mb_release_group_id == other.mb_release_group_id
    }

    pub fn conflicts_with(&self, other: &Self) -> bool {
        matches!((self.deezer_album_id, other.deezer_album_id), (Some(a), Some(b)) if a != b)
            || matches!(
                (&self.mb_release_group_id, &other.mb_release_group_id),
                (Some(a), Some(b)) if a != b
            )
    }

    fn is_complementary_to(&self, other: &Self) -> bool {
        (self.deezer_album_id.is_some()
            && self.mb_release_group_id.is_none()
            && other.deezer_album_id.is_none()
            && other.mb_release_group_id.is_some())
            || (other.deezer_album_id.is_some()
                && other.mb_release_group_id.is_none()
                && self.deezer_album_id.is_none()
                && self.mb_release_group_id.is_some())
    }
}

/// Return the selected row and, when unambiguous, another row to consolidate.
/// Rows must include all same-title candidates for this artist, including rows
/// with conflicting provider IDs: those conflicts are evidence of ambiguity.
pub(super) fn choose_release_match(
    incoming: &ReleaseIdentity,
    rows: &[ReleaseIdentity],
    allow_cross_type: bool,
) -> (Option<usize>, Option<usize>) {
    let direct: Vec<usize> = rows
        .iter()
        .enumerate()
        .filter_map(|(index, row)| incoming.shares_id_with(row).then_some(index))
        .collect();

    if let Some(&selected) = direct.first() {
        // More than one row may already contain the same provider ID. Merge a
        // pair only if no third row competes and their other IDs don't disagree.
        if direct.len() > 1 {
            let merge = (direct.len() == 2 && !rows[selected].conflicts_with(&rows[direct[1]]))
                .then_some(direct[1]);
            return (Some(selected), merge);
        }

        let base = &rows[selected];
        let shared: Vec<usize> = rows
            .iter()
            .enumerate()
            .filter_map(|(index, row)| {
                (index != selected && base.shares_id_with(row)).then_some(index)
            })
            .collect();
        if !shared.is_empty() {
            let merge =
                (shared.len() == 1 && !base.conflicts_with(&rows[shared[0]])).then_some(shared[0]);
            return (Some(selected), merge);
        }

        let merge = unique_title_match(base, rows, Some(selected), allow_cross_type);
        (Some(selected), merge)
    } else {
        (
            unique_title_match(incoming, rows, None, allow_cross_type),
            None,
        )
    }
}

fn unique_title_match(
    base: &ReleaseIdentity,
    rows: &[ReleaseIdentity],
    excluded: Option<usize>,
    allow_cross_type: bool,
) -> Option<usize> {
    let plausible: Vec<usize> = rows
        .iter()
        .enumerate()
        .filter_map(|(index, row)| {
            (Some(index) != excluded
                && titles_match(base, row)
                && dates_match(&base.release_date, &row.release_date))
            .then_some(index)
        })
        .collect();

    // Agreement on type is stronger than a cross-type guess. Do not filter out
    // conflicting IDs before counting: two same-title releases are ambiguous.
    let same_type: Vec<usize> = plausible
        .iter()
        .copied()
        .filter(|&index| rows[index].release_type == base.release_type)
        .collect();
    if !same_type.is_empty() {
        return (same_type.len() == 1 && !base.conflicts_with(&rows[same_type[0]]))
            .then_some(same_type[0]);
    }

    // Providers sometimes disagree on Album / EP / Single. In that case require
    // one complementary Deezer/MB pair and the exact same full release date.
    // If a provider's fetched batch has competing titles, the caller disables
    // this weaker fallback for every member of that batch.
    if allow_cross_type && plausible.len() == 1 {
        let index = plausible[0];
        let row = &rows[index];
        if !base.conflicts_with(row)
            && base.is_complementary_to(row)
            && full_date(&base.release_date)
            && base.release_date == row.release_date
        {
            return Some(index);
        }
    }
    None
}

fn titles_match(left: &ReleaseIdentity, right: &ReleaseIdentity) -> bool {
    if left.normalized_title.is_empty() || left.normalized_title != right.normalized_title {
        return false;
    }
    let left_title = fold_diacritics_and_symbols(&left.title);
    let right_title = fold_diacritics_and_symbols(&right.title);
    left_title == right_title
        || (only_safe_suffix(&left_title, &left.normalized_title)
            && only_safe_suffix(&right_title, &right.normalized_title))
}

fn only_safe_suffix(title: &str, base: &str) -> bool {
    if title == base {
        return true;
    }
    let Some(suffix) = title
        .strip_prefix(base)
        .and_then(|rest| rest.strip_prefix(' '))
    else {
        return false;
    };
    let mut has_marker = false;
    for token in suffix.split_whitespace() {
        if matches!(
            token,
            "ep" | "single"
                | "deluxe"
                | "remaster"
                | "remastered"
                | "edition"
                | "expanded"
                | "explicit"
                | "clean"
                | "bonus"
                | "track"
                | "tracks"
                | "version"
                | "anniversary"
                | "reissue"
        ) {
            has_marker = true;
        } else if token.len() > 4 || !token.bytes().all(|byte| byte.is_ascii_digit()) {
            return false;
        }
    }
    has_marker
}

fn valid_date(date: &str) -> bool {
    let bytes = date.as_bytes();
    if !matches!(bytes.len(), 4 | 7 | 10)
        || !bytes.iter().enumerate().all(|(index, byte)| {
            if index == 4 || index == 7 {
                *byte == b'-'
            } else {
                byte.is_ascii_digit()
            }
        })
    {
        return false;
    }
    let year = date[..4].parse::<u32>().unwrap_or(0);
    if year == 0 {
        return false;
    }
    if bytes.len() == 4 {
        return true;
    }
    let month = date[5..7].parse::<u8>().unwrap_or(0);
    if !(1..=12).contains(&month) {
        return false;
    }
    if bytes.len() == 7 {
        return true;
    }
    let day = date[8..10].parse::<u8>().unwrap_or(0);
    let max_day = match month {
        4 | 6 | 9 | 11 => 30,
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        _ => 31,
    };
    (1..=max_day).contains(&day)
}

fn full_date(date: &str) -> bool {
    date.len() == 10 && valid_date(date)
}

fn dates_match(left: &str, right: &str) -> bool {
    if !valid_date(left) || !valid_date(right) {
        return false;
    }
    let common = left.len().min(right.len());
    left[..common] == right[..common]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::normalize::normalize_release_title;

    fn release(id: i64, title: &str, kind: &str, date: &str) -> ReleaseIdentity {
        ReleaseIdentity {
            id,
            title: title.to_string(),
            normalized_title: normalize_release_title(title),
            release_type: kind.to_string(),
            release_date: date.to_string(),
            deezer_album_id: None,
            mb_release_group_id: None,
        }
    }

    #[test]
    fn matches_a_unique_cross_provider_pair_in_both_orders() {
        let mut dz = release(1, "Test EP - EP", "album", "2026-01-02");
        dz.deezer_album_id = Some(1);
        let mut mb = release(2, "Test EP", "ep", "2026-01-02");
        mb.mb_release_group_id = Some("rg-1".into());
        assert_eq!(
            choose_release_match(&mb, &[dz.clone()], true),
            (Some(0), None)
        );
        assert_eq!(
            choose_release_match(&dz, &[mb.clone()], true),
            (Some(0), None)
        );
        assert_eq!(
            choose_release_match(&dz, &[dz.clone(), mb], true),
            (Some(0), Some(1))
        );
        assert_eq!(
            choose_release_match(&dz, std::slice::from_ref(&dz), false),
            (Some(0), None)
        );
    }

    #[test]
    fn leaves_same_title_ambiguities_conflicts_and_weak_dates_unmerged() {
        let mut dz = release(1, "Echo", "album", "2026-01-02");
        dz.deezer_album_id = Some(1);
        let mut mb = release(2, "Echo", "single", "2026-01-02");
        mb.mb_release_group_id = Some("rg-1".into());
        let mut second_mb = mb.clone();
        second_mb.id = 3;
        second_mb.mb_release_group_id = Some("rg-2".into());
        assert_eq!(
            choose_release_match(&dz, &[dz.clone(), mb.clone(), second_mb], true),
            (Some(0), None)
        );
        assert_eq!(
            choose_release_match(&mb, &[dz.clone()], false),
            (None, None)
        );
        let mut other_day = mb.clone();
        other_day.release_date = "2026-01-03".into();
        assert_eq!(
            choose_release_match(&other_day, &[dz.clone()], true),
            (None, None)
        );
        let mut incomplete = mb;
        incomplete.release_date = "2026".into();
        assert_eq!(choose_release_match(&incomplete, &[dz], true), (None, None));
    }

    #[test]
    fn rejects_invalid_calendar_dates_as_matching_evidence() {
        for invalid in [
            "",
            "0000",
            "2025-00",
            "2025-13",
            "2025-04-31",
            "2025-02-29",
            "2025-01-00",
        ] {
            assert!(!valid_date(invalid), "accepted {invalid}");
        }
        for valid in ["2025", "2025-02", "2024-02-29", "2000-02-29", "2025-04-30"] {
            assert!(valid_date(valid), "rejected {valid}");
        }
    }

    #[test]
    fn does_not_treat_features_or_arbitrary_editions_as_the_same_title() {
        let base = release(1, "Echo", "album", "2026-01-02");
        for title in ["Echo (feat. Someone)", "Echo (Edition of a Dream)"] {
            let other = release(2, title, "album", "2026-01-02");
            assert_eq!(
                choose_release_match(&other, std::slice::from_ref(&base), true),
                (None, None)
            );
        }
        let deluxe = release(3, "Echo (Deluxe Edition)", "album", "2026-01-02");
        assert_eq!(
            choose_release_match(&deluxe, &[base], true),
            (Some(0), None)
        );
    }
}
