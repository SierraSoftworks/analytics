use actix_web::http::header::ContentType;
use actix_web::{HttpRequest, HttpResponse};
use include_dir::{Dir, include_dir};

/// The frontend bundle, embedded at compile time. `agent/build.rs` guarantees the
/// directory exists (with a placeholder) even when `trunk build` has not been run.
static ASSETS: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/../ui/dist");

/// Serve a static asset by path, falling back to `index.html` so that client-side
/// routes are handled by the SPA.
pub async fn serve(req: HttpRequest) -> HttpResponse {
    let path = req.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };

    match ASSETS.get_file(path) {
        Some(file) => asset_response(path, file.contents()),
        None => index_response(),
    }
}

fn asset_response(path: &str, contents: &'static [u8]) -> HttpResponse {
    let content_type = match path.rsplit('.').next() {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "application/javascript; charset=utf-8",
        Some("wasm") => "application/wasm",
        Some("css") => "text/css; charset=utf-8",
        Some("json") => "application/json",
        Some("svg") => "image/svg+xml",
        Some("ico") => "image/x-icon",
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("woff2") => "font/woff2",
        Some("woff") => "font/woff",
        Some("ttf") => "font/ttf",
        Some("map") => "application/json",
        _ => "application/octet-stream",
    };

    HttpResponse::Ok()
        .insert_header((actix_web::http::header::CONTENT_TYPE, content_type))
        .body(contents)
}

fn index_response() -> HttpResponse {
    match ASSETS.get_file("index.html") {
        Some(file) => HttpResponse::Ok()
            .content_type(ContentType::html())
            .body(file.contents()),
        None => HttpResponse::InternalServerError()
            .content_type(ContentType::html())
            .body("<!DOCTYPE html><title>Analytics</title><p>The UI has not been built.</p>"),
    }
}
