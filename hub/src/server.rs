use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::{Context, Result};
use axum::Router;
use axum::extract::{DefaultBodyLimit, Request};
use axum::routing::{delete, get, post, put};
use hyper::body::Incoming;
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto::Builder;
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;
use tower::Service;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::trace::TraceLayer;

use crate::config::HubConfig;
use crate::state::HubState;
use crate::{api, auth, blob, device, health, history, relay, web};

const MAX_JSON_BYTES: usize = 1024 * 1024;
const MAX_CHUNK_BYTES: usize = 2 * 1024 * 1024;

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

    let tls = crate::tls::load_server_config(&config.tls.cert, &config.tls.key)?;
    let acceptor = TlsAcceptor::from(tls);
    let addr: SocketAddr = config.bind.parse().context("parse bind address")?;
    let listener = TcpListener::bind(addr).await?;
    tracing::info!(%addr, "asterism-hub listening (https)");

    let state = HubState::new(config, conn);
    let app = router(state);

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
            if let Err(err) =
                Builder::new(TokioExecutor::new()).serve_connection(io, hyper_service).await
            {
                tracing::debug!(error = %err, %peer, "connection error");
            }
        });
    }
}

fn router(state: Arc<HubState>) -> Router {
    Router::new()
        .route("/healthz", get(health::healthz))
        .route("/readyz", get(health::readyz))
        .route("/api/v1/auth/session", post(auth::session))
        .route("/api/v1/pairing/start", post(auth::pairing_start))
        .route("/api/v1/pairing/finish", post(auth::pairing_finish))
        .route("/api/v1/devices", get(device::list))
        .route("/api/v1/devices/{id}", delete(device::revoke))
        .route("/api/v1/history", get(history::list).post(history::create))
        .route("/api/v1/history/{id}", delete(history::delete))
        .route("/api/v1/blobs", post(blob::begin))
        .route("/api/v1/blobs/{id}/chunks/{index}", put(blob::put_chunk).get(blob::get_chunk))
        .route("/api/v1/blobs/{id}/commit", post(blob::commit))
        .route("/ws/v1/device", get(relay::ws))
        .route("/api/v1/signaling/{*rest}", get(api::not_implemented))
        .fallback(web::asset)
        .layer(TraceLayer::new_for_http())
        .layer(RequestBodyLimitLayer::new(MAX_CHUNK_BYTES))
        .layer(DefaultBodyLimit::max(MAX_JSON_BYTES.max(MAX_CHUNK_BYTES)))
        .with_state(state)
}
