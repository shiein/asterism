use axum::http::{StatusCode, Uri, header};
use axum::response::{Html, IntoResponse, Response};
use rust_embed::Embed;

#[derive(Embed)]
#[folder = "../web/dist/"]
struct Asset;

pub async fn asset(uri: Uri) -> Response {
    serve(uri)
}

fn serve(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };
    let file = Asset::get(path).or_else(|| Asset::get("index.html"));
    match file {
        Some(file) => (
            [
                (header::CONTENT_TYPE, mime_guess(path)),
                (header::CACHE_CONTROL, "no-store"),
                (header::HeaderName::from_static("x-content-type-options"), "nosniff"),
                (
                    header::HeaderName::from_static("content-security-policy"),
                    "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; connect-src 'self'; base-uri 'none'; object-src 'none'",
                ),
            ],
            file.data,
        )
            .into_response(),
        None => (StatusCode::NOT_FOUND, Html("<p>web assets missing</p>")).into_response(),
    }
}

fn mime_guess(path: &str) -> &'static str {
    if path.ends_with(".js") || path.ends_with(".mjs") {
        "application/javascript; charset=utf-8"
    } else if path.ends_with(".css") {
        "text/css; charset=utf-8"
    } else if path.ends_with(".json") || path.ends_with(".map") {
        "application/json"
    } else if path.ends_with(".png") {
        "image/png"
    } else if path.ends_with(".svg") {
        "image/svg+xml"
    } else if path.ends_with(".woff2") {
        "font/woff2"
    } else if path.ends_with(".woff") {
        "font/woff"
    } else if path.ends_with(".html") || path.is_empty() || path == "index.html" {
        "text/html; charset=utf-8"
    } else {
        "application/octet-stream"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embeds_built_react_application() {
        let index = Asset::get("index.html").expect("embedded index.html");
        let html = std::str::from_utf8(&index.data).unwrap();
        assert!(html.contains("id=\"root\""));
        assert!(Asset::iter().any(|path| path.starts_with("assets/") && path.ends_with(".js")));
    }
}
