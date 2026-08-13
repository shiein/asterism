use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde::Serialize;

#[derive(Serialize)]
struct ErrorBody {
    error: &'static str,
    phase: &'static str,
}

pub async fn not_implemented() -> impl IntoResponse {
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(ErrorBody { error: "not implemented in phase 1", phase: "hub_api" }),
    )
}
