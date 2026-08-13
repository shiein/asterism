use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use asterism_core::id::DeviceId;
use asterism_platform::local_candidates;
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use rustls::pki_types::ServerName;
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::{TlsAcceptor, TlsConnector};

use crate::cert::{DeviceCert, client_config_pinned, server_config};
use crate::codec::{read_envelope, write_envelope};
use crate::error::{Result, SyncError};
use crate::protocol::{Envelope, MDNS_SERVICE, PROTOCOL_VERSION};

const SERVICE_TYPE: &str = "_asterism._tcp.local.";

pub struct LanEndpoint {
    pub device_id: DeviceId,
    pub cert: DeviceCert,
    pub port: u16,
    daemon: ServiceDaemon,
}

impl LanEndpoint {
    pub fn announce(device_id: DeviceId, cert: DeviceCert, port: u16) -> Result<Self> {
        let daemon = ServiceDaemon::new().map_err(|e| SyncError::Failed(e.to_string()))?;
        let hostname = format!("{}.local.", device_id);
        let mut props = HashMap::new();
        props.insert("protocol_version".into(), PROTOCOL_VERSION.to_string());
        props.insert("device_id".into(), device_id.to_string());
        props.insert("port".into(), port.to_string());
        props.insert("fp".into(), cert.fingerprint_hex());
        let info = ServiceInfo::new(
            SERVICE_TYPE,
            &device_id.to_string()[..8.min(device_id.to_string().len())],
            &hostname,
            "",
            port,
            props,
        )
        .map_err(|e| SyncError::Failed(e.to_string()))?
        .enable_addr_auto();
        daemon.register(info).map_err(|e| SyncError::Failed(e.to_string()))?;
        Ok(Self { device_id, cert, port, daemon })
    }

    pub fn browse(&self) -> Result<mdns_sd::Receiver<ServiceEvent>> {
        self.daemon.browse(SERVICE_TYPE).map_err(|e| SyncError::Failed(e.to_string()))
    }

    pub fn candidates(&self) -> Vec<String> {
        local_candidates(self.port).into_iter().map(|c| c.endpoint()).collect()
    }

    pub fn shutdown(&self) {
        let _ = self.daemon.shutdown();
    }
}

pub async fn listen(cert: DeviceCert, port: u16) -> Result<TcpListener> {
    let _ = server_config(&cert, &[])?;
    TcpListener::bind(("0.0.0.0", port)).await.map_err(SyncError::from)
}

pub async fn accept_direct(
    listener: &TcpListener,
    cert: &DeviceCert,
    trusted_fps: &[[u8; 32]],
) -> Result<(tokio_rustls::server::TlsStream<TcpStream>, SocketAddr, Option<[u8; 32]>)> {
    let (tcp, peer) = listener.accept().await?;
    let acceptor = TlsAcceptor::from(server_config(cert, trusted_fps)?);
    let tls = acceptor.accept(tcp).await.map_err(|e| SyncError::Tls(e.to_string()))?;
    let fingerprint = tls
        .get_ref()
        .1
        .peer_certificates()
        .and_then(|certs| certs.first())
        .map(|der| crate::cert::fingerprint_of_der(der.as_ref()));
    Ok((tls, peer, fingerprint))
}

/// Direct 必须再次验证证书指纹。Hub 交换 IP 不等于信任 IP。
pub async fn connect_direct(
    endpoint: &str,
    expected_fingerprint: [u8; 32],
    local_cert: &DeviceCert,
    timeout: Duration,
) -> Result<tokio_rustls::client::TlsStream<TcpStream>> {
    let addr: SocketAddr =
        endpoint.parse().map_err(|e: std::net::AddrParseError| SyncError::Failed(e.to_string()))?;
    let connect = async {
        let tcp = TcpStream::connect(addr).await?;
        let cfg = client_config_pinned(expected_fingerprint, local_cert)?;
        let connector = TlsConnector::from(cfg);
        let name =
            ServerName::try_from("asterism.local").map_err(|e| SyncError::Tls(e.to_string()))?;
        connector.connect(name, tcp).await.map_err(|e| SyncError::Tls(e.to_string()))
    };
    tokio::time::timeout(timeout, connect).await.map_err(|_| SyncError::Timeout)?
}

pub async fn send(stream: &mut (impl tokio::io::AsyncWrite + Unpin), env: &Envelope) -> Result<()> {
    write_envelope(stream, env).await
}

pub async fn recv(stream: &mut (impl tokio::io::AsyncRead + Unpin)) -> Result<Envelope> {
    read_envelope(stream).await
}

#[derive(Clone, Debug)]
pub struct DiscoveredPeer {
    pub device_id: DeviceId,
    pub port: u16,
    pub fingerprint: Option<[u8; 32]>,
    pub addresses: Vec<String>,
}

pub fn parse_mdns_device(info: &ServiceInfo) -> Option<DiscoveredPeer> {
    let props = info.get_properties();
    let device_id = props.get("device_id")?.val_str().parse().ok()?;
    let port = props.get("port").and_then(|p| p.val_str().parse().ok()).unwrap_or(info.get_port());
    let fingerprint = props.get("fp").and_then(|p| {
        let raw = hex::decode(p.val_str()).ok()?;
        (raw.len() == 32).then(|| {
            let mut a = [0u8; 32];
            a.copy_from_slice(&raw);
            a
        })
    });
    let addresses = info
        .get_addresses()
        .iter()
        .map(|ip| match ip {
            std::net::IpAddr::V4(v) => format!("{v}:{port}"),
            std::net::IpAddr::V6(v) => format!("[{v}]:{port}"),
        })
        .collect();
    Some(DiscoveredPeer { device_id, port, fingerprint, addresses })
}

impl Drop for LanEndpoint {
    fn drop(&mut self) {
        self.shutdown();
    }
}

// 保留 MDNS_SERVICE 常量使用者。
#[allow(dead_code)]
fn _mdns_name() -> &'static str {
    MDNS_SERVICE
}

pub fn lan_timeout() -> Duration {
    Duration::from_millis(800)
}

pub fn share_cert_arc(cert: DeviceCert) -> Arc<DeviceCert> {
    Arc::new(cert)
}
