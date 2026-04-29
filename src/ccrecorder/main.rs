mod analyzer;
mod executor;
mod interactive;
mod planner;
mod rules;

use analyzer::scan_directory;
use clap::{Parser, Subcommand};
use colored::Colorize;
use executor::execute_steps;
use planner::validate_plan;
use rules::{
    error_response, escape_json_string, preview_step, resolve_base_dir, resolve_whitelist,
    sanitize_plan_id, ReorderPlan, ReorderRequest, ReorderResponse, DEFAULT_SCAN_MAX_ENTRIES,
    HARD_SCAN_MAX_ENTRIES,
};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

#[derive(Debug, Parser)]
#[command(
    name = "ccrecorder",
    version,
    about = "CCRecorder 文件整理技能：分析、计划校验、执行"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    Scan {
        #[arg(long)]
        dir: Option<String>,
        #[arg(long, default_value_t = DEFAULT_SCAN_MAX_ENTRIES)]
        max_entries: usize,
    },
    DryRun {
        #[arg(long)]
        dir: Option<String>,
        #[arg(long)]
        plan_file: String,
        #[arg(long)]
        plan_id: Option<String>,
        #[arg(long = "allow")]
        whitelist: Vec<String>,
    },
    Apply {
        #[arg(long)]
        dir: Option<String>,
        #[arg(long)]
        plan_file: String,
        #[arg(long)]
        plan_id: Option<String>,
        #[arg(long = "allow")]
        whitelist: Vec<String>,
        #[arg(long)]
        yes: bool,
    },
    Stdin,
}

fn read_request() -> Result<ReorderRequest, String> {
    let mut buf = String::new();
    std::io::stdin()
        .read_to_string(&mut buf)
        .map_err(|err| format!("读取输入失败: {}", err))?;

    serde_json::from_str::<ReorderRequest>(&buf).map_err(|err| format!("请求 JSON 解析失败: {}", err))
}

fn parse_plan_file(plan_file: &str) -> Result<ReorderPlan, String> {
    let raw = fs::read_to_string(plan_file).map_err(|err| format!("读取 plan 文件失败: {}", err))?;
    serde_json::from_str::<ReorderPlan>(&raw).map_err(|err| format!("plan 文件解析失败: {}", err))
}

fn handle_scan(req: &ReorderRequest, base_dir: &Path) -> ReorderResponse {
    let max_entries = req
        .max_entries
        .unwrap_or(DEFAULT_SCAN_MAX_ENTRIES)
        .clamp(1, HARD_SCAN_MAX_ENTRIES);
    let report = scan_directory(base_dir, max_entries);

    ReorderResponse {
        ok: true,
        action: "scan".to_string(),
        base_dir: base_dir.to_string_lossy().to_string(),
        text: format!("扫描完成，共输出 {} 行目录结构。", report.tree_lines.len()),
        plan_id: None,
        tree_lines: Some(report.tree_lines),
        duplicate_groups: Some(report.duplicate_groups),
        garbage_candidates: Some(report.garbage_candidates),
        dry_run: None,
        operations_preview: None,
        conflicts: None,
        warnings: None,
        log_path: None,
    }
}

fn build_validation_response(
    action: &str,
    base_dir: &Path,
    plan_id: Option<String>,
    steps: &[rules::PlannedStep],
    conflicts: Vec<String>,
    warnings: Vec<String>,
    dry_run: bool,
) -> ReorderResponse {
    ReorderResponse {
        ok: conflicts.is_empty(),
        action: action.to_string(),
        base_dir: base_dir.to_string_lossy().to_string(),
        text: if conflicts.is_empty() {
            if dry_run {
                format!("Dry-run 校验通过，可执行操作 {} 条。", steps.len())
            } else {
                format!("校验通过，可执行操作 {} 条。", steps.len())
            }
        } else {
            format!("检测到 {} 个冲突，已阻止执行。", conflicts.len())
        },
        plan_id,
        tree_lines: None,
        duplicate_groups: None,
        garbage_candidates: None,
        dry_run: Some(dry_run),
        operations_preview: Some(steps.iter().map(preview_step).collect()),
        conflicts: if conflicts.is_empty() {
            None
        } else {
            Some(conflicts)
        },
        warnings: if warnings.is_empty() {
            None
        } else {
            Some(warnings)
        },
        log_path: None,
    }
}

fn validate_steps(req: &ReorderRequest, base_dir: &Path) -> (Vec<rules::PlannedStep>, Vec<String>, Vec<String>) {
    let plan = req.plan.clone().unwrap_or(ReorderPlan {
        operations: Vec::new(),
        notes: Vec::new(),
        duplicates: Vec::new(),
        garbage_candidates: Vec::new(),
    });

    let roots = resolve_whitelist(base_dir, &req.whitelist);
    let validation = validate_plan(&plan, base_dir, &roots);
    (validation.steps, validation.conflicts, validation.warnings)
}

fn handle_dry_run_or_apply(req: &ReorderRequest, base_dir: &Path, apply: bool) -> ReorderResponse {
    if req.plan.is_none() {
        return error_response(
            if apply { "apply" } else { "dryRun" },
            base_dir,
            "缺少 plan 数据。".to_string(),
        );
    }

    let (steps, conflicts, warnings) = validate_steps(req, base_dir);

    if !conflicts.is_empty() {
        return build_validation_response(
            if apply { "apply" } else { "dryRun" },
            base_dir,
            req.plan_id.clone(),
            &steps,
            conflicts,
            warnings,
            !apply,
        );
    }

    if !apply {
        return build_validation_response("dryRun", base_dir, req.plan_id.clone(), &steps, Vec::new(), warnings, true);
    }

    let plan_id = sanitize_plan_id(req.plan_id.as_deref());
    match execute_steps(base_dir, &plan_id, &steps, false) {
        Ok(summary) => ReorderResponse {
            ok: true,
            action: "apply".to_string(),
            base_dir: base_dir.to_string_lossy().to_string(),
            text: format!("执行完成，共 {} 条操作。未删除任何文件。", summary.executed_count),
            plan_id: Some(plan_id),
            tree_lines: None,
            duplicate_groups: None,
            garbage_candidates: None,
            dry_run: Some(false),
            operations_preview: Some(steps.iter().map(preview_step).collect()),
            conflicts: None,
            warnings: if warnings.is_empty() {
                None
            } else {
                Some(warnings)
            },
            log_path: Some(summary.log_path),
        },
        Err(err) => error_response("apply", base_dir, err.to_string()),
    }
}

fn run_request(req: ReorderRequest) -> ReorderResponse {
    let base_dir = match resolve_base_dir(req.base_dir.as_deref()) {
        Ok(v) => v,
        Err(err) => {
            let base = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            return error_response("unknown", &base, err.to_string());
        }
    };

    let action = req.action.trim().to_lowercase();
    match action.as_str() {
        "scan" => handle_scan(&req, &base_dir),
        "dryrun" | "dry-run" => handle_dry_run_or_apply(&req, &base_dir, false),
        "apply" => handle_dry_run_or_apply(&req, &base_dir, true),
        "rollback" => error_response(
            "rollback",
            &base_dir,
            "ccrecorder 的 rollback 已下线，请使用 Git 快照回滚：/sort history -> /sort rollback <snapshotId>。".to_string(),
        ),
        _ => error_response(
            "unknown",
            &base_dir,
            format!("未知 action: {}，支持 scan/dryRun/apply。", req.action),
        ),
    }
}

fn print_cli_response(resp: &ReorderResponse) {
    let status = if resp.ok {
        "OK".green().bold()
    } else {
        "FAILED".red().bold()
    };

    println!("{} [{}] {}", status, resp.action, resp.text);
    println!("目录: {}", resp.base_dir);

    if let Some(preview) = &resp.operations_preview {
        if !preview.is_empty() {
            println!("预览操作(最多显示 20 条):");
            for line in preview.iter().take(20) {
                println!("  - {}", line);
            }
            if preview.len() > 20 {
                println!("  ... 其余 {} 条未显示", preview.len() - 20);
            }
        }
    }

    if let Some(conflicts) = &resp.conflicts {
        for item in conflicts {
            println!("冲突: {}", item.red());
        }
    }

    if let Some(warnings) = &resp.warnings {
        for item in warnings {
            println!("警告: {}", item.yellow());
        }
    }

    if let Some(log_path) = &resp.log_path {
        println!("日志: {}", log_path);
    }
}

fn run_scan_command(dir: Option<String>, max_entries: usize) -> ReorderResponse {
    let req = ReorderRequest {
        action: "scan".to_string(),
        base_dir: dir,
        max_entries: Some(max_entries),
        whitelist: Vec::new(),
        plan: None,
        plan_id: None,
    };
    run_request(req)
}

fn run_dry_run_command(
    dir: Option<String>,
    plan_file: String,
    plan_id: Option<String>,
    whitelist: Vec<String>,
) -> ReorderResponse {
    let plan = match parse_plan_file(&plan_file) {
        Ok(v) => v,
        Err(err) => {
            let base = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            return error_response("dryRun", &base, err);
        }
    };

    run_request(ReorderRequest {
        action: "dryRun".to_string(),
        base_dir: dir,
        max_entries: None,
        whitelist,
        plan: Some(plan),
        plan_id,
    })
}

fn run_apply_command(
    dir: Option<String>,
    plan_file: String,
    plan_id: Option<String>,
    whitelist: Vec<String>,
    yes: bool,
) -> ReorderResponse {
    let plan = match parse_plan_file(&plan_file) {
        Ok(v) => v,
        Err(err) => {
            let base = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            return error_response("apply", &base, err);
        }
    };

    let dry_req = ReorderRequest {
        action: "dryRun".to_string(),
        base_dir: dir.clone(),
        max_entries: None,
        whitelist: whitelist.clone(),
        plan: Some(plan.clone()),
        plan_id: plan_id.clone(),
    };

    let dry_resp = run_request(dry_req);
    if !dry_resp.ok {
        return ReorderResponse {
            action: "apply".to_string(),
            ..dry_resp
        };
    }

    let base_dir = match resolve_base_dir(dir.as_deref()) {
        Ok(v) => v,
        Err(err) => {
            let base = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            return error_response("apply", &base, err.to_string());
        }
    };

    let plan_id_for_prompt = sanitize_plan_id(plan_id.as_deref());
    let preview = dry_resp.operations_preview.clone().unwrap_or_default();

    if !yes && !interactive::confirm_apply(&plan_id_for_prompt, &base_dir, &preview) {
        return ReorderResponse {
            ok: false,
            action: "apply".to_string(),
            base_dir: base_dir.to_string_lossy().to_string(),
            text: "用户取消执行。".to_string(),
            plan_id: Some(plan_id_for_prompt),
            tree_lines: None,
            duplicate_groups: None,
            garbage_candidates: None,
            dry_run: Some(true),
            operations_preview: Some(preview),
            conflicts: None,
            warnings: dry_resp.warnings,
            log_path: None,
        };
    }

    let apply_req = ReorderRequest {
        action: "apply".to_string(),
        base_dir: dir,
        max_entries: None,
        whitelist,
        plan: Some(plan),
        plan_id: Some(plan_id_for_prompt),
    };
    run_request(apply_req)
}

pub fn run_from_stdin() -> ReorderResponse {
    let req = match read_request() {
        Ok(v) => v,
        Err(err) => {
            let base = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            return error_response("unknown", &base, err);
        }
    };

    run_request(req)
}

pub fn write_json_response(resp: &ReorderResponse) {
    match serde_json::to_string(resp) {
        Ok(text) => println!("{}", text),
        Err(err) => println!(
            "{{\"ok\":false,\"action\":\"serialize\",\"baseDir\":\"\",\"text\":\"输出序列化失败: {}\"}}",
            escape_json_string(&err.to_string())
        ),
    }
}

pub fn run_cli() -> i32 {
    let cli = Cli::parse();
    let stdin_mode = matches!(&cli.command, Commands::Stdin);

    let resp = match cli.command {
        Commands::Scan { dir, max_entries } => run_scan_command(dir, max_entries),
        Commands::DryRun {
            dir,
            plan_file,
            plan_id,
            whitelist,
        } => run_dry_run_command(dir, plan_file, plan_id, whitelist),
        Commands::Apply {
            dir,
            plan_file,
            plan_id,
            whitelist,
            yes,
        } => run_apply_command(dir, plan_file, plan_id, whitelist, yes),
        Commands::Stdin => run_from_stdin(),
    };

    if stdin_mode {
        write_json_response(&resp);
    } else {
        print_cli_response(&resp);
    }

    if resp.ok { 0 } else { 1 }
}
