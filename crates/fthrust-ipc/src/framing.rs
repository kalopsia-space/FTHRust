//! Length-prefixed JSON frames: a little-endian `u32` byte count followed
//! by the JSON document.

use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::{IpcError, MAX_FRAME_BYTES};

/// Encodes one message as a complete frame (header + body).
pub fn encode<T: Serialize>(msg: &T) -> Result<Vec<u8>, IpcError> {
    let body = serde_json::to_vec(msg)?;
    if body.len() > MAX_FRAME_BYTES {
        return Err(IpcError::FrameTooLarge(body.len()));
    }
    let mut out = Vec::with_capacity(4 + body.len());
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&body);
    Ok(out)
}

/// Decodes one frame body.
pub fn decode<T: DeserializeOwned>(body: &[u8]) -> Result<T, IpcError> {
    Ok(serde_json::from_slice(body)?)
}

/// Reads frames from an async byte stream.
pub struct FrameReader<R> {
    inner: R,
}

impl<R: AsyncRead + Unpin> FrameReader<R> {
    /// Wraps a reader.
    pub fn new(inner: R) -> Self {
        Self { inner }
    }

    /// Reads the next message; `Ok(None)` on a clean end of stream (between
    /// frames), `Truncated` when the stream ends inside a frame.
    pub async fn read<T: DeserializeOwned>(&mut self) -> Result<Option<T>, IpcError> {
        let mut header = [0u8; 4];
        let mut filled = 0;
        while filled < 4 {
            let n = self.inner.read(&mut header[filled..]).await?;
            if n == 0 {
                return if filled == 0 {
                    Ok(None)
                } else {
                    Err(IpcError::Truncated)
                };
            }
            filled += n;
        }
        let len = u32::from_le_bytes(header) as usize;
        if len > MAX_FRAME_BYTES {
            return Err(IpcError::FrameTooLarge(len));
        }
        let mut body = vec![0u8; len];
        self.inner.read_exact(&mut body).await.map_err(|e| {
            if e.kind() == std::io::ErrorKind::UnexpectedEof {
                IpcError::Truncated
            } else {
                IpcError::Io(e)
            }
        })?;
        decode(&body).map(Some)
    }
}

/// Writes frames to an async byte stream.
pub struct FrameWriter<W> {
    inner: W,
}

impl<W: AsyncWrite + Unpin> FrameWriter<W> {
    /// Wraps a writer.
    pub fn new(inner: W) -> Self {
        Self { inner }
    }

    /// Writes one message as a frame and flushes.
    pub async fn write<T: Serialize>(&mut self, msg: &T) -> Result<(), IpcError> {
        let frame = encode(msg)?;
        self.inner.write_all(&frame).await?;
        self.inner.flush().await?;
        Ok(())
    }
}
