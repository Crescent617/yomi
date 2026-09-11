//! ConnectionMode::displays_as_local 的回环判定用例。

use super::{is_loopback_socket_addr, ConnectionMode};
use kernel::transport::SocketAddr;

#[test]
fn local_mode_displays_as_local() {
    assert!(ConnectionMode::Local.displays_as_local());
}

#[test]
fn loopback_ws_displays_as_local() {
    for addr in [
        "127.0.0.1:9541",
        "127.0.0.2:9541", // 127/8 整段回环
        "localhost:9541",
        "LOCALHOST:9541",
        "[::1]:9541",
    ] {
        let mode = ConnectionMode::Remote(SocketAddr::Ws(addr.to_string()));
        assert!(mode.displays_as_local(), "{addr}");
    }
    // wss 同口径。
    let mode = ConnectionMode::Remote(SocketAddr::Wss("localhost:443".to_string()));
    assert!(mode.displays_as_local());
}

#[test]
fn non_loopback_displays_as_remote() {
    for addr in ["192.168.1.10:9541", "10.0.0.2:9541", "example.com:443"] {
        let mode = ConnectionMode::Remote(SocketAddr::Ws(addr.to_string()));
        assert!(!mode.displays_as_local(), "{addr}");
    }
    let mode = ConnectionMode::Remote(SocketAddr::Wss("example.com:443".to_string()));
    assert!(!mode.displays_as_local());
}

#[test]
fn unix_socket_addr_is_loopback() {
    assert!(is_loopback_socket_addr(&SocketAddr::Unix(
        "/tmp/yomi.sock".into()
    )));
}
