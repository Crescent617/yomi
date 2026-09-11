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

/// 洪泛输出超出 drain cap：保留开头与结尾（各半额度），丢中间——
/// 构建日志的错误行通常在尾部，不能只截头。
#[tokio::test]
async fn flood_capture_keeps_head_and_tail() {
    let dir = tempfile::TempDir::new().unwrap();
    // 5000 行、每行 "line-N"：总量约 40KB，cap 给 1000。
    let script = sh_script(
        &dir,
        "flood",
        "i=1; while [ $i -le 5000 ]; do echo line-$i; i=$((i+1)); done\n",
    );
    let mut cmd = tokio::process::Command::new(&script);
    let c =
        super::spawn_captured_with_cap(&mut cmd, None, Duration::from_secs(30), None, 1000, None)
            .await
            .unwrap();
    assert_eq!(c.exit_code, Some(0));
    let out = String::from_utf8_lossy(&c.stdout);
    assert!(c.stdout.len() <= 1000, "captured {} bytes", c.stdout.len());
    assert!(out.starts_with("line-1\n"), "head lost: {:.50}", out);
    assert!(out.ends_with("line-5000\n"), "tail lost: {:.50}", out);
    assert!(
        !out.contains("line-2500"),
        "middle should be dropped: {:.100}",
        out
    );
    assert!(c.log_files.is_empty(), "no overflow log configured");
}

/// 输出未超 cap：内容与无界捕获一致（head 即全部，tail 为空）。
#[tokio::test]
async fn under_cap_capture_is_byte_exact() {
    let dir = tempfile::TempDir::new().unwrap();
    let script = sh_script(&dir, "small", "echo aaa\necho bbb\n");
    let mut cmd = tokio::process::Command::new(&script);
    let c =
        super::spawn_captured_with_cap(&mut cmd, None, Duration::from_secs(5), None, 1000, None)
            .await
            .unwrap();
    assert_eq!(String::from_utf8_lossy(&c.stdout), "aaa\nbbb\n");
}

/// 配置溢出落盘 + 输出超 cap：生成自第一字节完整的日志文件；未超的
/// 流不建文件；unix 权限 0600。
#[tokio::test]
async fn overflow_writes_complete_log_file() {
    let dir = tempfile::TempDir::new().unwrap();
    let script = sh_script(
        &dir,
        "flood",
        "i=1; while [ $i -le 5000 ]; do echo line-$i; i=$((i+1)); done\n",
    );
    let mut cmd = tokio::process::Command::new(&script);
    let overflow = super::OverflowLog {
        dir: dir.path().to_path_buf(),
        stem: "task".to_string(),
    };
    let c = super::spawn_captured_with_cap(
        &mut cmd,
        None,
        Duration::from_secs(30),
        None,
        1000,
        Some(overflow),
    )
    .await
    .unwrap();
    assert_eq!(c.exit_code, Some(0));

    assert_eq!(c.log_files.len(), 1, "only stdout floods");
    let (path, written) = &c.log_files[0];
    assert_eq!(path.file_name().unwrap(), "task_stdout.log");
    let content = std::fs::read_to_string(path).unwrap();
    assert!(content.starts_with("line-1\n"), "file head lost");
    assert!(content.ends_with("line-5000\n"), "file tail lost");
    assert!(content.contains("line-2500\n"), "file middle lost");
    assert_eq!(*written, content.len() as u64, "written counter");
    // 内存捕获仍受 cap 约束。
    assert!(c.stdout.len() <= 1000);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "log file must be owner-only");
    }
}

/// 配置溢出落盘但输出未超 cap：零 IO，不建文件。
#[tokio::test]
async fn under_cap_creates_no_log_file() {
    let dir = tempfile::TempDir::new().unwrap();
    let script = sh_script(&dir, "small", "echo hi\n");
    let mut cmd = tokio::process::Command::new(&script);
    let overflow = super::OverflowLog {
        dir: dir.path().to_path_buf(),
        stem: "task".to_string(),
    };
    let c = super::spawn_captured_with_cap(
        &mut cmd,
        None,
        Duration::from_secs(5),
        None,
        1000,
        Some(overflow),
    )
    .await
    .unwrap();
    assert!(c.log_files.is_empty());
    assert!(!dir.path().join("task_stdout.log").exists());
    assert!(!dir.path().join("task_stderr.log").exists());
}

/// overflow 首次打开失败即永久禁用、绝不重试（drain 直测，无进程无
/// 墙钟）：缓冲丢中段后若重试，写出的文件缺段却仍会被引用为「full
/// output」。同步点 = `StreamCapture.total`：open 尝试在同一把锁内
/// 先于 total 自增——total 到位即失败已处理完，旗标/时序竞态不存在。
#[tokio::test]
async fn overflow_open_failure_disables_without_retry() {
    use tokio::io::AsyncWriteExt as _;

    let dir = tempfile::TempDir::new().unwrap();
    let blocked = dir.path().join("task_stdout.log");
    std::fs::create_dir(&blocked).unwrap(); // 占位成目录：open 必败

    let (mut w, r) = tokio::io::duplex(64 * 1024);
    let state = std::sync::Arc::new(tokio::sync::Mutex::new(super::StreamCapture::default()));
    let drain = tokio::spawn(super::drain(
        r,
        std::sync::Arc::clone(&state),
        500,
        Some(blocked.clone()),
    ));
    let wait_total = |want: u64| {
        let state = std::sync::Arc::clone(&state);
        async move {
            for _ in 0..200 {
                if state.lock().await.total >= want {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            panic!("drain did not reach total={want}");
        }
    };

    // 第一块即越 cap：open 必败 → 禁用。total=900 到位时失败已处理完。
    w.write_all(&vec![b'x'; 900]).await.unwrap();
    wait_total(900).await;
    // 解锁路径：若重试存在，下一块起将成功建出文件。
    std::fs::remove_dir(&blocked).unwrap();
    w.write_all(b"more").await.unwrap();
    wait_total(904).await;
    drop(w);
    drain.await.unwrap();

    let s = state.lock().await;
    assert!(s.log.is_none(), "disabled overflow must not retry");
    assert!(
        !blocked.exists(),
        "retry would have created the log after the unblock"
    );
    // 内存捕获照旧 head+tail 截断（无日志可引时输出本身仍是诚实的）。
    assert_eq!(s.total, 904);
    assert!(s.buf.head.len() + s.buf.tail.len() <= 500);
}
