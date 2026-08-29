//! Tests for socket auth (hash/verify) and the ws handshake gate.

use super::super::{
    auth_verifier, bind, connect_with_token, hash_password, recv_frame, send_frame, SocketAddr,
};
use super::*;
use crate::wire::WireMsg;

// ── hash / verifier ─────────────────────────────────────────────────────

#[test]
fn hash_password_format() {
    let hash = hash_password("secret");
    let hex = hash.strip_prefix("blake3:").unwrap();
    assert_eq!(hex.len(), 64);
    assert!(hex.chars().all(|c| c.is_ascii_hexdigit()));
    // Stable: same input, same hash.
    assert_eq!(hash, hash_password("secret"));
    assert_ne!(hash, hash_password("other"));
}

#[test]
fn verifier_accepts_correct_password() {
    let verify = auth_verifier(&hash_password("correct horse"));
    assert!(verify("correct horse"));
    assert!(!verify("wrong"));
    assert!(!verify(""));
}

#[test]
fn verifier_tolerates_bare_hex_and_case() {
    let bare = blake3::hash(b"pw").to_hex().to_string();
    assert!(auth_verifier(&bare)("pw"));
    // Optional prefix and case-insensitive hex.
    let prefixed_upper = format!("blake3:{}", bare.to_uppercase());
    assert!(auth_verifier(&prefixed_upper)("pw"));
    // Surrounding whitespace.
    assert!(auth_verifier(&format!("  {bare} \n"))("pw"));
}

#[test]
fn verifier_fails_closed_on_malformed_hash() {
    let verify = auth_verifier("not-a-hash");
    assert!(!verify("not-a-hash"));
    assert!(!verify("anything"));
}

#[test]
fn hash_format_validation() {
    let hex = blake3::hash(b"pw").to_hex().to_string();
    // Canonical, bare hex, uppercase, surrounding whitespace.
    assert!(is_valid_hash_format(&format!("blake3:{hex}")));
    assert!(is_valid_hash_format(&hex));
    assert!(is_valid_hash_format(&hex.to_uppercase()));
    assert!(is_valid_hash_format(&format!("  blake3:{hex} \n")));
    // Malformed: non-hex, wrong length, garbage, empty.
    assert!(!is_valid_hash_format("not-a-hash"));
    assert!(!is_valid_hash_format(&format!("blake3:g{}", &hex[1..])));
    assert!(!is_valid_hash_format(&hex[..63]));
    assert!(!is_valid_hash_format(&format!("{hex}00")));
    assert!(!is_valid_hash_format(""));
    assert!(!is_valid_hash_format("blake3:"));
}

#[test]
fn bearer_token_extraction() {
    assert_eq!(bearer_token("Bearer abc"), Some("abc"));
    // RFC 6750: scheme is case-insensitive.
    assert_eq!(bearer_token("bearer abc"), Some("abc"));
    assert_eq!(bearer_token("BEARER abc"), Some("abc"));
    assert_eq!(bearer_token("Bearer  abc"), Some("abc"));
    assert_eq!(bearer_token("Basic abc"), None);
    assert_eq!(bearer_token("Bearer"), None);
    assert_eq!(bearer_token(""), None);
}

#[test]
fn generate_token_is_random_and_ulid_sized() {
    let a = generate_token();
    let b = generate_token();
    assert_eq!(a.len(), 26);
    assert_ne!(a, b);
    assert!(a.chars().all(|c| c.is_ascii_alphanumeric()));
}

// ── ws handshake gate ───────────────────────────────────────────────────

/// Bind a ws listener on an ephemeral port; return its `host:port`.
async fn bind_ws(auth: Option<AuthVerifier>) -> (String, super::super::Listener) {
    let listener = bind(&SocketAddr::Ws("127.0.0.1:0".into()), auth)
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    (addr, listener)
}

/// This host's primary non-loopback IPv4, if it has one — lets the
/// remote-peer auth path run from the same machine (connecting to one's
/// own interface address keeps that address as the peer's source).
fn non_loopback_host() -> Option<String> {
    let sock = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.connect("8.8.8.8:80").ok()?;
    let ip = sock.local_addr().ok()?.ip();
    (!ip.is_loopback()).then(|| ip.to_string())
}

/// Bind ws on 0.0.0.0 and return a dialable non-loopback address;
/// `None` when the host has no non-loopback interface (test skips).
async fn bind_ws_exposed(auth: Option<AuthVerifier>) -> Option<(String, super::super::Listener)> {
    let host = non_loopback_host()?;
    let listener = bind(&SocketAddr::Ws("0.0.0.0:0".into()), auth)
        .await
        .unwrap();
    let port = listener.local_addr().unwrap().port();
    Some((format!("{host}:{port}"), listener))
}

/// Probe whether dialing `addr` reaches this listener as a *remote*
/// (non-loopback) peer: macOS hairpins self-dials through lo0 (the peer
/// then shows up as 127.0.0.1) and fake-IP VPNs hijack the dial before
/// it reaches us — in both cases the remote path is untestable here.
/// Consumes one accepted connection off the listener.
async fn dial_arrives_remote(listener: &super::super::Listener, addr: &str, token: &str) -> bool {
    let client = tokio::spawn({
        let addr = addr.to_string();
        let token = token.to_string();
        async move { client_ping(&addr, Some(&token)).await }
    });
    let accepted = tokio::time::timeout(std::time::Duration::from_secs(3), listener.accept()).await;
    let remote = match accepted {
        Ok(Ok((stream, Some(peer)))) => {
            drop(stream);
            !peer.ip().is_loopback()
        }
        // Hijacked dial (never reached us) or failed accept: not testable.
        _ => false,
    };
    let _ = client.await;
    remote
}

/// Accept one connection, expect a Ping, answer with a Pong.
async fn ping_pong_once(listener: super::super::Listener) {
    let (stream, _) = listener.accept().await.unwrap();
    let (mut r, mut w) = stream.into_split();
    let msg = recv_frame(&mut r).await.unwrap();
    assert_eq!(msg, WireMsg::Ping);
    send_frame(&mut w, &WireMsg::Pong).await.unwrap();
}

async fn client_ping(addr: &str, token: Option<&str>) -> std::io::Result<()> {
    let stream = connect_with_token(&SocketAddr::Ws(addr.into()), token).await?;
    let (mut r, mut w) = stream.into_split();
    send_frame(&mut w, &WireMsg::Ping).await?;
    let msg = recv_frame(&mut r).await?;
    assert_eq!(msg, WireMsg::Pong);
    Ok(())
}

#[test]
fn bind_exposure_by_address() {
    use super::super::bind_is_exposed;
    // Loopback literals and unix sockets are never exposed.
    assert!(!bind_is_exposed(&SocketAddr::Ws("127.0.0.1:9000".into())));
    assert!(!bind_is_exposed(&SocketAddr::Ws("127.0.0.8:9000".into())));
    assert!(!bind_is_exposed(&SocketAddr::Ws("[::1]:9000".into())));
    assert!(!bind_is_exposed(&SocketAddr::Unix("/tmp/x.sock".into())));
    // Wildcard / LAN / wss / unresolvable hostnames are exposed.
    assert!(bind_is_exposed(&SocketAddr::Ws("0.0.0.0:9000".into())));
    assert!(bind_is_exposed(&SocketAddr::Ws("[::]:9000".into())));
    assert!(bind_is_exposed(&SocketAddr::Ws("192.168.1.2:9000".into())));
    assert!(bind_is_exposed(&SocketAddr::Wss("example.com:443".into())));
}

#[tokio::test]
async fn ws_without_auth_accepts_any_client() {
    let (addr, listener) = bind_ws(None).await;
    let server = tokio::spawn(ping_pong_once(listener));
    client_ping(&addr, None).await.unwrap();
    server.await.unwrap();
}

#[test]
fn peer_gate_exempts_only_loopback() {
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
    let verify = auth_verifier(&hash_password("s3cret"));
    let gate = |ip: IpAddr| super::super::peer_gate(&Some(verify.clone()), ip).is_some();
    // Loopback v4/v6 exempt (incl. the full 127/8).
    assert!(!gate(IpAddr::V4(Ipv4Addr::LOCALHOST)));
    assert!(!gate(IpAddr::V4(Ipv4Addr::new(127, 0, 1, 2))));
    assert!(!gate(IpAddr::V6(Ipv6Addr::LOCALHOST)));
    // Everything remote must authenticate.
    assert!(gate(IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8))));
    assert!(gate(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 20))));
    assert!(gate(IpAddr::V6("fe80::1".parse().unwrap())));
    assert!(gate(IpAddr::V6("2408::1".parse().unwrap())));
    // No verifier configured: nothing to gate with.
    assert!(super::super::peer_gate(&None, IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8))).is_none());
}

#[tokio::test]
async fn ws_loopback_peer_bypasses_auth() {
    let verify = auth_verifier(&hash_password("s3cret"));
    let (addr, listener) = bind_ws(Some(verify)).await;
    let server = tokio::spawn(ping_pong_once(listener));
    // No token at all: loopback peers are exempt from socket auth.
    client_ping(&addr, None).await.unwrap();
    server.await.unwrap();
}

/// The remote-peer 401 path is exercised through the host's own
/// non-loopback NIC address; `dial_arrives_remote` probes first and the
/// test skips when self-dials can't present a remote peer (macOS lo0
/// hairpin, fake-IP VPN hijack).
#[tokio::test]
async fn ws_with_auth_rejects_missing_token() {
    let verify = auth_verifier(&hash_password("s3cret"));
    let Some((addr, listener)) = bind_ws_exposed(Some(verify)).await else {
        eprintln!("skip: host has no non-loopback interface");
        return;
    };
    if !dial_arrives_remote(&listener, &addr, "s3cret").await {
        eprintln!("skip: self-dial does not present a remote peer on this host");
        return;
    }
    let server = tokio::spawn(async move {
        // The handshake is rejected, so accept() errors instead of
        // producing a stream.
        assert!(listener.accept().await.is_err());
    });
    let start = std::time::Instant::now();
    let err = client_ping(&addr, None).await.unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);
    server.await.unwrap();
    // Failed handshakes are held back to throttle online brute force.
    assert!(start.elapsed() >= super::super::WS_HANDSHAKE_FAILURE_DELAY);
}

#[tokio::test]
async fn ws_with_auth_rejects_wrong_token() {
    let verify = auth_verifier(&hash_password("s3cret"));
    let Some((addr, listener)) = bind_ws_exposed(Some(verify)).await else {
        eprintln!("skip: host has no non-loopback interface");
        return;
    };
    if !dial_arrives_remote(&listener, &addr, "s3cret").await {
        eprintln!("skip: self-dial does not present a remote peer on this host");
        return;
    }
    let server = tokio::spawn(async move {
        assert!(listener.accept().await.is_err());
    });
    let err = client_ping(&addr, Some("nope")).await.unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);
    server.await.unwrap();
}

#[tokio::test]
async fn ws_with_auth_accepts_correct_token() {
    let verify = auth_verifier(&hash_password("s3cret"));
    let Some((addr, listener)) = bind_ws_exposed(Some(verify)).await else {
        eprintln!("skip: host has no non-loopback interface");
        return;
    };
    if !dial_arrives_remote(&listener, &addr, "s3cret").await {
        eprintln!("skip: self-dial does not present a remote peer on this host");
        return;
    }
    let server = tokio::spawn(ping_pong_once(listener));
    client_ping(&addr, Some("s3cret")).await.unwrap();
    server.await.unwrap();
}
