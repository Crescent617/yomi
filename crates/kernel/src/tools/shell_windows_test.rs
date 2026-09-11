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
