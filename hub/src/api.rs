use axum::Json;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::Serialize;

#[derive(Serialize)]
struct ErrorBody {
    error: &'static str,
    phase: &'static str,
}

pub async fn not_implemented() -> impl IntoResponse {
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(ErrorBody { error: "reserved signaling endpoint", phase: "v1" }),
    )
}
