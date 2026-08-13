use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::codec::{read_envelope, write_envelope};
use crate::error::{Result, SyncError};
use crate::protocol::{Envelope, ItemAck, LanItem, MessageBody};

pub async fn send_lan_item<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    env: &Envelope,
    payload: &[u8],
) -> Result<bool> {
    write_envelope(&mut *stream, env).await?;
    stream.write_u64(payload.len() as u64).await?;
    stream.write_all(payload).await?;
    stream.flush().await?;
    let ack = read_envelope(&mut *stream).await?;
    match ack.body {
        MessageBody::ItemAck(ItemAck { accepted, .. }) => Ok(accepted),
        _ => Err(SyncError::Protocol("expected ITEM_ACK".into())),
    }
}

const MAX_LAN_PAYLOAD: usize = 64 * 1024 * 1024;

pub async fn recv_lan_item<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    local_device: asterism_core::DeviceId,
    trusted: bool,
) -> Result<(asterism_core::DeviceId, LanItem, Vec<u8>)> {
    let env = read_envelope(&mut *stream).await?;
    let from = env.device_id;
    let MessageBody::LanItem(item) = env.body else {
        return Err(SyncError::Protocol("expected LAN_ITEM".into()));
    };
    if !trusted {
        let ack = Envelope::new(
            local_device,
            MessageBody::ItemAck(ItemAck {
                item_id: item.offer.item_id,
                accepted: false,
                reason: Some("untrusted device".into()),
            }),
        );
        write_envelope(&mut *stream, &ack).await?;
        return Err(SyncError::Failed("untrusted lan peer".into()));
    }
    let len = stream.read_u64().await? as usize;
    if len > MAX_LAN_PAYLOAD {
        return Err(SyncError::Protocol("lan payload too large".into()));
    }
    let mut payload = Vec::new();
    payload
        .try_reserve_exact(len)
        .map_err(|_| SyncError::Protocol("lan payload allocation failed".into()))?;
    if len > 0 {
        let mut remaining = len;
        let mut buf = [0u8; 64 * 1024];
        while remaining > 0 {
            let n = remaining.min(buf.len());
            stream.read_exact(&mut buf[..n]).await?;
            payload.extend_from_slice(&buf[..n]);
            remaining -= n;
        }
    }
    let ack = Envelope::new(
        local_device,
        MessageBody::ItemAck(ItemAck { item_id: item.offer.item_id, accepted: true, reason: None }),
    );
    write_envelope(&mut *stream, &ack).await?;
    Ok((from, item, payload))
}
