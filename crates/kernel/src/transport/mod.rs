use crate::wire::WireMsg;
use futures::{SinkExt, StreamExt};
use std::io;
use std::path::PathBuf;
use std::pin::Pin;
use std::str::FromStr;
use std::task::Poll;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Convert a tungstenite error into an `io::Error`, preserving
/// underlying I/O errors (connection refused, DNS failure, TLS …).
fn map_tungstenite_err(e: tokio_tungstenite::tungstenite::Error) -> io::Error {
    match e {
        tokio_tungstenite::tungstenite::Error::Io(io_err) => io_err,
        _ => io::Error::new(io::ErrorKind::InvalidData, e),
    }
}

/// IPC address: either a Unix domain socket path, a TCP endpoint, or a WebSocket endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SocketAddr {
    Unix(PathBuf),
    Tcp(String),
    Ws(String),
    Wss(String),
}

impl SocketAddr {
    /// Default TCP endpoint used on Windows (or when explicitly requested).
    pub fn localhost(port: u16) -> Self {
        Self::Tcp(format!("127.0.0.1:{port}"))
    }
}

impl FromStr for SocketAddr {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if let Some(path) = s.strip_prefix("unix://") {
            Ok(Self::Unix(path.into()))
        } else if let Some(addr) = s.strip_prefix("tcp://") {
            Ok(Self::Tcp(addr.to_string()))
        } else if let Some(addr) = s.strip_prefix("ws://") {
            Ok(Self::Ws(addr.to_string()))
        } else if let Some(addr) = s.strip_prefix("wss://") {
            Ok(Self::Wss(addr.to_string()))
        } else if s.contains('/')
            || std::path::Path::new(s)
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("sock"))
        {
            Ok(Self::Unix(s.into()))
        } else {
            Ok(Self::Tcp(s.to_string()))
        }
    }
}

impl std::fmt::Display for SocketAddr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unix(p) => write!(f, "unix://{}", p.display()),
            Self::Tcp(s) => write!(f, "tcp://{s}"),
            Self::Ws(s) => write!(f, "ws://{s}"),
            Self::Wss(s) => write!(f, "wss://{s}"),
        }
    }
}

/// Resolve the daemon socket address from environment or platform default.
///
/// Priority:
/// 1. `YOMI_SOCKET` environment variable (if set)
/// 2. Unix: `$XDG_RUNTIME_DIR/yomi/daemon.sock`
/// 3. Unix fallback: `directories::BaseDirs::data_dir()/yomi/daemon.sock` (macOS: `~/Library/Application Support/`)
/// 4. Final fallback: `/tmp/yomi-daemon.sock`
/// 5. Windows: `Tcp("127.0.0.1:57231")`
pub fn socket_addr() -> SocketAddr {
    let socket_env = format!("{}SOCKET", crate::ENV_PREFIX);
    if let Ok(val) = std::env::var(&socket_env) {
        return val.parse().expect("Invalid YOMI_SOCKET format");
    }
    #[cfg(unix)]
    {
        SocketAddr::Unix(std::env::var_os("XDG_RUNTIME_DIR").map_or_else(
            || {
                directories::BaseDirs::new().map_or_else(
                    || std::path::PathBuf::from("/tmp/yomi-daemon.sock"),
                    |b| b.data_dir().join("yomi/daemon.sock"),
                )
            },
            |p| std::path::PathBuf::from(p).join("yomi/daemon.sock"),
        ))
    }
    #[cfg(not(unix))]
    {
        SocketAddr::Tcp("127.0.0.1:57231".to_string())
    }
}

/// PID file used for daemon process tracking.
///
/// Derived from [`socket_addr()`]:
/// - Unix socket: sibling `.pid` file next to the socket
/// - TCP: `data_dir()/yomi-daemon-{port}.pid`
pub fn pid_file_path() -> PathBuf {
    match socket_addr() {
        SocketAddr::Unix(path) => {
            let mut p = path;
            p.set_extension("pid");
            p
        }
        SocketAddr::Tcp(ref addr_str)
        | SocketAddr::Ws(ref addr_str)
        | SocketAddr::Wss(ref addr_str) => {
            let port = addr_str.rsplit_once(':').map_or("tcp", |(_, p)| p);
            directories::BaseDirs::new().map_or_else(
                || std::env::temp_dir().join(format!("yomi-daemon-{port}.pid")),
                |b| b.data_dir().join(format!("yomi-daemon-{port}.pid")),
            )
        }
    }
}

pub type WsStream = tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>;
pub type WssStream =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// Platform-agnostic stream.
#[allow(clippy::large_enum_variant)]
pub enum Stream {
    #[cfg(unix)]
    Unix(tokio::net::UnixStream),
    Tcp(tokio::net::TcpStream),
    Ws(WsStream),
    Wss(WssStream),
}

/// Read half of a WebSocket connection bridged through an async task.
struct WsReadHalf {
    rx: tokio::sync::mpsc::UnboundedReceiver<Vec<u8>>,
    buffer: Vec<u8>,
    pos: usize,
}

impl AsyncRead for WsReadHalf {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        loop {
            if self.pos < self.buffer.len() {
                let avail = self.buffer.len() - self.pos;
                let to_copy = avail.min(buf.remaining());
                buf.put_slice(&self.buffer[self.pos..self.pos + to_copy]);
                self.pos += to_copy;
                return Poll::Ready(Ok(()));
            }

            match self.rx.poll_recv(cx) {
                Poll::Ready(Some(data)) => {
                    self.buffer = data;
                    self.pos = 0;
                }
                Poll::Ready(None) => return Poll::Ready(Ok(())), // EOF
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

/// Write half of a WebSocket connection bridged through an async task.
struct WsWriteHalf {
    tx: tokio::sync::mpsc::UnboundedSender<Vec<u8>>,
    buffer: Vec<u8>,
}

impl AsyncWrite for WsWriteHalf {
    fn poll_write(
        mut self: Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        self.buffer.extend_from_slice(buf);
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(
        mut self: Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> Poll<io::Result<()>> {
        if self.buffer.is_empty() {
            return Poll::Ready(Ok(()));
        }
        let cap = self.buffer.capacity();
        let data = std::mem::replace(&mut self.buffer, Vec::with_capacity(cap));
        self.tx
            .send(data)
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "ws write channel closed"))?;
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<io::Result<()>> {
        self.as_mut().poll_flush(cx)
    }
}

/// Spawn a bridge task that forwards between a WebSocket stream and
/// unbounded byte channels, returning `(ReadHalf, WriteHalf)`.
///
/// Uses unbounded channels because this is internal IPC with a single
/// consumer (`send_frame` / `recv_frame`) that always flushes promptly.
fn spawn_ws_bridge<S, E>(ws_stream: S) -> (ReadHalf, WriteHalf)
where
    S: futures::Sink<tokio_tungstenite::tungstenite::Message, Error = E>
        + futures::Stream<Item = Result<tokio_tungstenite::tungstenite::Message, E>>
        + Unpin
        + Send
        + 'static,
    E: std::fmt::Display + Send,
{
    let (read_tx, read_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
    let (write_tx, mut write_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();

    tokio::spawn(async move {
        let mut ws_stream = ws_stream;
        loop {
            tokio::select! {
                maybe_data = write_rx.recv() => {
                    match maybe_data {
                        Some(data) => {
                            let msg = tokio_tungstenite::tungstenite::Message::Binary(data.into());
                            if let Err(e) = ws_stream.send(msg).await {
                                tracing::warn!("WebSocket send error: {e}");
                                break;
                            }
                        }
                        None => {
                            let _ = tokio::time::timeout(
                                std::time::Duration::from_secs(5),
                                ws_stream.close(),
                            )
                            .await;
                            break;
                        }
                    }
                }
                msg = ws_stream.next() => {
                    use tokio_tungstenite::tungstenite::Message;
                    match msg {
                        Some(Ok(Message::Binary(data))) => {
                            if read_tx.send(data.to_vec()).is_err() {
                                break;
                            }
                        }
                        Some(Ok(Message::Text(text))) => {
                            if read_tx.send(text.as_str().as_bytes().to_vec()).is_err() {
                                break;
                            }
                        }
                        Some(Ok(Message::Close(_))) | None => break,
                        Some(Ok(_)) => {} // Ping/Pong handled by tungstenite internally
                        Some(Err(e)) => {
                            tracing::warn!("WebSocket recv error: {e}");
                            break;
                        }
                    }
                }
            }
        }
    });

    (
        Box::new(WsReadHalf {
            rx: read_rx,
            buffer: Vec::new(),
            pos: 0,
        }),
        Box::new(WsWriteHalf {
            tx: write_tx,
            buffer: Vec::new(),
        }),
    )
}

impl Stream {
    pub fn into_split(self) -> (ReadHalf, WriteHalf) {
        match self {
            #[cfg(unix)]
            Self::Unix(s) => {
                let (r, w) = s.into_split();
                (Box::new(r), Box::new(w))
            }
            Self::Tcp(s) => {
                let (r, w) = s.into_split();
                (Box::new(r), Box::new(w))
            }
            Self::Ws(ws_stream) => spawn_ws_bridge(ws_stream),
            Self::Wss(ws_stream) => spawn_ws_bridge(ws_stream),
        }
    }
}

/// Platform-agnostic listener.
pub enum Listener {
    #[cfg(unix)]
    Unix(tokio::net::UnixListener),
    Tcp(tokio::net::TcpListener),
    Ws(tokio::net::TcpListener),
}

impl Listener {
    pub async fn accept(&self) -> io::Result<(Stream, Option<std::net::SocketAddr>)> {
        match self {
            #[cfg(unix)]
            Self::Unix(l) => {
                let (stream, _) = l.accept().await?;
                Ok((Stream::Unix(stream), None))
            }
            Self::Tcp(l) => {
                let (stream, addr) = l.accept().await?;
                stream.set_nodelay(true)?;
                Ok((Stream::Tcp(stream), Some(addr)))
            }
            Self::Ws(l) => {
                let (stream, addr) = l.accept().await?;
                let ws_stream = tokio_tungstenite::accept_async(stream)
                    .await
                    .map_err(map_tungstenite_err)?;
                Ok((Stream::Ws(ws_stream), Some(addr)))
            }
        }
    }
}

pub type ReadHalf = Box<dyn AsyncRead + Unpin + Send>;
pub type WriteHalf = Box<dyn AsyncWrite + Unpin + Send>;

const TCP_LISTEN_BACKLOG: u32 = 128;

fn bind_tcp(addr_str: &str) -> io::Result<tokio::net::TcpListener> {
    let addr: std::net::SocketAddr = addr_str
        .parse()
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
    let socket = if addr.is_ipv4() {
        tokio::net::TcpSocket::new_v4()?
    } else {
        tokio::net::TcpSocket::new_v6()?
    };
    socket.set_reuseaddr(true)?;
    socket.bind(addr)?;
    socket.listen(TCP_LISTEN_BACKLOG)
}

/// Bind a listener at the given address.
pub async fn bind(addr: &SocketAddr) -> io::Result<Listener> {
    match addr {
        SocketAddr::Unix(path) => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Some(parent) = path.parent() {
                    tokio::fs::create_dir_all(parent).await?;
                }
                match tokio::net::UnixStream::connect(path).await {
                    Ok(_) => {
                        return Err(io::Error::new(
                            io::ErrorKind::AddrInUse,
                            format!("Unix socket is already in use: {}", path.display()),
                        ));
                    }
                    Err(error)
                        if matches!(
                            error.kind(),
                            io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
                        ) =>
                    {
                        let _ = tokio::fs::remove_file(path).await;
                    }
                    Err(_) => {}
                }
                let std_listener = std::os::unix::net::UnixListener::bind(path)?;
                let perms = std::fs::Permissions::from_mode(0o600);
                tokio::fs::set_permissions(path, perms).await?;
                std_listener.set_nonblocking(true)?;
                Ok(Listener::Unix(tokio::net::UnixListener::from_std(
                    std_listener,
                )?))
            }
            #[cfg(not(unix))]
            {
                Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "Unix sockets not supported on this platform",
                ))
            }
        }
        SocketAddr::Tcp(addr_str) => {
            let listener = bind_tcp(addr_str)?;
            Ok(Listener::Tcp(listener))
        }
        SocketAddr::Ws(addr_str) => {
            let listener = bind_tcp(addr_str)?;
            Ok(Listener::Ws(listener))
        }
        SocketAddr::Wss(_) => Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "WebSocket Secure (wss://) listener is not supported; use ws:// or tcp://",
        )),
    }
}

/// Connect to a remote address.
pub async fn connect(addr: &SocketAddr) -> io::Result<Stream> {
    match addr {
        SocketAddr::Unix(path) => {
            #[cfg(unix)]
            {
                let stream = tokio::net::UnixStream::connect(path).await?;
                Ok(Stream::Unix(stream))
            }
            #[cfg(not(unix))]
            {
                Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "Unix sockets not supported on this platform",
                ))
            }
        }
        SocketAddr::Tcp(addr_str) => {
            let stream = tokio::net::TcpStream::connect(addr_str).await?;
            stream.set_nodelay(true)?;
            Ok(Stream::Tcp(stream))
        }
        SocketAddr::Ws(addr_str) => {
            let stream = tokio::net::TcpStream::connect(addr_str).await?;
            stream.set_nodelay(true)?;
            let url = format!("ws://{addr_str}");
            let (ws_stream, _) = tokio_tungstenite::client_async(url, stream)
                .await
                .map_err(map_tungstenite_err)?;
            Ok(Stream::Ws(ws_stream))
        }
        SocketAddr::Wss(addr_str) => {
            let url = format!("wss://{addr_str}");
            let (ws_stream, _) = tokio_tungstenite::connect_async(url)
                .await
                .map_err(map_tungstenite_err)?;
            Ok(Stream::Wss(ws_stream))
        }
    }
}

/// Maximum frame size: 8 MiB.
const MAX_FRAME_SIZE: usize = 8 * 1024 * 1024;

/// Send a length-prefixed JSON frame.
pub async fn send_frame<W: AsyncWrite + Unpin>(writer: &mut W, msg: &WireMsg) -> io::Result<()> {
    let payload = serde_json::to_vec(msg)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("serialize error: {e}")))?;
    if payload.len() > MAX_FRAME_SIZE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("frame too large: {} > {MAX_FRAME_SIZE}", payload.len()),
        ));
    }
    let len = payload.len() as u32;
    writer.write_all(&len.to_be_bytes()).await?;
    writer.write_all(&payload).await?;
    writer.flush().await
}

/// Receive a length-prefixed JSON frame.
pub async fn recv_frame<R: AsyncRead + Unpin>(reader: &mut R) -> io::Result<WireMsg> {
    let mut len_buf = [0u8; 4];
    reader.read_exact(&mut len_buf).await?;
    let len = u32::from_be_bytes(len_buf) as usize;
    if len > MAX_FRAME_SIZE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("frame too large: {len} > {MAX_FRAME_SIZE}"),
        ));
    }
    let mut payload = vec![0u8; len];
    reader.read_exact(&mut payload).await?;
    serde_json::from_slice(&payload).map_err(|e| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("deserialize error: {e}"),
        )
    })
}
