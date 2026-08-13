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
use crate::{api, health, web};

const MAX_JSON_BYTES: usize = 1024 * 1024;
const MAX_CHUNK_BYTES: usize = 2 * 1024 * 1024;

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<HubConfig>,
}

pub async fn run(config: HubConfig) -> Result<()> {
    std::fs::create_dir_all(config.blob_root())?;
    crate::db::migrate(&config.db_path())?;

    let tls = crate::tls::load_server_config(&config.tls.cert, &config.tls.key)?;
    let acceptor = TlsAcceptor::from(tls);
    let addr: SocketAddr = config.bind.parse().context("parse bind address")?;
    let listener = TcpListener::bind(addr).await?;
    tracing::info!(%addr, "asterism-hub listening (https)");

    let state = AppState { config: Arc::new(config) };
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
            if let Err(err) = Builder::new(TokioExecutor::new()).serve_connection(io, hyper_service).await {
                tracing::debug!(error = %err, %peer, "connection error");
            }
        });
    }
}

fn router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(health::healthz))
        .route("/readyz", get(health::readyz))
        .route("/api/v1/auth/session", post(api::not_implemented))
        .route("/api/v1/pairing/start", post(api::not_implemented))
        .route("/api/v1/pairing/finish", post(api::not_implemented))
        .route("/api/v1/devices", get(api::not_implemented))
        .route("/api/v1/devices/{id}", delete(api::not_implemented))
        .route("/api/v1/history", get(api::not_implemented))
        .route("/api/v1/history/{id}", delete(api::not_implemented))
        .route("/api/v1/blobs", post(api::not_implemented))
        .route("/api/v1/blobs/{id}/chunks/{index}", put(api::not_implemented).get(api::not_implemented))
        .route("/api/v1/blobs/{id}/commit", post(api::not_implemented))
        .route("/ws/v1/device", get(api::not_implemented))
        .fallback(web::index)
        .layer(TraceLayer::new_for_http())
        .layer(RequestBodyLimitLayer::new(MAX_CHUNK_BYTES))
        .layer(DefaultBodyLimit::max(MAX_JSON_BYTES.max(MAX_CHUNK_BYTES)))
        .with_state(state)
}
