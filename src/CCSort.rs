use anyhow::{Context, Result};
use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use walkdir::{DirEntry, WalkDir};

const DEFAULT_MAX_DEPTH: usize = 2;
const DEFAULT_PREVIEW_LIMIT: usize = 60;
const MAX_ENTRY_EXPORT: usize = 500;
const EXCLUDED_DIR_NAMES: &[&str] = &[
    ".git",
    ".venv",
    ".vscode",
    ".idea",
    ".cc_skill_logs",
    "node_modules",
    "target",
    "Projects",
    "Documents",
    "Images",
    "Videos",
    "Music",
    "Archives",
    "Code",
    "Others",
];

#[cfg(windows)]
mod win {
    use std::env;
    use std::fs;
    use std::io;
    use std::path::{Path, PathBuf};

    const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;

    fn home_dir() -> PathBuf {
        if let Some(home) = env::var_os("USERPROFILE") {
            return PathBuf::from(home);
        }
        env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
    }

    pub fn get_desktop() -> PathBuf {
        home_dir().join("Desktop")
    }

    pub fn get_documents() -> PathBuf {
        home_dir().join("Documents")
    }

    pub fn get_downloads() -> PathBuf {
        home_dir().join("Downloads")
    }

    pub fn is_hidden(path: &Path) -> bool {
        use std::os::windows::fs::MetadataExt;

        if path
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.starts_with('.'))
            .unwrap_or(false)
        {
            return true;
        }

        match fs::metadata(path) {
            Ok(meta) => (meta.file_attributes() & FILE_ATTRIBUTE_HIDDEN) != 0,
            Err(_) => false,
        }
    }

    pub fn move_file_native(from: &Path, to: &Path) -> io::Result<()> {
        if let Some(parent) = to.parent() {
            fs::create_dir_all(parent)?;
        }

        match fs::rename(from, to) {
            Ok(_) => Ok(()),
            Err(_) => {
                fs::copy(from, to)?;
                fs::remove_file(from)
            }
        }
    }
}

#[derive(Debug, Clone)]
enum FileCategory {
    Project(ProjectType),
    Document,
    Image,
    Video,
    Audio,
    Archive,
    Code,
    Other,
}

#[derive(Debug, Clone)]
enum ProjectType {
    NodeJS,
    Python,
    Rust,
    DotNet,
    Java,
    Git,
}

#[derive(Debug, Clone)]
struct FileEntry {
    path: PathBuf,
    category: FileCategory,
    size: u64,
    modified: DateTime<Local>,
}

#[derive(Debug, Clone)]
enum OperationKind {
    CreateDir,
    MoveFile,
    Skip(String),
}

#[derive(Debug, Clone)]
struct Operation {
    kind: OperationKind,
    from: PathBuf,
    to: Option<PathBuf>,
}

#[derive(Debug, Clone)]
struct Plan {
    operations: Vec<Operation>,
    summary: Summary,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub total_files: usize,
    pub total_size_bytes: u64,
    pub projects: usize,
    pub documents: usize,
    pub images: usize,
    pub videos: usize,
    pub audios: usize,
    pub archives: usize,
    pub code: usize,
    pub others: usize,
    pub planned_moves: usize,
    pub skipped: usize,
}

#[derive(Debug, Clone)]
enum Strategy {
    ByType,
    ByProject,
    ByDate,
    ByAi,
}

impl Strategy {
    fn from_input(value: Option<&str>) -> Self {
        let raw = value.unwrap_or("byType").trim().to_ascii_lowercase();
        match raw.as_str() {
            "project" | "byproject" => Self::ByProject,
            "date" | "bydate" => Self::ByDate,
            "ai" | "byai" | "llm" | "smart" | "智能" => Self::ByAi,
            _ => Self::ByType,
        }
    }

    fn as_str(&self) -> &'static str {
        match self {
            Self::ByType => "byType",
            Self::ByProject => "byProject",
            Self::ByDate => "byDate",
            Self::ByAi => "byAi",
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SortRequest {
    #[serde(default)]
    pub action: Option<String>,
    #[serde(default)]
    pub base_dir: Option<String>,
    #[serde(default)]
    pub strategy: Option<String>,
    #[serde(default)]
    pub skip_hidden: Option<bool>,
    #[serde(default)]
    pub include_shortcuts: Option<bool>,
    #[serde(default)]
    pub max_depth: Option<usize>,
    #[serde(default)]
    pub include_entries: Option<bool>,
    #[serde(default)]
    pub dry_run: Option<bool>,
    #[serde(default)]
    pub plan: Option<PlanPayload>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SortResponse {
    pub ok: bool,
    pub action: String,
    pub text: String,
    pub base_dir: String,
    pub strategy: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<Summary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operations_preview: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan: Option<PlanPayload>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub entries: Option<Vec<FileEntryPayload>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub execution: Option<ExecutionStats>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileEntryPayload {
    pub path: String,
    pub relative_path: String,
    pub name: String,
    pub extension: String,
    pub category: String,
    pub size_bytes: u64,
    pub modified: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanPayload {
    pub operations: Vec<OperationPayload>,
    pub summary: Summary,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationPayload {
    #[serde(rename = "type")]
    pub op_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionStats {
    pub moved: usize,
    pub failed: usize,
    pub created_dirs: usize,
    pub skipped: usize,
    pub dry_run: bool,
}

fn trim_non_empty(value: Option<&str>) -> Option<String> {
    let text = value.unwrap_or("").trim();
    if text.is_empty() {
        None
    } else {
        Some(text.to_string())
    }
}

fn resolve_base_dir(input: Option<&str>) -> Result<PathBuf> {
    let token = trim_non_empty(input).unwrap_or_else(|| "desktop".to_string());
    let lowered = token.to_ascii_lowercase();

    let resolved = match lowered.as_str() {
        "desktop" | "桌面" => {
            #[cfg(windows)]
            {
                win::get_desktop()
            }
            #[cfg(not(windows))]
            {
                std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
            }
        }
        "documents" | "document" | "docs" | "文档" => {
            #[cfg(windows)]
            {
                win::get_documents()
            }
            #[cfg(not(windows))]
            {
                std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
            }
        }
        "downloads" | "download" | "下载" => {
            #[cfg(windows)]
            {
                win::get_downloads()
            }
            #[cfg(not(windows))]
            {
                std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
            }
        }
        "current" | "workspace" | "当前目录" => std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        _ => {
            let candidate = PathBuf::from(&token);
            if candidate.is_absolute() {
                candidate
            } else {
                std::env::current_dir()
                    .unwrap_or_else(|_| PathBuf::from("."))
                    .join(candidate)
            }
        }
    };

    if resolved.exists() && resolved.is_dir() {
        Ok(resolved)
    } else {
        Err(anyhow::anyhow!("目标目录不存在: {}", resolved.display()))
    }
}

fn is_hidden_file(path: &Path) -> bool {
    #[cfg(windows)]
    {
        return win::is_hidden(path);
    }

    #[cfg(not(windows))]
    {
        path.file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.starts_with('.'))
            .unwrap_or(false)
    }
}

fn is_excluded_dir_name(name: &str) -> bool {
    EXCLUDED_DIR_NAMES
        .iter()
        .any(|v| v.eq_ignore_ascii_case(name.trim()))
}

fn has_hidden_component(path: &Path, base: &Path) -> bool {
    let relative = path.strip_prefix(base).unwrap_or(path);
    relative
        .components()
        .filter_map(|comp| comp.as_os_str().to_str())
        .any(|part| part.starts_with('.'))
}

fn should_visit_entry(entry: &DirEntry, base: &Path, skip_hidden: bool) -> bool {
    let path = entry.path();
    if path == base {
        return true;
    }

    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();

    if entry.file_type().is_dir() && is_excluded_dir_name(name) {
        return false;
    }

    if skip_hidden && (name.starts_with('.') || has_hidden_component(path, base)) {
        return false;
    }

    true
}

fn is_project_member_file(path: &Path) -> bool {
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    if matches!(
        file_name.as_str(),
        "package.json"
            | "package-lock.json"
            | "yarn.lock"
            | "pnpm-lock.yaml"
            | "cargo.toml"
            | "cargo.lock"
            | "pyproject.toml"
            | "requirements.txt"
            | "pom.xml"
            | "build.gradle"
            | "settings.gradle"
            | ".gitignore"
    ) {
        return true;
    }

    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    matches!(
        ext.as_str(),
        "rs"
            | "py"
            | "js"
            | "ts"
            | "tsx"
            | "java"
            | "cpp"
            | "c"
            | "h"
            | "cs"
            | "go"
            | "php"
            | "html"
            | "css"
            | "json"
            | "toml"
            | "yaml"
            | "yml"
            | "sln"
            | "csproj"
    )
}

fn detect_project(path: &Path) -> Option<ProjectType> {
    if !is_project_member_file(path) {
        return None;
    }

    let parent = path.parent()?;

    if parent.join("package.json").exists() {
        Some(ProjectType::NodeJS)
    } else if parent.join("Cargo.toml").exists() {
        Some(ProjectType::Rust)
    } else if parent.join("requirements.txt").exists() || parent.join("pyproject.toml").exists() {
        Some(ProjectType::Python)
    } else if parent.join(".git").exists() {
        Some(ProjectType::Git)
    } else if parent.join("pom.xml").exists() {
        Some(ProjectType::Java)
    } else if parent.read_dir().ok()?.any(|e| {
        e.ok()
            .and_then(|entry| entry.path().extension().map(|ext| ext.to_string_lossy().to_string()))
            .map(|ext| {
                let lowered = ext.to_ascii_lowercase();
                lowered == "csproj" || lowered == "sln"
            })
            .unwrap_or(false)
    }) {
        Some(ProjectType::DotNet)
    } else {
        None
    }
}

fn categorize_file(path: &Path) -> FileCategory {
    if let Some(project_type) = detect_project(path) {
        return FileCategory::Project(project_type);
    }

    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    match ext.as_str() {
        "pdf" | "doc" | "docx" | "txt" | "md" | "rtf" | "xls" | "xlsx" | "ppt" | "pptx" => {
            FileCategory::Document
        }
        "jpg" | "jpeg" | "png" | "gif" | "svg" | "webp" | "bmp" | "ico" => FileCategory::Image,
        "mp4" | "avi" | "mkv" | "mov" | "wmv" => FileCategory::Video,
        "mp3" | "wav" | "flac" | "m4a" | "aac" => FileCategory::Audio,
        "zip" | "rar" | "7z" | "tar" | "gz" => FileCategory::Archive,
        "rs" | "py" | "js" | "ts" | "tsx" | "java" | "cpp" | "c" | "h" | "cs" | "go" | "php"
        | "html" | "css" | "json" | "toml" | "yaml" | "yml" => FileCategory::Code,
        _ => FileCategory::Other,
    }
}

fn is_shortcut_file(path: &Path) -> bool {
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    matches!(
        ext.as_str(),
        "lnk" | "url" | "pif" | "website" | "appref-ms"
    )
}

fn is_system_noise_file(path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    matches!(name.as_str(), "desktop.ini" | "thumbs.db")
}

fn analyze_directory(
    path: &Path,
    skip_hidden: bool,
    include_shortcuts: bool,
    max_depth: usize,
) -> Result<Vec<FileEntry>> {
    let mut entries = Vec::new();

    for entry in WalkDir::new(path)
        .max_depth(max_depth)
        .into_iter()
        .filter_entry(|e| should_visit_entry(e, path, skip_hidden))
        .filter_map(|e| e.ok())
    {
        if !entry.file_type().is_file() {
            continue;
        }

        let file_path = entry.path();
        if skip_hidden && is_hidden_file(file_path) {
            continue;
        }
        if is_system_noise_file(file_path) {
            continue;
        }
        if !include_shortcuts && is_shortcut_file(file_path) {
            continue;
        }

        let metadata = fs::metadata(file_path)?;
        let modified: DateTime<Local> = DateTime::<Local>::from(metadata.modified()?);

        entries.push(FileEntry {
            path: file_path.to_path_buf(),
            category: categorize_file(file_path),
            size: metadata.len(),
            modified,
        });
    }

    Ok(entries)
}

fn calculate_summary(entries: &[FileEntry], operations: &[Operation]) -> Summary {
    let mut summary = Summary {
        total_files: entries.len(),
        total_size_bytes: entries.iter().map(|e| e.size).sum(),
        projects: 0,
        documents: 0,
        images: 0,
        videos: 0,
        audios: 0,
        archives: 0,
        code: 0,
        others: 0,
        planned_moves: 0,
        skipped: 0,
    };

    for entry in entries {
        match entry.category {
            FileCategory::Project(_) => summary.projects += 1,
            FileCategory::Document => summary.documents += 1,
            FileCategory::Image => summary.images += 1,
            FileCategory::Video => summary.videos += 1,
            FileCategory::Audio => summary.audios += 1,
            FileCategory::Archive => summary.archives += 1,
            FileCategory::Code => summary.code += 1,
            FileCategory::Other => summary.others += 1,
        }
    }

    for op in operations {
        match op.kind {
            OperationKind::MoveFile => summary.planned_moves += 1,
            OperationKind::Skip(_) => summary.skipped += 1,
            OperationKind::CreateDir => {}
        }
    }

    summary
}

fn get_target_by_type(category: &FileCategory, base: &Path) -> PathBuf {
    match category {
        FileCategory::Project(_) => base.join("Projects"),
        FileCategory::Document => base.join("Documents"),
        FileCategory::Image => base.join("Images"),
        FileCategory::Video => base.join("Videos"),
        FileCategory::Audio => base.join("Music"),
        FileCategory::Archive => base.join("Archives"),
        FileCategory::Code => base.join("Code"),
        FileCategory::Other => base.join("Others"),
    }
}

fn get_target_by_project(category: &FileCategory, path: &Path, base: &Path) -> PathBuf {
    if let FileCategory::Project(project_type) = category {
        let project_root = path.parent().unwrap_or(path);
        let project_name = project_root
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown");

        let kind = match project_type {
            ProjectType::NodeJS => "NodeJS",
            ProjectType::Python => "Python",
            ProjectType::Rust => "Rust",
            ProjectType::DotNet => "DotNet",
            ProjectType::Java => "Java",
            ProjectType::Git => "Git",
        };

        base.join("Projects").join(kind).join(project_name)
    } else {
        get_target_by_type(category, base)
    }
}

fn get_target_by_date(date: &DateTime<Local>, base: &Path) -> PathBuf {
    base.join(date.format("%Y-%m").to_string())
}

fn category_label(category: &FileCategory) -> &'static str {
    match category {
        FileCategory::Project(_) => "Project",
        FileCategory::Document => "Document",
        FileCategory::Image => "Image",
        FileCategory::Video => "Video",
        FileCategory::Audio => "Audio",
        FileCategory::Archive => "Archive",
        FileCategory::Code => "Code",
        FileCategory::Other => "Other",
    }
}

fn build_entry_payloads(entries: &[FileEntry], base_dir: &Path, limit: usize) -> Vec<FileEntryPayload> {
    entries
        .iter()
        .take(limit)
        .map(|entry| {
            let path = entry.path.to_string_lossy().to_string();
            let relative_path = entry
                .path
                .strip_prefix(base_dir)
                .ok()
                .map(|v| v.to_string_lossy().to_string())
                .unwrap_or_else(|| path.clone());
            let name = entry
                .path
                .file_name()
                .and_then(|v| v.to_str())
                .unwrap_or("")
                .to_string();
            let extension = entry
                .path
                .extension()
                .and_then(|v| v.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();

            FileEntryPayload {
                path,
                relative_path,
                name,
                extension,
                category: category_label(&entry.category).to_string(),
                size_bytes: entry.size,
                modified: entry.modified.to_rfc3339(),
            }
        })
        .collect()
}

fn next_available_path(desired: &Path) -> PathBuf {
    if !desired.exists() {
        return desired.to_path_buf();
    }

    let parent = desired.parent().unwrap_or_else(|| Path::new("."));
    let stem = desired
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("file")
        .to_string();
    let ext = desired
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| format!(".{}", e))
        .unwrap_or_default();

    for idx in 1..=9999 {
        let candidate = parent.join(format!("{} ({}){}", stem, idx, ext));
        if !candidate.exists() {
            return candidate;
        }
    }

    desired.to_path_buf()
}

fn generate_plan(entries: &[FileEntry], strategy: Strategy, base_path: &Path) -> Plan {
    let mut operations = Vec::new();
    let mut created_dirs: HashSet<PathBuf> = HashSet::new();

    for entry in entries {
        let target_dir = match strategy {
            Strategy::ByType => get_target_by_type(&entry.category, base_path),
            Strategy::ByProject => get_target_by_project(&entry.category, &entry.path, base_path),
            Strategy::ByDate => get_target_by_date(&entry.modified, base_path),
            Strategy::ByAi => get_target_by_type(&entry.category, base_path),
        };

        if !created_dirs.contains(&target_dir) {
            operations.push(Operation {
                kind: OperationKind::CreateDir,
                from: target_dir.clone(),
                to: None,
            });
            created_dirs.insert(target_dir.clone());
        }

        let file_name = match entry.path.file_name() {
            Some(name) => name,
            None => {
                operations.push(Operation {
                    kind: OperationKind::Skip("missing file name".to_string()),
                    from: entry.path.clone(),
                    to: None,
                });
                continue;
            }
        };

        let mut target_file = target_dir.join(file_name);
        if target_file == entry.path {
            operations.push(Operation {
                kind: OperationKind::Skip("already in target".to_string()),
                from: entry.path.clone(),
                to: None,
            });
            continue;
        }

        target_file = next_available_path(&target_file);

        operations.push(Operation {
            kind: OperationKind::MoveFile,
            from: entry.path.clone(),
            to: Some(target_file),
        });
    }

    let summary = calculate_summary(entries, &operations);

    Plan { operations, summary }
}

fn normalize_path_for_cmp(path: &Path) -> String {
    path.to_string_lossy()
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_ascii_lowercase()
}

fn is_path_within(target: &Path, base: &Path) -> bool {
    let target_norm = normalize_path_for_cmp(target);
    let base_norm = normalize_path_for_cmp(base);
    if target_norm == base_norm {
        return true;
    }
    target_norm.starts_with(&(base_norm + "/"))
}

fn validate_plan_payload(plan: &PlanPayload, base_dir: &Path) -> Result<()> {
    for op in &plan.operations {
        match op.op_type.as_str() {
            "createDir" => {
                let dir = op
                    .path
                    .as_ref()
                    .map(PathBuf::from)
                    .context("createDir 缺少 path")?;
                if !is_path_within(&dir, base_dir) {
                    anyhow::bail!("createDir 超出目标目录范围: {}", dir.display());
                }
            }
            "moveFile" => {
                let from = op
                    .from
                    .as_ref()
                    .map(PathBuf::from)
                    .context("moveFile 缺少 from")?;
                let to = op
                    .to
                    .as_ref()
                    .map(PathBuf::from)
                    .context("moveFile 缺少 to")?;
                if !is_path_within(&from, base_dir) || !is_path_within(&to, base_dir) {
                    anyhow::bail!(
                        "moveFile 超出目标目录范围: {} -> {}",
                        from.display(),
                        to.display()
                    );
                }
            }
            "skip" => {}
            _ => anyhow::bail!("不支持的操作类型: {}", op.op_type),
        }
    }

    Ok(())
}

fn move_file(from: &Path, to: &Path) -> Result<()> {
    #[cfg(windows)]
    {
        return win::move_file_native(from, to).context("移动文件失败");
    }

    #[cfg(not(windows))]
    {
        if let Some(parent) = to.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::rename(from, to).context("移动文件失败")
    }
}

fn execute_plan_payload(plan: &PlanPayload, dry_run: bool) -> ExecutionStats {
    let mut moved = 0usize;
    let mut failed = 0usize;
    let mut created_dirs = 0usize;
    let mut skipped = 0usize;

    for op in &plan.operations {
        match op.op_type.as_str() {
            "createDir" => {
                if let Some(path_text) = &op.path {
                    if !dry_run {
                        if fs::create_dir_all(path_text).is_ok() {
                            created_dirs += 1;
                        } else {
                            failed += 1;
                        }
                    } else {
                        created_dirs += 1;
                    }
                } else {
                    failed += 1;
                }
            }
            "moveFile" => {
                let from = op.from.as_deref().unwrap_or("");
                let to = op.to.as_deref().unwrap_or("");
                if from.is_empty() || to.is_empty() {
                    failed += 1;
                    continue;
                }

                if dry_run {
                    moved += 1;
                    continue;
                }

                match move_file(Path::new(from), Path::new(to)) {
                    Ok(_) => moved += 1,
                    Err(_) => failed += 1,
                }
            }
            _ => skipped += 1,
        }
    }

    ExecutionStats {
        moved,
        failed,
        created_dirs,
        skipped,
        dry_run,
    }
}

fn plan_to_payload(plan: &Plan) -> PlanPayload {
    let operations = plan
        .operations
        .iter()
        .map(|op| match &op.kind {
            OperationKind::CreateDir => OperationPayload {
                op_type: "createDir".to_string(),
                path: Some(op.from.to_string_lossy().to_string()),
                from: None,
                to: None,
                reason: None,
            },
            OperationKind::MoveFile => OperationPayload {
                op_type: "moveFile".to_string(),
                path: None,
                from: Some(op.from.to_string_lossy().to_string()),
                to: op.to.as_ref().map(|v| v.to_string_lossy().to_string()),
                reason: None,
            },
            OperationKind::Skip(reason) => OperationPayload {
                op_type: "skip".to_string(),
                path: Some(op.from.to_string_lossy().to_string()),
                from: None,
                to: None,
                reason: Some(reason.clone()),
            },
        })
        .collect();

    PlanPayload {
        operations,
        summary: plan.summary.clone(),
    }
}

fn build_preview(plan: &PlanPayload, limit: usize) -> Vec<String> {
    let mut rows = Vec::new();
    let mut grouped: HashMap<String, usize> = HashMap::new();

    for op in &plan.operations {
        match op.op_type.as_str() {
            "createDir" => {
                let dir = op.path.clone().unwrap_or_default();
                rows.push(format!("mkdir {}", dir));
            }
            "moveFile" => {
                let from = op.from.clone().unwrap_or_default();
                let to = op.to.clone().unwrap_or_default();
                rows.push(format!("move {} -> {}", from, to));

                let key = Path::new(&to)
                    .parent()
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_else(|| "(unknown)".to_string());
                *grouped.entry(key).or_insert(0) += 1;
            }
            "skip" => {
                let reason = op.reason.clone().unwrap_or_else(|| "skip".to_string());
                let file = op.path.clone().unwrap_or_default();
                rows.push(format!("skip {} ({})", file, reason));
            }
            _ => {}
        }
    }

    let mut result = Vec::new();
    result.push(format!(
        "summary: files={} moves={} skipped={} size={} bytes",
        plan.summary.total_files, plan.summary.planned_moves, plan.summary.skipped, plan.summary.total_size_bytes
    ));

    let mut groups: Vec<(String, usize)> = grouped.into_iter().collect();
    groups.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    for (dir, count) in groups.into_iter().take(8) {
        result.push(format!("target {} files -> {}", count, dir));
    }

    for line in rows.into_iter().take(limit) {
        result.push(line);
    }

    result
}

fn error_response(action: &str, base_dir: &Path, strategy: &Strategy, text: String) -> SortResponse {
    SortResponse {
        ok: false,
        action: action.to_string(),
        text,
        base_dir: base_dir.to_string_lossy().to_string(),
        strategy: strategy.as_str().to_string(),
        summary: None,
        operations_preview: None,
        plan: None,
        entries: None,
        execution: None,
    }
}

pub fn run_from_stdin() -> SortResponse {
    let mut buf = String::new();
    if let Err(err) = std::io::stdin().read_to_string(&mut buf) {
        let fallback = PathBuf::from(".");
        let strategy = Strategy::ByType;
        return error_response("plan", &fallback, &strategy, format!("读取输入失败: {}", err));
    }

    let req = if buf.trim().is_empty() {
        SortRequest {
            action: Some("plan".to_string()),
            base_dir: Some("desktop".to_string()),
            strategy: Some("byType".to_string()),
            skip_hidden: Some(true),
            include_shortcuts: Some(false),
            max_depth: Some(DEFAULT_MAX_DEPTH),
            include_entries: Some(false),
            dry_run: Some(false),
            plan: None,
        }
    } else {
        match serde_json::from_str::<SortRequest>(&buf) {
            Ok(v) => v,
            Err(err) => {
                let fallback = PathBuf::from(".");
                let strategy = Strategy::ByType;
                return error_response("plan", &fallback, &strategy, format!("请求 JSON 解析失败: {}", err));
            }
        }
    };

    let action = req
        .action
        .as_deref()
        .map(|v| v.trim().to_ascii_lowercase())
        .unwrap_or_else(|| "plan".to_string());
    let strategy = Strategy::from_input(req.strategy.as_deref());
    let base_dir = match resolve_base_dir(req.base_dir.as_deref()) {
        Ok(v) => v,
        Err(err) => {
            let fallback = PathBuf::from(".");
            return error_response("plan", &fallback, &strategy, err.to_string());
        }
    };

    if action == "apply" {
        let dry_run = req.dry_run.unwrap_or(false);
        let plan = match req.plan {
            Some(v) => v,
            None => {
                return error_response(
                    "apply",
                    &base_dir,
                    &strategy,
                    "apply 缺少 plan 数据。请先执行 /sort 生成计划。".to_string(),
                )
            }
        };

        if let Err(err) = validate_plan_payload(&plan, &base_dir) {
            return error_response("apply", &base_dir, &strategy, format!("计划校验失败: {}", err));
        }

        let execution = execute_plan_payload(&plan, dry_run);
        let ok = execution.failed == 0;
        let text = if dry_run {
            format!(
                "dry-run 完成: 预计移动 {}，创建目录 {}，跳过 {}，失败 {}。",
                execution.moved, execution.created_dirs, execution.skipped, execution.failed
            )
        } else {
            format!(
                "执行完成: 已移动 {}，创建目录 {}，跳过 {}，失败 {}。",
                execution.moved, execution.created_dirs, execution.skipped, execution.failed
            )
        };

        return SortResponse {
            ok,
            action: "apply".to_string(),
            text,
            base_dir: base_dir.to_string_lossy().to_string(),
            strategy: strategy.as_str().to_string(),
            summary: Some(plan.summary.clone()),
            operations_preview: Some(build_preview(&plan, DEFAULT_PREVIEW_LIMIT)),
            plan: Some(plan),
            entries: None,
            execution: Some(execution),
        };
    }

    let skip_hidden = req.skip_hidden.unwrap_or(true);
    let include_shortcuts = req.include_shortcuts.unwrap_or(false);
    let max_depth = req.max_depth.unwrap_or(DEFAULT_MAX_DEPTH).clamp(1, 6);
    let include_entries = req.include_entries.unwrap_or(false);

    let entries = match analyze_directory(&base_dir, skip_hidden, include_shortcuts, max_depth) {
        Ok(v) => v,
        Err(err) => {
            return error_response("plan", &base_dir, &strategy, format!("扫描失败: {}", err));
        }
    };

    let plan = generate_plan(&entries, strategy.clone(), &base_dir);
    let payload = plan_to_payload(&plan);
    let preview = build_preview(&payload, DEFAULT_PREVIEW_LIMIT);
    let entry_payloads = if include_entries {
        Some(build_entry_payloads(&entries, &base_dir, MAX_ENTRY_EXPORT))
    } else {
        None
    };

    SortResponse {
        ok: true,
        action: "plan".to_string(),
        text: format!(
            "计划生成完成: 扫描 {} 个文件，可移动 {} 个。{}{}",
            payload.summary.total_files,
            payload.summary.planned_moves,
            if matches!(strategy, Strategy::ByAi) {
                "当前为 AI 规划模式。"
            } else {
                ""
            },
            if include_shortcuts {
                "已包含快捷方式。"
            } else {
                "默认已忽略快捷方式（.lnk/.url 等）。"
            }
        ),
        base_dir: base_dir.to_string_lossy().to_string(),
        strategy: strategy.as_str().to_string(),
        summary: Some(payload.summary.clone()),
        operations_preview: Some(preview),
        plan: Some(payload),
        entries: entry_payloads,
        execution: None,
    }
}

pub fn write_json_response(resp: &SortResponse) {
    match serde_json::to_string(resp) {
        Ok(text) => {
            println!("{}", text);
        }
        Err(err) => {
            println!(
                "{{\"ok\":false,\"action\":\"plan\",\"text\":\"响应序列化失败: {}\",\"baseDir\":\".\",\"strategy\":\"byType\"}}",
                err.to_string().replace('"', "'")
            );
        }
    }
}
