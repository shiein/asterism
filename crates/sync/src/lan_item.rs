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

pub async fn recv_lan_item<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    local_device: asterism_core::DeviceId,
) -> Result<(asterism_core::DeviceId, LanItem, Vec<u8>)> {
    let env = read_envelope(&mut *stream).await?;
    let from = env.device_id;
    let MessageBody::LanItem(item) = env.body else {
        return Err(SyncError::Protocol("expected LAN_ITEM".into()));
    };
    let len = stream.read_u64().await? as usize;
    if len > 512 * 1024 * 1024 {
        return Err(SyncError::Protocol("lan payload too large".into()));
    }
    let mut payload = vec![0u8; len];
    if len > 0 {
        stream.read_exact(&mut payload).await?;
    }
    let ack = Envelope::new(
        local_device,
        MessageBody::ItemAck(ItemAck { item_id: item.offer.item_id, accepted: true, reason: None }),
    );
    write_envelope(&mut *stream, &ack).await?;
    Ok((from, item, payload))
}
