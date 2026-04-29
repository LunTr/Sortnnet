use serde_json::{json, Value};
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

fn resolve_cceverything_launch() -> (String, Vec<String>, PathBuf) {
    let workspace_root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let bin_name = if cfg!(windows) {
        "cceverything.exe"
    } else {
        "cceverything"
    };
    let exe_path = workspace_root.join("target").join("debug").join(bin_name);

    if exe_path.exists() {
        return (
            exe_path.to_string_lossy().to_string(),
            Vec::new(),
            workspace_root,
        );
    }

    (
        "cargo".to_string(),
        vec!["run".to_string(), "--quiet".to_string(), "--bin".to_string(), "cceverything".to_string()],
        workspace_root,
    )
}

fn invoke_cceverything(query: &str, scope: &str, count: u32) -> Result<Value, String> {
    let payload = json!({
        "query": query,
        "scope": scope,
        "count": count,
    });

    let (program, args, cwd) = resolve_cceverything_launch();
    let mut cmd = Command::new(program);
    cmd.args(args)
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = cmd
        .spawn()
        .map_err(|err| format!("启动 cceverything 失败: {}", err))?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(payload.to_string().as_bytes())
            .map_err(|err| format!("写入请求失败: {}", err))?;
    }

    let output = child
        .wait_with_output()
        .map_err(|err| format!("等待进程结束失败: {}", err))?;

    let stdout_text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr_text = String::from_utf8_lossy(&output.stderr).trim().to_string();

    if !output.status.success() {
        return Err(format!(
            "cceverything 退出失败: code={:?}, stderr={}, stdout={}",
            output.status.code(),
            stderr_text,
            stdout_text,
        ));
    }

    serde_json::from_str::<Value>(&stdout_text)
        .map_err(|err| format!("响应 JSON 解析失败: {}; stdout={}", err, stdout_text))
}

fn get_u64_field(obj: &Value, key: &str) -> u64 {
    obj.get(key).and_then(|v| v.as_u64()).unwrap_or(0)
}

fn print_case(label: &str, response: &Value) {
    let ok = response.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
    let returned = get_u64_field(response, "returned");
    let endpoint = response
        .get("endpoint")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let note = response.get("text").and_then(|v| v.as_str()).unwrap_or("");

    println!("==== {} ====", label);
    println!("ok: {}", ok);
    println!("returned: {}", returned);
    println!("endpoint: {}", endpoint);
    if !note.is_empty() {
        println!("note: {}", note);
    }

    if let Some(results) = response.get("results").and_then(|v| v.as_array()) {
        for (idx, item) in results.iter().take(3).enumerate() {
            let full_path = item
                .get("fullPath")
                .or_else(|| item.get("full_path"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            println!("result[{}]: {}", idx, full_path);
        }
    }
}

fn main() {
    let exact_query = "v1-autosave.ustx";
    let natural_query = "查找名为v1-autosave.ustx的文件看看能不能查到";

    let exact_resp = match invoke_cceverything(exact_query, "path", 20) {
        Ok(v) => v,
        Err(err) => {
            println!("精确查询执行失败: {}", err);
            return;
        }
    };

    let natural_resp = match invoke_cceverything(natural_query, "path", 20) {
        Ok(v) => v,
        Err(err) => {
            println!("自然语句查询执行失败: {}", err);
            return;
        }
    };

    print_case("exact-query", &exact_resp);
    print_case("natural-query", &natural_resp);

    let exact_returned = get_u64_field(&exact_resp, "returned");
    let natural_returned = get_u64_field(&natural_resp, "returned");

    println!("==== diagnosis ====");
    if exact_returned > 0 && natural_returned == 0 {
        println!("判定: 检索核心能力可用，问题主要在上游查询词抽取（自然语句未规范化为文件名）。");
    } else if exact_returned == 0 {
        println!("判定: 连精确文件名都查不到，优先排查 Everything/本地搜索通道。\n");
    } else {
        println!("判定: 两种输入都可命中，问题可能在前端触发条件或上下文拼接。\n");
    }
}
