use super::*;
use std::collections::HashMap;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// Env getter backed by a map, so proxy resolution is testable without touching
/// the process environment.
fn getter<'a>(
    map: &'a HashMap<&'static str, &'static str>,
) -> impl Fn(&str) -> Option<String> + 'a {
    move |k| map.get(k).map(|v| (*v).to_string())
}

fn authority(proxy: &HttpProxy) -> &str {
    &proxy.authority
}

#[test]
fn parse_proxy_accepts_scheme_bare_authority_and_credentials() {
    assert_eq!(
        authority(&parse_proxy("http://127.0.0.1:7890").unwrap()),
        "127.0.0.1:7890"
    );
    assert_eq!(
        authority(&parse_proxy("  127.0.0.1:7890  ").unwrap()),
        "127.0.0.1:7890"
    );
    // Scheme is case-insensitive; trailing path is dropped.
    assert_eq!(
        authority(&parse_proxy("HTTP://host:3128/").unwrap()),
        "host:3128"
    );

    // Port defaults to 80 when omitted.
    assert_eq!(
        authority(&parse_proxy("http://proxy.local").unwrap()),
        "proxy.local:80"
    );

    // Credentials are split off the authority.
    let with_creds = parse_proxy("http://user:pass@proxy.corp:8080").unwrap();
    assert_eq!(authority(&with_creds), "proxy.corp:8080");
    assert_eq!(with_creds.credentials.as_deref(), Some("user:pass"));
}

#[test]
fn parse_proxy_rejects_empty_and_unsupported_schemes() {
    assert!(parse_proxy("").is_none());
    assert!(parse_proxy("   ").is_none());
    assert!(parse_proxy("socks5://127.0.0.1:1080").is_none());
    assert!(parse_proxy("https://proxy.local:8080").is_none());
    assert!(parse_proxy("http://").is_none());
}

#[test]
fn parse_proxy_handles_bracketed_ipv6_and_rejects_bare() {
    assert_eq!(
        authority(&parse_proxy("http://[::1]:7890").unwrap()),
        "[::1]:7890"
    );
    // The default port is inserted inside the bracketed authority.
    assert_eq!(authority(&parse_proxy("http://[::1]").unwrap()), "[::1]:80");
    // A bare IPv6 literal cannot form a `host:port` authority.
    assert!(parse_proxy("http://::1").is_none());
}

#[test]
fn no_proxy_match_handles_wildcard_hosts_ports_and_ipv6() {
    assert!(no_proxy_match("*", "open.feishu.cn"));
    assert!(no_proxy_match("localhost,127.0.0.1,::1", "127.0.0.1"));
    assert!(no_proxy_match("localhost,127.0.0.1,::1", "::1"));
    // An unbracketed host still matches a bracketed entry.
    assert!(no_proxy_match("[::1]", "::1"));
    assert!(!no_proxy_match("localhost,127.0.0.1,::1", "open.feishu.cn"));
    assert!(no_proxy_match(".example.com", "api.example.com"));
    assert!(no_proxy_match("example.com", "api.example.com"));
    assert!(!no_proxy_match("example.com", "notexample.com"));
    assert!(no_proxy_match("example.com:8080", "example.com"));
}

#[test]
fn tls_proxy_with_precedence_and_no_proxy() {
    let only_upper = HashMap::from([("HTTPS_PROXY", "http://p:1")]);
    assert_eq!(
        authority(&tls_proxy_with(&getter(&only_upper), "open.feishu.cn").unwrap()),
        "p:1"
    );

    // Lowercase is honored too.
    let lower = HashMap::from([("https_proxy", "http://p:1")]);
    assert_eq!(
        authority(&tls_proxy_with(&getter(&lower), "open.feishu.cn").unwrap()),
        "p:1"
    );

    // A set-but-empty or unusable value falls through to the next variable.
    let empty = HashMap::from([("HTTPS_PROXY", ""), ("ALL_PROXY", "http://a:2")]);
    assert_eq!(
        authority(&tls_proxy_with(&getter(&empty), "open.feishu.cn").unwrap()),
        "a:2"
    );
    let unusable = HashMap::from([("HTTPS_PROXY", "socks5://x:1"), ("ALL_PROXY", "http://a:2")]);
    assert_eq!(
        authority(&tls_proxy_with(&getter(&unusable), "open.feishu.cn").unwrap()),
        "a:2"
    );

    // NO_PROXY bypasses an otherwise valid proxy.
    let bypassed = HashMap::from([
        ("HTTPS_PROXY", "http://p:1"),
        ("NO_PROXY", "open.feishu.cn"),
    ]);
    assert!(tls_proxy_with(&getter(&bypassed), "open.feishu.cn").is_none());

    assert!(tls_proxy_with(&getter(&HashMap::new()), "open.feishu.cn").is_none());
}

#[tokio::test]
async fn proxy_connect_sends_connect_and_accepts_200() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        let mut buf = [0u8; 256];
        let n = sock.read(&mut buf).await.unwrap();
        sock.write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
            .await
            .unwrap();
        String::from_utf8_lossy(&buf[..n]).to_string()
    });

    let mut client = tokio::net::TcpStream::connect(addr).await.unwrap();
    proxy_connect(&mut client, "open.feishu.cn", 443, None)
        .await
        .unwrap();

    let request = server.await.unwrap();
    assert!(
        request.starts_with("CONNECT open.feishu.cn:443 HTTP/1.1\r\n"),
        "got: {request:?}"
    );
    assert!(
        request.contains("Host: open.feishu.cn:443\r\n"),
        "got: {request:?}"
    );
}

#[tokio::test]
async fn proxy_connect_sends_basic_credentials() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        let mut buf = [0u8; 256];
        let n = sock.read(&mut buf).await.unwrap();
        sock.write_all(b"HTTP/1.1 200 OK\r\n\r\n").await.unwrap();
        String::from_utf8_lossy(&buf[..n]).to_string()
    });

    let mut client = tokio::net::TcpStream::connect(addr).await.unwrap();
    proxy_connect(&mut client, "open.feishu.cn", 443, Some("user:pass"))
        .await
        .unwrap();

    let request = server.await.unwrap();
    // base64("user:pass") == "dXNlcjpwYXNz"
    assert!(
        request.contains("Proxy-Authorization: Basic dXNlcjpwYXNz\r\n"),
        "got: {request:?}"
    );
}

#[tokio::test]
async fn proxy_connect_brackets_ipv6_targets() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        let mut buf = [0u8; 256];
        let n = sock.read(&mut buf).await.unwrap();
        sock.write_all(b"HTTP/1.1 200 OK\r\n\r\n").await.unwrap();
        String::from_utf8_lossy(&buf[..n]).to_string()
    });

    let mut client = tokio::net::TcpStream::connect(addr).await.unwrap();
    proxy_connect(&mut client, "::1", 443, None).await.unwrap();

    let request = server.await.unwrap();
    assert!(
        request.starts_with("CONNECT [::1]:443 "),
        "got: {request:?}"
    );
}

#[tokio::test]
async fn proxy_connect_reports_non_200() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        let mut buf = [0u8; 256];
        let _ = sock.read(&mut buf).await;
        let _ = sock
            .write_all(b"HTTP/1.1 407 Proxy Authentication Required\r\n\r\n")
            .await;
    });

    let mut client = tokio::net::TcpStream::connect(addr).await.unwrap();
    let err = proxy_connect(&mut client, "open.feishu.cn", 443, None)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("407"), "got: {err}");
}

#[tokio::test]
async fn proxy_connect_errors_when_proxy_closes() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        let mut buf = [0u8; 256];
        let _ = sock.read(&mut buf).await;
        drop(sock);
    });

    let mut client = tokio::net::TcpStream::connect(addr).await.unwrap();
    let err = proxy_connect(&mut client, "open.feishu.cn", 443, None)
        .await
        .unwrap_err();
    assert!(
        matches!(err, Error::Io(ref e) if e.kind() == io::ErrorKind::UnexpectedEof),
        "got: {err}"
    );
}
