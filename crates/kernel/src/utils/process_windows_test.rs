//! Windows 实机 e2e：Job Object 进程树（仅 windows 编译执行）。

use super::*;

/// spawn_in_new_tree 在 Windows 必须挂上 Job Object——FFI 路径（
/// CreateJobObjectW / SetInformationJobObject / OpenProcess /
/// AssignProcessToJobObject）第一次实机运行。
#[tokio::test]
async fn spawned_child_is_assigned_to_job() {
    let mut cmd = tokio::process::Command::new("cmd.exe");
    cmd.args(["/D", "/C", "timeout /t 60 >nul"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    let (mut child, tree) = spawn_in_new_tree(&mut cmd).unwrap();
    assert!(tree.job.is_some(), "job object must be assigned");
    kill_tree(&mut child, &tree).await;
}

/// kill_tree 连后裔一起收：主脚本 start /b 一个 2 秒后落 marker 的
/// 后裔，自身挂 60s；100ms 时杀树——树若收干净，marker 永不出现
/// （若只杀主进程，后裔会在 2 秒后落 marker）。
#[tokio::test]
async fn kill_tree_reaps_descendants() {
    let dir = tempfile::TempDir::new().unwrap();
    let marker = dir.path().join("descendant_alive");
    let descendant = dir.path().join("descendant.bat");
    std::fs::write(
        &descendant,
        "@echo off\r\ntimeout /t 2 >nul\r\necho x>\"%~1\"\r\n",
    )
    .unwrap();
    let main = dir.path().join("main.bat");
    std::fs::write(
        &main,
        format!(
            "@echo off\r\nstart /b \"\" call \"{}\" \"{}\"\r\ntimeout /t 60 >nul\r\n",
            descendant.display(),
            marker.display()
        ),
    )
    .unwrap();

    let mut cmd = tokio::process::Command::new("cmd.exe");
    cmd.arg("/D")
        .arg("/C")
        .arg(format!("\"{}\"", main.display()))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    let (mut child, tree) = spawn_in_new_tree(&mut cmd).unwrap();

    // 给 cmd 一点时间把后裔 start 出来。
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    kill_tree(&mut child, &tree).await;

    // 后裔若活着，2 秒时必落 marker；留足调度余量。
    tokio::time::sleep(std::time::Duration::from_secs(4)).await;
    assert!(
        !marker.exists(),
        "descendant survived tree-kill (marker appeared)"
    );
}
