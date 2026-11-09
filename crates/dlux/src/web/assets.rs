//! The embedded web UI (see `build.rs`): files are served with their MIME type and an ETag.
//! Vite's hashed `assets/*` are cached for a year; everything else is revalidated so a new dlux
//! build shows its new UI. Unknown paths without an extension get `index.html` (app routes).

use axum::http::{HeaderMap, HeaderValue, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};

#[derive(rust_embed::Embed)]
#[folder = "$OUT_DIR/web"]
struct Assets;

fn etag(file: &rust_embed::EmbeddedFile) -> String {
    let hash: String = file.metadata.sha256_hash()[..12]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    format!("\"{hash}\"")
}

fn respond(path: &str, file: rust_embed::EmbeddedFile, headers: &HeaderMap) -> Response {
    let tag = etag(&file);
    let cache = if path.starts_with("assets/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    let not_modified = headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.split(',').any(|t| t.trim() == tag));
    let mut resp = if not_modified {
        StatusCode::NOT_MODIFIED.into_response()
    } else {
        (
            [(header::CONTENT_TYPE, file.metadata.mimetype().to_owned())],
            file.data.into_owned(),
        )
            .into_response()
    };
    let h = resp.headers_mut();
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
    if let Ok(v) = HeaderValue::from_str(&tag) {
        h.insert(header::ETAG, v);
    }
    resp
}

pub async fn serve(uri: Uri, headers: HeaderMap) -> Response {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };
    if let Some(file) = Assets::get(path) {
        return respond(path, file, &headers);
    }
    let last = path.rsplit('/').next().unwrap_or("");
    match Assets::get("index.html") {
        Some(index) if !last.contains('.') => respond("index.html", index, &headers),
        _ => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn get(path: &str, headers: HeaderMap) -> Response {
        serve(path.parse().unwrap(), headers).await
    }

    #[tokio::test]
    async fn serves_the_shell_with_etags() {
        let r = get("/", HeaderMap::new()).await;
        assert_eq!(r.status(), StatusCode::OK);
        assert!(
            r.headers()[header::CONTENT_TYPE]
                .to_str()
                .unwrap()
                .starts_with("text/html")
        );
        assert_eq!(r.headers()[header::CACHE_CONTROL], "no-cache");
        let tag = r.headers()[header::ETAG].clone();
        let mut h = HeaderMap::new();
        h.insert(header::IF_NONE_MATCH, tag);
        assert_eq!(get("/", h).await.status(), StatusCode::NOT_MODIFIED);
        assert_eq!(
            get("/downloads/abc", HeaderMap::new()).await.status(),
            StatusCode::OK,
            "app route"
        );
        assert_eq!(
            get("/missing.js", HeaderMap::new()).await.status(),
            StatusCode::NOT_FOUND
        );
    }
}
