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

#[tokio::test]
async fn ws_without_auth_accepts_any_client() {
    let (addr, listener) = bind_ws(None).await;
    let server = tokio::spawn(ping_pong_once(listener));
    client_ping(&addr, None).await.unwrap();
    server.await.unwrap();
}

#[tokio::test]
async fn ws_with_auth_rejects_missing_token() {
    let verify = auth_verifier(&hash_password("s3cret"));
    let (addr, listener) = bind_ws(Some(verify)).await;
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
    let (addr, listener) = bind_ws(Some(verify)).await;
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
    let (addr, listener) = bind_ws(Some(verify)).await;
    let server = tokio::spawn(ping_pong_once(listener));
    client_ping(&addr, Some("s3cret")).await.unwrap();
    server.await.unwrap();
}
