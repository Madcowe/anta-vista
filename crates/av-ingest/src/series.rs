//! Compaction of raw watch‑list media item names into compact descriptions.
//!
//! Raw filenames like `Petticoat Junction S01E02.mp4` … `S01E15.mp4` are
//! collapsed into a single natural‑language entry, e.g.
//! `Petticoat Junction season 1 episodes 1-15`. Standalone movies are kept
//! individually, and near‑duplicate movie entries are deduped.

use std::collections::{BTreeMap, HashSet};

/// A single parsed media item.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ParsedItem {
    /// A movie / media with no episode token. `key` is used for dedupe,
    /// `display` is what gets rendered.
    Movie { key: String, display: String },
    /// One episode of a tracked series. `title_key` groups episodes,
    /// `title_display` preserves the original casing for rendering.
    Episode {
        title_key: String,
        title_display: String,
        season: u32,
        episode: u32,
    },
}

/// A series title with per‑season episode numbers.
#[derive(Debug, Clone, Default)]
struct TitleGroup {
    display: String,
    seasons: BTreeMap<u32, Vec<u32>>,
}

/// Compact a raw item list into display entries.
///
/// - Series episodes are reduced to one entry per title, placed at the first
///   occurrence of an episode of that title, sorted/deduped by episode number.
/// - Other items are listed individually; duplicates (same title key) are
///   reduced to the first occurrence.
/// - Ordering is otherwise preserved (deterministic).
pub fn compact_items(items: &[String]) -> Vec<String> {
    let parsed: Vec<ParsedItem> = items.iter().filter_map(|i| parse_item(i)).collect();

    let mut groups: BTreeMap<String, TitleGroup> = BTreeMap::new();
    let mut movies: Vec<(String, String)> = Vec::new();

    for item in &parsed {
        match item {
            ParsedItem::Movie { key, display } => movies.push((key.clone(), display.clone())),
            ParsedItem::Episode {
                title_key,
                title_display,
                season,
                episode,
            } => {
                let group = groups.entry(title_key.clone()).or_insert_with(|| TitleGroup {
                    display: title_display.clone(),
                    seasons: BTreeMap::new(),
                });
                group.seasons.entry(*season).or_default().push(*episode);
            }
        }
    }

    let mut emitted_titles: HashSet<String> = HashSet::new();
    for group in groups.values_mut() {
        for eps in group.seasons.values_mut() {
            eps.sort_unstable();
            eps.dedup();
        }
    }
    let mut out: Vec<String> = Vec::new();

    for item in &parsed {
        match item {
            ParsedItem::Movie { key, display } => {
                if emitted_titles.insert(key.clone()) {
                    out.push(display.clone());
                }
            }
            ParsedItem::Episode {
                title_key, ..
            } => {
                if emitted_titles.insert(title_key.clone()) {
                    let group = &groups[title_key];
                    let mut seasons: Vec<(u32, &Vec<u32>)> =
                        group.seasons.iter().map(|(s, e)| (*s, e)).collect();
                    seasons.sort_by_key(|(s, _)| *s);
                    out.push(render_title(&group.display, &seasons));
                }
            }
        }
    }

    out
}

/// Parse one raw item into `Movie` or `Episode`.
fn parse_item(raw: &str) -> Option<ParsedItem> {
    let stem = clean_stem(raw)?;

    // Prefer S#E# then NxN episode tokens.
    if let Some((start, season, episode)) = find_episode_token_se(&stem) {
        let prefix = &stem[..start];
        return Some(ParsedItem::Episode {
            title_key: normalize_key(prefix),
            title_display: display_title(prefix),
            season,
            episode,
        });
    }
    if let Some((start, season, episode)) = find_episode_token_nxn(&stem) {
        let prefix = &stem[..start];
        return Some(ParsedItem::Episode {
            title_key: normalize_key(prefix),
            title_display: display_title(prefix),
            season,
            episode,
        });
    }

    Some(ParsedItem::Movie {
        key: movie_key(&stem),
        display: stem,
    })
}

/// Strip extension + trailing hex junk from a filename.
/// `Night of the Living Dead (1968) [1080p].mp4.66cacd06`
/// → `Night of the Living Dead (1968) [1080p]`
fn clean_stem(raw: &str) -> Option<String> {
    let mut s = raw.trim().to_string();
    if s.is_empty() {
        return None;
    }
    // Remove trailing ".<hex>" junk (e.g. "...mp4.66cacd06").
    loop {
        let Some((head, tail)) = s.rsplit_once('.') else { break };
        if !tail.is_empty()
            && tail.chars().all(|c| c.is_ascii_hexdigit())
            && tail.len() >= 6
        {
            s = head.to_string();
        } else {
            break;
        }
    }
    // Strip the real extension (last dot segment).
    if let Some((head, tail)) = s.rsplit_once('.') {
        if !tail.is_empty() {
            s = head.to_string();
        }
    }
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// Find an `S<season>E<episode>` token (case‑insensitive). Returns the byte
/// index of the `S`, plus season/episode numbers.
fn find_episode_token_se(s: &str) -> Option<(usize, u32, u32)> {
    let bytes = s.as_bytes();
    let n = bytes.len();
    let mut i = 0;
    while i < n {
        if bytes[i].eq_ignore_ascii_case(&b's') {
            let mut j = i + 1;
            while j < n && bytes[j].is_ascii_digit() {
                j += 1;
            }
            if j > i + 1 && j < n && bytes[j].eq_ignore_ascii_case(&b'e') {
                let mut k = j + 1;
                while k < n && bytes[k].is_ascii_digit() {
                    k += 1;
                }
                if k > j + 1 {
                    let season: u32 = s[i + 1..j].parse().ok()?;
                    let episode: u32 = s[j + 1..k].parse().ok()?;
                    return Some((i, season, episode));
                }
            }
            i = j.max(i + 1);
        } else {
            i += 1;
        }
    }
    None
}

/// Find an `NxN` episode token (case‑insensitive, e.g. `1x05`).
fn find_episode_token_nxn(s: &str) -> Option<(usize, u32, u32)> {
    let bytes = s.as_bytes();
    let n = bytes.len();
    let mut i = 0;
    while i < n {
        if bytes[i].is_ascii_digit() {
            let mut j = i + 1;
            while j < n && bytes[j].is_ascii_digit() {
                j += 1;
            }
            if j < n && bytes[j].eq_ignore_ascii_case(&b'x') {
                let mut k = j + 1;
                while k < n && bytes[k].is_ascii_digit() {
                    k += 1;
                }
                if k > j + 1 {
                    let season: u32 = s[i..j].parse().ok()?;
                    let episode: u32 = s[j + 1..k].parse().ok()?;
                    return Some((i, season, episode));
                }
            }
            i = j.max(i + 1);
        } else {
            i += 1;
        }
    }
    None
}

/// Grouping key: lowercased, separators collapsed, quality tokens dropped.
/// Year/imdb markers are dropped here so `Some Show` and `Some Show (2019)`
/// both map to the same series.
fn normalize_key(prefix: &str) -> String {
    tokens(prefix)
        .into_iter()
        .filter(|t| !is_ignored_token(t))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Display title: original casing, separators collapsed, quality tokens
/// dropped.
fn display_title(prefix: &str) -> String {
    tokens(prefix)
        .into_iter()
        .filter(|t| !is_ignored_token(t))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Movie dedupe key: like `normalize_key` but keeps year + imdb markers so
/// same‑name remakes are never falsely merged. Quality tokens still dropped.
fn movie_key(stem: &str) -> String {
    tokens(stem)
        .into_iter()
        .filter(|t| !is_quality_token(t))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Split a string into alphanumeric tokens (separators collapsed).
fn tokens(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_alphanumeric())
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(|t| t.to_string())
        .collect()
}

fn is_ignored_token(t: &str) -> bool {
    is_quality_token(t)
        || (t.len() == 4 && t.chars().all(|c| c.is_ascii_digit()))
        // imdb id tokens: "tt" + digits
        || (t.len() >= 7 && t.len() <= 10 && t.starts_with("tt") && t[2..].chars().all(|c| c.is_ascii_digit()))
}

fn is_quality_token(t: &str) -> bool {
    const QUALITY: &[&str] = &[
        "480p", "576p", "720p", "1080p", "1440p", "2160p", "4320p", "4k", "8k", "uhd", "fhd",
        "qhd", "hdr", "hdr10", "hdr10plus", "dolbyvision", "web", "webrip", "webdl", "web-dl",
        "bluray", "brrip", "bdrip", "dvdrip", "hdtv", "hdtvrip", "pdtv", "remux", "x264", "x265",
        "h264", "h265", "hevc", "avc", "av1", "vp9", "proper", "repack", "extended", "unrated",
        "directors", "cut", "screener", "cam", "tsrip",
    ];
    QUALITY.contains(&t.to_ascii_lowercase().as_str())
}

/// Render a title + per-season episode sets.
fn render_title(title: &str, seasons: &[(u32, &Vec<u32>)]) -> String {
    if seasons.len() == 1 {
        let (season, eps) = seasons[0];
        format!("{} {}", title, render_season(season, eps))
    } else {
        let clauses: Vec<String> = seasons
            .iter()
            .map(|(season, eps)| render_season(*season, eps))
            .collect();
        format!("{} {}", title, clauses.join(", "))
    }
}

fn render_season(season: u32, eps: &[u32]) -> String {
    match eps.len() {
        0 => String::new(),
        1 => format!("season {} episode {}", season, eps[0]),
        _ => format!("season {} episodes {}", season, render_runs(eps)),
    }
}

/// Collapse consecutive sorted episode numbers into inclusive runs.
fn collapse_runs(eps: &[u32]) -> Vec<(u32, u32)> {
    let mut runs: Vec<(u32, u32)> = Vec::new();
    for &ep in eps {
        match runs.last_mut() {
            Some((start, end)) if ep == *end + 1 => *end = ep,
            _ => runs.push((ep, ep)),
        }
    }
    runs
}

fn render_runs(eps: &[u32]) -> String {
    collapse_runs(eps)
        .iter()
        .map(|&(a, b)| {
            if a == b {
                a.to_string()
            } else {
                format!("{}-{}", a, b)
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collapses_consecutive_run() {
        let items: Vec<String> = (2..=15)
            .map(|e| format!("Petticoat Junction S01E{e:02}.mp4"))
            .collect();
        let out = compact_items(&items);
        assert_eq!(out, ["Petticoat Junction season 1 episodes 2-15"]);
    }

    #[test]
    fn preserves_disjoint_gaps() {
        let items = vec![
            "Show S01E01.mkv".into(),
            "Show S01E03.mkv".into(),
            "Show S01E04.mkv".into(),
        ];
        assert_eq!(compact_items(&items), ["Show season 1 episodes 1, 3-4"]);
    }

    #[test]
    fn dedupes_duplicate_episodes() {
        let items = vec![
            "Show S01E01.mkv".into(),
            "Show S01E01.mkv".into(),
            "Show S01E02.mkv".into(),
        ];
        assert_eq!(compact_items(&items), ["Show season 1 episodes 1-2"]);
    }

    #[test]
    fn quality_tags_do_not_split_series() {
        let items = vec![
            "Petticoat Junction - [1080p] S01E01.mp4".into(),
            "Petticoat.Junction.S01E02.2160p.WEB.mp4".into(),
        ];
        assert_eq!(
            compact_items(&items),
            ["Petticoat Junction season 1 episodes 1-2"]
        );
    }

    #[test]
    fn merges_series_across_years_and_separators() {
        let items = vec![
            "One Step Beyond (2019) S1E01.mkv".into(),
            "One_Step_Beyond S1E02.mkv".into(),
        ];
        assert_eq!(
            compact_items(&items),
            ["One Step Beyond season 1 episodes 1-2"]
        );
    }

    #[test]
    fn multi_season_renders_clauses() {
        let items = vec![
            "Petticoat Junction S01E01.mp4".into(),
            "Petticoat Junction S01E05.mp4".into(),
            "Petticoat Junction S02E02.mp4".into(),
        ];
        assert_eq!(
            compact_items(&items),
            ["Petticoat Junction season 1 episodes 1, 5, season 2 episode 2"]
        );
    }

    #[test]
    fn single_episode_uses_episode_form() {
        let items = vec!["The Outer Limit S01E09.mkv".into()];
        assert_eq!(compact_items(&items), ["The Outer Limit season 1 episode 9"]);
    }

    #[test]
    fn movies_stay_individual_but_dedupe_variants() {
        let items = vec![
            "The General (1926) {imdb-tt0017925}.mp4".into(),
            "The Cabinet of Dr. Caligari (1920) {imdb-tt0010323}.mp4".into(),
            "Night of the Living Dead (1968) {imdb-tt0063350} - [1080p].mp4".into(),
            "Night of the Living Dead (1968) {imdb-tt0063350} - [1080p].mp4.66cacd06".into(),
        ];
        let out = compact_items(&items);
        for entry in &out {
            assert!(!entry.contains("66cacd06"), "hex junk leaked: {entry}");
        }
        // The two same-title/quality variants collapse to one entry.
        let living_dead: Vec<&String> = out
            .iter()
            .filter(|e| e.starts_with("Night of the Living Dead"))
            .collect();
        assert_eq!(living_dead.len(), 1, "movie variants not deduped: {out:?}");
        // Distinct titles are untouched.
        assert_eq!(out.len(), 3, "unexpected output: {out:?}");
    }

    #[test]
    fn different_series_never_merge() {
        let items = vec![
            "Petticoat Junction S01E01.mp4".into(),
            "One Step Beyond S01E01.mp4".into(),
        ];
        let out = compact_items(&items);
        assert_eq!(
            out,
            [
                "Petticoat Junction season 1 episode 1",
                "One Step Beyond season 1 episode 1"
            ]
        );
    }

    #[test]
    fn no_episode_token_movie_passthrough() {
        let items = vec!["A Plain OLD Movie (1955).mp4".into()];
        assert_eq!(compact_items(&items), ["A Plain OLD Movie (1955)"]);
    }

    #[test]
    fn mixed_bundle_preserves_order() {
        let items = vec![
            "The General (1926) {imdb-tt0017925}.mp4".into(),
            "Petticoat Junction S01E01.mp4".into(),
            "Petticoat Junction S01E02.mp4".into(),
            "Charade (1963) {imdb-tt0056923}.mp4".into(),
        ];
        assert_eq!(
            compact_items(&items),
            [
                "The General (1926) {imdb-tt0017925}",
                "Petticoat Junction season 1 episodes 1-2",
                "Charade (1963) {imdb-tt0056923}"
            ]
        );
    }

    #[test]
    fn nxn_syntax_supported() {
        let items = vec!["Show 1x01.mkv".into(), "Show 1x02.mkv".into()];
        assert_eq!(compact_items(&items), ["Show season 1 episodes 1-2"]);
    }

    #[test]
    fn hex_suffix_cleaned_in_movies() {
        let items = vec!["Some.File.S01E01.mp4.9f3a1b2c".into()];
        assert_eq!(compact_items(&items), ["Some File season 1 episode 1"]);
    }
}