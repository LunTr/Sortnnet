use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine;
use regex::RegexBuilder;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use walkdir::{DirEntry, WalkDir};

const DEFAULT_HTTP_HOST: &str = "127.0.0.1";
const DEFAULT_HTTP_PORT: u16 = 80;
const DEFAULT_COUNT: u32 = 100;
const MAX_COUNT: u32 = 5000;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EverythingSearchRequest {
    #[serde(default)]
    pub query: String,
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub count: Option<u32>,
    #[serde(default)]
    pub offset: Option<u32>,
    #[serde(default)]
    pub regex: bool,
    #[serde(default)]
    pub whole_word: bool,
    #[serde(default)]
    pub match_case: bool,
    #[serde(default)]
    pub host: Option<String>,
    #[serde(default)]
    pub port: Option<u16>,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub password: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EverythingResultItem {
    pub name: String,
    pub path: String,
    pub full_path: String,
    pub is_folder: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub date_modified: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EverythingSearchResponse {
    pub ok: bool,
    pub query: String,
    pub scope: String,
    pub endpoint: String,
    pub returned: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<u64>,
    pub results: Vec<EverythingResultItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

#[derive(Debug, Clone, Copy)]
enum SearchScope {
    FileName,
    WholePath,
}

impl SearchScope {
    fn from_input(scope: Option<&str>) -> Self {
        let value = scope.unwrap_or("file").trim().to_lowercase();
        match value.as_str() {
            "path" | "dir" | "directory" | "folder" => Self::WholePath,
            _ => Self::FileName,
        }
    }

    fn as_str(&self) -> &'static str {
        match self {
            Self::FileName => "file",
            Self::WholePath => "path",
        }
    }

    fn to_everything_path_flag(&self) -> &'static str {
        match self {
            Self::FileName => "0",
            Self::WholePath => "1",
        }
    }
}

fn trim_non_empty(value: Option<&str>) -> Option<String> {
    let text = value.unwrap_or("").trim();
    if text.is_empty() {
        None
    } else {
        Some(text.to_string())
    }
}

fn normalize_host(value: Option<&str>) -> String {
    trim_non_empty(value).unwrap_or_else(|| DEFAULT_HTTP_HOST.to_string())
}

fn normalize_port(value: Option<u16>) -> u16 {
    value.unwrap_or(DEFAULT_HTTP_PORT)
}

fn normalize_count(value: Option<u32>) -> u32 {
    value.unwrap_or(DEFAULT_COUNT).clamp(1, MAX_COUNT)
}

fn normalize_offset(value: Option<u32>) -> u32 {
    value.unwrap_or(0)
}

fn parse_u64_value(v: &Value) -> Option<u64> {
    if let Some(n) = v.as_u64() {
        return Some(n);
    }

    if let Some(s) = v.as_str() {
        let cleaned = s.replace(',', "").trim().to_string();
        if cleaned.is_empty() {
            return None;
        }
        return cleaned.parse::<u64>().ok();
    }

    None
}

fn get_string_field<'a>(map: &'a serde_json::Map<String, Value>, keys: &[&str]) -> Option<&'a str> {
    for key in keys {
        if let Some(value) = map.get(*key).and_then(|v| v.as_str()) {
            if !value.trim().is_empty() {
                return Some(value);
            }
        }
    }

    None
}

fn build_full_path(path: &str, name: &str) -> String {
    if path.is_empty() {
        return name.to_string();
    }

    if name.is_empty() {
        return path.to_string();
    }

    if path.ends_with('\\') || path.ends_with('/') {
        format!("{}{}", path, name)
    } else {
        format!("{}\\{}", path, name)
    }
}

fn parse_total(value: &Value) -> Option<u64> {
    let obj = value.as_object()?;
    let keys = ["totalResults", "totalresults", "total", "TotalResults"];

    for key in keys {
        if let Some(v) = obj.get(key) {
            if let Some(total) = parse_u64_value(v) {
                return Some(total);
            }
        }
    }

    None
}

fn parse_results(value: &Value) -> Vec<EverythingResultItem> {
    let obj = match value.as_object() {
        Some(v) => v,
        None => return Vec::new(),
    };

    let rows = obj
        .get("results")
        .or_else(|| obj.get("Results"))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let mut items = Vec::with_capacity(rows.len());

    for row in rows {
        let row_obj = match row.as_object() {
            Some(v) => v,
            None => continue,
        };

        let name = get_string_field(row_obj, &["name", "Name", "filename", "fileName"])
            .unwrap_or("")
            .to_string();
        let path = get_string_field(row_obj, &["path", "Path"]).unwrap_or("").to_string();
        let full_path = build_full_path(&path, &name);

        let is_folder = row_obj
            .get("is_folder")
            .or_else(|| row_obj.get("isFolder"))
            .and_then(|v| v.as_bool())
            .or_else(|| {
                row_obj
                    .get("type")
                    .and_then(|v| v.as_str())
                    .map(|t| t.eq_ignore_ascii_case("folder"))
            })
            .unwrap_or_else(|| {
                // Everything folder results typically have no size.
                !row_obj.contains_key("size") && !row_obj.contains_key("Size")
            });

        let size = row_obj
            .get("size")
            .or_else(|| row_obj.get("Size"))
            .and_then(parse_u64_value);

        let date_modified = get_string_field(
            row_obj,
            &["date_modified", "dateModified", "DateModified", "dm"],
        )
        .map(|s| s.to_string());

        items.push(EverythingResultItem {
            name,
            path,
            full_path,
            is_folder,
            size,
            date_modified,
        });
    }

    items
}

fn build_endpoint(host: &str, port: u16) -> String {
    format!("http://{}:{}/", host, port)
}

fn is_skip_dir(entry: &DirEntry) -> bool {
    if !entry.file_type().is_dir() {
        return false;
    }

    let name = entry.file_name().to_string_lossy().to_lowercase();
    matches!(
        name.as_str(),
        "node_modules" | "target" | ".git" | ".idea" | ".vscode" | "appdata"
    )
}

fn normalize_for_compare(text: &str, match_case: bool) -> String {
    if match_case {
        text.to_string()
    } else {
        text.to_lowercase()
    }
}

fn tokenize_for_whole_word(text: &str) -> Vec<String> {
    text
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.'))
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect()
}

fn build_local_matcher(req: &EverythingSearchRequest) -> Result<Box<dyn Fn(&str) -> bool>, String> {
    let query = req.query.trim().to_string();
    if query.is_empty() {
        return Err("缺少查询词".to_string());
    }

    if req.regex {
        let mut builder = RegexBuilder::new(&query);
        builder.case_insensitive(!req.match_case);
        let re = builder
            .build()
            .map_err(|err| format!("正则表达式无效: {}", err))?;
        return Ok(Box::new(move |candidate: &str| re.is_match(candidate)));
    }

    let query_cmp = normalize_for_compare(&query, req.match_case);
    let whole_word = req.whole_word;
    let match_case = req.match_case;

    Ok(Box::new(move |candidate: &str| {
        let candidate_cmp = normalize_for_compare(candidate, match_case);
        if whole_word {
            let tokens = tokenize_for_whole_word(&candidate_cmp);
            tokens.iter().any(|t| t == &query_cmp)
        } else {
            candidate_cmp.contains(&query_cmp)
        }
    }))
}

fn collect_local_search_roots() -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();

    let mut push_root = |path: PathBuf| {
        if !path.exists() || !path.is_dir() {
            return;
        }
        let key = path.to_string_lossy().to_lowercase();
        if seen.insert(key) {
            roots.push(path);
        }
    };

    if let Ok(cwd) = std::env::current_dir() {
        push_root(cwd.clone());
        for ancestor in cwd.ancestors() {
            let name = ancestor
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            if name.eq_ignore_ascii_case("desktop") || name == "桌面" {
                push_root(ancestor.to_path_buf());
            }
        }
    }

    if let Ok(user_profile) = std::env::var("USERPROFILE") {
        push_root(Path::new(&user_profile).join("Desktop"));
        push_root(Path::new(&user_profile).join("桌面"));
    }

    roots
}

fn build_local_item(path: &Path, metadata: &fs::Metadata) -> EverythingResultItem {
    let name = path
        .file_name()
        .map(|v| v.to_string_lossy().to_string())
        .unwrap_or_default();
    let parent = path
        .parent()
        .map(|v| v.to_string_lossy().to_string())
        .unwrap_or_default();
    let full_path = path.to_string_lossy().to_string();

    EverythingResultItem {
        name,
        path: parent,
        full_path,
        is_folder: metadata.is_dir(),
        size: if metadata.is_file() { Some(metadata.len()) } else { None },
        date_modified: None,
    }
}

fn local_fallback_search(req: &EverythingSearchRequest, endpoint: &str, reason: &str) -> Result<EverythingSearchResponse, String> {
    let scope = SearchScope::from_input(req.scope.as_deref());
    let count = normalize_count(req.count) as usize;
    let offset = normalize_offset(req.offset) as usize;
    let target_len = offset.saturating_add(count);

    let matcher = build_local_matcher(req)?;
    let mut matched: Vec<EverythingResultItem> = Vec::new();
    let mut seen_full_paths: BTreeSet<String> = BTreeSet::new();

    for root in collect_local_search_roots() {
        for entry in WalkDir::new(&root)
            .follow_links(false)
            .into_iter()
            .filter_entry(|e| !is_skip_dir(e))
            .filter_map(Result::ok)
        {
            let md = match entry.metadata() {
                Ok(v) => v,
                Err(_) => continue,
            };

            let candidate = match scope {
                SearchScope::FileName => entry.file_name().to_string_lossy().to_string(),
                SearchScope::WholePath => entry.path().to_string_lossy().to_string(),
            };

            if !matcher(&candidate) {
                continue;
            }

            let item = build_local_item(entry.path(), &md);
            if !seen_full_paths.insert(item.full_path.to_lowercase()) {
                continue;
            }

            matched.push(item);
            if matched.len() >= target_len {
                break;
            }
        }

        if matched.len() >= target_len {
            break;
        }
    }

    let total = matched.len() as u64;
    let paged = matched.into_iter().skip(offset).take(count).collect::<Vec<_>>();
    let fallback_hint = format!(
        "Everything HTTP 不可用，已回退本地搜索。原因: {}",
        reason.trim()
    );

    Ok(EverythingSearchResponse {
        ok: true,
        query: req.query.clone(),
        scope: scope.as_str().to_string(),
        endpoint: endpoint.to_string(),
        returned: paged.len(),
        total: Some(total),
        results: paged,
        text: Some(fallback_hint),
    })
}

fn everything_http_search(req: &EverythingSearchRequest) -> Result<(Vec<EverythingResultItem>, Option<u64>, String), String> {
    let scope = SearchScope::from_input(req.scope.as_deref());
    let host = normalize_host(req.host.as_deref());
    let port = normalize_port(req.port);
    let endpoint = build_endpoint(&host, port);
    let count = normalize_count(req.count);
    let offset = normalize_offset(req.offset);

    let agent = ureq::AgentBuilder::new()
        .timeout_connect(std::time::Duration::from_secs(2))
        .timeout_read(std::time::Duration::from_secs(10))
        .build();

    let mut request = agent
        .get(&endpoint)
        .query("json", "1")
        .query("search", &req.query)
        .query("offset", &offset.to_string())
        .query("count", &count.to_string())
        .query("path", scope.to_everything_path_flag())
        .query("regex", if req.regex { "1" } else { "0" })
        .query("wholeword", if req.whole_word { "1" } else { "0" })
        .query("case", if req.match_case { "1" } else { "0" })
        .query("path_column", "1")
        .query("size_column", "1")
        .query("date_modified_column", "1");

    if let Some(username) = trim_non_empty(req.username.as_deref()) {
        let password = req.password.clone().unwrap_or_default();
        let auth = BASE64_STANDARD.encode(format!("{}:{}", username, password));
        request = request.set("Authorization", &format!("Basic {}", auth));
    }

    let response_text = match request.call() {
        Ok(response) => response
            .into_string()
            .map_err(|err| format!("读取 Everything HTTP 响应失败: {}", err))?,
        Err(ureq::Error::Status(code, response)) => {
            let body = response.into_string().unwrap_or_default();
            return Err(format!(
                "Everything HTTP 返回状态 {}: {}",
                code,
                body.trim()
            ));
        }
        Err(ureq::Error::Transport(err)) => {
            return Err(format!(
                "无法连接 Everything HTTP 服务 ({}): {}。请确认 Everything 已启用 HTTP Server。",
                endpoint, err
            ));
        }
    };

    let json: Value = serde_json::from_str(&response_text)
        .map_err(|err| format!("Everything HTTP 响应不是有效 JSON: {}", err))?;

    let results = parse_results(&json);
    let total = parse_total(&json);

    Ok((results, total, endpoint))
}

fn build_error_response(req: Option<&EverythingSearchRequest>, message: String) -> EverythingSearchResponse {
    let scope = SearchScope::from_input(req.and_then(|v| v.scope.as_deref()));
    let host = normalize_host(req.and_then(|v| v.host.as_deref()));
    let port = normalize_port(req.and_then(|v| v.port));

    EverythingSearchResponse {
        ok: false,
        query: req.map(|v| v.query.clone()).unwrap_or_default(),
        scope: scope.as_str().to_string(),
        endpoint: build_endpoint(&host, port),
        returned: 0,
        total: None,
        results: Vec::new(),
        text: Some(message),
    }
}

pub fn run_from_stdin() -> EverythingSearchResponse {
    let mut buf = String::new();
    if let Err(err) = std::io::stdin().read_to_string(&mut buf) {
        return build_error_response(None, format!("读取输入失败: {}", err));
    }

    let req: EverythingSearchRequest = match serde_json::from_str(&buf) {
        Ok(v) => v,
        Err(err) => {
            return build_error_response(None, format!("请求 JSON 解析失败: {}", err));
        }
    };

    let endpoint = build_endpoint(
        &normalize_host(req.host.as_deref()),
        normalize_port(req.port),
    );

    match everything_http_search(&req) {
        Ok((results, total, endpoint)) => EverythingSearchResponse {
            ok: true,
            query: req.query,
            scope: SearchScope::from_input(req.scope.as_deref()).as_str().to_string(),
            endpoint,
            returned: results.len(),
            total,
            results,
            text: None,
        },
        Err(err) => match local_fallback_search(&req, &endpoint, &err) {
            Ok(resp) => resp,
            Err(fallback_err) => build_error_response(
                Some(&req),
                format!("{}；本地搜索回退失败: {}", err, fallback_err),
            ),
        },
    }
}

pub fn write_json_response(resp: &EverythingSearchResponse) {
    match serde_json::to_string(resp) {
        Ok(text) => println!("{}", text),
        Err(err) => println!(
            "{{\"ok\":false,\"query\":\"\",\"scope\":\"file\",\"endpoint\":\"\",\"returned\":0,\"results\":[],\"text\":\"输出序列化失败: {}\"}}",
            escape_json_string(&err.to_string())
        ),
    }
}

fn escape_json_string(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}
