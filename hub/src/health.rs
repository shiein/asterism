use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::Serialize;

use crate::state::HubState;

#[derive(Serialize)]
struct HealthBody {
    status: &'static str,
    service: &'static str,
}

pub async fn healthz() -> impl IntoResponse {
    Json(HealthBody { status: "ok", service: "asterism-hub" })
}

pub async fn readyz(State(state): State<Arc<HubState>>) -> impl IntoResponse {
    if state.config.db_path().exists() {
        (StatusCode::OK, Json(HealthBody { status: "ready", service: "asterism-hub" }))
    } else {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(HealthBody { status: "db-missing", service: "asterism-hub" }),
        )
    }
}
