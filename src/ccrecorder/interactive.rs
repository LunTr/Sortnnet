use colored::Colorize;
use dialoguer::Confirm;
use std::path::Path;

pub fn confirm_apply(plan_id: &str, base_dir: &Path, operations_preview: &[String]) -> bool {
    println!("{}", "即将执行文件整理计划".bold().cyan());
    println!("计划 ID: {}", plan_id.yellow());
    println!("目录: {}", base_dir.to_string_lossy().green());
    println!("操作数: {}", operations_preview.len().to_string().yellow());

    let preview_limit = operations_preview.len().min(12);
    if preview_limit > 0 {
        println!("{}", "预览（最多 12 条）:".bold());
        for line in operations_preview.iter().take(preview_limit) {
            println!("  - {}", line);
        }
        if operations_preview.len() > preview_limit {
            println!("  ... 其余 {} 条未显示", operations_preview.len() - preview_limit);
        }
    }

    Confirm::new()
        .with_prompt("确认执行以上操作吗？")
        .default(false)
        .interact()
        .unwrap_or(false)
}
