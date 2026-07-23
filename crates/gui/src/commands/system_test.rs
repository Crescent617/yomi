use super::*;

#[test]
fn connection_info_uses_snake_case_fields_for_local_daemon() {
    let info = connection_info_json(&crate::state::ConnectionMode::Local, true);

    assert_eq!(info["mode"], "local");
    assert_eq!(info["addr"], crate::daemon::socket_addr().to_string());
    assert_eq!(info["managed"], true);
}

#[test]
fn connection_info_reports_remote_daemon() {
    let addr = kernel::transport::SocketAddr::Wss("example.com/kernel".to_string());
    let info = connection_info_json(&crate::state::ConnectionMode::Remote(addr), false);

    assert_eq!(info["mode"], "remote");
    assert_eq!(info["addr"], "wss://example.com/kernel");
    assert_eq!(info["managed"], false);
}
