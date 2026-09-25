//! Path utilities for the kernel crate

use std::path::PathBuf;
use std::sync::LazyLock;

/// Default data directory path
pub const DEFAULT_DATA_DIR: &str = "~/.yomi";

static HOME_DIR: LazyLock<Option<PathBuf>> = LazyLock::new(|| {
    directories::BaseDirs::new()
        .map(|b| b.home_dir().to_path_buf())
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
        .or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from))
});

/// Expand `~` to the user's home directory
pub fn expand_tilde(path: impl AsRef<str>) -> PathBuf {
    let path = path.as_ref();
    if let Some(stripped) = path.strip_prefix("~/") {
        if let Some(ref home) = *HOME_DIR {
            // 逐段 join：整体 join 含 `/` 的串会在 Windows 上得到
            // `C:\home\.agents/skills` 这类混合分隔符路径。
            return stripped
                .split('/')
                .fold((*home).clone(), |p, seg| p.join(seg));
        }
    }
    PathBuf::from(path)
}

/// sidecar CLI（GUI bundle `externalBin` 打进包的 yomi，或 zip 分发
/// 时与主二进制并列的 yomi）与当前 exe 同目录——把该目录 prepend
/// 进 PATH，daemon 子树（agent 的 shell 工具、hook、cron shell
/// job）即可直接调用与宿主严格同版的 CLI，无需依赖外部安装。只
/// 改本进程环境（子进程继承），不影响系统其他进程；同目录 CLI
/// 在子树内遮蔽系统里的其他 yomi 是有意的同版保证。
///
/// 宿主两侧都调：GUI（in-process daemon 共享本进程 env，时序在
/// logging 之后、spawn 任何子进程之前）与 CLI daemon 入口
/// （init_logging 之后）。dev 模式 `current_exe` 在
/// target/{debug,release}，同目录的 cargo 构建产物里通常也有
/// yomi，行为一致。
pub fn prepend_exe_dir_to_path() {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let Some(dir) = exe.parent() else {
        return;
    };
    let Ok(path) = std::env::var("PATH") else {
        return;
    };
    let new_path = prepend_path_dir(&path, dir);
    if new_path == path {
        tracing::debug!(dir = %dir.display(), "exe dir already on PATH");
    } else {
        // 日志行是 e2e 与排障的取证点：daemon 子树的 agent shell 能
        // 用哪个 yomi，看这行就知道。
        tracing::info!(dir = %dir.display(), "prepended exe dir to PATH");
    }
    std::env::set_var("PATH", new_path);
}

/// 把 `dir` 放到 PATH 最前；已存在则原样返回（幂等）。
pub fn prepend_path_dir(path: &str, dir: &std::path::Path) -> String {
    let sep = if cfg!(windows) { ';' } else { ':' };
    if path.split(sep).any(|p| p == dir.as_os_str()) {
        return path.to_string();
    }
    format!("{}{sep}{}", dir.display(), path)
}

/// Generate default skill folders based on `data_dir`, ordered by
/// precedence (low → high): the data-dir layer wins on name collision.
/// (Project-level skills are appended after these per session, making them
/// the highest-precedence layer.)
pub fn default_skill_folders(data_dir: &std::path::Path) -> Vec<PathBuf> {
    vec![expand_tilde("~/.agents/skills"), data_dir.join("skills")]
}

/// Session workspace cwd rule: the session's `working_dir` when set, else
/// `<data_dir>/workspace`. Subagent spawn (`conductor`, `subagent` tool) and
/// workspace-layer asset resolution (agent templates) must all agree on
/// this rule.
pub fn session_workspace_dir(data_dir: &std::path::Path, working_dir: Option<PathBuf>) -> PathBuf {
    working_dir.unwrap_or_else(|| data_dir.join("workspace"))
}

#[cfg(test)]
#[path = "path_test.rs"]
mod tests;
