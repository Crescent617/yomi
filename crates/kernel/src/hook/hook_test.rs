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
        "slow",
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
            "spawner",
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
            "cat > {}; printf '%s|%s|%s|%s|%s' \"$YOMI_HOOK_EVENT\" \"$YOMI_SESSION_ID\" \"$YOMI_DATA_DIR\" \"$YOMI_EVENT\" \"$YOMI_STATE_DIR\" > {}; exit 0\n",
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
        format!(
            "pre_tool_use|sess_abc|{0}|pre_tool_use|{0}/state/hooks/pre_tool_use/10-dump",
            tmp.path().display()
        )
    );
    // state 目录已按 YOMI_STATE_DIR 惰性创建。
    assert!(tmp.path().join("state/hooks/pre_tool_use/10-dump").is_dir());
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

#[cfg(unix)]
#[tokio::test]
async fn daemon_point_runs_in_order_and_delivers_contract() {
    let tmp = tempdir();
    let dir = point_dir(tmp.path(), POINT_DAEMON_UP);
    std::fs::create_dir_all(&dir).unwrap();
    let order = tmp.path().join("order.txt");
    write_script(
        &dir,
        "20-second",
        &format!(
            "echo second >> {}; cat > \"$YOMI_STATE_DIR/stdin.json\"; exit 0\n",
            order.display()
        ),
        true,
    );
    write_script(
        &dir,
        "10-first",
        &format!(
            "echo first >> {}; printf '%s|%s' \"$YOMI_EVENT\" \"${{YOMI_SESSION_ID:-unset}}\" > \"$YOMI_STATE_DIR/env\"; exit 0\n",
            order.display()
        ),
        true,
    );
    write_script(&dir, "30-off", "exit 0\n", false); // 无执行位：跳过
    run_daemon_point(tmp.path(), POINT_DAEMON_UP).await;

    // 字典序串行；无执行位的 30-off 未参与。
    assert_eq!(std::fs::read_to_string(&order).unwrap(), "first\nsecond\n");
    // env 契约：YOMI_EVENT=point；YOMI_SESSION_ID 显式移除（无会话语义）。
    let env =
        std::fs::read_to_string(tmp.path().join("state/hooks/daemon_up/10-first/env")).unwrap();
    assert_eq!(env, "daemon_up|unset");
    // stdin 精简契约：event/cwd，无 session_id。
    let payload: serde_json::Value = serde_json::from_slice(
        &std::fs::read(
            tmp.path()
                .join("state/hooks/daemon_up/20-second/stdin.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(payload["event"], "daemon_up");
    assert_eq!(payload["cwd"], tmp.path().to_string_lossy().as_ref());
    assert!(payload.get("session_id").is_none());
}

#[cfg(unix)]
#[tokio::test]
async fn daemon_point_failure_does_not_break_chain() {
    let tmp = tempdir();
    let dir = point_dir(tmp.path(), POINT_DAEMON_DOWN);
    std::fs::create_dir_all(&dir).unwrap();
    let ran = tmp.path().join("ran.txt");
    write_script(&dir, "10-boom", "echo why >&2\nexit 3\n", true);
    write_script(
        &dir,
        "20-next",
        &format!("echo next >> {}; exit 0\n", ran.display()),
        true,
    );
    run_daemon_point(tmp.path(), POINT_DAEMON_DOWN).await;
    // 通知型语义：前者非零只留痕，不中断后续脚本。
    assert_eq!(std::fs::read_to_string(&ran).unwrap(), "next\n");
}

/// 防残留是 daemon hook 的关键契约：父进程带着残留 `YOMI_SESSION_ID`
/// 时（daemon 从 hook/工具环境里被拉起的场景），子进程必须读不到——
/// `inject_child_env(None)` 是 `env_remove` 而非不设置。不清除则脚本读到
/// 残留值，断言必炸。
#[cfg(unix)]
#[tokio::test]
async fn daemon_point_removes_stale_session_env() {
    std::env::set_var("YOMI_SESSION_ID", "sess_stale");
    let tmp = tempdir();
    let dir = point_dir(tmp.path(), POINT_DAEMON_UP);
    std::fs::create_dir_all(&dir).unwrap();
    write_script(
        &dir,
        "10-env",
        "printf '%s' \"${YOMI_SESSION_ID:-unset}\" > \"$YOMI_STATE_DIR/env\"; exit 0\n",
        true,
    );
    run_daemon_point(tmp.path(), POINT_DAEMON_UP).await;
    std::env::remove_var("YOMI_SESSION_ID");
    let env = std::fs::read_to_string(tmp.path().join("state/hooks/daemon_up/10-env/env")).unwrap();
    assert_eq!(env, "unset", "stale YOMI_SESSION_ID must be removed");
}

/// 目录形态 hook（`<名>/run`）：与 tools 同约定——排序/state 目录按
/// 条目名（目录名而非 `run`），`dirname "$0"` 即包目录可带伴生文件；
/// 否决语义与文件形态一致；无执行位 `run` 的目录跳过。
#[cfg(unix)]
#[tokio::test]
async fn directory_form_hook_runs_and_denies() {
    let tmp = tempdir();
    let dir = point_dir_of(tmp.path());
    // 目录形态：20-guard/run + 伴生文件 patterns.txt。
    let pkg = dir.join("20-guard");
    std::fs::create_dir_all(&pkg).unwrap();
    std::fs::write(pkg.join("patterns.txt"), "rm -rf").unwrap();
    write_script(
        &pkg,
        "run",
        "cat \"$(dirname \"$0\")/patterns.txt\" > \"$YOMI_STATE_DIR/companion\"\necho denied-by-pkg >&2\nexit 2\n",
        true,
    );
    // 无执行位 run 的目录：开关关，跳过。
    let off = dir.join("30-off");
    std::fs::create_dir_all(&off).unwrap();
    write_script(&off, "run", "exit 0\n", false);
    let calls = [call("c1", "shell")];
    let outcome = run_pre_tool_use(tmp.path(), "sess_x", tmp.path(), &calls, &no_cancel()).await;
    assert!(outcome.approved.is_empty());
    assert_eq!(outcome.denied.len(), 1, "dir-form hook must deny");
    assert!(outcome.denied[0].1.contains("denied-by-pkg"));
    // state 目录按目录名寻址；伴生文件经包目录引用。
    let companion = std::fs::read_to_string(
        tmp.path()
            .join("state/hooks/pre_tool_use/20-guard/companion"),
    )
    .unwrap();
    assert_eq!(companion, "rm -rf");
}

/// 文件与目录形态混挂：按条目名字典序交错执行。
#[cfg(unix)]
#[tokio::test]
async fn file_and_dir_forms_interleave_by_entry_name() {
    let tmp = tempdir();
    let dir = point_dir_of(tmp.path());
    let order = tmp.path().join("order.txt");
    let pkg = dir.join("20-pkg");
    std::fs::create_dir_all(&pkg).unwrap();
    write_script(
        &pkg,
        "run",
        &format!("echo pkg >> {}; exit 0\n", order.display()),
        true,
    );
    write_script(
        &dir,
        "10-file",
        &format!("echo file >> {}; exit 0\n", order.display()),
        true,
    );
    let calls = [call("c1", "shell")];
    let outcome = run_pre_tool_use(tmp.path(), "sess_x", tmp.path(), &calls, &no_cancel()).await;
    assert_eq!(outcome.approved.len(), 1);
    assert_eq!(std::fs::read_to_string(&order).unwrap(), "file\npkg\n");
}
