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
