//! Environment variable utilities for the kernel crate

/// 子进程注入的标准环境变量名（编译期拼出 `"YOMI_..."`）。shell 工具、
/// `/workflow run`、cron shell job 的子进程都带这些变量，脚本据此
/// 回连 yomi（如 `"$YOMI_DATA_DIR/workflows/..."`、`yomi session send
/// --steer -s "$YOMI_SESSION_ID"`）。
pub const YOMI_SESSION_ID: &str = crate::env_name!("SESSION_ID");
pub const YOMI_DATA_DIR: &str = crate::env_name!("DATA_DIR");
/// 外挂（hook/tool）的持久状态目录：hook 为
/// `<data_dir>/state/hooks/<point>/<脚本名>/`，tool 为
/// `<data_dir>/state/tools/<名>/`，daemon 惰性创建，脚本自管内容
/// （去重水位、缓存、留档）。
pub const YOMI_STATE_DIR: &str = crate::env_name!("STATE_DIR");
/// 外挂子进程注入的事件标识（值：hook=hook point 名，tool="tool"）。
pub const YOMI_EVENT: &str = crate::env_name!("EVENT");

/// 给 tokio `Command` 注入 yomi 标准环境变量，返回 `&mut` 便于链式。
/// `None` 的项会被**显式移除**而非保留继承值：父进程自身可能带着这些
/// 变量（daemon 从 shell 工具里被拉起、测试跑在 yomi 会话内），不主动
/// 清掉会让子进程拿到指向错误会话/目录的残留值。
///
/// 各调用点按手头上下文传参：shell 工具总有 session、`data_dir` 视构造
/// 而定；workflow run 必有 `data_dir`、session 视会话；cron shell 只有
/// `data_dir`。
pub fn inject_child_env<'a>(
    cmd: &'a mut tokio::process::Command,
    data_dir: Option<&std::path::Path>,
    session_id: Option<&str>,
) -> &'a mut tokio::process::Command {
    match data_dir {
        Some(dir) => {
            cmd.env(YOMI_DATA_DIR, dir);
        }
        None => {
            cmd.env_remove(YOMI_DATA_DIR);
        }
    }
    match session_id {
        Some(sid) => {
            cmd.env(YOMI_SESSION_ID, sid);
        }
        None => {
            cmd.env_remove(YOMI_SESSION_ID);
        }
    }
    cmd
}

/// 注入 `YOMI_STATE_DIR`（`None` 时显式移除，同 `inject_child_env` 的
/// 防残留语义）。与 `inject_child_env` 分开：state 目录只对有名字的外挂
/// （hook 文件名 / tool 目录名）有意义，shell/cron/workflow 不注入。
pub fn inject_state_dir<'a>(
    cmd: &'a mut tokio::process::Command,
    state_dir: Option<&std::path::Path>,
) -> &'a mut tokio::process::Command {
    match state_dir {
        Some(dir) => {
            cmd.env(YOMI_STATE_DIR, dir);
        }
        None => {
            cmd.env_remove(YOMI_STATE_DIR);
        }
    }
    cmd
}

/// state 目录惰性创建：失败仅 debug 不致命——脚本可自行 mkdir。
/// hook（state/hooks/<point>/<脚本名>）/tool（state/tools/<名>）共用。
pub async fn ensure_state_dir(kind: &'static str, name: &str, dir: &std::path::Path) {
    if let Err(e) = tokio::fs::create_dir_all(dir).await {
        tracing::debug!(kind, name, dir = %dir.display(), error = %e, "state dir create failed");
    }
}

/// Get environment variable - inlined for performance
#[inline]
pub fn env_var(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

/// Try multiple env vars in order, return first set value
#[inline]
pub fn env_first(names: &[&str]) -> Option<String> {
    names.iter().find_map(|name| env_var(name))
}

/// Parse environment variable as a specific type
#[inline]
pub fn env_parse<T: std::str::FromStr>(name: &str) -> Option<T> {
    env_var(name).and_then(|s| s.parse().ok())
}

/// Parse boolean from environment variable
#[inline]
pub fn env_bool(name: &str) -> bool {
    std::env::var(name).is_ok_and(|s| {
        matches!(
            s.as_bytes(),
            b"true" | b"1" | b"yes" | b"TRUE" | b"YES" | b"on"
        )
    })
}

/// Parse optional boolean from environment variable
#[inline]
pub fn env_bool_opt(name: &str) -> Option<bool> {
    std::env::var(name).ok().map(|s| {
        matches!(
            s.as_bytes(),
            b"true" | b"1" | b"yes" | b"TRUE" | b"YES" | b"on"
        )
    })
}

/// Parse number with unit suffix (k/m) from string
/// Supports formats like "131072", "128k", "200k", "1m"
pub fn parse_number_with_unit(s: &str) -> Option<u32> {
    let s = s.trim().to_lowercase();

    // Check for 'k' suffix (thousands)
    if let Some(num_str) = s.strip_suffix('k') {
        let num: f32 = num_str.parse().ok()?;
        return Some((num * 1000.0) as u32);
    }

    // Check for 'm' suffix (millions)
    if let Some(num_str) = s.strip_suffix('m') {
        let num: f32 = num_str.parse().ok()?;
        return Some((num * 1_000_000.0) as u32);
    }

    // Plain number
    s.parse().ok()
}

#[cfg(test)]
#[path = "env_test.rs"]
mod tests;
