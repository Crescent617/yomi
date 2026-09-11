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

#[test]
fn validate_name_rules() {
    assert!(validate_name("deploy.sh").is_ok());
    assert!(validate_name("a-b_c.py").is_ok());
    assert!(validate_name("").is_err());
    assert!(validate_name("../etc").is_err());
    assert!(validate_name("a/b").is_err());
    assert!(validate_name("a\\b").is_err());
    assert!(validate_name(".hidden").is_err());
}

#[tokio::test]
async fn list_missing_dir_is_empty() {
    let tmp = tempdir();
    assert!(list(tmp.path()).await.unwrap().is_empty());
}

#[cfg(unix)]
#[tokio::test]
async fn list_filters_sorts_and_marks_executable() {
    let tmp = tempdir();
    let dir = workflows_dir(tmp.path());
    std::fs::create_dir_all(&dir).unwrap();
    write_script(&dir, "b.sh", "true\n", true);
    write_script(&dir, "a.sh", "true\n", false);
    write_script(&dir, ".hidden.sh", "true\n", true);
    std::fs::create_dir(dir.join("sub")).unwrap();

    let entries = list(tmp.path()).await.unwrap();
    assert_eq!(
        entries,
        vec![
            WorkflowEntry {
                name: "a.sh".to_string(),
                executable: false,
            },
            WorkflowEntry {
                name: "b.sh".to_string(),
                executable: true,
            },
        ]
    );
}

#[cfg(unix)]
#[tokio::test]
async fn resolve_and_remove() {
    let tmp = tempdir();
    let dir = workflows_dir(tmp.path());
    std::fs::create_dir_all(&dir).unwrap();
    write_script(&dir, "x.sh", "true\n", true);

    assert!(resolve(tmp.path(), "x.sh").await.unwrap().is_some());
    assert!(resolve(tmp.path(), "nope.sh").await.unwrap().is_none());
    assert!(resolve(tmp.path(), "../x.sh").await.is_err());

    assert!(remove(tmp.path(), "x.sh").await.unwrap());
    assert!(!remove(tmp.path(), "x.sh").await.unwrap());
}

#[cfg(unix)]
#[tokio::test]
async fn run_captures_merged_output_and_env() {
    let tmp = tempdir();
    let dir = workflows_dir(tmp.path());
    std::fs::create_dir_all(&dir).unwrap();
    let path = write_script(
        &dir,
        "env.sh",
        "echo out; echo err >&2; echo \"dir=$YOMI_DATA_DIR\"; echo \"sid=$YOMI_SESSION_ID\"; pwd\n",
        true,
    );

    let cwd = tempdir();
    let outcome = run(
        &path,
        &[],
        cwd.path(),
        tmp.path(),
        Some("sess_wf"),
        RUN_TIMEOUT,
    )
    .await
    .unwrap();
    assert_eq!(outcome.exit_code, Some(0));
    assert!(!outcome.timed_out);
    assert!(outcome.output.contains("out"), "{}", outcome.output);
    assert!(outcome.output.contains("err"), "{}", outcome.output);
    assert!(
        outcome
            .output
            .contains(&format!("dir={}", tmp.path().display())),
        "{}",
        outcome.output
    );
    assert!(outcome.output.contains("sid=sess_wf"), "{}", outcome.output);
    // cwd 生效。stderr 与 stdout 分管采集、按到达顺序并入，"err" 行可能
    // 落在 pwd 之后——按行匹配而不是取最后一行（macOS 上 tempdir 可能
    // 带 /private 前缀，按 canonical 比较）。
    let want = cwd.path().canonicalize().unwrap();
    assert!(
        outcome.output.lines().any(|l| std::path::Path::new(l)
            .canonicalize()
            .is_ok_and(|p| p == want)),
        "{}",
        outcome.output
    );
}

#[cfg(unix)]
#[tokio::test]
async fn run_reports_exit_code_and_args() {
    let tmp = tempdir();
    let dir = workflows_dir(tmp.path());
    std::fs::create_dir_all(&dir).unwrap();
    let path = write_script(&dir, "fail.sh", "echo \"arg=$1\"; exit 3\n", true);

    let outcome = run(
        &path,
        &["hello".to_string()],
        tmp.path(),
        tmp.path(),
        None,
        RUN_TIMEOUT,
    )
    .await
    .unwrap();
    assert_eq!(outcome.exit_code, Some(3));
    assert!(outcome.output.contains("arg=hello"), "{}", outcome.output);
}

#[cfg(unix)]
#[tokio::test]
async fn run_timeout_kills_and_keeps_partial_output() {
    let tmp = tempdir();
    let dir = workflows_dir(tmp.path());
    std::fs::create_dir_all(&dir).unwrap();
    // 外部 /bin/echo 而不是 sh 内建 echo：内建 stdout 在非 tty 上全缓冲，
    // SIGKILL 时未 flush 即丢。循环输出保部分输出可见。超时不取短上界：
    // macOS 对新文件首 exec 的安全评估跨进程串行（~400ms/文件，全套
    // 件并发 exec 全新脚本时队列可达数秒，2026-09-11 对抗 review 根
    // 因），3s 曾在全量并行下等不到脚本起跑而 flake。成本模型如实：
    // 脚本死循环 ⇒ run() 恒在 timeout 返回，15s 是**每次运行的固定
    // 开销**（套件墙钟 +8s 可见），不是"病态才多等"——用它换 flake
    // 免疫；再压（如 10s）对 ~5s 观测队尾只剩 2× 头量，不值再赌。
    let path = write_script(
        &dir,
        "hang.sh",
        "while :; do /bin/echo before; /bin/sleep 1; done\n",
        true,
    );

    let outcome = run(
        &path,
        &[],
        tmp.path(),
        tmp.path(),
        None,
        Duration::from_secs(15),
    )
    .await
    .unwrap();
    assert!(outcome.timed_out);
    assert_eq!(outcome.exit_code, None);
    assert!(outcome.output.contains("before"), "{}", outcome.output);
}
