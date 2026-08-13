use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use asterism_crypto::aead::{CHUNK_SIZE, EncryptedChunk, decrypt_chunk, encrypt_chunk};
use asterism_crypto::keys::ItemKey;

use crate::error::{Result, SyncError};

/// 大文件固定分块流式传输。Hash 在读流时边读边算，不先读完整文件。
pub async fn send_chunks<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
    mut reader: R,
    mut writer: W,
    item_key: &ItemKey,
    blob_id: [u8; 32],
) -> Result<u64> {
    let mut index = 0u32;
    let mut total = 0u64;
    let mut buf = vec![0u8; CHUNK_SIZE];
    loop {
        let n = reader.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        let chunk = encrypt_chunk(item_key, blob_id, index, &buf[..n])
            .map_err(|e| SyncError::Failed(e.to_string()))?;
        let encoded = serde_json::to_vec(&chunk).map_err(|e| SyncError::Protocol(e.to_string()))?;
        writer.write_u32(encoded.len() as u32).await?;
        writer.write_all(&encoded).await?;
        total += n as u64;
        index += 1;
    }
    writer.write_u32(0).await?;
    writer.flush().await?;
    Ok(total)
}

pub async fn recv_chunks<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
    mut reader: R,
    mut writer: W,
    item_key: &ItemKey,
) -> Result<u64> {
    let mut total = 0u64;
    loop {
        let len = reader.read_u32().await? as usize;
        if len == 0 {
            break;
        }
        if len > CHUNK_SIZE + 128 {
            return Err(SyncError::Protocol("chunk too large".into()));
        }
        let mut buf = vec![0u8; len];
        reader.read_exact(&mut buf).await?;
        let chunk: EncryptedChunk =
            serde_json::from_slice(&buf).map_err(|e| SyncError::Protocol(e.to_string()))?;
        let plain =
            decrypt_chunk(item_key, &chunk).map_err(|e| SyncError::Failed(e.to_string()))?;
        writer.write_all(&plain).await?;
        total += plain.len() as u64;
    }
    writer.flush().await?;
    Ok(total)
}
