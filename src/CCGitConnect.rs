use anyhow::{Context, Result};
use chrono::{Local, TimeZone};
use git2::build::CheckoutBuilder;
use git2::{ErrorCode, IndexAddOption, ObjectType, Oid, Repository, ResetType, Signature};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::env;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

const SNAPSHOT_REF_PREFIX: &str = "refs/snapshots/";
const SNAPSHOT_NOTES_REF: &str = "refs/notes/gitconnect";
const DEFAULT_KEEP_LATEST: usize = 50;
const DEFAULT_LIST_LIMIT: usize = 20;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitRequest {
    #[serde(default)]
    pub action: Option<String>,
    #[serde(default)]
    pub workspace_dir: Option<String>,
    #[serde(default)]
    pub snapshot_id: Option<String>,
    #[serde(default)]
    pub operation_name: Option<String>,
    #[serde(default)]
    pub keep_latest: Option<usize>,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub metadata: Option<Value>,
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitResponse {
    pub ok: bool,
    pub action: String,
    pub text: String,
    pub workspace_dir: String,
    pub repo_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub initialized: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshot_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshots: Option<Vec<SnapshotItem>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub storage: Option<StorageInfo>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotItem {
    pub snapshot_id: String,
    pub commit: String,
    pub message: String,
    pub created_at: String,
    pub status: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageInfo {
    pub git_dir: String,
    pub bytes: u64,
    pub human: String,
    pub snapshot_refs: usize,
    pub keep_latest: usize,
}

#[derive(Debug)]
struct SnapshotRow {
    snapshot_id: String,
    ref_name: String,
    commit_oid: Oid,
    commit_time: i64,
    message: String,
    created_at: String,
    status: String,
}

fn trim_non_empty(input: Option<&str>) -> Option<String> {
    let text = input.unwrap_or("").trim();
    if text.is_empty() {
        None
    } else {
        Some(text.to_string())
    }
}

fn clamp_keep_latest(value: usize) -> usize {
    value.clamp(10, 500)
}

fn clamp_list_limit(value: usize) -> usize {
    value.clamp(1, 200)
}

fn resolve_workspace_dir(input: Option<&str>) -> PathBuf {
    let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let raw = trim_non_empty(input).unwrap_or_else(|| ".".to_string());
    let candidate = PathBuf::from(raw);
    if candidate.is_absolute() {
        candidate
    } else {
        cwd.join(candidate)
    }
}

fn ensure_workspace_exists(path: &Path) -> Result<()> {
    fs::create_dir_all(path).context("创建 workspace 目录失败")
}

fn open_or_init_repo(path: &Path) -> Result<(Repository, bool)> {
    match Repository::open(path) {
        Ok(repo) => Ok((repo, false)),
        Err(err) if err.code() == ErrorCode::NotFound => {
            let repo = Repository::init(path).context("初始化 Git 仓库失败")?;
            Ok((repo, true))
        }
        Err(err) => Err(anyhow::anyhow!("打开 Git 仓库失败: {}", err)),
    }
}

fn resolve_signature(repo: &Repository) -> Result<Signature<'static>> {
    match repo.signature() {
        Ok(sig) => Ok(Signature::now(sig.name().unwrap_or("GitConnect"), sig.email().unwrap_or("gitconnect@local"))?),
        Err(_) => Signature::now("GitConnect", "gitconnect@local").context("创建提交签名失败"),
    }
}

fn format_commit_time(seconds: i64) -> String {
    Local
        .timestamp_opt(seconds, 0)
        .single()
        .map(|dt| dt.to_rfc3339())
        .unwrap_or_else(|| seconds.to_string())
}

fn snapshot_note_to_status(note: Option<Value>) -> String {
    note.and_then(|v| v.get("status").and_then(|s| s.as_str()).map(|s| s.to_string()))
        .unwrap_or_else(|| "ok".to_string())
}

fn read_snapshot_note(repo: &Repository, oid: Oid) -> Option<Value> {
    match repo.find_note(Some(SNAPSHOT_NOTES_REF), oid) {
        Ok(note) => {
            let text = note.message().unwrap_or("").trim();
            if text.is_empty() {
                None
            } else {
                serde_json::from_str::<Value>(text).ok()
            }
        }
        Err(_) => None,
    }
}

fn write_snapshot_note(repo: &Repository, oid: Oid, payload: &Value) -> Result<()> {
    let sig = resolve_signature(repo)?;
    let note = serde_json::to_string(payload).context("序列化快照注记失败")?;
    repo.note(&sig, &sig, Some(SNAPSHOT_NOTES_REF), oid, &note, true)
        .context("写入 Git notes 失败")?;
    Ok(())
}

fn build_snapshot_message(operation_name: &str, metadata: &Option<Value>) -> String {
    let mut lines = vec![
        format!("snapshot: {}", operation_name),
        format!("time: {}", Local::now().to_rfc3339()),
    ];

    if let Some(meta) = metadata {
        if let Some(plan_id) = meta.get("planId").and_then(|v| v.as_str()) {
            lines.push(format!("planId: {}", plan_id));
        }
        if let Some(strategy) = meta.get("strategy").and_then(|v| v.as_str()) {
            lines.push(format!("strategy: {}", strategy));
        }

        if let Some(changed_files) = meta.get("changedFiles").and_then(|v| v.as_array()) {
            let mut count = 0usize;
            lines.push("changedFiles:".to_string());
            for item in changed_files {
                if count >= 12 {
                    lines.push("- ...".to_string());
                    break;
                }
                if let Some(text) = item.as_str() {
                    lines.push(format!("- {}", text));
                    count += 1;
                }
            }
        }
    }

    lines.join("\n")
}

fn create_snapshot(
    workspace_dir: &Path,
    operation_name: &str,
    metadata: &Option<Value>,
) -> Result<(String, String, bool)> {
    ensure_workspace_exists(workspace_dir)?;
    let (repo, initialized) = open_or_init_repo(workspace_dir)?;

    let mut index = repo.index().context("读取 Git index 失败")?;
    index
        .add_all(["*"], IndexAddOption::DEFAULT, None)
        .context("暂存工作区文件失败")?;
    index.write().context("写入 Git index 失败")?;

    let tree_oid = index.write_tree().context("写入 tree 失败")?;
    let tree = repo.find_tree(tree_oid).context("查找 tree 失败")?;
    let sig = resolve_signature(&repo)?;
    let message = build_snapshot_message(operation_name, metadata);

    let commit_oid = match repo.head() {
        Ok(head) => match head.target() {
            Some(parent_oid) => {
                let parent = repo.find_commit(parent_oid).context("读取父提交失败")?;
                repo.commit(Some("HEAD"), &sig, &sig, &message, &tree, &[&parent])
                    .context("创建快照提交失败")?
            }
            None => repo
                .commit(Some("HEAD"), &sig, &sig, &message, &tree, &[])
                .context("创建初始快照提交失败")?,
        },
        Err(_) => repo
            .commit(Some("HEAD"), &sig, &sig, &message, &tree, &[])
            .context("创建初始快照提交失败")?,
    };

    let commit_hex = commit_oid.to_string();
    let snapshot_id = commit_hex.chars().take(12).collect::<String>();
    let snapshot_ref = format!("{}{}", SNAPSHOT_REF_PREFIX, snapshot_id);
    repo.reference(&snapshot_ref, commit_oid, true, "create snapshot ref")
        .context("创建快照引用失败")?;

    let mut note_payload = metadata.clone().unwrap_or_else(|| json!({}));
    if !note_payload.is_object() {
        note_payload = json!({ "metadata": note_payload });
    }
    if let Some(obj) = note_payload.as_object_mut() {
        obj.insert("status".to_string(), json!("ok"));
        obj.insert("operation".to_string(), json!(operation_name));
        obj.insert("createdAt".to_string(), json!(Local::now().to_rfc3339()));
    }
    let _ = write_snapshot_note(&repo, commit_oid, &note_payload);

    Ok((snapshot_id, commit_hex, initialized))
}

fn list_snapshot_rows(repo: &Repository, limit: usize) -> Result<Vec<SnapshotRow>> {
    let mut rows = Vec::new();
    let glob = format!("{}*", SNAPSHOT_REF_PREFIX);
    let references = repo
        .references_glob(&glob)
        .context("读取快照引用失败")?;

    for item in references {
        let reference = match item {
            Ok(v) => v,
            Err(_) => continue,
        };

        let ref_name = match reference.name() {
            Some(v) => v.to_string(),
            None => continue,
        };

        let commit_oid = match reference.target() {
            Some(v) => v,
            None => continue,
        };

        let commit = match repo.find_commit(commit_oid) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let snapshot_id = ref_name
            .strip_prefix(SNAPSHOT_REF_PREFIX)
            .unwrap_or(&ref_name)
            .to_string();

        let commit_time = commit.time().seconds();
        let created_at = format_commit_time(commit_time);
        let message = commit.summary().unwrap_or("(no message)").to_string();
        let status = snapshot_note_to_status(read_snapshot_note(repo, commit_oid));

        rows.push(SnapshotRow {
            snapshot_id,
            ref_name,
            commit_oid,
            commit_time,
            message,
            created_at,
            status,
        });
    }

    rows.sort_by(|a, b| b.commit_time.cmp(&a.commit_time));
    rows.truncate(limit);
    Ok(rows)
}

fn resolve_snapshot_oid(repo: &Repository, input: &str) -> Result<(Oid, Option<String>)> {
    let raw = input.trim();
    if raw.is_empty() {
        anyhow::bail!("snapshotId 不能为空");
    }

    if raw.starts_with("refs/") {
        let rf = repo
            .find_reference(raw)
            .with_context(|| format!("未找到快照引用: {}", raw))?;
        let oid = rf
            .target()
            .with_context(|| format!("快照引用无效: {}", raw))?;
        return Ok((oid, Some(raw.to_string())));
    }

    let direct_ref = format!("{}{}", SNAPSHOT_REF_PREFIX, raw);
    if let Ok(rf) = repo.find_reference(&direct_ref) {
        if let Some(oid) = rf.target() {
            return Ok((oid, Some(direct_ref)));
        }
    }

    let mut matched: Option<(Oid, Option<String>)> = None;
    let rows = list_snapshot_rows(repo, 500)?;
    for row in rows {
        let commit_hex = row.commit_oid.to_string();
        if row.snapshot_id.starts_with(raw) || commit_hex.starts_with(raw) {
            if matched.is_some() {
                anyhow::bail!("snapshotId 命中多个快照，请提供更长前缀");
            }
            matched = Some((row.commit_oid, Some(row.ref_name)));
        }
    }

    if let Some(v) = matched {
        return Ok(v);
    }

    let obj = repo
        .revparse_single(raw)
        .with_context(|| format!("未找到快照或提交: {}", raw))?;
    Ok((obj.id(), None))
}

fn mark_snapshot_failed(workspace_dir: &Path, snapshot_id: &str, reason: &str) -> Result<String> {
    let repo = Repository::open(workspace_dir).context("打开 Git 仓库失败")?;
    let (oid, _) = resolve_snapshot_oid(&repo, snapshot_id)?;

    let mut payload = read_snapshot_note(&repo, oid).unwrap_or_else(|| json!({}));
    if !payload.is_object() {
        payload = json!({ "metadata": payload });
    }
    if let Some(obj) = payload.as_object_mut() {
        obj.insert("status".to_string(), json!("failed"));
        obj.insert("failedAt".to_string(), json!(Local::now().to_rfc3339()));
        obj.insert("failureReason".to_string(), json!(reason));
    }

    write_snapshot_note(&repo, oid, &payload)?;
    Ok(oid.to_string())
}

fn rollback_to_snapshot(workspace_dir: &Path, snapshot_id: &str) -> Result<String> {
    let repo = Repository::open(workspace_dir).context("打开 Git 仓库失败")?;
    let (oid, _) = resolve_snapshot_oid(&repo, snapshot_id)?;
    let obj = repo
        .find_object(oid, Some(ObjectType::Commit))
        .context("查找快照提交失败")?;
    repo.reset(&obj, ResetType::Hard, None)
        .context("执行 Git hard reset 失败")?;

    // reset 只会恢复已跟踪内容；这里补充清理未跟踪文件/目录，避免回滚后残留新建目录。
    let mut checkout = CheckoutBuilder::new();
    checkout
        .force()
        .recreate_missing(true)
        .remove_untracked(true)
        .remove_ignored(false);
    repo.checkout_head(Some(&mut checkout))
        .context("执行 checkout 清理未跟踪内容失败")?;

    Ok(oid.to_string())
}

fn count_snapshot_refs(repo: &Repository) -> usize {
    let glob = format!("{}*", SNAPSHOT_REF_PREFIX);
    match repo.references_glob(&glob) {
        Ok(iter) => iter.filter_map(Result::ok).count(),
        Err(_) => 0,
    }
}

fn dir_size_bytes(path: &Path) -> u64 {
    let mut total = 0u64;
    let entries = match fs::read_dir(path) {
        Ok(v) => v,
        Err(_) => return 0,
    };

    for item in entries.flatten() {
        let p = item.path();
        match item.metadata() {
            Ok(meta) if meta.is_file() => total = total.saturating_add(meta.len()),
            Ok(meta) if meta.is_dir() => total = total.saturating_add(dir_size_bytes(&p)),
            _ => {}
        }
    }

    total
}

fn human_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;

    let b = bytes as f64;
    if b >= GB {
        format!("{:.2} GB", b / GB)
    } else if b >= MB {
        format!("{:.2} MB", b / MB)
    } else if b >= KB {
        format!("{:.2} KB", b / KB)
    } else {
        format!("{} B", bytes)
    }
}

fn build_storage_info(repo: &Repository, keep_latest: usize) -> StorageInfo {
    let git_dir = repo.path().to_path_buf();
    let bytes = dir_size_bytes(&git_dir);

    StorageInfo {
        git_dir: git_dir.to_string_lossy().to_string(),
        bytes,
        human: human_size(bytes),
        snapshot_refs: count_snapshot_refs(repo),
        keep_latest,
    }
}

fn prune_snapshot_refs(repo: &Repository, keep_latest: usize) -> Result<usize> {
    let rows = list_snapshot_rows(repo, 500)?;
    if rows.len() <= keep_latest {
        return Ok(0);
    }

    let mut removed = 0usize;
    for row in rows.into_iter().skip(keep_latest) {
        if let Ok(mut rf) = repo.find_reference(&row.ref_name) {
            if rf.delete().is_ok() {
                removed += 1;
            }
        }
    }

    Ok(removed)
}

fn run_git_gc(workspace_dir: &Path) -> Result<String> {
    let output = Command::new("git")
        .arg("gc")
        .arg("--prune=now")
        .current_dir(workspace_dir)
        .output()
        .context("调用 git gc 失败，请确认系统已安装 Git")?;

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();

    if !output.status.success() {
        let msg = if !stderr.is_empty() {
            stderr
        } else if !stdout.is_empty() {
            stdout
        } else {
            format!("git gc 失败，退出码: {:?}", output.status.code())
        };
        anyhow::bail!(msg);
    }

    if !stdout.is_empty() {
        Ok(stdout)
    } else if !stderr.is_empty() {
        Ok(stderr)
    } else {
        Ok("git gc 完成".to_string())
    }
}

fn success_response(action: &str, workspace_dir: &Path, repo_path: String, text: String) -> GitResponse {
    GitResponse {
        ok: true,
        action: action.to_string(),
        text,
        workspace_dir: workspace_dir.to_string_lossy().to_string(),
        repo_path,
        initialized: None,
        snapshot_id: None,
        commit: None,
        snapshots: None,
        storage: None,
    }
}

fn error_response(action: &str, workspace_dir: &Path, text: String) -> GitResponse {
    GitResponse {
        ok: false,
        action: action.to_string(),
        text,
        workspace_dir: workspace_dir.to_string_lossy().to_string(),
        repo_path: workspace_dir.join(".git").to_string_lossy().to_string(),
        initialized: None,
        snapshot_id: None,
        commit: None,
        snapshots: None,
        storage: None,
    }
}

pub fn run_from_stdin() -> GitResponse {
    let mut buf = String::new();
    if let Err(err) = std::io::stdin().read_to_string(&mut buf) {
        let fallback = PathBuf::from(".");
        return error_response("init", &fallback, format!("读取输入失败: {}", err));
    }

    let req = if buf.trim().is_empty() {
        GitRequest {
            action: Some("init".to_string()),
            workspace_dir: Some(".".to_string()),
            snapshot_id: None,
            operation_name: None,
            keep_latest: Some(DEFAULT_KEEP_LATEST),
            limit: Some(DEFAULT_LIST_LIMIT),
            metadata: None,
            reason: None,
        }
    } else {
        match serde_json::from_str::<GitRequest>(&buf) {
            Ok(v) => v,
            Err(err) => {
                let fallback = PathBuf::from(".");
                return error_response("init", &fallback, format!("请求 JSON 解析失败: {}", err));
            }
        }
    };

    let action = req
        .action
        .as_deref()
        .map(|v| v.trim().to_ascii_lowercase())
        .unwrap_or_else(|| "init".to_string());
    let workspace_dir = resolve_workspace_dir(req.workspace_dir.as_deref());

    match action.as_str() {
        "init" => {
            if let Err(err) = ensure_workspace_exists(&workspace_dir) {
                return error_response("init", &workspace_dir, err.to_string());
            }

            match open_or_init_repo(&workspace_dir) {
                Ok((repo, initialized)) => {
                    let mut resp = success_response(
                        "init",
                        &workspace_dir,
                        repo.path().to_string_lossy().to_string(),
                        if initialized {
                            "Git 仓库初始化完成。".to_string()
                        } else {
                            "Git 仓库已就绪。".to_string()
                        },
                    );
                    resp.initialized = Some(initialized);
                    resp
                }
                Err(err) => error_response("init", &workspace_dir, err.to_string()),
            }
        }
        "snapshotcreate" | "create_snapshot" | "snapshot" => {
            let operation_name = trim_non_empty(req.operation_name.as_deref())
                .unwrap_or_else(|| "manual.operation".to_string());
            match create_snapshot(&workspace_dir, &operation_name, &req.metadata) {
                Ok((snapshot_id, commit, initialized)) => {
                    let repo_path = workspace_dir.join(".git").to_string_lossy().to_string();
                    let mut resp = success_response(
                        "snapshotCreate",
                        &workspace_dir,
                        repo_path,
                        format!("快照创建成功: {}", snapshot_id),
                    );
                    resp.initialized = Some(initialized);
                    resp.snapshot_id = Some(snapshot_id);
                    resp.commit = Some(commit);
                    resp
                }
                Err(err) => error_response("snapshotCreate", &workspace_dir, err.to_string()),
            }
        }
        "snapshotlist" | "history" => {
            if let Err(err) = ensure_workspace_exists(&workspace_dir) {
                return error_response("snapshotList", &workspace_dir, err.to_string());
            }

            let (repo, _) = match open_or_init_repo(&workspace_dir) {
                Ok(v) => v,
                Err(err) => return error_response("snapshotList", &workspace_dir, err.to_string()),
            };

            let limit = clamp_list_limit(req.limit.unwrap_or(DEFAULT_LIST_LIMIT));
            match list_snapshot_rows(&repo, limit) {
                Ok(rows) => {
                    let snapshots = rows
                        .into_iter()
                        .map(|row| SnapshotItem {
                            snapshot_id: row.snapshot_id,
                            commit: row.commit_oid.to_string(),
                            message: row.message,
                            created_at: row.created_at,
                            status: row.status,
                        })
                        .collect::<Vec<_>>();

                    let mut resp = success_response(
                        "snapshotList",
                        &workspace_dir,
                        repo.path().to_string_lossy().to_string(),
                        format!("已返回 {} 条快照。", snapshots.len()),
                    );
                    resp.snapshots = Some(snapshots);
                    resp
                }
                Err(err) => error_response("snapshotList", &workspace_dir, err.to_string()),
            }
        }
        "rollback" | "rollback_to_snapshot" => {
            let snapshot_id = match trim_non_empty(req.snapshot_id.as_deref()) {
                Some(v) => v,
                None => {
                    return error_response(
                        "rollback",
                        &workspace_dir,
                        "缺少 snapshotId。".to_string(),
                    )
                }
            };

            match rollback_to_snapshot(&workspace_dir, &snapshot_id) {
                Ok(commit) => {
                    let mut resp = success_response(
                        "rollback",
                        &workspace_dir,
                        workspace_dir.join(".git").to_string_lossy().to_string(),
                        format!("已回滚到快照: {}", snapshot_id),
                    );
                    resp.snapshot_id = Some(snapshot_id);
                    resp.commit = Some(commit);
                    resp
                }
                Err(err) => error_response("rollback", &workspace_dir, err.to_string()),
            }
        }
        "snapshotmarkfailed" | "mark_failed" => {
            let snapshot_id = match trim_non_empty(req.snapshot_id.as_deref()) {
                Some(v) => v,
                None => {
                    return error_response(
                        "snapshotMarkFailed",
                        &workspace_dir,
                        "缺少 snapshotId。".to_string(),
                    )
                }
            };
            let reason = trim_non_empty(req.reason.as_deref())
                .unwrap_or_else(|| "operation failed".to_string());

            match mark_snapshot_failed(&workspace_dir, &snapshot_id, &reason) {
                Ok(commit) => {
                    let mut resp = success_response(
                        "snapshotMarkFailed",
                        &workspace_dir,
                        workspace_dir.join(".git").to_string_lossy().to_string(),
                        format!("已标记快照失败: {}", snapshot_id),
                    );
                    resp.snapshot_id = Some(snapshot_id);
                    resp.commit = Some(commit);
                    resp
                }
                Err(err) => error_response("snapshotMarkFailed", &workspace_dir, err.to_string()),
            }
        }
        "storageinfo" => {
            if let Err(err) = ensure_workspace_exists(&workspace_dir) {
                return error_response("storageInfo", &workspace_dir, err.to_string());
            }

            let (repo, _) = match open_or_init_repo(&workspace_dir) {
                Ok(v) => v,
                Err(err) => return error_response("storageInfo", &workspace_dir, err.to_string()),
            };
            let keep_latest = clamp_keep_latest(req.keep_latest.unwrap_or(DEFAULT_KEEP_LATEST));
            let info = build_storage_info(&repo, keep_latest);

            let mut resp = success_response(
                "storageInfo",
                &workspace_dir,
                repo.path().to_string_lossy().to_string(),
                format!("Git 存储占用: {}", info.human),
            );
            resp.storage = Some(info);
            resp
        }
        "compactstorage" | "compact" => {
            if let Err(err) = ensure_workspace_exists(&workspace_dir) {
                return error_response("compactStorage", &workspace_dir, err.to_string());
            }

            let (repo, _) = match open_or_init_repo(&workspace_dir) {
                Ok(v) => v,
                Err(err) => return error_response("compactStorage", &workspace_dir, err.to_string()),
            };

            let keep_latest = clamp_keep_latest(req.keep_latest.unwrap_or(DEFAULT_KEEP_LATEST));
            let pruned = match prune_snapshot_refs(&repo, keep_latest) {
                Ok(v) => v,
                Err(err) => return error_response("compactStorage", &workspace_dir, err.to_string()),
            };

            let gc_output = match run_git_gc(&workspace_dir) {
                Ok(v) => v,
                Err(err) => return error_response("compactStorage", &workspace_dir, err.to_string()),
            };

            let info = build_storage_info(&repo, keep_latest);
            let mut resp = success_response(
                "compactStorage",
                &workspace_dir,
                repo.path().to_string_lossy().to_string(),
                format!("存储压缩完成，清理快照引用 {} 条。{}", pruned, gc_output),
            );
            resp.storage = Some(info);
            resp
        }
        _ => error_response(
            &action,
            &workspace_dir,
            format!(
                "未知 action: {}，支持 init/snapshotCreate/snapshotList/rollback/snapshotMarkFailed/storageInfo/compactStorage。",
                action
            ),
        ),
    }
}

pub fn write_json_response(resp: &GitResponse) {
    match serde_json::to_string(resp) {
        Ok(text) => println!("{}", text),
        Err(err) => println!(
            "{{\"ok\":false,\"action\":\"init\",\"text\":\"响应序列化失败: {}\",\"workspaceDir\":\".\",\"repoPath\":\".git\"}}",
            err.to_string().replace('"', "'")
        ),
    }
}
