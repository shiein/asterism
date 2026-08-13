use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::error::{Result, SyncError};
use crate::protocol::Envelope;

pub const MAX_FRAME: usize = 1024 * 1024;

pub async fn write_envelope<W: AsyncWrite + Unpin>(mut w: W, env: &Envelope) -> Result<()> {
    let bytes = serde_json::to_vec(env).map_err(|e| SyncError::Protocol(e.to_string()))?;
    if bytes.len() > MAX_FRAME {
        return Err(SyncError::Protocol("frame too large".into()));
    }
    w.write_u32(bytes.len() as u32).await?;
    w.write_all(&bytes).await?;
    w.flush().await?;
    Ok(())
}

pub async fn read_envelope<R: AsyncRead + Unpin>(mut r: R) -> Result<Envelope> {
    let len = r.read_u32().await? as usize;
    if len == 0 || len > MAX_FRAME {
        return Err(SyncError::Protocol(format!("invalid frame length {len}")));
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf).await?;
    serde_json::from_slice(&buf).map_err(|e| SyncError::Protocol(e.to_string()))
}
