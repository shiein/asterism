use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::{Context, Result};
use axum::extract::Request;
use hyper::body::Incoming;
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto::Builder;
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;
use tower::Service;

use crate::blob;
use crate::config::HubConfig;
use crate::routes::HubRouter;
use crate::state::HubState;

use asterism_kernel::ServiceRegistry;

fn ensure_bootstrap(mut config: HubConfig) -> Result<HubConfig> {
    if config.bootstrap_secret_hash.is_some() {
        return Ok(config);
    }
    let secret = asterism_sync::pairing::generate_bootstrap_secret();
    let hash = hex::encode(asterism_sync::pairing::hash_bootstrap(&secret));
    config.bootstrap_secret_hash = Some(hash);
    let secret_path = config.data_dir.join("bootstrap.secret");
    std::fs::write(&secret_path, format!("{secret}\n"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&secret_path, std::fs::Permissions::from_mode(0o600));
    }
    let config_path = config.data_dir.join("config.toml");
    if let Ok(toml) = toml::to_string_pretty(&config) {
        let _ = std::fs::write(config_path, toml);
    }
    tracing::warn!(
        path = %secret_path.display(),
        "generated bootstrap secret for existing hub; save it for the first device"
    );
    println!("Bootstrap secret (save once): {secret}");
    Ok(config)
}

pub async fn run(config: HubConfig) -> Result<()> {
    std::fs::create_dir_all(config.blob_root())?;
    crate::db::migrate(&config.db_path())?;
    let conn = rusqlite::Connection::open(config.db_path())?;
    conn.execute_batch(
        r#"
        PRAGMA journal_mode=WAL;
        PRAGMA busy_timeout=5000;
        PRAGMA foreign_keys=ON;
        "#,
    )?;

    let config = ensure_bootstrap(config)?;
    let tls = crate::tls::load_server_config(&config.tls.cert, &config.tls.key)?;
    let acceptor = TlsAcceptor::from(tls);
    let addr: SocketAddr = config.bind.parse().context("parse bind address")?;
    let listener = TcpListener::bind(addr).await?;
    tracing::info!(%addr, "asterism-hub listening (https)");

    let state = HubState::new(config, conn);
    let gc_state = Arc::clone(&state);
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(60 * 60));
        loop {
            interval.tick().await;
            let state = Arc::clone(&gc_state);
            match tokio::task::spawn_blocking(move || {
                blob::gc_unused(&state, std::time::Duration::from_secs(24 * 60 * 60))
            })
            .await
            {
                Ok(Ok(removed)) if removed > 0 => tracing::info!(removed, "hub blob GC"),
                Ok(Ok(_)) => {}
                Ok(Err(err)) => tracing::warn!(error = %err, "hub blob GC failed"),
                Err(err) => tracing::warn!(error = %err, "hub blob GC task failed"),
            }
        }
    });
    let routes = Arc::new(HubRouter::new());
    let mut registry = ServiceRegistry::new();
    registry.provide(Arc::clone(&routes)).map_err(|err| anyhow::anyhow!(err))?;
    let runtime =
        crate::host::hub_boot_plan().mount_with(registry).map_err(|err| anyhow::anyhow!(err))?;
    tracing::info!(order = ?runtime.boot_order(), "hub boot plan");
    let app = routes.finish(runtime.boot_order(), state);
    let _runtime = runtime;

    loop {
        let (tcp, peer) = listener.accept().await?;
        let acceptor = acceptor.clone();
        let app = app.clone();
        tokio::spawn(async move {
            let stream = match acceptor.accept(tcp).await {
                Ok(s) => s,
                Err(err) => {
                    tracing::debug!(error = %err, %peer, "tls accept failed");
                    return;
                }
            };
            let io = TokioIo::new(stream);
            let hyper_service = hyper::service::service_fn(move |request: Request<Incoming>| {
                app.clone().call(request)
            });
            if let Err(err) = Builder::new(TokioExecutor::new())
                .serve_connection_with_upgrades(io, hyper_service)
                .await
            {
                tracing::debug!(error = %err, %peer, "connection error");
            }
        });
    }
}
