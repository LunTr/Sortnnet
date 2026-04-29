use super::rules::{now_compact_label, PlannedStep, LOG_DIR_NAME};
use anyhow::{Context, Result};
use indicatif::{ProgressBar, ProgressStyle};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct ExecuteSummary {
    pub log_path: String,
    pub executed_count: usize,
}

fn open_log_files(base_dir: &Path, plan_id: &str) -> Result<(PathBuf, fs::File)> {
    let logs_dir = base_dir.join(LOG_DIR_NAME);
    fs::create_dir_all(&logs_dir).context("创建日志目录失败")?;

    let ts = now_compact_label();
    let log_path = logs_dir.join(format!("reorder-{}-{}.log", plan_id, ts));

    let log_file = fs::File::create(&log_path).context("创建日志文件失败")?;
    Ok((log_path, log_file))
}

fn write_log_line(file: &mut fs::File, content: &str) {
    let _ = file.write_all(content.as_bytes());
    let _ = file.write_all(b"\n");
}

fn build_progress(steps_len: usize, show_progress: bool) -> Option<ProgressBar> {
    if !show_progress {
        return None;
    }

    let pb = ProgressBar::new(steps_len as u64);
    if let Ok(style) = ProgressStyle::with_template("{bar:40.cyan/blue} {pos}/{len} {msg}") {
        pb.set_style(style.progress_chars("##-"));
    }

    Some(pb)
}

pub fn execute_steps(
    base_dir: &Path,
    plan_id: &str,
    steps: &[PlannedStep],
    show_progress: bool,
) -> Result<ExecuteSummary> {
    let (log_path, mut log_file) = open_log_files(base_dir, plan_id)?;
    let progress = build_progress(steps.len(), show_progress);

    for step in steps {
        match step {
            PlannedStep::Mkdir { to, .. } => {
                if let Err(err) = fs::create_dir_all(to) {
                    write_log_line(
                        &mut log_file,
                        &format!("ERROR mkdir {}: {}", to.to_string_lossy(), err),
                    );
                    anyhow::bail!("执行 mkdir 失败: {}", err);
                }

                write_log_line(&mut log_file, &format!("mkdir {}", to.to_string_lossy()));
                if let Some(pb) = &progress {
                    pb.inc(1);
                }
            }
            PlannedStep::Move { from, to, .. } => {
                if let Some(parent) = to.parent() {
                    if let Err(err) = fs::create_dir_all(parent) {
                        write_log_line(
                            &mut log_file,
                            &format!("ERROR create_parent {}: {}", parent.to_string_lossy(), err),
                        );
                        anyhow::bail!("创建目标父目录失败: {}", err);
                    }
                }

                if let Err(err) = fs::rename(from, to) {
                    write_log_line(
                        &mut log_file,
                        &format!(
                            "ERROR move {} -> {}: {}",
                            from.to_string_lossy(),
                            to.to_string_lossy(),
                            err
                        ),
                    );
                    anyhow::bail!("执行 move/rename 失败: {}", err);
                }

                write_log_line(
                    &mut log_file,
                    &format!("move {} -> {}", from.to_string_lossy(), to.to_string_lossy()),
                );

                if let Some(pb) = &progress {
                    pb.inc(1);
                }
            }
        }
    }

    if let Some(pb) = &progress {
        pb.finish_with_message("执行完成");
    }

    Ok(ExecuteSummary {
        log_path: log_path.to_string_lossy().to_string(),
        executed_count: steps.len(),
    })
}
