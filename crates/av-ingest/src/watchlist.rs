use std::collections::{BTreeMap, HashSet};
use std::io::{Cursor, Read};

use av_core::constants::{EMBED_WINDOW_CHARS, WATCHLIST_MIME};
use av_core::types::ResourceDescriptor;
use serde_json::json;
use zip::ZipArchive;

use crate::describe::{fill_into_window, watchlist_base};

use crate::metadata::ExtractedMeta;

/// Parsed contents of a W@tch `.watch-list` bundle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchListMeta {
    /// Media file names, e.g. "Some Movie (2024) [2160p].mp4".
    /// From list.txt member-reference lines when present, else datamap member names.
    pub items: Vec<String>,
    /// Count of media files per extension (lowercased, no dot).
    pub media_types: BTreeMap<String, usize>,
}

/// Detect whether `bytes` are a W@tch `.watch-list` bundle.
/// Content-based: a ZIP that carries a `*.datamap` member or a `list.txt`,
/// or whose filename hint ends with `.watch-list`.
pub fn classify(bytes: &[u8], filename_hint: Option<&str>) -> Option<WatchListMeta> {
    let hint_is_watchlist = filename_hint
        .map(|f| f.to_ascii_lowercase().ends_with(".watch-list"))
        .unwrap_or(false);

    let mut archive = ZipArchive::new(Cursor::new(bytes)).ok()?;

    // Scan central directory for detection signals + collect member names
    let mut member_names = Vec::new();
    let mut has_datamap = false;
    let mut has_list_txt = false;
    let mut list_txt: Option<String> = None;

    for i in 0..archive.len() {
        let Ok(mut file) = archive.by_index(i) else { continue };
        let name = file.name().to_string();
        let lower = name.to_ascii_lowercase();

        if lower == "list.txt" {
            has_list_txt = true;
            // Try to read list.txt content (should be tiny).
            let mut buf = String::new();
            if file.read_to_string(&mut buf).is_ok() {
                list_txt = Some(buf);
            }
        }

        if lower.ends_with(".datamap") {
            has_datamap = true;
        }

        member_names.push(name);
    }

    if !has_datamap && !has_list_txt && !hint_is_watchlist {
        return None;
    }

    // Compute items: from list.txt when present, else from datamap members.
    let items = match &list_txt {
        Some(txt) => items_from_list_txt(txt, &member_names),
        None => items_from_members(&member_names),
    };
    // Fallback: list.txt may have referenced missing members → use members.
    let items = if items.is_empty() {
        items_from_members(&member_names)
    } else {
        items
    };

    Some(WatchListMeta {
        media_types: media_type_counts(&items),
        items,
    })
}

fn items_from_list_txt(txt: &str, member_names: &[String]) -> Vec<String> {
    // Build set of member basenames (including `.datamap`) for referencing.
    let member_bases: HashSet<String> = member_names
        .iter()
        .map(|n| n.rsplit('/').next().unwrap_or(n).to_string())
        .collect();

    let mut seen = HashSet::new();
    let mut out = Vec::new();

    for raw in txt.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        let lower = line.to_ascii_lowercase();
        if lower.starts_with("listname=\"") || lower.starts_with("listname='") {
            // ListName="..." marker — skip.
            continue;
        }
        if !lower.ends_with(".datamap") {
            // v1 hex lines ignored, per spec.
            continue;
        }
        if !member_bases.contains(line) {
            // Reference to a missing member – skip.
            continue;
        }

        let media = &line[..line.len() - ".datamap".len()];
        if seen.insert(media.to_string()) {
            out.push(media.to_string());
        }
    }
    out
}

fn items_from_members(member_names: &[String]) -> Vec<String> {
    // Process datamaps/ prefixed members first (spec: datamaps/ wins duplicates),
    // then root members.
    let (datamaps, rest): (Vec<&String>, Vec<&String>) = member_names
        .iter()
        .partition(|n| n.to_ascii_lowercase().starts_with("datamaps/"));

    let mut seen = HashSet::new();
    let mut out = Vec::new();

    for name in datamaps.into_iter().chain(rest.into_iter()) {
        if name.ends_with('/') {
            // Directory entry – ignore.
            continue;
        }
        let basename = name.rsplit('/').next().unwrap_or(name);
        let lower = basename.to_ascii_lowercase();
        if !lower.ends_with(".datamap") {
            continue;
        }
        let media = &basename[..basename.len() - ".datamap".len()];
        if seen.insert(media.to_string()) {
            out.push(media.to_string());
        }
    }
    out
}

fn media_type_counts(items: &[String]) -> BTreeMap<String, usize> {
    let mut map = BTreeMap::new();
    for item in items {
        let ext = item.rsplit('.').next().unwrap_or("");
        if !ext.is_empty() {
            *map.entry(ext.to_ascii_lowercase()).or_insert(0) += 1;
        }
    }
    map
}

fn bundle_stem(filename: &str) -> Option<String> {
    let path = std::path::Path::new(filename);
    let stem = path.file_stem()?;
    Some(stem.to_string_lossy().to_string())
}

/// Clean display name for the bundle, used as title and base‑description input.
/// Prefers the user‑supplied filename; otherwise pulls the real filename from
/// the location URL's last path segment (not the flattened inferred filename
/// which merges host/directories into a single string).
pub fn bundle_display_name(filename: Option<&str>, location: &str) -> Option<String> {
    if let Some(f) = filename {
        if let Some(stem) = bundle_stem(f) {
            return Some(stem);
        }
    }
    let last = crate::location::last_url_segment(location)?;
    let has_ext = last
        .rsplit_once('.')
        .map(|(before, _)| !before.is_empty())
        .unwrap_or(false);
    if has_ext {
        bundle_stem(&last)
    } else {
        None
    }
}

pub fn to_extracted_meta(
    meta: &WatchListMeta,
    filename_hint: Option<&str>,
) -> ExtractedMeta {
    let title = filename_hint.and_then(bundle_stem);
    let title_meta = ExtractedMeta {
        title: title.clone(),
        ..Default::default()
    };
    // Build base description using the same logic that `synthesize` would use
    // when it does not find a stored base.
    let base = watchlist_base(WATCHLIST_MIME, filename_hint, &title_meta);

    let content_preview = meta.items.join("\n");
    let extra = json!({
        "watch_list": {
            "base": base,
            "item_count": meta.items.len(),
            "media_types": meta.media_types,
            "items": meta.items,
        }
    });

    ExtractedMeta {
        title,
        content_preview: if content_preview.is_empty() {
            None
        } else {
            Some(content_preview)
        },
        extra,
        ..Default::default()
    }
}

pub fn items_from_meta(meta: &ExtractedMeta) -> Vec<String> {
    meta.extra
        .get("watch_list")
        .and_then(|w| w.get("items"))
        .and_then(|items| items.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default()
}

pub fn base_from_meta(meta: &ExtractedMeta) -> Option<String> {
    meta.extra
        .get("watch_list")
        .and_then(|w| w.get("base"))
        .and_then(|b| b.as_str())
        .map(|s| s.to_string())
}

/// Apply user tags to a descriptor's description_text.
/// For watch‑list resources the tags are inserted BEFORE the item list and the
/// item list is re‑filled so the whole description fits the embed window.
/// For all other types, reproduces the legacy append.
pub fn apply_tags(resource: &mut ResourceDescriptor, tags: &[String]) {
    let tags: Vec<String> = tags.iter().filter(|t| !t.is_empty()).cloned().collect();
    if tags.is_empty() {
        return;
    }

    if resource.mime_type == WATCHLIST_MIME {
        // Extract base and items directly from metadata_json Value.
        let maybe_base = resource
            .metadata_json
            .get("watch_list")
            .and_then(|w| w.get("base"))
            .and_then(|b| b.as_str())
            .map(|s| s.to_string());
        let items: Vec<String> = resource
            .metadata_json
            .get("watch_list")
            .and_then(|w| w.get("items"))
            .and_then(|items| items.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
            .unwrap_or_default();
        let base = maybe_base.unwrap_or_else(|| {
            // Fallback: reconstruct base from descriptor fields.
            // We need an ExtractedMeta for watchlist_base; create a dummy one with the title.
            let mut dummy = ExtractedMeta::default();
            if let Some(title) = &resource.metadata_json.get("title").and_then(|t| t.as_str()) {
                dummy.title = Some(title.to_string());
            }
            watchlist_base(
                &resource.mime_type,
                resource.filename.as_deref(),
                &dummy,
            )
        });
        resource.description_text = fill_into_window(&base, &items, Some(&tags), EMBED_WINDOW_CHARS);
    } else {
        let tag_str = tags.join(", ");
        resource.description_text = format!("{} tagged as: {}", resource.description_text, tag_str);
    }
}

// -----------------------------------------------------------------------------
// TESTS
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use av_core::types::ResourceKind;
    use std::io::{Write, Cursor};
    use zip::{ZipWriter, write::SimpleFileOptions};
    use zip::CompressionMethod;

    fn make_bundle(
        datamap_members: &[&str],
        datamaps_dir: bool,
        list_txt: Option<&str>,
    ) -> Vec<u8> {
        let mut buf = Vec::new();
        let mut writer = ZipWriter::new(Cursor::new(&mut buf));
        let opts = SimpleFileOptions::default();
        // Using Stored compression because deflate requires writers dependency.
        // For a test fixture, storing is fine.
        let opts = opts.compression_method(CompressionMethod::Stored);

        for name in datamap_members {
            let full_name = if datamaps_dir {
                format!("datamaps/{}", name)
            } else {
                name.to_string()
            };
            writer.start_file(full_name, opts).unwrap();
            // Write dummy data (a single byte) so the file is non‑empty.
            writer.write_all(&[0]).unwrap();
        }

        if let Some(content) = list_txt {
            writer.start_file("list.txt", opts).unwrap();
            writer.write_all(content.as_bytes()).unwrap();
        }

        writer.finish().unwrap();
        buf
    }

    #[test]
    fn classify_floor_zip_with_root_datamaps() {
        // Hand‑made floor format: root member.datamap files.
        let bytes = make_bundle(
            &["Some Show S01E01 (2023) [1080p].mkv.datamap", "Some Movie (2024) [2160p].mp4.datamap"],
            false,
            None,
        );
        let meta = classify(&bytes, None).unwrap();
        assert_eq!(meta.items, vec![
            "Some Show S01E01 (2023) [1080p].mkv",
            "Some Movie (2024) [2160p].mp4",
        ]);
        assert_eq!(*meta.media_types.get("mkv").unwrap(), 1);
        assert_eq!(*meta.media_types.get("mp4").unwrap(), 1);
    }

    #[test]
    fn classify_zip_with_list_txt_ordering() {
        let bytes = make_bundle(
            &["datamaps/Some Show S01E01 (2023) [1080p].mkv.datamap"],
            true,
            Some("ListName=\"TV Series\"\nSome Show S01E01 (2023) [1080p].mkv.datamap\n"),
        );
        let meta = classify(&bytes, None).unwrap();
        assert_eq!(meta.items, vec!["Some Show S01E01 (2023) [1080p].mkv"]);
    }

    #[test]
    fn classify_zip_no_datamap_or_list_fails() {
        // Zip with only unrelated files.
        let bytes = make_bundle(&["readme.txt"], false, None);
        assert!(classify(&bytes, None).is_none());
        // But if filename hint ends .watch‑list, detection still triggers.
        assert!(classify(&bytes, Some("My Library.watch-list")).is_some());
        // When detected via hint, items will be empty (no datamap members).
        let meta = classify(&bytes, Some("My Library.watch-list")).unwrap();
        assert!(meta.items.is_empty());
    }

    #[test]
    fn items_from_meta_roundtrip() {
        let mut meta = ExtractedMeta::default();
        meta.extra = json!({
            "watch_list": {
                "base": "a demo watch list",
                "item_count": 2,
                "media_types": { "mkv": 2 },
                "items": ["Item1.mkv", "Item2.mkv"],
            }
        });
        let items = items_from_meta(&meta);
        assert_eq!(items, vec!["Item1.mkv", "Item2.mkv"]);
        let base = base_from_meta(&meta).unwrap();
        assert_eq!(base, "a demo watch list");
    }

    #[test]
    fn apply_tags_watchlist_recomposes() {
        let mut desc = ResourceDescriptor {
            id: "test".into(),
            kind: ResourceKind::WatchList,
            location: "file:///tmp/bundle.watch-list".into(),
            location_scheme: Some("file".into()),
            location_canonical: None,
            mime_type: WATCHLIST_MIME.into(),
            filename: Some("bundle.watch-list".into()),
            metadata_json: json!({
                "watch_list": {
                    "base": "a library watch list",
                    "item_count": 3,
                    "media_types": { "mkv": 3 },
                    "items": ["Movie1.mkv", "Movie2.mkv", "Movie3.mkv"],
                }
            }),
            description_text: String::new(), // will be overwritten
            created_at: 0,
        };
        let tags = vec!["faves".into(), "scifi".into()];
        apply_tags(&mut desc, &tags);
        // Check that tagged description includes “tagged as” before item list.
        assert!(desc.description_text.contains("tagged as: faves, scifi"));
        assert!(desc.description_text.contains("Movie1.mkv"));
    }

    #[test]
    fn apply_tags_non_watchlist_legacy() {
        let mut desc = ResourceDescriptor {
            id: "test".into(),
            kind: ResourceKind::Text,
            location: "https://example.com/readme.txt".into(),
            location_scheme: Some("https".into()),
            location_canonical: None,
            mime_type: "text/plain".into(),
            filename: Some("readme.txt".into()),
            metadata_json: json!({}),
            description_text: "a readme text document file in plain format".into(),
            created_at: 0,
        };
        let original = desc.description_text.clone();
        let tags = vec!["docs".into()];
        apply_tags(&mut desc, &tags);
        assert_eq!(desc.description_text, format!("{} tagged as: docs", original));
    }
}