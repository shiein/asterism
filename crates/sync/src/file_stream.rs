use std::collections::HashSet;
use std::io::{Read, Seek, SeekFrom, Write};

use asterism_crypto::AccountVaultKey;
use asterism_crypto::aead::{CHUNK_SIZE, EncryptedChunk, decrypt_chunk, encrypt_chunk};
use asterism_crypto::keys::ItemKey;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::error::{Result, SyncError};
use crate::hub_client::HubClient;
use crate::payload::{BlobChunkDecryptor, BlobChunkEncryptor};

/// 大文件分块规划器：支持从任意已上传分块索引断点续传。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResumableUploadPlanner {
    pub file_size: u64,
    pub total_chunks: u32,
}

impl ResumableUploadPlanner {
    pub fn new(file_size: u64) -> Self {
        let total_chunks =
            if file_size == 0 { 1 } else { file_size.div_ceil(CHUNK_SIZE as u64) as u32 };
        Self { file_size, total_chunks }
    }

    pub fn missing_chunks(&self, present: &[u32]) -> Vec<u32> {
        let present_set: HashSet<u32> = present.iter().copied().collect();
        (0..self.total_chunks).filter(|idx| !present_set.contains(idx)).collect()
    }
}

/// 流式断点上传：不将整个大文件读入内存，按 4MB 分块流式读取与加密，自动跳过已存在的 Chunk。
pub async fn stream_upload_resumable<R: Read + Seek>(
    mut reader: R,
    file_size: u64,
    plaintext_hash: [u8; 32],
    avk: &AccountVaultKey,
    client: &HubClient,
    blob_id: &str,
    mut progress_callback: impl FnMut(u32, u32),
) -> Result<u32> {
    let planner = ResumableUploadPlanner::new(file_size);
    let status =
        client.blob_status(blob_id).await.unwrap_or_else(|_| crate::hub_client::BlobStatusDto {
            blob_id: blob_id.to_string(),
            chunks_present: Vec::new(),
            committed: false,
        });

    if status.committed {
        return Ok(planner.total_chunks);
    }

    let missing = planner.missing_chunks(&status.chunks_present);
    let encryptor = BlobChunkEncryptor::new(avk, plaintext_hash)?;
    let mut buf = vec![0u8; CHUNK_SIZE];

    for &index in &missing {
        let offset = (index as u64) * (CHUNK_SIZE as u64);
        reader.seek(SeekFrom::Start(offset)).map_err(|e| SyncError::Failed(e.to_string()))?;

        let expected_read = if index == planner.total_chunks - 1 {
            let rem = (file_size - offset) as usize;
            if rem == 0 { 0 } else { rem }
        } else {
            CHUNK_SIZE
        };

        let mut read_bytes = 0usize;
        while read_bytes < expected_read {
            let n = reader
                .read(&mut buf[read_bytes..expected_read])
                .map_err(|e| SyncError::Failed(e.to_string()))?;
            if n == 0 {
                break;
            }
            read_bytes += n;
        }

        let encrypted = encryptor.encrypt(index, &buf[..read_bytes])?;
        client.put_chunk(blob_id, index, encrypted).await?;
        progress_callback(index + 1, planner.total_chunks);
    }

    client.commit_blob(blob_id, planner.total_chunks).await?;
    Ok(planner.total_chunks)
}

/// 流式断点下载：按分块下载并流式解密写入目标文件，内存常驻仅为单个分块大小。
pub async fn stream_download_resumable<W: Write + Seek>(
    mut writer: W,
    total_chunks: u32,
    avk: &AccountVaultKey,
    client: &HubClient,
    blob_id: &str,
    mut progress_callback: impl FnMut(u32, u32),
) -> Result<()> {
    let mut decryptor = BlobChunkDecryptor::default();

    for index in 0..total_chunks {
        let chunk_data = client.get_chunk(blob_id, index).await?;
        let plaintext = decryptor.decrypt(avk, index, &chunk_data)?;

        let offset = (index as u64) * (CHUNK_SIZE as u64);
        writer.seek(SeekFrom::Start(offset)).map_err(|e| SyncError::Failed(e.to_string()))?;
        writer.write_all(&plaintext).map_err(|e| SyncError::Failed(e.to_string()))?;
        progress_callback(index + 1, total_chunks);
    }

    decryptor.finish(avk)?;
    writer.flush().map_err(|e| SyncError::Failed(e.to_string()))?;
    Ok(())
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn planner_computes_exact_chunks_and_missing_indices() {
        let planner = ResumableUploadPlanner::new(CHUNK_SIZE as u64 * 3 + 512);
        assert_eq!(planner.total_chunks, 4);

        let missing = planner.missing_chunks(&[0, 2]);
        assert_eq!(missing, vec![1, 3]);

        let missing_none = planner.missing_chunks(&[0, 1, 2, 3]);
        assert!(missing_none.is_empty());
    }

    #[test]
    fn planner_handles_zero_length_file() {
        let planner = ResumableUploadPlanner::new(0);
        assert_eq!(planner.total_chunks, 1);
        assert_eq!(planner.missing_chunks(&[]), vec![0]);
    }
}
