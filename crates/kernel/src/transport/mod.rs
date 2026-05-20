#[cfg(unix)]
pub mod unix;

use crate::wire::WireMsg;
use std::io;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Maximum frame size: 8 MiB (prevent malicious peers from `OOMing` us).
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
