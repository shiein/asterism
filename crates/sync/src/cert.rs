use std::sync::Arc;

use rcgen::{CertificateParams, DistinguishedName, DnType, KeyPair, SanType};
use rustls::DigitallySignedStruct;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, PrivatePkcs8KeyDer, ServerName, UnixTime};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::{ClientConfig, ServerConfig, SignatureScheme};
use sha2::{Digest, Sha256};

use crate::error::{Result, SyncError};

#[derive(Clone)]
pub struct DeviceCert {
    pub cert_pem: String,
    pub key_pem: String,
    pub fingerprint: [u8; 32],
}

impl DeviceCert {
    pub fn load_or_create(dir: &std::path::Path, device_name: &str) -> Result<Self> {
        std::fs::create_dir_all(dir)?;
        let cert_p = dir.join("device.cert.pem");
        let key_p = dir.join("device.key.pem");
        if cert_p.exists() && key_p.exists() {
            let cert_pem = std::fs::read_to_string(cert_p)?;
            let key_pem = std::fs::read_to_string(key_p)?;
            return Self::from_pem(cert_pem, key_pem);
        }
        let cert = Self::generate(device_name)?;
        std::fs::write(cert_p, &cert.cert_pem)?;
        std::fs::write(key_p, &cert.key_pem)?;
        Ok(cert)
    }

    pub fn from_pem(cert_pem: String, key_pem: String) -> Result<Self> {
        let certs = rustls_pemfile::certs(&mut cert_pem.as_bytes())
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| SyncError::Tls(e.to_string()))?;
        let der = certs.first().ok_or_else(|| SyncError::Tls("empty cert".into()))?;
        Ok(Self { fingerprint: fingerprint_of_der(der.as_ref()), cert_pem, key_pem })
    }

    pub fn generate(device_name: &str) -> Result<Self> {
        let key = KeyPair::generate().map_err(|e| SyncError::Tls(e.to_string()))?;
        let mut params = CertificateParams::new(vec!["asterism.local".into(), "localhost".into()])
            .map_err(|e| SyncError::Tls(e.to_string()))?;
        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, device_name);
        params.distinguished_name = dn;
        params.subject_alt_names = vec![SanType::DnsName(
            "asterism.local".try_into().map_err(|e: rcgen::Error| SyncError::Tls(e.to_string()))?,
        )];
        let certified = params.self_signed(&key).map_err(|e| SyncError::Tls(e.to_string()))?;
        let cert_der = certified.der().to_vec();
        let mut hasher = Sha256::new();
        hasher.update(&cert_der);
        let fingerprint: [u8; 32] = hasher.finalize().into();
        Ok(Self { cert_pem: certified.pem(), key_pem: key.serialize_pem(), fingerprint })
    }

    pub fn fingerprint_hex(&self) -> String {
        hex::encode(self.fingerprint)
    }
}

pub fn fingerprint_of_der(der: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(der);
    hasher.finalize().into()
}

pub fn server_config(cert: &DeviceCert, trusted_fps: &[[u8; 32]]) -> Result<Arc<ServerConfig>> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let certs = rustls_pemfile::certs(&mut cert.cert_pem.as_bytes())
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|e| SyncError::Tls(e.to_string()))?;
    let mut keys = rustls_pemfile::pkcs8_private_keys(&mut cert.key_pem.as_bytes())
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|e| SyncError::Tls(e.to_string()))?;
    let key = keys.pop().ok_or_else(|| SyncError::Tls("no key".into()))?;
    let verifier = Arc::new(TrustedClient { allowed: trusted_fps.to_vec() });
    let config = ServerConfig::builder()
        .with_client_cert_verifier(verifier)
        .with_single_cert(
            certs,
            rustls::pki_types::PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(
                key.secret_pkcs8_der().to_vec(),
            )),
        )
        .map_err(|e| SyncError::Tls(e.to_string()))?;
    Ok(Arc::new(config))
}

pub fn client_config_pinned(
    expected: [u8; 32],
    client_cert: &DeviceCert,
) -> Result<Arc<ClientConfig>> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let certs = rustls_pemfile::certs(&mut client_cert.cert_pem.as_bytes())
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|e| SyncError::Tls(e.to_string()))?;
    let mut keys = rustls_pemfile::pkcs8_private_keys(&mut client_cert.key_pem.as_bytes())
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|e| SyncError::Tls(e.to_string()))?;
    let key = keys.pop().ok_or_else(|| SyncError::Tls("no key".into()))?;
    let config = ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(Pinned { expected }))
        .with_client_auth_cert(
            certs,
            rustls::pki_types::PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(
                key.secret_pkcs8_der().to_vec(),
            )),
        )
        .map_err(|e| SyncError::Tls(e.to_string()))?;
    Ok(Arc::new(config))
}

#[derive(Debug)]
struct Pinned {
    expected: [u8; 32],
}

impl ServerCertVerifier for Pinned {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> std::result::Result<ServerCertVerified, rustls::Error> {
        if fingerprint_of_der(end_entity.as_ref()) == self.expected {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::General("certificate fingerprint mismatch".into()))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        verify_peer_tls12(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        verify_peer_tls13(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        supported_schemes()
    }
}

#[derive(Debug)]
struct TrustedClient {
    allowed: Vec<[u8; 32]>,
}

impl ClientCertVerifier for TrustedClient {
    fn root_hint_subjects(&self) -> &[rustls::DistinguishedName] {
        &[]
    }

    fn verify_client_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _now: UnixTime,
    ) -> std::result::Result<ClientCertVerified, rustls::Error> {
        let fp = fingerprint_of_der(end_entity.as_ref());
        if self.allowed.contains(&fp) {
            Ok(ClientCertVerified::assertion())
        } else {
            Err(rustls::Error::General("untrusted client certificate".into()))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        verify_peer_tls12(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        verify_peer_tls13(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        supported_schemes()
    }
}

fn verify_algs() -> rustls::crypto::WebPkiSupportedAlgorithms {
    rustls::crypto::ring::default_provider().signature_verification_algorithms
}

fn supported_schemes() -> Vec<SignatureScheme> {
    verify_algs().supported_schemes()
}

fn verify_peer_tls12(
    message: &[u8],
    cert: &CertificateDer<'_>,
    dss: &DigitallySignedStruct,
) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
    rustls::crypto::verify_tls12_signature(message, cert, dss, &verify_algs())
}

fn verify_peer_tls13(
    message: &[u8],
    cert: &CertificateDer<'_>,
    dss: &DigitallySignedStruct,
) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
    rustls::crypto::verify_tls13_signature(message, cert, dss, &verify_algs())
}

/// Hub HTTPS/WSS：已 pin 则只认该指纹；未 pin 则 TOFU 并记下观察值。
#[derive(Clone, Debug)]
pub struct HubTls {
    pub config: Arc<ClientConfig>,
    observed: Arc<std::sync::Mutex<Option<[u8; 32]>>>,
}

impl HubTls {
    pub fn new(pin: Option<[u8; 32]>) -> Result<Self> {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let observed = Arc::new(std::sync::Mutex::new(None));
        let mut config = ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(HubPin {
                expected: pin,
                observed: Arc::clone(&observed),
            }))
            .with_no_client_auth();
        config.alpn_protocols = vec![b"http/1.1".to_vec()];
        Ok(Self { config: Arc::new(config), observed })
    }

    pub fn observed_hex(&self) -> Option<String> {
        self.observed.lock().ok().and_then(|g| g.map(hex::encode))
    }
}

#[derive(Debug)]
struct HubPin {
    expected: Option<[u8; 32]>,
    observed: Arc<std::sync::Mutex<Option<[u8; 32]>>>,
}

impl ServerCertVerifier for HubPin {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> std::result::Result<ServerCertVerified, rustls::Error> {
        let fp = fingerprint_of_der(end_entity.as_ref());
        if let Ok(mut slot) = self.observed.lock() {
            *slot = Some(fp);
        }
        if let Some(expected) = self.expected
            && fp != expected
        {
            return Err(rustls::Error::General("hub certificate fingerprint mismatch".into()));
        }
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        verify_peer_tls12(message, cert, dss)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        verify_peer_tls13(message, cert, dss)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        supported_schemes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lan_and_hub_verifiers_use_rustls_signature_schemes() {
        let schemes = supported_schemes();
        assert!(schemes.contains(&SignatureScheme::ECDSA_NISTP256_SHA256));
        assert!(schemes.contains(&SignatureScheme::ED25519));
    }

    #[test]
    fn tofu_hub_tls_records_no_pin_until_handshake() {
        let tls = HubTls::new(None).unwrap();
        assert!(tls.observed_hex().is_none());
    }
}
