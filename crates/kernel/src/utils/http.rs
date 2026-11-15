use std::fmt::Write as _;
use std::io;
use std::sync::OnceLock;
use std::time::Duration;

use base64::Engine as _;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Error;

const REQUEST_TIMEOUT: Duration = Duration::from_mins(1);
const MAX_REDIRECTS: usize = 5;
/// Budget for reaching a proxy and opening the tunnel, before TLS starts.
const PROXY_CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// Shared HTTP client for general-purpose web requests.
static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();

/// Returns the shared HTTP client with connection pooling.
pub fn client() -> &'static reqwest::Client {
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .redirect(reqwest::redirect::Policy::limited(MAX_REDIRECTS))
            .build()
            .expect("failed to build shared HTTP client")
    })
}

/// Proxy environment variables consulted for outbound TLS connections, in
/// precedence order.
const TLS_PROXY_ENV: [&str; 4] = ["HTTPS_PROXY", "https_proxy", "ALL_PROXY", "all_proxy"];
/// Host lists that must be reached without a proxy.
const NO_PROXY_ENV: [&str; 2] = ["NO_PROXY", "no_proxy"];
/// Port assumed when a proxy URL omits one (only `http` proxies are supported).
const DEFAULT_PROXY_PORT: u16 = 80;

/// An HTTP proxy parsed from the environment.
struct HttpProxy {
    /// `host:port` to open the TCP connection and issue `CONNECT` against.
    authority: String,
    /// `user:pass` from the proxy URL, sent as Basic proxy auth when present.
    credentials: Option<String>,
}

/// Connect a `wss://` handshake, tunnelling through the environment proxy when
/// one is set. Without a proxy (or for a host listed in `NO_PROXY`) this is a
/// plain `connect_async`; with one it opens a TCP connection to the proxy,
/// issues an HTTP `CONNECT` to the target, then runs the TLS and WebSocket
/// handshakes over that tunnel — so a secure WebSocket honors the same proxy
/// variables as the REST calls.
pub async fn connect_wss<R>(request: R) -> Result<crate::transport::WssStream, Error>
where
    R: IntoClientRequest + Unpin,
{
    let request = request.into_client_request()?;
    let uri = request.uri().clone();
    // `Uri::host` keeps the brackets around IPv6 literals; strip them so both
    // the CONNECT authority and the NO_PROXY match are well-formed.
    let host = uri
        .host()
        .unwrap_or_default()
        .trim_matches(['[', ']'])
        .to_string();
    let proxy = if host.is_empty() {
        None
    } else {
        tls_proxy_with(&|k| std::env::var(k).ok(), &host)
    };
    let Some(proxy) = proxy else {
        let (ws, _) = tokio_tungstenite::connect_async(request).await?;
        return Ok(ws);
    };

    let port = uri
        .port_u16()
        .unwrap_or(if uri.scheme_str() == Some("wss") {
            443
        } else {
            80
        });

    let stream = tokio::time::timeout(PROXY_CONNECT_TIMEOUT, async {
        let mut stream = tokio::net::TcpStream::connect(&proxy.authority)
            .await
            .map_err(|e| {
                Error::Io(io::Error::new(
                    e.kind(),
                    format!("proxy connect {} failed: {e}", proxy.authority),
                ))
            })?;
        stream.set_nodelay(true)?;
        proxy_connect(&mut stream, &host, port, proxy.credentials.as_deref()).await?;
        Ok::<_, Error>(stream)
    })
    .await
    .map_err(|_| {
        Error::Io(io::Error::new(
            io::ErrorKind::TimedOut,
            format!("proxy {} CONNECT timed out", proxy.authority),
        ))
    })??;

    let (ws, _) = tokio_tungstenite::client_async_tls(request, stream).await?;
    Ok(ws)
}

/// Resolve the proxy for `host` from the environment, honoring `NO_PROXY`.
fn tls_proxy_with(get: &dyn Fn(&str) -> Option<String>, host: &str) -> Option<HttpProxy> {
    if NO_PROXY_ENV
        .iter()
        .filter_map(|k| get(k))
        .any(|list| no_proxy_match(&list, host))
    {
        return None;
    }
    // The first variable that is set *and* parses wins, so an HTTPS_PROXY with
    // an unusable value still falls through to ALL_PROXY.
    TLS_PROXY_ENV
        .iter()
        .filter_map(|k| get(k))
        .find_map(|raw| parse_proxy(&raw))
}

/// Whether `host` matches a `NO_PROXY` list: `*` or any comma-separated entry
/// equal to the host or one of its parent domains.
fn no_proxy_match(list: &str, host: &str) -> bool {
    list.split(',').map(str::trim).any(|entry| {
        if entry.is_empty() {
            return false;
        }
        if entry == "*" {
            return true;
        }
        let entry = entry.strip_prefix('.').unwrap_or(entry);
        // Drop a trailing `:port`, but only when it can't be an IPv6 literal.
        let entry = match entry.rsplit_once(':') {
            Some((head, port)) if !head.contains(':') && port.parse::<u16>().is_ok() => head,
            _ => entry,
        };
        let entry = entry.trim_matches(['[', ']']);
        let host = host.to_ascii_lowercase();
        let entry = entry.to_ascii_lowercase();
        !entry.is_empty() && (host == entry || host.ends_with(&format!(".{entry}")))
    })
}

/// Parse a proxy environment value into an [`HttpProxy`]. Accepts an optional
/// `http://` scheme and `user:pass@` credentials, ignores any path, and returns
/// `None` for empty input or an unsupported scheme (e.g. `https`, `socks5`),
/// so the caller connects directly instead.
fn parse_proxy(raw: &str) -> Option<HttpProxy> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    match raw.split_once("://") {
        None => {}
        Some((scheme, rest)) if scheme.eq_ignore_ascii_case("http") => {
            return build_proxy(rest);
        }
        Some((scheme, _)) => {
            tracing::warn!(scheme, "ignoring unsupported proxy scheme for wss");
            return None;
        }
    }
    build_proxy(raw)
}

/// Split `user:pass@host:port` and default the port when absent.
fn build_proxy(rest: &str) -> Option<HttpProxy> {
    let (credentials, authority) = match rest.rsplit_once('@') {
        Some((creds, authority)) => (Some(creds.to_string()), authority),
        None => (None, rest),
    };
    let authority = authority.split(['/', '?', '#']).next().unwrap_or("");
    if authority.is_empty() {
        return None;
    }
    // An unbracketed IPv6 literal cannot form a `host:port` authority.
    if !authority.starts_with('[') && authority.matches(':').count() > 1 {
        tracing::warn!(authority, "ignoring proxy URL with unbracketed IPv6 host");
        return None;
    }
    Some(HttpProxy {
        authority: with_default_port(authority),
        credentials,
    })
}

/// Append the default proxy port when `authority` carries none.
fn with_default_port(authority: &str) -> String {
    let has_port = if authority.starts_with('[') {
        authority.contains("]:")
    } else {
        authority.matches(':').count() == 1
    };
    if has_port {
        authority.to_string()
    } else {
        format!("{authority}:{DEFAULT_PROXY_PORT}")
    }
}

/// Send an HTTP `CONNECT` and read the reply, returning once the proxy confirms
/// a `200` tunnel to `host:port`.
async fn proxy_connect(
    stream: &mut tokio::net::TcpStream,
    host: &str,
    port: u16,
    credentials: Option<&str>,
) -> Result<(), Error> {
    let authority = if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    };
    let mut request = format!("CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\n");
    if let Some(credentials) = credentials {
        let encoded = base64::engine::general_purpose::STANDARD.encode(credentials);
        let _ = writeln!(request, "Proxy-Authorization: Basic {encoded}\r");
    }
    request.push_str("\r\n");
    stream.write_all(request.as_bytes()).await?;

    // Read the status line and headers one byte at a time, so no part of the
    // tunnelled stream is consumed.
    let mut head = Vec::with_capacity(128);
    let mut byte = [0u8; 1];
    loop {
        if stream.read(&mut byte).await? == 0 {
            return Err(Error::Io(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "proxy closed before CONNECT reply",
            )));
        }
        head.push(byte[0]);
        if head.ends_with(b"\r\n\r\n") {
            break;
        }
        if head.len() > 8192 {
            return Err(Error::Io(io::Error::new(
                io::ErrorKind::InvalidData,
                "proxy CONNECT reply too large",
            )));
        }
    }

    let status = std::str::from_utf8(&head)
        .ok()
        .and_then(|s| s.split_whitespace().nth(1))
        .and_then(|code| code.parse::<u16>().ok());
    if status != Some(200) {
        let head = String::from_utf8_lossy(&head);
        let line = head.lines().next().unwrap_or("").trim();
        return Err(Error::Io(io::Error::other(format!(
            "proxy CONNECT to {authority} failed: {line}"
        ))));
    }
    Ok(())
}

#[cfg(test)]
#[path = "http_test.rs"]
mod tests;
