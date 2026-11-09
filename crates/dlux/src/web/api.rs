//! REST handlers and the live-update WebSocket. Every handler is a thin wrapper around
//! [`Service`]; see the route table in [`super::router`].

use axum::Json;
use axum::body::Bytes;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{Html, IntoResponse, Response};
use dlux_core::limits::{self, Limits, Target};
use dlux_core::service::{self, FileKind, Service};
use dlux_core::sources::AddOptions;
use dlux_core::ytdlp::{DownloadSpec, VideoInfo};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;
use tokio::sync::watch;

/// Shared by all handlers.
#[derive(Clone)]
pub struct Ctx {
    pub service: Service,
    /// The latest state as JSON, updated once per second (and on changes).
    pub state: watch::Receiver<Arc<str>>,
    /// Used when a request names no directory.
    pub default_dir: String,
}

/// An error response: `{"error": "…"}` with a fitting status.
pub struct ApiError(StatusCode, String);

impl From<service::Error> for ApiError {
    fn from(e: service::Error) -> Self {
        let status = match &e {
            service::Error::Invalid(_) => StatusCode::BAD_REQUEST,
            service::Error::NotFound(_) => StatusCode::NOT_FOUND,
            service::Error::Failed(_) => StatusCode::BAD_GATEWAY,
        };
        Self(status, e.to_string())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({ "error": self.1 }))).into_response()
    }
}

type ApiResult<T> = Result<T, ApiError>;

fn ok() -> Json<serde_json::Value> {
    Json(json!({ "ok": true }))
}

pub async fn not_found() -> ApiError {
    ApiError(StatusCode::NOT_FOUND, "no such endpoint".into())
}

// ----- state ----------------------------------------------------------------------------------

pub async fn state(State(ctx): State<Ctx>) -> Response {
    let body = ctx.state.borrow().clone();
    (
        [(header::CONTENT_TYPE, "application/json")],
        body.to_string(),
    )
        .into_response()
}

pub async fn ws(State(ctx): State<Ctx>, upgrade: WebSocketUpgrade) -> Response {
    upgrade.on_upgrade(move |socket| live(socket, ctx.state))
}

/// Push the state on connect and after every change until the client goes away.
async fn live(mut socket: WebSocket, mut state: watch::Receiver<Arc<str>>) {
    loop {
        let msg = state.borrow_and_update().clone();
        if socket
            .send(Message::Text(msg.as_ref().into()))
            .await
            .is_err()
        {
            return;
        }
        tokio::select! {
            changed = state.changed() => if changed.is_err() { return },
            incoming = socket.recv() => match incoming {
                // Clients don't need to send anything; a close or error ends the stream.
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => return,
                Some(Ok(_)) => {}
            },
        }
    }
}

// ----- adding ---------------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct AddBody {
    /// Free-form input as in the Add dialog: one URL / magnet / path per line.
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub urls: Vec<String>,
    pub dir: Option<String>,
    #[serde(default)]
    pub paused: bool,
}

impl Ctx {
    fn options(&self, dir: Option<String>, paused: bool) -> AddOptions {
        let dir = dir
            .filter(|d| !d.trim().is_empty())
            .unwrap_or_else(|| self.default_dir.clone());
        AddOptions {
            dir: Some(dir),
            paused,
        }
    }
}

pub async fn add(State(ctx): State<Ctx>, Json(body): Json<AddBody>) -> ApiResult<Response> {
    let mut text = body.text.unwrap_or_default();
    for url in body.urls {
        text.push('\n');
        text.push_str(&url);
    }
    let summary = ctx
        .service
        .add_text(&text, &ctx.options(body.dir, body.paused))
        .await?;
    let status = if summary.added.is_empty() {
        StatusCode::BAD_GATEWAY
    } else {
        StatusCode::OK
    };
    let added: Vec<_> = summary
        .added
        .iter()
        .map(|(s, gids)| json!({ "source": s.to_string(), "gids": gids }))
        .collect();
    let failed: Vec<_> = summary
        .failed
        .iter()
        .map(|(s, e)| json!({ "source": s.to_string(), "error": e }))
        .collect();
    Ok((
        status,
        Json(json!({ "message": summary.message(), "added": added, "failed": failed })),
    )
        .into_response())
}

#[derive(Debug, Deserialize)]
pub struct UploadQuery {
    /// "torrent" or "metalink"; otherwise taken from the Content-Type or `name`.
    pub kind: Option<String>,
    /// Original file name, used to tell .torrent from .metalink.
    pub name: Option<String>,
    pub dir: Option<String>,
    #[serde(default)]
    pub paused: bool,
}

fn upload_kind(q: &UploadQuery, headers: &HeaderMap) -> Option<FileKind> {
    let ct = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let name = q.name.as_deref().unwrap_or("").to_ascii_lowercase();
    match q.kind.as_deref() {
        Some("torrent") => return Some(FileKind::Torrent),
        Some("metalink") => return Some(FileKind::Metalink),
        _ => {}
    }
    if ct.starts_with("application/x-bittorrent") || name.ends_with(".torrent") {
        Some(FileKind::Torrent)
    } else if ct.contains("metalink") || name.ends_with(".metalink") || name.ends_with(".meta4") {
        Some(FileKind::Metalink)
    } else {
        None
    }
}

/// Upload a `.torrent` or `.metalink` file as the raw request body (drag-and-drop in the
/// browser).
pub async fn upload(
    State(ctx): State<Ctx>,
    Query(q): Query<UploadQuery>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Json<serde_json::Value>> {
    let kind = upload_kind(&q, &headers).ok_or_else(|| {
        ApiError(
            StatusCode::BAD_REQUEST,
            "only .torrent and .metalink files can be uploaded".into(),
        )
    })?;
    let gids = ctx
        .service
        .add_file(kind, &body, &ctx.options(q.dir.clone(), q.paused))
        .await?;
    Ok(Json(json!({ "gids": gids })))
}

// ----- controls -------------------------------------------------------------------------------

pub async fn pause(
    State(ctx): State<Ctx>,
    Path(gid): Path<String>,
) -> ApiResult<impl IntoResponse> {
    ctx.service.pause(&gid).await?;
    Ok(ok())
}

pub async fn resume(
    State(ctx): State<Ctx>,
    Path(gid): Path<String>,
) -> ApiResult<impl IntoResponse> {
    ctx.service.resume(&gid).await?;
    Ok(ok())
}

pub async fn remove(
    State(ctx): State<Ctx>,
    Path(gid): Path<String>,
) -> ApiResult<impl IntoResponse> {
    ctx.service.remove(&gid).await?;
    Ok(ok())
}

pub async fn pause_all(State(ctx): State<Ctx>) -> ApiResult<impl IntoResponse> {
    ctx.service.pause_all().await?;
    Ok(ok())
}

pub async fn resume_all(State(ctx): State<Ctx>) -> ApiResult<impl IntoResponse> {
    ctx.service.resume_all().await?;
    Ok(ok())
}

pub async fn purge(State(ctx): State<Ctx>) -> ApiResult<impl IntoResponse> {
    ctx.service.purge().await?;
    Ok(ok())
}

// ----- limits ---------------------------------------------------------------------------------

/// A limit as a number of bytes per second or a string like "500K"; 0 / "" = unlimited.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum LimitValue {
    Bytes(u64),
    Text(String),
}

impl LimitValue {
    fn bytes(&self) -> Result<u64, ApiError> {
        match self {
            Self::Bytes(b) => Ok(*b),
            Self::Text(t) => limits::parse(t).map_err(|e| ApiError(StatusCode::BAD_REQUEST, e)),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct LimitsBody {
    pub download: Option<LimitValue>,
    pub upload: Option<LimitValue>,
}

#[derive(Debug, Serialize)]
pub struct LimitsView {
    pub download: u64,
    pub upload: u64,
    /// Ready for an input box: "2M", "0".
    pub download_text: String,
    pub upload_text: String,
}

impl From<Limits> for LimitsView {
    fn from(l: Limits) -> Self {
        Self {
            download: l.download,
            upload: l.upload,
            download_text: limits::format_input(l.download),
            upload_text: limits::format_input(l.upload),
        }
    }
}

async fn get_limits(ctx: &Ctx, target: Target) -> ApiResult<Json<LimitsView>> {
    Ok(Json(ctx.service.limits(&target).await?.into()))
}

/// Change the given limits; a missing field keeps its current value.
async fn put_limits(ctx: &Ctx, target: Target, body: LimitsBody) -> ApiResult<Json<LimitsView>> {
    let current = ctx.service.limits(&target).await?;
    let new = Limits {
        download: match &body.download {
            Some(v) => v.bytes()?,
            None => current.download,
        },
        upload: match &body.upload {
            Some(v) => v.bytes()?,
            None => current.upload,
        },
    };
    ctx.service.set_limits(&target, new).await?;
    Ok(Json(new.into()))
}

pub async fn global_limits(State(ctx): State<Ctx>) -> ApiResult<Json<LimitsView>> {
    get_limits(&ctx, Target::Global).await
}

pub async fn set_global_limits(
    State(ctx): State<Ctx>,
    Json(body): Json<LimitsBody>,
) -> ApiResult<Json<LimitsView>> {
    put_limits(&ctx, Target::Global, body).await
}

pub async fn download_limits(
    State(ctx): State<Ctx>,
    Path(gid): Path<String>,
) -> ApiResult<Json<LimitsView>> {
    get_limits(&ctx, Target::Download(gid)).await
}

pub async fn set_download_limits(
    State(ctx): State<Ctx>,
    Path(gid): Path<String>,
    Json(body): Json<LimitsBody>,
) -> ApiResult<Json<LimitsView>> {
    put_limits(&ctx, Target::Download(gid), body).await
}

// ----- video ----------------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct ProbeBody {
    pub url: String,
}

pub async fn probe(
    State(ctx): State<Ctx>,
    Json(body): Json<ProbeBody>,
) -> ApiResult<Json<VideoInfo>> {
    Ok(Json(ctx.service.probe(body.url.trim()).await?))
}

/// One of the choices from a probe.
#[derive(Debug, Deserialize)]
pub struct VideoBody {
    pub url: String,
    pub selector: String,
    pub title: Option<String>,
    pub dir: Option<String>,
    #[serde(default)]
    pub playlist: bool,
    pub size: Option<u64>,
    #[serde(default)]
    pub needs_merge: bool,
}

pub async fn start_video(
    State(ctx): State<Ctx>,
    Json(body): Json<VideoBody>,
) -> ApiResult<Json<serde_json::Value>> {
    let dir = ctx.options(body.dir, false).dir.unwrap_or_default();
    let spec = DownloadSpec {
        title: body.title.unwrap_or_else(|| body.url.clone()),
        url: body.url,
        selector: body.selector,
        dir: dlux_core::paths::expand_tilde(dir.trim()),
        playlist: body.playlist,
        size: body.size,
        needs_merge: body.needs_merge,
    };
    let gid = ctx.service.start_video(spec)?;
    Ok(Json(json!({ "gid": gid })))
}

pub async fn update_ytdlp(State(ctx): State<Ctx>) -> ApiResult<Json<serde_json::Value>> {
    let message = ctx.service.update_ytdlp().await?;
    Ok(Json(json!({ "message": message })))
}

// ----- index ----------------------------------------------------------------------------------

/// Placeholder until the web frontend (milestone 7) is embedded.
pub async fn index() -> Html<&'static str> {
    Html(include_str!("placeholder.html"))
}
