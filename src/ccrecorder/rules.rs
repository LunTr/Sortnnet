use anyhow::{anyhow, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use walkdir::DirEntry;

pub const LOG_DIR_NAME: &str = ".cc_skill_logs";
pub const DEFAULT_SCAN_MAX_ENTRIES: usize = 600;
pub const HARD_SCAN_MAX_ENTRIES: usize = 5000;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReorderRequest {
    #[serde(default)]
    pub action: String,
    #[serde(default, alias = "base_dir")]
    pub base_dir: Option<String>,
    #[serde(default, alias = "max_entries")]
    pub max_entries: Option<usize>,
    #[serde(default)]
    pub whitelist: Vec<String>,
    #[serde(default)]
    pub plan: Option<ReorderPlan>,
    #[serde(default, alias = "plan_id")]
    pub plan_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReorderPlan {
    #[serde(default)]
    pub operations: Vec<ReorderOperation>,
    #[serde(default)]
    pub notes: Vec<String>,
    #[serde(default)]
    pub duplicates: Vec<String>,
    #[serde(default)]
    pub garbage_candidates: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReorderOperation {
    #[serde(rename = "type")]
    pub op_type: String,
    #[serde(default)]
    pub from: Option<String>,
    #[serde(default)]
    pub to: Option<String>,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Debug, Clone)]
pub enum PlannedStep {
    Mkdir { to: PathBuf, reason: String },
    Move {
        from: PathBuf,
        to: PathBuf,
        reason: String,
    },
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReorderResponse {
    pub ok: bool,
    pub action: String,
    pub base_dir: String,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tree_lines: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duplicate_groups: Option<Vec<Vec<String>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub garbage_candidates: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dry_run: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operations_preview: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conflicts: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warnings: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log_path: Option<String>,
}

pub fn now_epoch_ms() -> u128 {
    Utc::now().timestamp_millis() as u128
}

pub fn now_compact_label() -> String {
    Utc::now().format("%Y%m%d-%H%M%S").to_string()
}

pub fn trim_non_empty(value: Option<&str>) -> Option<String> {
    let text = value.unwrap_or("").trim();
    if text.is_empty() {
        None
    } else {
        Some(text.to_string())
    }
}

pub fn skip_dir(entry: &DirEntry) -> bool {
    if !entry.file_type().is_dir() {
        return false;
    }

    let name = entry.file_name().to_string_lossy().to_lowercase();
    matches!(
        name.as_str(),
        ".git" | ".vscode" | ".idea" | "node_modules" | "target" | "__pycache__" | LOG_DIR_NAME
    )
}

pub fn normalize_key(path: &Path) -> String {
    let raw = path.to_string_lossy().to_string();
    let normalized = if let Some(rest) = raw.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{}", rest)
    } else if let Some(rest) = raw.strip_prefix(r"\\?\") {
        rest.to_string()
    } else {
        raw
    };

    normalized
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_lowercase()
}

pub fn is_within_root(target: &Path, root: &Path) -> bool {
    let target_key = normalize_key(target);
    let root_key = normalize_key(root);
    target_key == root_key || target_key.starts_with(&(root_key + "/"))
}

pub fn resolve_base_dir(base_dir: Option<&str>) -> Result<PathBuf> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let requested = trim_non_empty(base_dir).unwrap_or_else(|| cwd.to_string_lossy().to_string());

    let candidate = PathBuf::from(&requested);
    let absolute = if candidate.is_absolute() {
        candidate
    } else {
        cwd.join(candidate)
    };

    if !absolute.exists() || !absolute.is_dir() {
        return Err(anyhow!(
            "baseDir 不存在或不是目录: {}",
            absolute.to_string_lossy()
        ));
    }

    Ok(fs::canonicalize(&absolute).unwrap_or(absolute))
}

pub fn resolve_whitelist(base_dir: &Path, whitelist: &[String]) -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

    let mut push_root = |candidate: PathBuf| {
        let resolved = if candidate.is_absolute() {
            candidate
        } else {
            cwd.join(candidate)
        };

        if !resolved.exists() || !resolved.is_dir() {
            return;
        }

        let canonical = fs::canonicalize(&resolved).unwrap_or(resolved);
        let key = normalize_key(&canonical);
        if seen.insert(key) {
            roots.push(canonical);
        }
    };

    push_root(base_dir.to_path_buf());
    for item in whitelist {
        if let Some(text) = trim_non_empty(Some(item.as_str())) {
            push_root(PathBuf::from(text));
        }
    }

    roots
}

pub fn resolve_candidate_path(base_dir: &Path, raw: &str) -> PathBuf {
    let candidate = PathBuf::from(raw.trim());
    if candidate.is_absolute() {
        candidate
    } else {
        base_dir.join(candidate)
    }
}

pub fn is_path_allowed(target: &Path, roots: &[PathBuf]) -> bool {
    roots.iter().any(|root| is_within_root(target, root))
}

pub fn sanitize_plan_id(raw: Option<&str>) -> String {
    let text = raw.unwrap_or("plan").trim();
    let mut out = String::new();

    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
            out.push(ch);
        }
    }

    if out.is_empty() {
        format!("plan-{}", now_epoch_ms())
    } else {
        out
    }
}

pub fn preview_step(step: &PlannedStep) -> String {
    match step {
        PlannedStep::Mkdir { to, reason } => {
            if reason.trim().is_empty() {
                format!("mkdir {}", to.to_string_lossy())
            } else {
                format!("mkdir {}  # {}", to.to_string_lossy(), reason)
            }
        }
        PlannedStep::Move { from, to, reason } => {
            if reason.trim().is_empty() {
                format!("move {} -> {}", from.to_string_lossy(), to.to_string_lossy())
            } else {
                format!(
                    "move {} -> {}  # {}",
                    from.to_string_lossy(),
                    to.to_string_lossy(),
                    reason
                )
            }
        }
    }
}

pub fn error_response(action: &str, base_dir: &Path, message: String) -> ReorderResponse {
    ReorderResponse {
        ok: false,
        action: action.to_string(),
        base_dir: base_dir.to_string_lossy().to_string(),
        text: message,
        plan_id: None,
        tree_lines: None,
        duplicate_groups: None,
        garbage_candidates: None,
        dry_run: None,
        operations_preview: None,
        conflicts: None,
        warnings: None,
        log_path: None,
    }
}

pub fn escape_json_string(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}
