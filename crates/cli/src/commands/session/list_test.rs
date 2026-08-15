use super::*;

fn info(dir: Option<&str>) -> kernel::storage::session::SessionInfo {
    kernel::storage::session::SessionInfo {
        id: kernel::types::SessionId::new(),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        parent_id: None,
        title: None,
        message_count: 0,
        working_dir: dir.map(str::to_string),
        project_id: None,
        auto_approve_level: None,
        model_key: None,
        template: None,
    }
}

#[test]
fn filter_by_dir_none_passes_all_through() {
    let sessions = vec![info(Some("/a")), info(Some("/b")), info(None)];
    let out = filter_by_dir(sessions, None);
    assert_eq!(out.len(), 3);
}

#[test]
fn filter_by_dir_matches_session_working_dir_exactly() {
    let dir = std::path::Path::new("/a");
    let sessions = vec![info(Some("/a")), info(Some("/b")), info(None)];
    let out = filter_by_dir(sessions, Some(dir));
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].working_dir.as_deref(), Some("/a"));
}

#[test]
fn filter_by_dir_caps_at_50() {
    let sessions: Vec<_> = (0..60).map(|_| info(Some("/a"))).collect();
    let out = filter_by_dir(sessions, Some(std::path::Path::new("/a")));
    assert_eq!(out.len(), 50);
}
