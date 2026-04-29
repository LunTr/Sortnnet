use super::rules::{skip_dir, HARD_SCAN_MAX_ENTRIES};
use std::collections::BTreeMap;
use std::path::Path;
use walkdir::WalkDir;

#[derive(Debug, Clone)]
pub struct ScanReport {
    pub tree_lines: Vec<String>,
    pub duplicate_groups: Vec<Vec<String>>,
    pub garbage_candidates: Vec<String>,
}

pub fn scan_directory(base_dir: &Path, max_entries: usize) -> ScanReport {
    let cap = max_entries.clamp(1, HARD_SCAN_MAX_ENTRIES);
    let mut lines: Vec<String> = Vec::new();
    let mut duplicate_map: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut garbage: Vec<String> = Vec::new();

    for entry in WalkDir::new(base_dir)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| !skip_dir(e))
        .filter_map(Result::ok)
    {
        if lines.len() >= cap {
            break;
        }

        let rel = entry
            .path()
            .strip_prefix(base_dir)
            .unwrap_or(entry.path())
            .to_string_lossy()
            .to_string();

        let rel_display = if rel.is_empty() {
            ".".to_string()
        } else {
            rel.replace('\\', "/")
        };

        let tag = if entry.file_type().is_dir() { "[D]" } else { "[F]" };
        lines.push(format!("{} {}", tag, rel_display));

        if !entry.file_type().is_file() {
            continue;
        }

        let name = entry.file_name().to_string_lossy().to_string();
        duplicate_map
            .entry(name.to_lowercase())
            .or_default()
            .push(entry.path().to_string_lossy().to_string());

        let lowered = name.to_lowercase();
        let is_garbage = lowered == "thumbs.db"
            || lowered == "desktop.ini"
            || lowered == ".ds_store"
            || lowered.ends_with(".tmp")
            || lowered.ends_with(".bak")
            || lowered.ends_with(".old")
            || lowered.ends_with('~');

        if is_garbage {
            garbage.push(entry.path().to_string_lossy().to_string());
        }
    }

    let mut duplicate_groups: Vec<Vec<String>> = duplicate_map
        .into_values()
        .filter(|group| group.len() > 1)
        .collect();
    duplicate_groups.sort_by_key(|group| std::cmp::Reverse(group.len()));
    duplicate_groups.truncate(50);

    garbage.sort();
    garbage.dedup();
    garbage.truncate(100);

    ScanReport {
        tree_lines: lines,
        duplicate_groups,
        garbage_candidates: garbage,
    }
}
