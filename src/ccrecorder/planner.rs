use super::rules::{
    is_path_allowed, is_within_root, normalize_key, resolve_candidate_path, trim_non_empty, PlannedStep,
    ReorderPlan,
};
use std::path::{Path, PathBuf};

#[derive(Debug, Default)]
pub struct PlanValidation {
    pub steps: Vec<PlannedStep>,
    pub conflicts: Vec<String>,
    pub warnings: Vec<String>,
}

pub fn validate_plan(plan: &ReorderPlan, base_dir: &Path, roots: &[PathBuf]) -> PlanValidation {
    let mut result = PlanValidation::default();

    for (index, op) in plan.operations.iter().enumerate() {
        let index_display = index + 1;
        let op_type = op.op_type.trim().to_lowercase();
        let reason = op.reason.clone().unwrap_or_default();

        if op_type.contains("delete") || op_type.contains("remove") {
            result
                .conflicts
                .push(format!("操作 #{} 含删除语义，已禁止。", index_display));
            continue;
        }

        match op_type.as_str() {
            "mkdir" | "create_dir" => {
                let raw_target = trim_non_empty(op.path.as_deref())
                    .or_else(|| trim_non_empty(op.to.as_deref()));

                let Some(target) = raw_target else {
                    result
                        .conflicts
                        .push(format!("操作 #{} 缺少 path/to。", index_display));
                    continue;
                };

                let to_path = resolve_candidate_path(base_dir, &target);
                if !is_path_allowed(&to_path, roots) {
                    result.conflicts.push(format!(
                        "操作 #{} 越权路径: {}",
                        index_display,
                        to_path.to_string_lossy()
                    ));
                    continue;
                }

                if to_path.exists() && !to_path.is_dir() {
                    result.conflicts.push(format!(
                        "操作 #{} 目标已存在且非目录: {}",
                        index_display,
                        to_path.to_string_lossy()
                    ));
                    continue;
                }

                result.steps.push(PlannedStep::Mkdir {
                    to: to_path,
                    reason,
                });
            }
            "move" | "rename" => {
                let from_raw = trim_non_empty(op.from.as_deref());
                let to_raw = trim_non_empty(op.to.as_deref());
                let (Some(from), Some(to)) = (from_raw, to_raw) else {
                    result
                        .conflicts
                        .push(format!("操作 #{} 缺少 from/to。", index_display));
                    continue;
                };

                let from_path = resolve_candidate_path(base_dir, &from);
                let to_path = resolve_candidate_path(base_dir, &to);

                if !is_path_allowed(&from_path, roots) || !is_path_allowed(&to_path, roots) {
                    result.conflicts.push(format!(
                        "操作 #{} 存在越权路径: from={}, to={}",
                        index_display,
                        from_path.to_string_lossy(),
                        to_path.to_string_lossy()
                    ));
                    continue;
                }

                if !from_path.exists() {
                    result.conflicts.push(format!(
                        "操作 #{} 源路径不存在: {}",
                        index_display,
                        from_path.to_string_lossy()
                    ));
                    continue;
                }

                if to_path.exists() && normalize_key(&to_path) != normalize_key(&from_path) {
                    result.conflicts.push(format!(
                        "操作 #{} 目标路径已存在(冲突): {}",
                        index_display,
                        to_path.to_string_lossy()
                    ));
                    continue;
                }

                if from_path.is_dir() && is_within_root(&to_path, &from_path) {
                    result.conflicts.push(format!(
                        "操作 #{} 禁止把目录移动到其子目录内: from={}, to={}",
                        index_display,
                        from_path.to_string_lossy(),
                        to_path.to_string_lossy()
                    ));
                    continue;
                }

                if normalize_key(&to_path) == normalize_key(&from_path) {
                    result
                        .warnings
                        .push(format!("操作 #{} 源目标一致，已视为无操作。", index_display));
                    continue;
                }

                result.steps.push(PlannedStep::Move {
                    from: from_path,
                    to: to_path,
                    reason,
                });
            }
            _ => {
                result
                    .conflicts
                    .push(format!("操作 #{} 类型不支持: {}", index_display, op.op_type));
            }
        }
    }

    result
}
