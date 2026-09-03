use super::*;

use std::io::Write as _;

fn tempdir() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

/// 写一个带 shebang 的脚本；`chmod +x` 由 `exec` 控制。
#[cfg(unix)]
fn write_script(dir: &Path, name: &str, body: &str, exec: bool) -> PathBuf {
    use std::os::unix::fs::PermissionsExt as _;
    let path = dir.join(name);
    let mut f = std::fs::File::create(&path).unwrap();
    writeln!(f, "#!/bin/sh").unwrap();
    write!(f, "{body}").unwrap();
    if exec {
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    path
}

/// 在 tempdir 下建好 `hooks/pre_tool_use/` 并返回其路径。
fn point_dir_of(data_dir: &Path) -> PathBuf {
    let dir = point_dir(data_dir, POINT_PRE_TOOL_USE);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn call(id: &str, name: &str) -> ToolCall {
    ToolCall {
        id: id.to_string(),
        name: name.to_string(),
        arguments: serde_json::json!({"command": "echo hi"}),
    }
}

fn no_cancel() -> CancellationToken {
    CancellationToken::new()
}

#[tokio::test]
async fn missing_dir_allows_all() {
    let tmp = tempdir();
    let outcome = run_pre_tool_use(
        tmp.path(),
        "sess_t",
        tmp.path(),
        &[call("c1", "shell")],
        &no_cancel(),
    )
    .await;
    assert_eq!(outcome.approved.len(), 1);
    assert!(outcome.denied.is_empty());
}

#[cfg(unix)]
#[tokio::test]
async fn allow_hook_passes() {
    let tmp = tempdir();
    let dir = point_dir_of(tmp.path());
    write_script(&dir, "10-ok", "exit 0\n", true);
    let outcome = run_pre_tool_use(
        tmp.path(),
        "sess_t",
        tmp.path(),
        &[call("c1", "shell")],
        &no_cancel(),
    )
    .await;
    assert_eq!(outcome.approved.len(), 1);
    assert!(outcome.denied.is_empty());
}

#[cfg(unix)]
#[tokio::test]
async fn deny_hook_blocks_with_stderr_reason() {
    let tmp = tempdir();
    let dir = point_dir_of(tmp.path());
    write_script(&dir, "10-guard", "echo 'no rm -rf' >&2; exit 2\n", true);
    let outcome = run_pre_tool_use(
        tmp.path(),
        "sess_t",
        tmp.path(),
        &[call("c1", "shell")],
        &no_cancel(),
    )
    .await;
    assert!(outcome.approved.is_empty());
    assert_eq!(outcome.denied.len(), 1);
    assert_eq!(outcome.denied[0].0.id, "c1");
    assert_eq!(outcome.denied[0].1, "[hook:10-guard] no rm -rf");
}

#[cfg(unix)]
#[tokio::test]
async fn deny_without_stderr_falls_back() {
    let tmp = tempdir();
    let dir = point_dir_of(tmp.path());
    write_script(&dir, "10-guard", "exit 2\n", true);
    let outcome = run_pre_tool_use(
        tmp.path(),
        "sess_t",
        tmp.path(),
        &[call("c1", "shell")],
        &no_cancel(),
    )
    .await;
    assert_eq!(outcome.denied.len(), 1);
    assert_eq!(outcome.denied[0].1, "[hook:10-guard] denied without reason");
}

#[cfg(unix)]
#[tokio::test]
async fn first_deny_short_circuits() {
    let tmp = tempdir();
    let dir = point_dir_of(tmp.path());
    let marker = tmp.path().join("reached");
    write_script(&dir, "10-guard", "exit 2\n", true);
    write_script(
        &dir,
        "20-later",
        &format!("touch {}\nexit 0\n", marker.display()),
        true,
    );
    let outcome = run_pre_tool_use(
        tmp.path(),
        "sess_t",
        tmp.path(),
        &[call("c1", "shell")],
        &no_cancel(),
    )
    .await;
    assert_eq!(outcome.denied.len(), 1);
    assert!(
        !marker.exists(),
        "second hook must not run after a deny (short-circuit)"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn failing_hook_fails_open() {
    let tmp = tempdir();
    let dir = point_dir_of(tmp.path());
    write_script(&dir, "10-broken", "echo oops >&2; exit 1\n", true);
    let outcome = run_pre_tool_use(
        tmp.path(),
        "sess_t",
        tmp.path(),
        &[call("c1", "shell")],
        &no_cancel(),
    )
    .await;
    assert_eq!(outcome.approved.len(), 1);
    assert!(outcome.denied.is_empty());
}

#[cfg(unix)]
#[tokio::test]
async fn non_executable_hidden_and_subdir_skipped() {
    let tmp = tempdir();
    let dir = point_dir_of(tmp.path());
    write_script(&dir, "10-noexec", "exit 2\n", false);
    write_script(&dir, ".hidden", "exit 2\n", true);
    // 子目录（即使名字排在前面且内含脚本）不参与执行。
    let sub = dir.join("05-sub");
    std::fs::create_dir(&sub).unwrap();
    write_script(&sub, "inner", "exit 2\n", true);
    let outcome = run_pre_tool_use(
        tmp.path(),
        "sess_t",
        tmp.path(),
        &[call("c1", "shell")],
        &no_cancel(),
    )
    .await;
    assert_eq!(outcome.approved.len(), 1);
}

/// symlink 是 stow/nix 式部署的常见安装方式，必须跟随。
#[cfg(unix)]
#[tokio::test]
async fn symlink_hook_is_followed() {
    let tmp = tempdir();
    let dir = point_dir_of(tmp.path());
    let real = write_script(tmp.path(), "real-guard", "exit 2\n", true);
    std::os::unix::fs::symlink(&real, dir.join("10-link")).unwrap();
    let outcome = run_pre_tool_use(
        tmp.path(),
        "sess_t",
        tmp.path(),
        &[call("c1", "shell")],
        &no_cancel(),
    )
    .await;
    assert_eq!(outcome.denied.len(), 1);
}

/// 破损 symlink（目标不存在）跳过而非整批失败。
#[cfg(unix)]
#[tokio::test]
async fn broken_symlink_is_skipped() {
    let tmp = tempdir();
    let dir = point_dir_of(tmp.path());
    std::os::unix::fs::symlink(tmp.path().join("nonexistent"), dir.join("10-broken")).unwrap();
    write_script(&dir, "20-guard", "exit 2\n", true);
    let outcome = run_pre_tool_use(
        tmp.path(),
        "sess_t",
        tmp.path(),
        &[call("c1", "shell")],
        &no_cancel(),
    )
    .await;
    assert_eq!(
        outcome.denied.len(),
        1,
        "guard after broken symlink must still run"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn hook_timeout_fails_open() {
    let tmp = tempdir();
    let script = write_script(tmp.path(), "slow", "sleep 5\n", true);
    let started = std::time::Instant::now();
    let verdict = run_one_hook(
        &script,
        b"{}",
        POINT_PRE_TOOL_USE,
        tmp.path(),
        tmp.path(),
        "sess_t",
        Duration::from_millis(200),
    )
    .await;
    assert_eq!(verdict, Verdict::Allow);
    assert!(started.elapsed() < Duration::from_secs(3));
}

/// 超时按进程组强杀：hook 的后裔（独立 sleep）也要一起死。
/// 不用固定墙钟等后裔 spawn——throttled 机器（后台调度策略下的开发
/// 机）上 sh 启动可能耗掉数秒。改为轮询 pidfile 确认后裔已 spawn
/// （超时给足 10s），再验证连坐：hook 被 timeout 强杀时后裔同组陪葬。
#[cfg(unix)]
#[tokio::test]
async fn timeout_kills_process_group() {
    let tmp = tempdir();
    let pidfile = tmp.path().join("child.pid");
    let script = write_script(
        tmp.path(),
        "spawner",
        &format!("sleep 60 & echo $! > {}; wait\n", pidfile.display()),
        true,
    );
    let data = tmp.path().to_path_buf();
    let gate = tokio::spawn(async move {
        run_one_hook(
            &script,
            b"{}",
            POINT_PRE_TOOL_USE,
            &data,
            &data,
            "sess_t",
            Duration::from_secs(15),
        )
        .await
    });
    // 轮询等后裔 spawn（throttled 机器上 sh 启动可达数秒，全套并行更慢），
    // 确认后等超时强杀。
    let mut spawned = false;
    for _ in 0..130 {
        if pidfile.exists() {
            spawned = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(spawned, "descendant did not spawn within 13s");
    let verdict = gate.await.unwrap();
    assert_eq!(verdict, Verdict::Allow);
    let pid = std::fs::read_to_string(&pidfile).unwrap();
    let alive = std::process::Command::new("kill")
        .args(["-0", pid.trim()])
        .status()
        .unwrap()
        .success();
    assert!(!alive, "descendant must be killed with the process group");
}

/// 后裔继续持有 stderr 管道（不见 EOF）时，已写出的否决原因不丢。
#[cfg(unix)]
#[tokio::test]
async fn deny_reason_survives_descendant_holding_stderr() {
    let tmp = tempdir();
    let dir = point_dir_of(tmp.path());
    write_script(
        &dir,
        "10-guard",
        "echo 'real reason' >&2; sleep 30 >&2 2>&1 & exit 2\n",
        true,
    );
    let started = std::time::Instant::now();
    let outcome = run_pre_tool_use(
        tmp.path(),
        "sess_t",
        tmp.path(),
        &[call("c1", "shell")],
        &no_cancel(),
    )
    .await;
    assert_eq!(outcome.denied.len(), 1);
    assert_eq!(outcome.denied[0].1, "[hook:10-guard] real reason");
    // drain 宽限（2s）会到期，但不能更久。
    assert!(started.elapsed() < Duration::from_secs(10));
}

/// cancel 生效时：当前 hook 立即让位，剩余 call 全部 flush 进 approved
/// （下游持同一 token 在执行前拦下）。
#[cfg(unix)]
#[tokio::test]
async fn cancel_flushes_remaining_calls() {
    let tmp = tempdir();
    let dir = point_dir_of(tmp.path());
    write_script(&dir, "10-slow", "sleep 30\n", true);
    let cancel = CancellationToken::new();
    let cancel2 = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        cancel2.cancel();
    });
    let started = std::time::Instant::now();
    let outcome = run_pre_tool_use(
        tmp.path(),
        "sess_t",
        tmp.path(),
        &[call("c1", "shell"), call("c2", "read")],
        &cancel,
    )
    .await;
    assert!(started.elapsed() < Duration::from_secs(5));
    assert_eq!(outcome.approved.len(), 2, "remaining calls flushed ungated");
    assert!(outcome.denied.is_empty());
}

#[cfg(unix)]
#[tokio::test]
async fn stdin_payload_and_env() {
    let tmp = tempdir();
    let dir = point_dir_of(tmp.path());
    let dump = tmp.path().join("stdin.json");
    let envdump = tmp.path().join("env.txt");
    write_script(
        &dir,
        "10-dump",
        &format!(
            "cat > {}; printf '%s|%s|%s' \"$YOMI_HOOK_EVENT\" \"$YOMI_SESSION_ID\" \"$YOMI_DATA_DIR\" > {}; exit 0\n",
            dump.display(),
            envdump.display()
        ),
        true,
    );
    let calls = [call("c1", "shell")];
    let outcome = run_pre_tool_use(tmp.path(), "sess_abc", tmp.path(), &calls, &no_cancel()).await;
    assert_eq!(outcome.approved.len(), 1);

    let payload: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&dump).unwrap()).unwrap();
    assert_eq!(payload["session_id"], "sess_abc");
    assert_eq!(payload["cwd"], tmp.path().to_string_lossy().as_ref());
    assert_eq!(payload["hook_event_name"], "pre_tool_use");
    assert_eq!(payload["tool_name"], "shell");
    assert_eq!(payload["tool_input"], calls[0].arguments);

    let env = std::fs::read_to_string(&envdump).unwrap();
    assert_eq!(
        env,
        format!("pre_tool_use|sess_abc|{}", tmp.path().display())
    );
}

#[cfg(unix)]
#[tokio::test]
async fn multiple_calls_independent() {
    let tmp = tempdir();
    let dir = point_dir_of(tmp.path());
    // 只否决 shell 工具：过滤下沉到脚本自身的示范。锚定相邻字段——
    // payload 字段序由 PreToolUseInput 结构体定义保证（tool_name 紧邻
    // tool_input），避免 tool_input 内容里的巧合子串误判。
    write_script(
        &dir,
        "10-shell-guard",
        "grep -q '\"tool_name\":\"shell\",\"tool_input\"' && { echo no >&2; exit 2; }; exit 0\n",
        true,
    );
    let outcome = run_pre_tool_use(
        tmp.path(),
        "sess_t",
        tmp.path(),
        &[call("c1", "shell"), call("c2", "read")],
        &no_cancel(),
    )
    .await;
    assert_eq!(outcome.approved.len(), 1);
    assert_eq!(outcome.approved[0].name, "read");
    assert_eq!(outcome.denied.len(), 1);
    assert_eq!(outcome.denied[0].0.name, "shell");
}

#[test]
fn truncate_reason_caps_long_text() {
    let short = "reason";
    assert_eq!(truncate_reason(short), short);
    let long = "x".repeat(MAX_REASON_CHARS + 100);
    let truncated = truncate_reason(&long);
    assert_eq!(truncated.chars().count(), MAX_REASON_CHARS + 1); // +1 为省略号
}
