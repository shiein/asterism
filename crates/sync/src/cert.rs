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

pub fn server_config(cert: &DeviceCert) -> Result<Arc<ServerConfig>> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let certs = rustls_pemfile::certs(&mut cert.cert_pem.as_bytes())
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|e| SyncError::Tls(e.to_string()))?;
    let mut keys = rustls_pemfile::pkcs8_private_keys(&mut cert.key_pem.as_bytes())
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|e| SyncError::Tls(e.to_string()))?;
    let key = keys.pop().ok_or_else(|| SyncError::Tls("no key".into()))?;
    let verifier = Arc::new(AnyClient);
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
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![
            SignatureScheme::ECDSA_NISTP256_SHA256,
            SignatureScheme::ED25519,
            SignatureScheme::RSA_PSS_SHA256,
        ]
    }
}

#[derive(Debug)]
struct AnyClient;

impl ClientCertVerifier for AnyClient {
    fn root_hint_subjects(&self) -> &[rustls::DistinguishedName] {
        &[]
    }

    fn verify_client_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _now: UnixTime,
    ) -> std::result::Result<ClientCertVerified, rustls::Error> {
        Ok(ClientCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![
            SignatureScheme::ECDSA_NISTP256_SHA256,
            SignatureScheme::ED25519,
            SignatureScheme::RSA_PSS_SHA256,
        ]
    }
}
