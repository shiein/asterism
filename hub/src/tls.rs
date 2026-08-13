use std::fs;
use std::io::BufReader;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use rustls::ServerConfig;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};

pub fn write_self_signed(cert_path: &Path, key_path: &Path) -> Result<()> {
    let certified =
        rcgen::generate_simple_self_signed(vec!["localhost".into(), "127.0.0.1".into()])
            .context("generate self-signed certificate")?;
    fs::write(cert_path, certified.cert.pem())?;
    fs::write(key_path, certified.key_pair.serialize_pem())?;
    Ok(())
}

pub fn load_server_config(cert_path: &Path, key_path: &Path) -> Result<Arc<ServerConfig>> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let certs = load_certs(cert_path)?;
    let key = load_key(key_path)?;
    let mut config = ServerConfig::builder().with_no_client_auth().with_single_cert(certs, key)?;
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(Arc::new(config))
}

fn load_certs(path: &Path) -> Result<Vec<CertificateDer<'static>>> {
    let file = fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut reader = BufReader::new(file);
    let certs: Vec<CertificateDer<'static>> = rustls_pemfile::certs(&mut reader)
        .collect::<std::result::Result<Vec<_>, _>>()
        .context("parse certificates")?;
    if certs.is_empty() {
        bail!("no certificates in {}", path.display());
    }
    Ok(certs)
}

fn load_key(path: &Path) -> Result<PrivateKeyDer<'static>> {
    let file = fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut reader = BufReader::new(file);
    let keys: Vec<PrivatePkcs8KeyDer<'static>> = rustls_pemfile::pkcs8_private_keys(&mut reader)
        .collect::<std::result::Result<Vec<_>, _>>()
        .context("parse private key")?;
    let key =
        keys.into_iter().next().with_context(|| format!("no PKCS8 key in {}", path.display()))?;
    Ok(PrivateKeyDer::Pkcs8(key))
}
