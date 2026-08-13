use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use reqwest::{Method, Url};
use serde::{Deserialize, Serialize};
use tokio_tungstenite::connect_async;

use asterism_core::id::{AccountId, DeviceId};

use crate::error::{Result, SyncError};
use crate::pairing::{PairingFinish, PairingOffer};
use crate::protocol::Envelope;

#[derive(Clone, Debug)]
pub struct HubClient {
    pub base: String,
    pub token: Option<String>,
    http: reqwest::Client,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionResponse {
    pub token: String,
    pub account_id: AccountId,
    pub device_id: DeviceId,
    pub avk_wrapped_hex: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DeviceDto {
    pub id: DeviceId,
    pub name: String,
    pub platform: String,
    pub last_seen_at_ms: i64,
    pub revoked: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HistoryDto {
    pub id: String,
    pub origin_device_id: DeviceId,
    pub kind: String,
    pub created_at_ms: i64,
    pub logical_size: u64,
    pub payload_size: u64,
    pub dedup_tag: String,
    pub flags: u32,
    pub encrypted_metadata: String,
    pub blob_id: Option<String>,
}

impl HubClient {
    pub fn new(base: impl Into<String>) -> Result<Self> {
        let http = reqwest::Client::builder()
            .use_rustls_tls()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| SyncError::Failed(e.to_string()))?;
        Ok(Self { base: base.into().trim_end_matches('/').to_string(), token: None, http })
    }

    pub fn with_token(mut self, token: String) -> Self {
        self.token = Some(token);
        self
    }

    pub async fn pairing_start(&self) -> Result<PairingOffer> {
        self.json(Method::POST, "/api/v1/pairing/start", None::<&()>).await
    }

    pub async fn pairing_finish(&self, req: &PairingFinish) -> Result<SessionResponse> {
        self.json(Method::POST, "/api/v1/pairing/finish", Some(req)).await
    }

    pub async fn devices(&self) -> Result<Vec<DeviceDto>> {
        self.json(Method::GET, "/api/v1/devices", None::<&()>).await
    }

    pub async fn revoke_device(&self, id: DeviceId) -> Result<()> {
        let path = format!("/api/v1/devices/{id}");
        self.empty(Method::DELETE, &path).await
    }

    pub async fn history(&self, cursor: Option<&str>, limit: u32) -> Result<Vec<HistoryDto>> {
        let mut path = format!("/api/v1/history?limit={limit}");
        if let Some(c) = cursor {
            path.push_str("&cursor=");
            path.push_str(c);
        }
        self.json(Method::GET, &path, None::<&()>).await
    }

    pub async fn delete_history(&self, id: &str) -> Result<()> {
        self.empty(Method::DELETE, &format!("/api/v1/history/{id}")).await
    }

    pub async fn begin_blob(&self) -> Result<String> {
        #[derive(Deserialize)]
        struct Body {
            blob_id: String,
        }
        let body: Body = self.json(Method::POST, "/api/v1/blobs", None::<&()>).await?;
        Ok(body.blob_id)
    }

    pub async fn put_chunk(&self, blob_id: &str, index: u32, body: Vec<u8>) -> Result<()> {
        let url = format!("{}/api/v1/blobs/{blob_id}/chunks/{index}", self.base);
        let res = self
            .req(Method::PUT, url)
            .header("content-type", "application/octet-stream")
            .body(body)
            .send()
            .await
            .map_err(|e| SyncError::Failed(e.to_string()))?;
        if res.status().is_success() {
            Ok(())
        } else {
            Err(SyncError::Failed(res.status().to_string()))
        }
    }

    pub async fn get_chunk(&self, blob_id: &str, index: u32) -> Result<Vec<u8>> {
        let url = format!("{}/api/v1/blobs/{blob_id}/chunks/{index}", self.base);
        let res = self
            .req(Method::GET, url)
            .send()
            .await
            .map_err(|e| SyncError::Failed(e.to_string()))?;
        if !res.status().is_success() {
            return Err(SyncError::Failed(res.status().to_string()));
        }
        Ok(res.bytes().await.map_err(|e| SyncError::Failed(e.to_string()))?.to_vec())
    }

    pub async fn commit_blob(&self, blob_id: &str, chunks: u32) -> Result<()> {
        #[derive(Serialize)]
        struct Body {
            chunks: u32,
        }
        self.empty_json(Method::POST, &format!("/api/v1/blobs/{blob_id}/commit"), &Body { chunks })
            .await
    }

    pub async fn publish_history(&self, item: &HistoryDto) -> Result<()> {
        self.empty_json(Method::POST, "/api/v1/history", item).await
    }

    pub async fn connect_ws(
        &self,
    ) -> Result<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    > {
        let token = self.token.as_deref().ok_or(SyncError::NotConnected)?;
        let mut url =
            Url::parse(&self.base.replace("https://", "wss://").replace("http://", "ws://"))
                .map_err(|e| SyncError::Failed(e.to_string()))?;
        url.set_path("/ws/v1/device");
        url.query_pairs_mut().append_pair("token", token);
        let (stream, _) =
            connect_async(url.as_str()).await.map_err(|e| SyncError::Failed(e.to_string()))?;
        Ok(stream)
    }

    pub async fn send_control(
        ws: &mut tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        env: &Envelope,
    ) -> Result<()> {
        let text = serde_json::to_string(env).map_err(|e| SyncError::Protocol(e.to_string()))?;
        ws.send(tokio_tungstenite::tungstenite::Message::Text(text.into()))
            .await
            .map_err(|e| SyncError::Failed(e.to_string()))
    }

    pub async fn recv_control(
        ws: &mut tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    ) -> Result<Option<Envelope>> {
        match ws.next().await {
            Some(Ok(tokio_tungstenite::tungstenite::Message::Text(t))) => {
                Ok(Some(serde_json::from_str(&t).map_err(|e| SyncError::Protocol(e.to_string()))?))
            }
            Some(Ok(tokio_tungstenite::tungstenite::Message::Close(_))) | None => Ok(None),
            Some(Err(err)) => Err(SyncError::Failed(err.to_string())),
            _ => Ok(None),
        }
    }

    fn req(&self, method: Method, url: String) -> reqwest::RequestBuilder {
        let mut b = self.http.request(method, url);
        if let Some(token) = &self.token {
            b = b.bearer_auth(token);
        }
        b
    }

    async fn json<T: for<'de> Deserialize<'de>, B: Serialize>(
        &self,
        method: Method,
        path: &str,
        body: Option<&B>,
    ) -> Result<T> {
        let url = format!("{}{path}", self.base);
        let mut req = self.req(method, url);
        if let Some(body) = body {
            req = req.json(body);
        }
        let res = req.send().await.map_err(|e| SyncError::Failed(e.to_string()))?;
        if !res.status().is_success() {
            return Err(SyncError::Failed(format!("{} {path}", res.status())));
        }
        res.json().await.map_err(|e| SyncError::Failed(e.to_string()))
    }

    async fn empty(&self, method: Method, path: &str) -> Result<()> {
        let url = format!("{}{path}", self.base);
        let res =
            self.req(method, url).send().await.map_err(|e| SyncError::Failed(e.to_string()))?;
        if res.status().is_success() {
            Ok(())
        } else {
            Err(SyncError::Failed(res.status().to_string()))
        }
    }

    async fn empty_json<B: Serialize>(&self, method: Method, path: &str, body: &B) -> Result<()> {
        let url = format!("{}{path}", self.base);
        let res = self
            .req(method, url)
            .json(body)
            .send()
            .await
            .map_err(|e| SyncError::Failed(e.to_string()))?;
        if res.status().is_success() {
            Ok(())
        } else {
            Err(SyncError::Failed(res.status().to_string()))
        }
    }
}
