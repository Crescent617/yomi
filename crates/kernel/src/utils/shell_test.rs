use super::{detect_impl, kind_from_name, Platform, ShellKind};
use std::collections::HashSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// 伪造一个文件系统视图：`files` 里的路径「存在」。
fn fake_fs(files: &[&str]) -> impl Fn(&Path) -> bool {
    let set: HashSet<PathBuf> = files.iter().map(PathBuf::from).collect();
    move |p: &Path| set.contains(p)
}

fn env_of(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
    let map: std::collections::HashMap<String, OsString> = pairs
        .iter()
        .map(|(k, v)| (k.to_string(), OsString::from(v)))
        .collect();
    move |key| map.get(key).cloned()
}

#[test]
fn kind_inference_from_filename() {
    for (name, kind) in [
        ("bash", ShellKind::Posix),
        ("sh", ShellKind::Posix),
        ("zsh", ShellKind::Posix),
        ("pwsh", ShellKind::PowerShell),
        ("powershell", ShellKind::PowerShell),
        ("cmd", ShellKind::Cmd),
        ("bash.exe", ShellKind::Posix),
        ("pwsh.exe", ShellKind::PowerShell),
        ("CMD.EXE", ShellKind::Cmd), // 大小写不敏感（Windows 文件名习惯）
    ] {
        assert_eq!(kind_from_name(Path::new(name)), kind, "{name}");
    }
}

#[test]
fn unix_prefers_path_bash_then_known_paths_then_sh() {
    // PATH 里有 bash。
    let shell = detect_impl(
        Platform::Unix,
        None,
        &env_of(&[("PATH", "/usr/bin:/bin")]),
        &fake_fs(&["/usr/bin/bash", "/bin/sh"]),
    );
    assert_eq!(shell.path, PathBuf::from("/usr/bin/bash"));
    assert_eq!(shell.kind, ShellKind::Posix);

    // PATH 没有 bash，走 /bin/bash。
    let shell = detect_impl(
        Platform::Unix,
        None,
        &env_of(&[("PATH", "/usr/bin")]),
        &fake_fs(&["/bin/bash", "/bin/sh"]),
    );
    assert_eq!(shell.path, PathBuf::from("/bin/bash"));

    // 精简容器（无 bash）：回退 sh。
    let shell = detect_impl(
        Platform::Unix,
        None,
        &env_of(&[("PATH", "/usr/bin")]),
        &fake_fs(&["/bin/sh"]),
    );
    assert_eq!(shell.path, PathBuf::from("/bin/sh"));
}

#[test]
fn unix_override_wins() {
    let shell = detect_impl(
        Platform::Unix,
        Some(OsString::from("/opt/homebrew/bin/bash")),
        &env_of(&[]),
        &fake_fs(&[]),
    );
    assert_eq!(shell.path, PathBuf::from("/opt/homebrew/bin/bash"));
    assert_eq!(shell.kind, ShellKind::Posix);
}

#[test]
fn windows_excludes_wsl_bash_from_path() {
    // PATH 里只有 WSL 的 bash.exe：必须跳过，落到下一档。
    let shell = detect_impl(
        Platform::Windows,
        None,
        &env_of(&[
            ("PATH", r"C:\Windows\System32;C:\Tools"),
            ("SystemRoot", r"C:\Windows"),
        ]),
        &fake_fs(&[
            r"C:\Windows\System32\bash.exe",
            r"C:\Windows\System32\cmd.exe",
        ]),
    );
    assert_eq!(shell.kind, ShellKind::Cmd, "WSL bash must be excluded");

    // PATH 里另有真正的 Git Bash：正常选中。
    let shell = detect_impl(
        Platform::Windows,
        None,
        &env_of(&[
            ("PATH", r"C:\Windows\System32;C:\Program Files\Git\bin"),
            ("SystemRoot", r"C:\Windows"),
        ]),
        &fake_fs(&[
            r"C:\Windows\System32\bash.exe",
            r"C:\Program Files\Git\bin\bash.exe",
        ]),
    );
    assert_eq!(shell.kind, ShellKind::Posix);
    assert_eq!(
        shell.path,
        PathBuf::from(r"C:\Program Files\Git\bin\bash.exe")
    );
}

#[test]
fn windows_falls_back_through_git_bash_pwsh_powershell_cmd() {
    // Git Bash 经 ProgramFiles 命中。
    let shell = detect_impl(
        Platform::Windows,
        None,
        &env_of(&[
            ("PATH", r"C:\Windows\System32"),
            ("SystemRoot", r"C:\Windows"),
            ("ProgramFiles", r"C:\Program Files"),
        ]),
        &fake_fs(&[r"C:\Program Files\Git\bin\bash.exe"]),
    );
    assert_eq!(shell.kind, ShellKind::Posix);

    // 无 bash：pwsh。
    let shell = detect_impl(
        Platform::Windows,
        None,
        &env_of(&[
            ("PATH", r"C:\Windows\System32"),
            ("SystemRoot", r"C:\Windows"),
            ("ProgramFiles", r"C:\Program Files"),
        ]),
        &fake_fs(&[r"C:\Program Files\PowerShell\7\pwsh.exe"]),
    );
    assert_eq!(shell.kind, ShellKind::PowerShell);

    // 只有 Windows PowerShell。
    let shell = detect_impl(
        Platform::Windows,
        None,
        &env_of(&[
            ("PATH", r"C:\Windows\System32"),
            ("SystemRoot", r"C:\Windows"),
        ]),
        &fake_fs(&[r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe"]),
    );
    assert_eq!(shell.kind, ShellKind::PowerShell);

    // 啥都没有：cmd.exe 兜底（System32 已知路径）。
    let shell = detect_impl(
        Platform::Windows,
        None,
        &env_of(&[
            ("PATH", r"C:\Windows\System32"),
            ("SystemRoot", r"C:\Windows"),
        ]),
        &fake_fs(&[r"C:\Windows\System32\cmd.exe"]),
    );
    assert_eq!(shell.kind, ShellKind::Cmd);

    // 连 SystemRoot 都没有（异常环境）：裸名 cmd.exe 兜底。
    let shell = detect_impl(Platform::Windows, None, &env_of(&[]), &fake_fs(&[]));
    assert_eq!(shell.kind, ShellKind::Cmd);
    assert_eq!(shell.path, PathBuf::from("cmd.exe"));
}

#[test]
fn windows_override_wins_and_infers_kind() {
    let shell = detect_impl(
        Platform::Windows,
        Some(OsString::from(r"D:\tools\pwsh.exe")),
        &env_of(&[]),
        &fake_fs(&[]),
    );
    assert_eq!(shell.kind, ShellKind::PowerShell);
}

#[test]
fn windows_system32_exclusion_tolerates_non_ascii_path_dirs() {
    // PATH 里混有非 ASCII 目录（中文 Windows 的用户目录很常见）：前缀
    // 判断不能 panic（字节切片落在字符边界中间），也不能误判排除。
    let shell = detect_impl(
        Platform::Windows,
        None,
        &env_of(&[
            ("PATH", r"C:\用户\tools;C:\Windows\System32"),
            ("SystemRoot", r"C:\Windows"),
        ]),
        &fake_fs(&[r"C:\用户\tools\bash.exe", r"C:\Windows\System32\cmd.exe"]),
    );
    assert_eq!(shell.kind, ShellKind::Posix);
    assert_eq!(shell.path, PathBuf::from(r"C:\用户\tools\bash.exe"));
}

#[test]
fn leading_args_and_wrapping_per_kind() {
    use super::AgentShell;
    let mk = |kind, path| AgentShell {
        kind,
        path: PathBuf::from(path),
    };

    let posix = mk(ShellKind::Posix, "/bin/bash");
    assert_eq!(posix.leading_args(), &["-c"]);
    assert_eq!(posix.wrap_command("ls -la"), "ls -la");

    let ps = mk(ShellKind::PowerShell, "pwsh.exe");
    assert_eq!(
        ps.leading_args(),
        &["-NoProfile", "-NonInteractive", "-Command"]
    );
    assert!(ps
        .wrap_command("ls")
        .starts_with("[Console]::OutputEncoding="));

    let cmd = mk(ShellKind::Cmd, "cmd.exe");
    assert_eq!(cmd.leading_args(), &["/D", "/C"]);
    assert_eq!(cmd.wrap_command("dir"), "chcp 65001 >nul & dir");
}
