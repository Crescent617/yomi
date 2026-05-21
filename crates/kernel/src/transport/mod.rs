use crate::wire::WireMsg;
use std::io;
use std::path::PathBuf;
use std::str::FromStr;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// IPC address: either a Unix domain socket path or a TCP endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SocketAddr {
    Unix(PathBuf),
    Tcp(String),
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
        }
    }
}

/// Platform-agnostic stream.
pub enum Stream {
    #[cfg(unix)]
    Unix(tokio::net::UnixStream),
    Tcp(tokio::net::TcpStream),
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
        }
    }
}

/// Platform-agnostic listener.
pub enum Listener {
    #[cfg(unix)]
    Unix(tokio::net::UnixListener),
    Tcp(tokio::net::TcpListener),
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
        }
    }
}

pub type ReadHalf = Box<dyn AsyncRead + Unpin + Send>;
pub type WriteHalf = Box<dyn AsyncWrite + Unpin + Send>;

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
                let _ = tokio::fs::remove_file(path).await;
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
            let listener = socket.listen(128)?;
            Ok(Listener::Tcp(listener))
        }
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
