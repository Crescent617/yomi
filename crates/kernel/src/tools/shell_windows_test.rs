//! Windows 实机 e2e：逐档 shell 验证执行链路（仅 windows 编译执行；
//! 探测链逻辑的主机无关单测在 `utils/shell_test.rs`）。

use super::ShellTool;
use crate::utils::shell::{AgentShell, ShellKind};
use std::path::{Path, PathBuf};

/// 机器上实际存在的各档 shell（GitHub runner 与开发者机的常见位置）。
fn installed_shells() -> Vec<AgentShell> {
    [
        (ShellKind::Posix, r"C:\Program Files\Git\bin\bash.exe"),
        (
            ShellKind::PowerShell,
            r"C:\Program Files\PowerShell\7\pwsh.exe",
        ),
        (
            ShellKind::PowerShell,
            r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe",
        ),
        (ShellKind::Cmd, r"C:\Windows\System32\cmd.exe"),
    ]
    .into_iter()
    .filter(|(_, p)| Path::new(p).is_file())
    .map(|(kind, path)| AgentShell {
        kind,
        path: PathBuf::from(path),
    })
    .collect()
}

/// detect() 在真机上必须找到一个可用 shell（cmd.exe 是保底）。
#[test]
fn detect_finds_working_shell() {
    let shell = crate::utils::shell::detect();
    assert!(
        shell.path.is_file() || shell.kind == ShellKind::Cmd,
        "detected: {shell:?}"
    );
}

/// 默认探测到的 shell 能跑通完整链路（detect → wrap → spawn → 输出）。
#[tokio::test]
async fn default_shell_executes_echo() {
    let mut cmd = ShellTool::build_command("echo hello", Path::new("C:\\"), "sess_test", None);
    let out = cmd.output().await.unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("hello"));
}

/// 每一档已安装的 shell：echo 可跑、native 命令的退出码必须传播
/// （PowerShell `-Command` 不传播 `$LASTEXITCODE` 的修复在此实机验
/// 证）、中文输出不乱码（chcp/OutputEncoding 前缀的实机验证）。
#[tokio::test]
async fn each_installed_shell_works() {
    let shells = installed_shells();
    assert!(!shells.is_empty(), "no shell found on this machine");
    for shell in shells {
        let name = shell.path.display().to_string();

        let mut cmd = ShellTool::build_command_with_shell(
            &shell,
            "echo hello",
            Path::new("C:\\"),
            "sess_test",
            None,
        );
        let out = cmd.output().await.unwrap();
        assert!(
            out.status.success(),
            "{name}: echo failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            String::from_utf8_lossy(&out.stdout).contains("hello"),
            "{name}"
        );

        // cmd /c exit 7 是 native 命令失败：考的是退出码传播而非 shell
        // 自身语法（PowerShell 修复前会在这里报 0）。
        let fail = match shell.kind {
            ShellKind::PowerShell => "cmd /c exit 7",
            _ => "exit 7",
        };
        let mut cmd =
            ShellTool::build_command_with_shell(&shell, fail, Path::new("C:\\"), "sess_test", None);
        let out = cmd.output().await.unwrap();
        assert_eq!(
            out.status.code(),
            Some(7),
            "{name}: exit code not propagated"
        );

        let mut cmd = ShellTool::build_command_with_shell(
            &shell,
            "echo 中文输出",
            Path::new("C:\\"),
            "sess_test",
            None,
        );
        let out = cmd.output().await.unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(stdout.contains("中文输出"), "{name}: mojibake: {stdout:?}");
    }
}

/// cmd 档内嵌双引号不经 std argv 转义损坏（raw_arg 直传）：输出必须
/// 是带引号的原文，不残留反斜杠。
#[tokio::test]
async fn cmd_preserves_embedded_quotes() {
    let cmd_shell = AgentShell {
        kind: ShellKind::Cmd,
        path: PathBuf::from(r"C:\Windows\System32\cmd.exe"),
    };
    if !cmd_shell.path.is_file() {
        return;
    }
    let mut cmd = ShellTool::build_command_with_shell(
        &cmd_shell,
        r#"echo "a b""#,
        Path::new("C:\\"),
        "sess_test",
        None,
    );
    let out = cmd.output().await.unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("\"a b\""), "quotes mangled: {stdout:?}");
    assert!(!stdout.contains('\\'), "backslash leaked: {stdout:?}");
}

/// PowerShell 命令末行以 `#` 注释结尾时，`exit $LASTEXITCODE` 仍须
/// 执行（包装换行隔离的实机验证）：native 失败退出码必须传播，
/// 不被注释吞掉后误报成功。
#[tokio::test]
async fn powershell_exit_survives_trailing_comment() {
    for path in [
        r"C:\Program Files\PowerShell\7\pwsh.exe",
        r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe",
    ] {
        let ps = AgentShell {
            kind: ShellKind::PowerShell,
            path: PathBuf::from(path),
        };
        if !ps.path.is_file() {
            continue;
        }
        let mut cmd = ShellTool::build_command_with_shell(
            &ps,
            "cmd /c exit 7 # 模拟失败",
            Path::new("C:\\"),
            "sess_test",
            None,
        );
        let out = cmd.output().await.unwrap();
        assert_eq!(
            out.status.code(),
            Some(7),
            "{path}: exit swallowed by trailing comment"
        );
    }
}

/// 探测选出的 shell 必须通过实战同型验证：执行带内嵌引号的 echo
/// 成功且输出正确无反斜杠残留（busybox shim 冒名 bash 场景的最终
/// 防线，2026-09-11 Windows 实测）。
#[tokio::test]
async fn detected_shell_executes_quoted_echo() {
    let shell = crate::utils::shell::detect();
    let mut cmd =
        ShellTool::build_command(r#"echo "quoted ok""#, Path::new("C:\\"), "sess_test", None);
    let out = cmd.output().await.unwrap();
    assert!(
        out.status.success(),
        "{shell:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("quoted ok"), "{shell:?}: {stdout:?}");
    assert!(
        !stdout.contains('\\'),
        "{shell:?}: backslash leaked: {stdout:?}"
    );
}
