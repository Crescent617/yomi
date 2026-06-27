use std::path::Path;
use std::time::{Duration, SystemTime};

use crate::config::Config;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

/// 删除指定目录下超过 `days` 天的、文件名以 `prefix` 开头的 `.log` 文件。
///
/// 这是一个轻量同步操作，适合在日志初始化之前调用，避免清理到刚打开的文件。
/// 所有 IO 错误都被静默忽略，避免启动时因日志清理失败而阻塞。
pub fn cleanup_old_logs(log_dir: &Path, prefix: &str, days: u64) {
    let max_age = Duration::from_secs(days * 24 * 60 * 60);
    let now = SystemTime::now();

    let Ok(entries) = std::fs::read_dir(log_dir) else {
        return;
    };

    let prefix_dot = format!("{}.", prefix);

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("log") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if stem != prefix && !stem.starts_with(&prefix_dot) {
            continue;
        }
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        let Ok(modified) = meta.modified() else {
            continue;
        };
        let Ok(age) = now.duration_since(modified) else {
            continue;
        };

        if age > max_age {
            let _ = std::fs::remove_file(&path);
        }
    }
}

/// Initialize daily-rotating file logging and optional console output.
///
/// - `config`: used to resolve `log_dir`
/// - `prefix`: log file name prefix (e.g. `"gui"` or `"daemon"`)
/// - `console`: when `true` also log to **stderr**, otherwise file-only
///
/// On success returns `Some(guard)` which must be kept alive for the duration
/// of the process. Returns `None` if the tracing registry has already been
/// initialized (e.g. in tests). Returns `Err` when the log directory cannot
/// be created or the rolling appender fails to build.
pub fn init_logging(
    config: &Config,
    prefix: &str,
    console: bool,
) -> crate::Result<Option<tracing_appender::non_blocking::WorkerGuard>> {
    let log_dir = config.log_dir();

    std::fs::create_dir_all(&log_dir).map_err(|e| {
        crate::KernelError::Io(format!(
            "Failed to create log directory '{}': {e}",
            log_dir.display()
        ))
    })?;

    // Clean up old logs *before* opening the new appender so we don't
    // delete the file we just started writing to.
    cleanup_old_logs(&log_dir, prefix, 7);

    let file_appender = tracing_appender::rolling::Builder::new()
        .rotation(tracing_appender::rolling::Rotation::DAILY)
        .filename_prefix(prefix)
        .filename_suffix("log")
        .build(&log_dir)
        .map_err(|e| {
            crate::KernelError::Io(format!(
                "Failed to create rolling file appender in '{}': {e}",
                log_dir.display()
            ))
        })?;

    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);

    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info"));

    let file_layer = tracing_subscriber::fmt::layer()
        .with_writer(non_blocking)
        .with_ansi(false)
        .with_target(true)
        .with_thread_ids(true);

    let console_layer = console.then(|| {
        tracing_subscriber::fmt::layer()
            .with_writer(std::io::stderr)
            .with_ansi(true)
            .with_target(true)
            .with_thread_ids(true)
    });

    if tracing_subscriber::registry()
        .with(env_filter)
        .with(file_layer)
        .with(console_layer)
        .try_init()
        .is_err()
    {
        drop(guard);
        return Ok(None);
    }
    tracing::info!("Logging initialized. Log directory: {}", log_dir.display());
    Ok(Some(guard))
}
