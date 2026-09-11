//! Agent 命令执行用的 shell 探测与调用约定：shell 工具、cron shell
//! job 等「执行一段命令文本」的入口共用，全项目只此一处选择解释器。
//!
//! 选型原则：模型生成的命令几乎全是 bash/POSIX 方言，因此 bash 永远
//! 优先、逐档回退；不跟随用户 login shell（fish/nushell 会直接坏掉
//! 语法，且非交互 `-c` 不读 rc 文件，用户 shell 没有加成）。
//! `YOMI_SHELL` 环境变量可显式指定解释器路径（种类按文件名推断）。
//!
//! 回退链：
//! - unix：PATH bash → /bin/bash → PATH sh → /bin/sh；
//! - windows：Git Bash（排除 WSL 的 `System32\bash.exe`）→ pwsh →
//!   powershell → cmd.exe。
//!
//! PowerShell/cmd 的输出默认不是 UTF-8 代码页（cmd 通常是 GBK/936），
//! [`AgentShell::wrap_command`] 统一注入 UTF-8 输出前缀，调用方按
//! UTF-8 读字节即可。
//!
//! 探测结果进程级缓存：shell 不会在进程生命周期内变化。

use std::borrow::Cow;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// 平台标签：探测逻辑的入参（测试可注入另一平台的回退链），当前平台
/// 由 `cfg!` 编译期给出。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Platform {
    Unix,
    Windows,
}

/// shell 种类：决定调用参数与命令包装方式。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ShellKind {
    /// bash / sh / zsh 等 POSIX 方言：`-c <cmd>`。
    Posix,
    /// pwsh / powershell：`-NoProfile -NonInteractive -Command <cmd>`。
    PowerShell,
    /// cmd.exe：`/D /C <cmd>`。
    Cmd,
}

/// 探测到的 shell。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentShell {
    pub kind: ShellKind,
    pub path: PathBuf,
}

impl AgentShell {
    /// 执行一段命令文本的固定前导参数（命令文本由调用方追加在最后）。
    pub fn leading_args(&self) -> &'static [&'static str] {
        match self.kind {
            ShellKind::Posix => &["-c"],
            // NoProfile 跳过 profile 脚本（内容不可控且拖慢启动）；
            // NonInteractive 让需要输入的 cmdlet 直接报错而非挂起。
            ShellKind::PowerShell => &["-NoProfile", "-NonInteractive", "-Command"],
            // /D 跳过注册表 AutoRun（可能污染输出甚至直接失败）。
            ShellKind::Cmd => &["/D", "/C"],
        }
    }

    /// 包装命令文本：Windows 的两个解释器默认输出非 UTF-8 代码页，
    /// 统一注入 UTF-8 输出前缀；PowerShell 追加 `exit $LASTEXITCODE`
    /// ——`-Command` 不传播 native 命令的退出码（`git push` 失败
    /// PowerShell 仍退出 0），必须显式 exit。exit 另起一行而非 `;`
    /// 连接：命令末行若以 `#` 注释结尾，同行追加的 exit 会被注释
    /// 吞掉，失败命令被误报成功；`$LASTEXITCODE` 跨语句保持，换行
    /// 不影响取值。POSIX 原样返回。
    pub fn wrap_command<'a>(&self, command: &'a str) -> Cow<'a, str> {
        match self.kind {
            ShellKind::Posix => Cow::Borrowed(command),
            ShellKind::PowerShell => Cow::Owned(format!(
                "[Console]::OutputEncoding=[System.Text.Encoding]::UTF8; {command}\nexit $LASTEXITCODE"
            )),
            ShellKind::Cmd => Cow::Owned(format!("chcp 65001 >nul & {command}")),
        }
    }
}

/// 探测当前平台的 shell，进程级缓存。
pub fn detect() -> &'static AgentShell {
    static SHELL: OnceLock<AgentShell> = OnceLock::new();
    SHELL.get_or_init(|| {
        let platform = if cfg!(windows) {
            Platform::Windows
        } else {
            Platform::Unix
        };
        let shell = detect_impl(
            platform,
            std::env::var_os(crate::utils::env::YOMI_SHELL),
            &|key| std::env::var_os(key),
            &|path| path.is_file(),
        );
        tracing::debug!(shell = %shell.path.display(), kind = ?shell.kind, "detected agent shell");
        shell
    })
}

/// 按解释器文件名推断种类；认不出的按 POSIX 方言处理。
/// 文件名提取用字符串操作而非 `Path::file_stem`：Windows 路径拿到
/// unix 主机上（测试注入）也要能正确解析。
fn kind_from_name(path: &Path) -> ShellKind {
    let name = path.to_string_lossy();
    let base = name.rsplit(['/', '\\']).next().unwrap_or(&name);
    let stem = base
        .rsplit_once('.')
        .map_or(base, |(stem, _)| stem)
        .to_ascii_lowercase();
    match stem.as_str() {
        "pwsh" | "powershell" => ShellKind::PowerShell,
        "cmd" => ShellKind::Cmd,
        _ => ShellKind::Posix,
    }
}

/// 路径前缀判断：Windows 大小写不敏感，unix 敏感。
fn starts_with_path(path: &str, prefix: &str, platform: Platform) -> bool {
    match platform {
        Platform::Unix => path.starts_with(prefix),
        // get() 而非直接切片：PATH 目录可能含非 ASCII 字符（如中文
        // Windows 的用户目录），字节切片落在字符边界中间会 panic。
        Platform::Windows => path
            .get(..prefix.len())
            .is_some_and(|head| head.eq_ignore_ascii_case(prefix)),
    }
}

/// 探测逻辑本体：环境变量与文件存在性全部经闭包注入，测试不碰真
/// 文件系统即可覆盖两个平台的完整回退链。
fn detect_impl(
    platform: Platform,
    override_path: Option<OsString>,
    get_env: &dyn Fn(&str) -> Option<OsString>,
    is_file: &dyn Fn(&Path) -> bool,
) -> AgentShell {
    if let Some(path) = override_path.filter(|p| !p.is_empty()) {
        let path = PathBuf::from(path);
        return AgentShell {
            kind: kind_from_name(&path),
            path,
        };
    }

    let (sep, path_delim) = match platform {
        Platform::Unix => ('/', ':'),
        Platform::Windows => ('\\', ';'),
    };
    let env_string = |key: &str| get_env(key).map(|v| v.to_string_lossy().into_owned());
    // 路径操作全部走字符串（分隔符按平台取）而非 `Path` 方法：Windows
    // 路径在 unix 主机上（测试注入）也要能正确拼拆。
    let path_dirs: Vec<String> = env_string("PATH")
        .map(|p| {
            p.split(path_delim)
                .filter(|d| !d.is_empty())
                .map(|d| d.trim_end_matches(['/', '\\']))
                .map(String::from)
                .collect()
        })
        .unwrap_or_default();
    // 在 PATH 里找第一个存在的候选；`skip_under` 用于排除特定目录
    // （Windows 上排除 System32，见下）。
    let in_path = |names: &[&str], skip_under: Option<&str>| -> Option<PathBuf> {
        path_dirs
            .iter()
            .flat_map(|d| names.iter().map(move |n| format!("{d}{sep}{n}")))
            .find(|candidate| {
                skip_under.is_none_or(|skip| !starts_with_path(candidate, skip, platform))
                    && is_file(Path::new(candidate))
            })
            .map(PathBuf::from)
    };
    let known = |paths: &[&str]| -> Option<PathBuf> {
        paths.iter().map(PathBuf::from).find(|p| is_file(p))
    };
    let shell = |kind: ShellKind, path: PathBuf| AgentShell { kind, path };

    match platform {
        Platform::Unix => {
            let path = in_path(&["bash"], None)
                .or_else(|| known(&["/bin/bash", "/usr/bin/bash", "/usr/local/bin/bash"]))
                .or_else(|| in_path(&["sh"], None))
                .or_else(|| known(&["/bin/sh"]));
            match path {
                Some(p) => shell(ShellKind::Posix, p),
                // 兜底一个总会让 spawn 报出清晰错误的路径。
                None => shell(ShellKind::Posix, PathBuf::from("/bin/sh")),
            }
        }
        Platform::Windows => {
            let env_path = |key: &str, rest: &str| {
                env_string(key)
                    .map(|base| format!("{}{sep}{rest}", base.trim_end_matches(['/', '\\'])))
            };
            let system32 = env_path("SystemRoot", "System32");
            // Git Bash 优先。PATH 里的 `System32\bash.exe` 是 WSL 启动器，
            // 用它命令会跑进 WSL，文件系统视图是错的，必须排除。
            if let Some(p) = in_path(&["bash.exe"], system32.as_deref()) {
                return shell(ShellKind::Posix, p);
            }
            for candidate in [
                env_path("ProgramFiles", r"Git\bin\bash.exe"),
                env_path("ProgramFiles(x86)", r"Git\bin\bash.exe"),
            ]
            .into_iter()
            .flatten()
            {
                if is_file(Path::new(&candidate)) {
                    return shell(ShellKind::Posix, PathBuf::from(candidate));
                }
            }
            if let Some(p) = in_path(&["pwsh.exe"], None).or_else(|| {
                env_path("ProgramFiles", r"PowerShell\7\pwsh.exe")
                    .filter(|p| is_file(Path::new(p)))
                    .map(PathBuf::from)
            }) {
                return shell(ShellKind::PowerShell, p);
            }
            if let Some(p) = in_path(&["powershell.exe"], None).or_else(|| {
                env_path(
                    "SystemRoot",
                    r"System32\WindowsPowerShell\v1.0\powershell.exe",
                )
                .filter(|p| is_file(Path::new(p)))
                .map(PathBuf::from)
            }) {
                return shell(ShellKind::PowerShell, p);
            }
            if let Some(p) = env_path("SystemRoot", r"System32\cmd.exe")
                .filter(|p| is_file(Path::new(p)))
                .map(PathBuf::from)
                .or_else(|| in_path(&["cmd.exe"], None))
            {
                return shell(ShellKind::Cmd, p);
            }
            // 兜底裸名：CreateProcess 的默认搜索路径含 System32。
            shell(ShellKind::Cmd, PathBuf::from("cmd.exe"))
        }
    }
}

#[cfg(test)]
#[path = "shell_test.rs"]
mod tests;
