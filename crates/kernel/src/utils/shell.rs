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
            &probe_shell,
        );
        tracing::debug!(shell = %shell.path.display(), kind = ?shell.kind, "detected agent shell");
        shell
    })
}

/// 候选 shell 的实战同型探针：用它自己的参数与包装执行一条带内嵌
/// 引号的 echo——「存在」不代表「能跑」：Scoop busybox 的 bash shim
/// 之类冒名者对嵌套引号命令会创建进程失败（2026-09-11 Windows
/// 实测）。同步阻塞执行（结果由 OnceLock 缓存，进程生命周期只探
/// 一轮），单候选 5s 超时防 shim 挂起。
#[cfg(windows)]
fn probe_shell(shell: &AgentShell) -> bool {
    use std::io::Read;
    const MAGIC: &str = "yomi shell probe ok";
    let mut cmd = std::process::Command::new(&shell.path);
    cmd.args(shell.leading_args());
    let wrapped = shell.wrap_command(&format!("echo \"{MAGIC}\""));
    // cmd 的 /C 串与 tools/shell 同因 raw_arg 直传；其余走 std 转义。
    if shell.kind == ShellKind::Cmd {
        use std::os::windows::process::CommandExt;
        cmd.raw_arg(wrapped.as_ref());
    } else {
        cmd.arg(wrapped.as_ref());
    }
    let Ok(mut child) = cmd
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
    else {
        return false;
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    return false;
                }
                // 单次 read：try_wait 已确认子进程退出，其输出（探针
                // echo 约 20 字节）已全部落入管道缓冲，一次 read 即返
                // 不等 EOF——read_to_string/take 都要等写端关闭，
                // shim 若 spawn 了继承 stdout 的长寿孙进程，EOF 永不
                // 至，探测会被无限挂起（2026-09-11 评审实测）。
                let mut buf = [0u8; 256];
                let Some(mut stdout) = child.stdout.take() else {
                    return false;
                };
                let n = match stdout.read(&mut buf) {
                    Ok(n) => n,
                    Err(_) => return false,
                };
                let out = String::from_utf8_lossy(&buf[..n]);
                // 精确判定而非 contains：引号保真是该探针的核心属性
                // ——能执行但篡改引号的 shim（输出残留 \" 或剥掉引号）
                // 必须判失败。cmd echo 原样回显引号属正确行为。
                let trimmed = out.trim_start_matches('\u{feff}').trim();
                return match shell.kind {
                    ShellKind::Cmd => trimmed == format!("\"{MAGIC}\""),
                    _ => trimmed == MAGIC,
                };
            }
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            _ => {
                let _ = child.kill();
                return false;
            }
        }
    }
}

/// unix 候选全是系统组件（/bin/bash、sh），无 shim 冒名场景，不做
/// 能力探针（探测零成本）。
#[cfg(not(windows))]
fn probe_shell(_shell: &AgentShell) -> bool {
    true
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

/// 探测逻辑本体：环境变量、文件存在性与候选能力探针全部经闭包注
/// 入，测试不碰真文件系统/进程即可覆盖两个平台的完整回退链。
///
/// `probe` 是实战同型能力验证（用候选自己的参数与包装执行一条带
/// 内嵌引号的 echo）：Windows 上「文件存在」不够——Scoop busybox
/// 的 bash shim 之类冒名者对嵌套引号命令会创建进程失败（2026-09-11
/// 实测）；unix 候选全是系统组件，分支内不调用。
fn detect_impl(
    platform: Platform,
    override_path: Option<OsString>,
    get_env: &dyn Fn(&str) -> Option<OsString>,
    is_file: &dyn Fn(&Path) -> bool,
    probe: &dyn Fn(&AgentShell) -> bool,
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
            // 候选逐个过能力探针：PATH 里的 `System32\bash.exe` 是 WSL
            // 启动器（用它命令会跑进 WSL，文件系统视图是错的，必须排
            // 除）；PATH 里的 shim 冒名 bash（Scoop busybox 等）对实战
            // 同型的嵌套引号命令会创建进程失败——存在性不够，probe
            // 通过才算数。
            //
            // 探针总预算 15s（单候选 5s）：PATH 里挂起 shim 的数量无
            // 上限，预算耗尽后按「全灭」处理落 cmd 兜底，探测不再拖住
            // 首次调用。
            let probe_deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
            let probe = |s: &AgentShell| std::time::Instant::now() < probe_deadline && probe(s);
            let find_working = |kind: ShellKind, candidates: Vec<PathBuf>| -> Option<AgentShell> {
                // 保序去重（同一 exe 可能经多个 PATH 目录与已知路径
                // 重复命中，重复探针是纯成本）。
                let mut seen = std::collections::HashSet::new();
                candidates
                    .into_iter()
                    .filter(|p| seen.insert(p.clone()))
                    .find_map(|path| {
                        let s = shell(kind, path);
                        probe(&s).then_some(s)
                    })
            };
            let in_path_all = |name: &str| -> Vec<PathBuf> {
                path_dirs
                    .iter()
                    .map(|d| format!("{d}{sep}{name}"))
                    .filter(|c| is_file(Path::new(c)))
                    .map(PathBuf::from)
                    .collect()
            };
            let mut push_known = |candidates: &mut Vec<PathBuf>, known: Option<String>| {
                if let Some(p) = known.filter(|p| is_file(Path::new(p))).map(PathBuf::from) {
                    if !candidates.contains(&p) {
                        candidates.push(p);
                    }
                }
            };

            // Git Bash 优先（排 WSL 的 System32\bash.exe）。
            let mut bash = in_path_all("bash.exe");
            bash.retain(|c| {
                system32
                    .as_deref()
                    .is_none_or(|s32| !starts_with_path(&c.to_string_lossy(), s32, platform))
            });
            push_known(&mut bash, env_path("ProgramFiles", r"Git\bin\bash.exe"));
            push_known(
                &mut bash,
                env_path("ProgramFiles(x86)", r"Git\bin\bash.exe"),
            );
            if let Some(s) = find_working(ShellKind::Posix, bash) {
                return s;
            }

            let mut pwsh = in_path_all("pwsh.exe");
            push_known(
                &mut pwsh,
                env_path("ProgramFiles", r"PowerShell\7\pwsh.exe"),
            );
            if let Some(s) = find_working(ShellKind::PowerShell, pwsh) {
                return s;
            }

            let mut powershell = in_path_all("powershell.exe");
            push_known(
                &mut powershell,
                env_path(
                    "SystemRoot",
                    r"System32\WindowsPowerShell\v1.0\powershell.exe",
                ),
            );
            if let Some(s) = find_working(ShellKind::PowerShell, powershell) {
                return s;
            }

            let mut cmd = Vec::new();
            push_known(&mut cmd, env_path("SystemRoot", r"System32\cmd.exe"));
            cmd.extend(in_path_all("cmd.exe"));
            if let Some(s) = find_working(ShellKind::Cmd, cmd) {
                return s;
            }
            // 兜底裸名：CreateProcess 的默认搜索路径含 System32。
            shell(ShellKind::Cmd, PathBuf::from("cmd.exe"))
        }
    }
}

#[cfg(test)]
#[path = "shell_test.rs"]
mod tests;
