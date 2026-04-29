use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::io::{ErrorKind, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

const DEFAULT_SYSTEM_PROMPT: &str = "你是一个AI交流助手，语气简洁。如果没有要求优先用中文回复，回答控制在 3-6 句。可以表达轻微情绪和陪伴感，但不要编造事实。";
const SETTINGS_FILE_NAME: &str = "settings.json";
const DEFAULT_THINKING_TEMPERATURE: f32 = 0.7;
const DEFAULT_THINKING_INTERVAL_MS: u64 = 1200;
const DEFAULT_GIT_SNAPSHOT_KEEP_LATEST: usize = 50;
const MAX_PROMPT_CHARS_FOR_CLAUDE_ARG: usize = 3600;

#[derive(Debug, Deserialize)]
pub struct CCRequest {
    pub prompt: String,
    #[serde(default)]
    pub attachment_dirs: Vec<String>,
    #[serde(default)]
    pub allow_file_edits: bool,
    #[serde(default)]
    pub disable_tools: bool,
    #[serde(default)]
    pub working_dir: Option<String>,
    #[serde(default)]
    pub system_prompt: Option<String>,
    #[serde(default)]
    pub thinking_temperature: Option<f32>,
}

#[derive(Debug, Serialize)]
pub struct CCResponse {
    pub ok: bool,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    #[serde(default = "default_working_directory")]
    pub working_directory: String,
    #[serde(default = "default_system_prompt_owned")]
    pub system_prompt: String,
    #[serde(default = "default_thinking_temperature")]
    pub thinking_temperature: f32,
    #[serde(default = "default_thinking_interval_ms")]
    pub thinking_interval_ms: u64,
    #[serde(default = "default_git_snapshot_keep_latest")]
    pub git_snapshot_keep_latest: usize,
    #[serde(default)]
    pub attachment_directories: Vec<String>,
    #[serde(default = "default_ui_texts")]
    pub ui_texts: Value,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            working_directory: default_working_directory(),
            system_prompt: default_system_prompt_owned(),
            thinking_temperature: default_thinking_temperature(),
            thinking_interval_ms: default_thinking_interval_ms(),
            git_snapshot_keep_latest: default_git_snapshot_keep_latest(),
            attachment_directories: Vec::new(),
            ui_texts: default_ui_texts(),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct SettingsResponse {
    pub ok: bool,
    pub settings: AppSettings,
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
}

#[derive(Debug, Clone)]
struct LoadedSettings {
    settings: AppSettings,
    source: String,
    warning: Option<String>,
}

#[derive(Debug, Clone)]
struct EffectiveRunOptions {
    working_dir: PathBuf,
    system_prompt: String,
    thinking_temperature: Option<f32>,
}

fn default_working_directory() -> String {
    ".".to_string()
}

fn default_system_prompt_owned() -> String {
    DEFAULT_SYSTEM_PROMPT.to_string()
}

fn default_thinking_temperature() -> f32 {
    DEFAULT_THINKING_TEMPERATURE
}

fn default_thinking_interval_ms() -> u64 {
    DEFAULT_THINKING_INTERVAL_MS
}

fn default_git_snapshot_keep_latest() -> usize {
    DEFAULT_GIT_SNAPSHOT_KEEP_LATEST
}

fn default_ui_texts() -> Value {
    Value::Object(serde_json::Map::new())
}

fn clamp_temperature(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(0.0, 1.0)
    } else {
        DEFAULT_THINKING_TEMPERATURE
    }
}

fn clamp_interval_ms(value: u64) -> u64 {
    value.clamp(600, 10_000)
}

fn clamp_git_snapshot_keep_latest(value: usize) -> usize {
    value.clamp(10, 500)
}

fn trim_non_empty(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn normalize_settings(mut settings: AppSettings) -> AppSettings {
    settings.working_directory = trim_non_empty(&settings.working_directory)
        .unwrap_or_else(default_working_directory);
    settings.system_prompt = trim_non_empty(&settings.system_prompt)
        .unwrap_or_else(default_system_prompt_owned);
    settings.thinking_temperature = clamp_temperature(settings.thinking_temperature);
    settings.thinking_interval_ms = clamp_interval_ms(settings.thinking_interval_ms);
    settings.git_snapshot_keep_latest =
        clamp_git_snapshot_keep_latest(settings.git_snapshot_keep_latest);
    settings.attachment_directories = settings
        .attachment_directories
        .into_iter()
        .filter_map(|v| trim_non_empty(&v))
        .collect();
    if !settings.ui_texts.is_object() {
        settings.ui_texts = default_ui_texts();
    }
    settings
}

fn resolve_settings_file_path() -> PathBuf {
    env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join(SETTINGS_FILE_NAME)
}

fn load_settings() -> LoadedSettings {
    let settings_path = resolve_settings_file_path();
    match fs::read_to_string(&settings_path) {
        Ok(raw) => match serde_json::from_str::<AppSettings>(&raw) {
            Ok(parsed) => LoadedSettings {
                settings: normalize_settings(parsed),
                source: settings_path.to_string_lossy().to_string(),
                warning: None,
            },
            Err(err) => LoadedSettings {
                settings: AppSettings::default(),
                source: "defaults".to_string(),
                warning: Some(format!("settings.json 解析失败: {}", err)),
            },
        },
        Err(err) if err.kind() == ErrorKind::NotFound => LoadedSettings {
            settings: AppSettings::default(),
            source: "defaults".to_string(),
            warning: None,
        },
        Err(err) => LoadedSettings {
            settings: AppSettings::default(),
            source: "defaults".to_string(),
            warning: Some(format!("读取 settings.json 失败: {}", err)),
        },
    }
}

fn resolve_working_dir(requested: Option<&str>, fallback: &str) -> PathBuf {
    let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

    let desired = requested
        .and_then(trim_non_empty)
        .or_else(|| trim_non_empty(fallback))
        .unwrap_or_else(default_working_directory);

    let candidate = PathBuf::from(desired);
    let resolved = if candidate.is_absolute() {
        candidate
    } else {
        cwd.join(candidate)
    };

    if resolved.is_dir() {
        resolved
    } else {
        cwd
    }
}

fn merge_attachment_dirs(from_request: &[String], from_settings: &[String]) -> Vec<String> {
    let mut unique_dirs: BTreeSet<String> = BTreeSet::new();

    for dir in from_settings {
        if let Some(v) = trim_non_empty(dir) {
            unique_dirs.insert(v);
        }
    }

    for dir in from_request {
        if let Some(v) = trim_non_empty(dir) {
            unique_dirs.insert(v);
        }
    }

    unique_dirs.into_iter().collect()
}

fn resolve_run_options(req: &CCRequest, settings: &AppSettings) -> EffectiveRunOptions {
    let system_prompt = req
        .system_prompt
        .as_deref()
        .and_then(trim_non_empty)
        .unwrap_or_else(|| settings.system_prompt.clone());

    let thinking_temperature = req
        .thinking_temperature
        .map(clamp_temperature)
        .or(Some(settings.thinking_temperature));

    let working_dir = resolve_working_dir(req.working_dir.as_deref(), &settings.working_directory);

    EffectiveRunOptions {
        working_dir,
        system_prompt,
        thinking_temperature,
    }
}

pub fn read_settings_for_frontend() -> SettingsResponse {
    let loaded = load_settings();
    let mut settings = loaded.settings.clone();
    settings.working_directory = resolve_working_dir(None, &settings.working_directory)
        .to_string_lossy()
        .to_string();
    settings.thinking_temperature = clamp_temperature(settings.thinking_temperature);
    settings.thinking_interval_ms = clamp_interval_ms(settings.thinking_interval_ms);
    settings.git_snapshot_keep_latest =
        clamp_git_snapshot_keep_latest(settings.git_snapshot_keep_latest);

    SettingsResponse {
        ok: true,
        settings,
        source: loaded.source,
        warning: loaded.warning,
    }
}

pub fn run_from_stdin() -> CCResponse {
    let mut buf = String::new();
    if let Err(err) = std::io::stdin().read_to_string(&mut buf) {
        return CCResponse {
            ok: false,
            text: format!("读取输入失败: {}", err),
        };
    }

    let req: CCRequest = match serde_json::from_str(&buf) {
        Ok(v) => v,
        Err(err) => {
            return CCResponse {
                ok: false,
                text: format!("请求 JSON 解析失败: {}", err),
            }
        }
    };

    let loaded_settings = load_settings();
    let options = resolve_run_options(&req, &loaded_settings.settings);
    let attachment_dirs = merge_attachment_dirs(&req.attachment_dirs, &loaded_settings.settings.attachment_directories);

    let text = run_claude_prompt_with_options(
        &req.prompt,
        &attachment_dirs,
        req.allow_file_edits,
        req.disable_tools,
        &options,
    );
    CCResponse { ok: true, text }
}

fn resolve_claude_candidates() -> Vec<String> {
    let mut candidates: BTreeSet<String> = BTreeSet::new();

    if let Ok(path) = env::var("CLAUDE_CLI_PATH") {
        let p = path.trim();
        if !p.is_empty() {
            let pbuf = PathBuf::from(p);
            if pbuf.is_dir() {
                candidates.insert(pbuf.join("claude.cmd").to_string_lossy().to_string());
                candidates.insert(pbuf.join("claude.exe").to_string_lossy().to_string());
                candidates.insert(pbuf.join("claude.ps1").to_string_lossy().to_string());
                candidates.insert(pbuf.join("claude").to_string_lossy().to_string());
            } else {
                candidates.insert(p.to_string());
            }
        }
    }

    if let Ok(appdata) = env::var("APPDATA") {
        candidates.insert(format!(r"{}\npm\claude.cmd", appdata));
        candidates.insert(format!(r"{}\npm\claude.exe", appdata));
        candidates.insert(format!(r"{}\npm\claude.ps1", appdata));
    }

    candidates.insert("claude".to_string());
    candidates.insert("claude.cmd".to_string());
    candidates.insert("claude.exe".to_string());
    candidates.insert("claude.ps1".to_string());

    candidates.into_iter().collect()
}

fn decode_output_bytes(bytes: &[u8]) -> String {
    if bytes.is_empty() {
        return String::new();
    }

    let nul_count = bytes.iter().filter(|&&b| b == 0).count();
    if nul_count > bytes.len() / 6 {
        let mut u16_buf = Vec::with_capacity(bytes.len() / 2);
        let mut i = 0;
        while i + 1 < bytes.len() {
            u16_buf.push(u16::from_le_bytes([bytes[i], bytes[i + 1]]));
            i += 2;
        }
        return String::from_utf16_lossy(&u16_buf).replace('\0', "");
    }

    String::from_utf8_lossy(bytes).to_string()
}

fn format_claude_output(output: &Output) -> String {
    let stdout_text = decode_output_bytes(&output.stdout).trim().to_string();
    let stderr_text = decode_output_bytes(&output.stderr).trim().to_string();

    if output.status.success() {
        if !stdout_text.is_empty() {
            if let Some(stream_text) = extract_text_from_stream_json(&stdout_text) {
                let cleaned = stream_text.trim();
                if !cleaned.is_empty() {
                    return clamp_render_text(cleaned);
                }
            }

            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&stdout_text) {
                if let Some(result) = json.get("result").and_then(|v| v.as_str()) {
                    let result = result.trim();
                    if !result.is_empty() {
                        return clamp_render_text(result);
                    }
                }

                if is_empty_result_envelope(&json) {
                    return String::new();
                }
            }
            return clamp_render_text(&stdout_text);
        }
        if !stderr_text.is_empty() {
            return format!("Claude 返回空 stdout，stderr: {}", stderr_text);
        }
        return String::new();
    }

    if !stderr_text.is_empty() {
        return format!("Claude 调用失败: {}", stderr_text);
    }
    format!("Claude 调用失败，退出码: {:?}", output.status.code())
}

fn is_empty_result_envelope(value: &serde_json::Value) -> bool {
    let obj = match value.as_object() {
        Some(v) => v,
        None => return false,
    };

    let kind = obj.get("type").and_then(|v| v.as_str()).unwrap_or("");
    let result = obj.get("result").and_then(|v| v.as_str()).unwrap_or("");
    kind.eq_ignore_ascii_case("result") && result.trim().is_empty()
}

fn extract_text_from_stream_json(stdout_text: &str) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();

    for line in stdout_text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let value: serde_json::Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let kind = value.get("type").and_then(|v| v.as_str()).unwrap_or("");
        if !kind.eq_ignore_ascii_case("assistant") {
            continue;
        }

        let content = match value
            .get("message")
            .and_then(|v| v.get("content"))
            .and_then(|v| v.as_array())
        {
            Some(v) => v,
            None => continue,
        };

        let mut chunk = String::new();
        for item in content {
            let item_type = item.get("type").and_then(|v| v.as_str()).unwrap_or("");
            if !item_type.eq_ignore_ascii_case("text") {
                continue;
            }

            let text = item.get("text").and_then(|v| v.as_str()).unwrap_or("").trim();
            if text.is_empty() {
                continue;
            }

            if !chunk.is_empty() {
                chunk.push('\n');
            }
            chunk.push_str(text);
        }

        if chunk.is_empty() {
            continue;
        }

        let duplicated = parts.last().map(|last| last == &chunk).unwrap_or(false);
        if !duplicated {
            parts.push(chunk);
        }
    }

    if parts.is_empty() {
        None
    } else {
        Some(parts.join("\n"))
    }
}

fn clamp_render_text(text: &str) -> String {
    let cleaned = text.replace('\0', " ");
    let max_chars = 5000usize;
    let chars: Vec<char> = cleaned.chars().collect();
    if chars.len() > max_chars {
        let clipped: String = chars.into_iter().take(max_chars).collect();
        return format!("{}\n\n(内容过长，已截断显示)", clipped);
    }
    cleaned
}

fn truncate_prompt_for_cli_arg(prompt: &str) -> String {
    let chars: Vec<char> = prompt.chars().collect();
    if chars.len() <= MAX_PROMPT_CHARS_FOR_CLAUDE_ARG {
        return prompt.to_string();
    }

    let clipped: String = chars
        .into_iter()
        .take(MAX_PROMPT_CHARS_FOR_CLAUDE_ARG)
        .collect();
    format!(
        "{}\n\n[提示] 为避免 Windows 命令行参数过长，提示词已自动截断。",
        clipped
    )
}

fn has_unsupported_option(stderr: &str) -> bool {
    let text = stderr.to_lowercase();
    text.contains("unknown option")
        || text.contains("unrecognized option")
        || text.contains("unknown argument")
        || text.contains("unexpected argument")
}

fn resolve_windows_launch(program: &str, args: &[String]) -> (String, Vec<String>) {
    if cfg!(windows) && program.to_lowercase().ends_with(".ps1") {
        let mut launch_args = vec![
            "-NoProfile".to_string(),
            "-ExecutionPolicy".to_string(),
            "Bypass".to_string(),
            "-File".to_string(),
            program.to_string(),
        ];
        launch_args.extend(args.iter().cloned());
        return ("powershell.exe".to_string(), launch_args);
    }

    (program.to_string(), args.to_vec())
}

fn run_with_program(program: &str, args: &[String], working_dir: &Path) -> std::io::Result<Output> {
    let (launch_program, launch_args) = resolve_windows_launch(program, args);

    let mut cmd = Command::new(launch_program);
    cmd.args(launch_args)
        .current_dir(working_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("PYTHONUTF8", "1")
        .env("PYTHONIOENCODING", "utf-8");

    let mut child = cmd.spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        use std::io::Write;
        let _ = stdin.write_all(b"");
    }

    child.wait_with_output()
}

fn build_profiles(
    prompt: &str,
    attachment_dirs: &[String],
    allow_file_edits: bool,
    disable_tools: bool,
    system_prompt: &str,
    thinking_temperature: Option<f32>,
) -> Vec<(String, Vec<String>)> {
    let mut unique_dirs: BTreeSet<String> = BTreeSet::new();
    unique_dirs.insert(".".to_string());
    for dir in attachment_dirs {
        let trimmed = dir.trim();
        if !trimmed.is_empty() {
            unique_dirs.insert(trimmed.to_string());
        }
    }

    let mut add_dir_args: Vec<String> = Vec::new();
    for dir in unique_dirs {
        add_dir_args.push("--add-dir".to_string());
        add_dir_args.push(dir);
    }

    let mut base: Vec<String> = vec!["-p".to_string()];
    base.extend(add_dir_args);
    if disable_tools {
        base.push("--tools".to_string());
        base.push("".to_string());
    } else if allow_file_edits {
        base.push("--tools".to_string());
        base.push("default".to_string());
        base.push("--permission-mode".to_string());
        base.push("bypassPermissions".to_string());
    }

    let mut full_json = base.clone();
    full_json.push("--effort".to_string());
    full_json.push("medium".to_string());
    full_json.push("--append-system-prompt".to_string());
    full_json.push(system_prompt.to_string());
    full_json.push("--output-format".to_string());
    full_json.push("json".to_string());
    full_json.push(prompt.to_string());

    let mut full_stream_json = base.clone();
    full_stream_json.push("--effort".to_string());
    full_stream_json.push("medium".to_string());
    full_stream_json.push("--append-system-prompt".to_string());
    full_stream_json.push(system_prompt.to_string());
    full_stream_json.push("--verbose".to_string());
    full_stream_json.push("--output-format".to_string());
    full_stream_json.push("stream-json".to_string());
    full_stream_json.push(prompt.to_string());

    let mut full_text = base.clone();
    full_text.push("--effort".to_string());
    full_text.push("medium".to_string());
    full_text.push("--append-system-prompt".to_string());
    full_text.push(system_prompt.to_string());
    full_text.push(prompt.to_string());

    let mut min_json = base.clone();
    min_json.push("--append-system-prompt".to_string());
    min_json.push(system_prompt.to_string());
    min_json.push("--output-format".to_string());
    min_json.push("json".to_string());
    min_json.push(prompt.to_string());

    let mut min_stream_json = base.clone();
    min_stream_json.push("--append-system-prompt".to_string());
    min_stream_json.push(system_prompt.to_string());
    min_stream_json.push("--verbose".to_string());
    min_stream_json.push("--output-format".to_string());
    min_stream_json.push("stream-json".to_string());
    min_stream_json.push(prompt.to_string());

    let mut min_text = base;
    min_text.push("--append-system-prompt".to_string());
    min_text.push(system_prompt.to_string());
    min_text.push(prompt.to_string());

    let base_profiles = vec![
        ("inline-stream-full".to_string(), full_stream_json),
        ("inline-stream-min".to_string(), min_stream_json),
        ("inline-json-full".to_string(), full_json),
        ("inline-text-full".to_string(), full_text),
        ("inline-json-min".to_string(), min_json),
        ("inline-text-min".to_string(), min_text),
    ];

    if let Some(temp) = thinking_temperature {
        let temp_value = clamp_temperature(temp);
        let mut with_temperature: Vec<(String, Vec<String>)> = Vec::with_capacity(base_profiles.len() * 2);
        for (name, args) in &base_profiles {
            let mut with_args = args.clone();
            let insert_index = with_args.len().saturating_sub(1);
            with_args.insert(insert_index, "--temperature".to_string());
            with_args.insert(insert_index + 1, format!("{:.2}", temp_value));
            with_temperature.push((format!("{}-temp", name), with_args));
        }
        with_temperature.extend(base_profiles);
        return with_temperature;
    }

    base_profiles
}

fn run_claude_prompt_with_options(
    prompt: &str,
    attachment_dirs: &[String],
    allow_file_edits: bool,
    disable_tools: bool,
    options: &EffectiveRunOptions,
) -> String {
    let safe_prompt = truncate_prompt_for_cli_arg(prompt);
    let candidates = resolve_claude_candidates();
    let profiles = build_profiles(
        &safe_prompt,
        attachment_dirs,
        allow_file_edits,
        disable_tools,
        &options.system_prompt,
        options.thinking_temperature,
    );

    let mut tried: Vec<String> = Vec::new();
    let mut errors: Vec<String> = Vec::new();

    for program in candidates {
        let mut missing_program = false;
        for (profile_name, args) in &profiles {
            match run_with_program(&program, args, &options.working_dir) {
                Ok(output) => {
                    if output.status.success() {
                        let rendered = format_claude_output(&output);
                        if rendered.trim().is_empty() {
                            continue;
                        }
                        return rendered;
                    }

                    let stderr = decode_output_bytes(&output.stderr);
                    if has_unsupported_option(&stderr) {
                        continue;
                    }

                    let rendered = format_claude_output(&output);
                    if rendered.contains("调用失败") {
                        errors.push(format!("{} [{}] => {}", program, profile_name, rendered));
                        continue;
                    }
                    return rendered;
                }
                Err(err) if err.kind() == ErrorKind::NotFound => {
                    missing_program = true;
                    break;
                }
                Err(err) => {
                    errors.push(format!("{} [{}] => {}", program, profile_name, err));
                }
            }
        }

        tried.push(program.clone());
        if missing_program {
            continue;
        }
    }

    if !errors.is_empty() {
        return format!(
            "无法启动 Claude CLI。已尝试: {}。错误: {}",
            tried.join(", "),
            errors.join(" | ")
        );
    }

    format!(
        "无法启动 Claude CLI: program not found。已尝试: {}。请先确认 claude --help 可运行，或设置环境变量 CLAUDE_CLI_PATH。",
        tried.join(", ")
    )
}

pub fn run_claude_prompt(prompt: &str, attachment_dirs: &[String], allow_file_edits: bool) -> String {
    let loaded_settings = load_settings();
    let options = EffectiveRunOptions {
        working_dir: resolve_working_dir(None, &loaded_settings.settings.working_directory),
        system_prompt: loaded_settings.settings.system_prompt,
        thinking_temperature: Some(loaded_settings.settings.thinking_temperature),
    };
    run_claude_prompt_with_options(prompt, attachment_dirs, allow_file_edits, false, &options)
}

fn write_json<T: Serialize>(value: &T) {
    match serde_json::to_string(value) {
        Ok(text) => {
            println!("{}", text);
        }
        Err(err) => {
            println!(
                "{{\"ok\":false,\"text\":\"输出序列化失败: {}\"}}",
                escape_json_string(&err.to_string())
            );
        }
    }
}

pub fn write_json_response(resp: &CCResponse) {
    write_json(resp);
}

pub fn write_settings_response(resp: &SettingsResponse) {
    write_json(resp);
}

fn escape_json_string(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}
