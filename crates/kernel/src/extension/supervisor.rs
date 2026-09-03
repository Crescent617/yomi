//! supervisor：supervised 扩展进程的生命周期——列出即拉起（boot
//! spawn）、daemon 死则组杀（shutdown token）、运行中崩溃固定 5s 退避
//! 重拉（无策略 knob）。进程组管理与 background shell 同语义
//! （`setsid` + 组杀），日志落 `<data_dir>/logs/ext-<name>.log`。

use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::config::ExtensionConfig;

/// 崩溃重拉的固定退避。
const RESPAWN_BACKOFF: std::time::Duration = std::time::Duration::from_secs(5);

/// 为全部配置的扩展启动 supervisor（每个扩展一个守护任务，
/// 随 daemon shutdown 收尾）。
///
/// `needless_pass_by_value` 不适用：所有权必须转进 spawned 的
/// `'static` 守护任务。
#[allow(clippy::needless_pass_by_value)]
pub fn start_supervisor(
    extensions: Vec<ExtensionConfig>,
    log_dir: std::path::PathBuf,
    shutdown: CancellationToken,
) {
    for ext in extensions {
        if ext.command.is_empty() {
            warn!(name = %ext.name, "extension has empty command, skipping");
            continue;
        }
        let token = shutdown.clone();
        let log_dir = log_dir.clone();
        tokio::spawn(async move { supervise(ext, log_dir, token).await });
    }
}

async fn supervise(ext: ExtensionConfig, log_dir: std::path::PathBuf, shutdown: CancellationToken) {
    loop {
        let mut cmd = tokio::process::Command::new(&ext.command[0]);
        cmd.args(&ext.command[1..]);
        cmd.stdin(std::process::Stdio::null());
        // 日志：每个扩展独立文件（append），便于排查注册/派单问题。
        let log_path = log_dir.join(format!("ext-{}.log", ext.name));
        match open_append(&log_path).await {
            Ok(file) => {
                let file = file.into_std().await;
                if let Ok(err) = file.try_clone() {
                    cmd.stdout(std::process::Stdio::from(file));
                    cmd.stderr(std::process::Stdio::from(err));
                }
            }
            Err(e) => {
                warn!(name = %ext.name, error = %e, "extension log open failed, output discarded");
                cmd.stdout(std::process::Stdio::null());
                cmd.stderr(std::process::Stdio::null());
            }
        }
        // 子进程自成进程组（pid 即 pgid），daemon 死时组杀能连带其子孙
        // （与 background shell 同语义）。
        crate::utils::process::pre_exec_new_session(&mut cmd);

        let child = cmd.spawn();
        let mut child = match child {
            Ok(c) => c,
            Err(e) => {
                warn!(name = %ext.name, error = %e, cmd = ?ext.command, "extension spawn failed");
                if backoff_or_shutdown(&shutdown).await {
                    return;
                }
                continue;
            }
        };
        let pid = child.id().unwrap_or(0);
        info!(name = %ext.name, pid, "extension spawned");

        tokio::select! {
            status = child.wait() => {
                match status {
                    Ok(s) => warn!(name = %ext.name, status = %s, "extension exited, respawning"),
                    Err(e) => warn!(name = %ext.name, error = %e, "extension wait failed, respawning"),
                }
                if backoff_or_shutdown(&shutdown).await {
                    return;
                }
            }
            () = shutdown.cancelled() => {
                // daemon 死则组杀（pid 即 pgid）。
                kill_group(pid, &ext.name).await;
                let _ = child.wait().await;
                return;
            }
        }
    }
}

/// true = shutdown 已触发，调用方应退出循环。
async fn backoff_or_shutdown(shutdown: &CancellationToken) -> bool {
    tokio::select! {
        () = tokio::time::sleep(RESPAWN_BACKOFF) => false,
        () = shutdown.cancelled() => true,
    }
}

async fn kill_group(pid: u32, name: &str) {
    if pid == 0 {
        return;
    }
    match tokio::process::Command::new("kill")
        .args(["-TERM", "--", &format!("-{pid}")])
        .status()
        .await
    {
        Ok(_) => info!(name = %name, pid, "extension process group terminated"),
        Err(e) => warn!(name = %name, pid, error = %e, "extension group kill failed"),
    }
}

async fn open_append(path: &std::path::Path) -> std::io::Result<tokio::fs::File> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .await
}
