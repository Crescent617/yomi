use super::*;

#[cfg(unix)]
#[tokio::test]
async fn child_becomes_session_leader() {
    let mut cmd = tokio::process::Command::new("/bin/sh");
    cmd.args(["-c", "test \"$$\" = \"$(ps -o pgid= -p $$ | tr -d ' ')\""])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    pre_exec_new_session(&mut cmd);
    let status = cmd.spawn().unwrap().wait().await.unwrap();
    assert!(
        status.success(),
        "child pid must equal its pgid (group leader)"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn kill_tree_reaps_whole_group() {
    // 子进程后台再留一个后裔；kill_tree 应把整个进程组收掉（只杀主
    // 进程的话，后裔会让进程组继续存在）。
    let mut cmd = tokio::process::Command::new("/bin/sh");
    cmd.args(["-c", "sleep 60 & sleep 60"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let (mut child, tree) = spawn_in_new_tree(&mut cmd).unwrap();
    let pid = child.id().unwrap();
    // 给 sh 一点时间把后台 sleep fork 出来。
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    kill_tree(&mut child, &tree).await;

    // 进程组已不存在：kill(1) 信号 0 探测不到任何成员。
    let probe = std::process::Command::new("kill")
        .args(["-0", "--", &format!("-{pid}")])
        .status()
        .unwrap();
    assert!(!probe.success(), "process group must be fully reaped");
}

#[cfg(unix)]
#[test]
fn terminate_tree_by_pid_errors_on_missing_group() {
    // 不存在的进程组：ESRCH 透传为 Err（调用方按「任务已不在」处理）。
    assert!(terminate_tree_by_pid(4_000_000).is_err());
}
