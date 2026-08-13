use axum::http::StatusCode;
use axum::response::{Html, IntoResponse};

pub async fn index() -> impl IntoResponse {
    (
        StatusCode::OK,
        Html(
            r#"<!doctype html>
<html lang="zh-CN">
<head><meta charset="utf-8"><title>Asterism Hub</title></head>
<body>
  <h1>Asterism Hub</h1>
  <p>Web 历史中心将在 Phase 3 内嵌到本二进制。当前仅提供健康检查与 API 骨架。</p>
</body>
</html>"#,
        ),
    )
}
