use super::*;

fn temp_dir() -> tempfile::TempDir {
    tempfile::tempdir().expect("create temp dir")
}

#[test]
fn gui_log_name_rejects_traversal_and_other_logs() {
    assert!(is_gui_log_name("gui.log"));
    assert!(is_gui_log_name("gui.2026-07-14.log"));
    assert!(!is_gui_log_name("daemon.log"));
    assert!(!is_gui_log_name("../gui.log"));
    assert!(!is_gui_log_name("gui.txt"));
}

#[test]
fn list_gui_logs_is_newest_first() {
    let dir = temp_dir();
    let older = dir.path().join("gui.2026-07-13.log");
    let newer = dir.path().join("gui.2026-07-14.log");
    std::fs::write(&older, "older").unwrap();
    std::fs::write(&newer, "newer").unwrap();
    let old_time = std::time::SystemTime::now() - std::time::Duration::from_mins(1);
    std::fs::File::options()
        .write(true)
        .open(&older)
        .unwrap()
        .set_modified(old_time)
        .unwrap();
    std::fs::write(dir.path().join("daemon.log"), "ignored").unwrap();

    let logs = list_gui_log_files(dir.path()).unwrap();
    assert_eq!(logs, ["gui.2026-07-14.log", "gui.2026-07-13.log"]);
}

#[cfg(unix)]
#[test]
fn list_gui_logs_rejects_symlinks() {
    let dir = temp_dir();
    let target = dir.path().join("target.log");
    let link = dir.path().join("gui.log");
    std::fs::write(&target, "secret").unwrap();
    std::os::unix::fs::symlink(target, &link).unwrap();
    assert!(list_gui_log_files(dir.path()).unwrap().is_empty());
}
