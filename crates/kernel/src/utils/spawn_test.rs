//! spawn 引擎的行为测试：捕获、stdin、超时组杀、上限、故障分层。

use std::io::Write as _;
use std::time::Duration;

use super::{spawn_captured, SpawnError, DRAIN_CAP};

fn sh_script(dir: &tempfile::TempDir, name: &str, body: &str) -> std::path::PathBuf {
    let path = dir.path().join(name);
    let mut f = std::fs::File::create(&path).unwrap();
    writeln!(f, "#!/bin/sh").unwrap();
    write!(f, "{body}").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    path
}

#[tokio::test]
async fn captures_stdout_stderr_and_exit_code() {
    let dir = tempfile::TempDir::new().unwrap();
    let script = sh_script(&dir, "ok", "echo hello\necho oops >&2\nexit 3\n");
    let mut cmd = tokio::process::Command::new(&script);
    let c = spawn_captured(&mut cmd, None, Duration::from_secs(5), None)
        .await
        .unwrap();
    assert_eq!(c.exit_code, Some(3));
    assert!(!c.timed_out);
    assert_eq!(String::from_utf8_lossy(&c.stdout).trim(), "hello");
    assert_eq!(String::from_utf8_lossy(&c.stderr).trim(), "oops");
}

#[tokio::test]
async fn stdin_roundtrip() {
    let dir = tempfile::TempDir::new().unwrap();
    let script = sh_script(&dir, "cat", "cat\n");
    let mut cmd = tokio::process::Command::new(&script);
    let c = spawn_captured(&mut cmd, Some(br#"{"a":1}"#), Duration::from_secs(5), None)
        .await
        .unwrap();
    assert_eq!(c.exit_code, Some(0));
    assert_eq!(String::from_utf8_lossy(&c.stdout).trim(), r#"{"a":1}"#);
}

#[tokio::test]
async fn timeout_kills_process_group() {
    let dir = tempfile::TempDir::new().unwrap();
    let marker = dir.path().join("survivor");
    // 后裔脱离直接子进程：只有按组杀才收得到。
    let script = sh_script(
        &dir,
        "hang",
        &format!("sleep 0.2 && touch {} & sleep 60\n", marker.display()),
    );
    let mut cmd = tokio::process::Command::new(&script);
    let c = spawn_captured(&mut cmd, None, Duration::from_millis(500), None)
        .await
        .unwrap();
    assert!(c.timed_out);
    assert_eq!(c.exit_code, None);
    // 后裔若活着会在 0.2s 后创建 marker；等 1s 仍未出现 = 组杀生效。
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(!marker.exists(), "descendant survived group kill");
}

#[tokio::test]
async fn drain_cap_stops_accumulation_not_reading() {
    let dir = tempfile::TempDir::new().unwrap();
    // 写 1MB 到 stderr：进程必须能正常跑完（管道被排空），捕获只有上限内。
    let script = sh_script(
        &dir,
        "flood",
        "dd if=/dev/zero bs=1024 count=1024 2>&1 1>/dev/null\n",
    );
    let mut cmd = tokio::process::Command::new(&script);
    let c = spawn_captured(&mut cmd, None, Duration::from_secs(30), None)
        .await
        .unwrap();
    assert_eq!(c.exit_code, Some(0));
    assert!(c.stderr.len() <= DRAIN_CAP);
}

#[tokio::test]
async fn missing_program_is_spawn_error_not_capture() {
    let mut cmd = tokio::process::Command::new("/nonexistent/ext-tool");
    let err = spawn_captured(&mut cmd, None, Duration::from_secs(1), None)
        .await
        .unwrap_err();
    assert!(matches!(err, SpawnError::Spawn(_)));
}

#[tokio::test]
async fn cancel_kills_process_group_like_timeout() {
    let dir = tempfile::TempDir::new().unwrap();
    let marker = dir.path().join("survivor");
    let script = sh_script(
        &dir,
        "hang",
        &format!("sleep 0.2 && touch {} & sleep 60\n", marker.display()),
    );
    let token = tokio_util::sync::CancellationToken::new();
    let t2 = token.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(300)).await;
        t2.cancel();
    });
    let mut cmd = tokio::process::Command::new(&script);
    let c = spawn_captured(&mut cmd, None, Duration::from_secs(30), Some(&token))
        .await
        .unwrap();
    assert!(c.cancelled);
    assert!(!c.timed_out);
    assert_eq!(c.exit_code, None);
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(!marker.exists(), "descendant survived group kill on cancel");
}

/// 后裔持有管道（不见 EOF）：drain 宽限到期 abort，已捕获内容仍读得到
/// （共享缓冲的存在理由），且不按后裔的寿命等待。
#[tokio::test]
async fn drain_grace_expiry_keeps_partial_capture() {
    let dir = tempfile::TempDir::new().unwrap();
    // 主进程立即退出；后台后裔继承 stdout 继续持有管道 30s——阈值要
    // 能承受全量并发跑的调度膨胀，同时显著小于后裔寿命。
    let script = sh_script(&dir, "orphan", "echo hello\nsleep 30 &\n");
    let mut cmd = tokio::process::Command::new(&script);
    let begin = std::time::Instant::now();
    let c = spawn_captured(&mut cmd, None, Duration::from_secs(60), None)
        .await
        .unwrap();
    assert_eq!(c.exit_code, Some(0));
    assert_eq!(String::from_utf8_lossy(&c.stdout).trim(), "hello");
    assert!(
        begin.elapsed() < Duration::from_secs(15),
        "must not wait out the descendant"
    );
}
