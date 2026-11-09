//! `dlux serve`: a REST API plus a WebSocket that pushes the live state every second. The web
//! frontend (milestone 7) talks only to this API; the aria2 RPC secret never leaves the server.
//!
//! | Method | Path                               | Body / query                         |
//! |--------|------------------------------------|--------------------------------------|
//! | GET    | `/api/state`                       | → full state (as pushed on the WS)   |
//! | GET    | `/api/ws`                          | WebSocket, one state message per tick|
//! | POST   | `/api/downloads`                   | `{text?, urls?, dir?, paused?}`      |
//! | POST   | `/api/upload`                      | raw .torrent/.metalink; `?kind&name&dir&paused` |
//! | POST   | `/api/downloads/{gid}/pause`       |                                      |
//! | POST   | `/api/downloads/{gid}/resume`      |                                      |
//! | DELETE | `/api/downloads/{gid}`             | stop if running, else forget         |
//! | POST   | `/api/pause-all`, `/api/resume-all`, `/api/purge` |                       |
//! | GET/PUT| `/api/limits`                      | `{download?, upload?}` ("2M" or bytes)|
//! | GET/PUT| `/api/downloads/{gid}/limits`      | same                                 |
//! | POST   | `/api/video/probe`                 | `{url}` → formats                    |
//! | POST   | `/api/video`                       | `{url, selector, title?, dir?, …}`   |
//! | POST   | `/api/ytdlp/update`                |                                      |

mod api;
mod guard;
mod view;

use crate::cli::ServeArgs;
use anyhow::{Context, Result, bail};
use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::routing::{get, post};
use dlux_core::Config;
use dlux_core::aria2::{Aria2, Origin};
use dlux_core::jobs::JobManager;
use dlux_core::service::Service;
use dlux_core::state::{AppState, POLL_INTERVAL, spawn_poller};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::watch;

/// Largest accepted upload (.torrent files with many pieces can be several MB).
const UPLOAD_LIMIT: usize = 32 << 20;

/// Build the router: API routes behind the [`guard`] checks.
fn router(ctx: api::Ctx, guard: guard::Guard) -> Router {
    let guard = Arc::new(guard);
    let api = Router::new()
        .route("/state", get(api::state))
        .route("/ws", get(api::ws))
        .route("/downloads", post(api::add))
        .route(
            "/upload",
            post(api::upload).layer(DefaultBodyLimit::max(UPLOAD_LIMIT)),
        )
        .route("/downloads/{gid}", axum::routing::delete(api::remove))
        .route("/downloads/{gid}/pause", post(api::pause))
        .route("/downloads/{gid}/resume", post(api::resume))
        .route(
            "/downloads/{gid}/limits",
            get(api::download_limits).put(api::set_download_limits),
        )
        .route("/pause-all", post(api::pause_all))
        .route("/resume-all", post(api::resume_all))
        .route("/purge", post(api::purge))
        .route(
            "/limits",
            get(api::global_limits).put(api::set_global_limits),
        )
        .route("/video/probe", post(api::probe))
        .route("/video", post(api::start_video))
        .route("/ytdlp/update", post(api::update_ytdlp))
        .fallback(api::not_found);
    Router::new()
        .route("/", get(api::index))
        .nest("/api", api)
        .with_state(ctx)
        .layer(axum::middleware::from_fn_with_state(
            guard,
            guard::middleware,
        ))
}

fn origin_text(aria2: &Aria2) -> String {
    match aria2.origin() {
        Origin::Spawned { .. } => "started by dlux".to_owned(),
        Origin::External => {
            let url = aria2.client().url();
            let host = url.host_str().unwrap_or("?");
            url.port()
                .map_or_else(|| host.to_owned(), |p| format!("{host}:{p}"))
        }
    }
}

/// Everything running behind the router; stop it with [`Background::stop`].
struct Background {
    hub: tokio::task::JoinHandle<()>,
    poller: dlux_core::state::PollerHandle,
    jobs: JobManager,
}

impl Background {
    async fn stop(self) {
        self.hub.abort();
        drop(self.poller);
        self.jobs.shutdown().await;
    }
}

/// Build the app: poller, job manager, the state hub and the router.
fn app(aria2: &Aria2, cfg: &Config, token: Option<String>) -> (Router, Background) {
    let jobs = JobManager::new(cfg.aria2c_binary());
    let (mut polls, poller) = spawn_poller(aria2.client().clone(), POLL_INTERVAL);
    let service = Service::new(aria2.client().clone(), jobs.clone(), poller.refresher());
    let (state_tx, state_rx) = watch::channel::<Arc<str>>(Arc::from("{}"));

    // The hub: one task owns the state and publishes a fresh JSON snapshot after every poll
    // and job change; every WebSocket client and /api/state read from it.
    let version = aria2.version().version.clone();
    let origin = origin_text(aria2);
    let hub_jobs = jobs.clone();
    let hub = tokio::spawn(async move {
        let mut state = AppState::default();
        let changed = hub_jobs.changed();
        loop {
            tokio::select! {
                poll = polls.recv() => match poll {
                    Some(event) => {
                        state.set_local(hub_jobs.downloads());
                        state.handle(event);
                    }
                    None => return,
                },
                () = changed.notified() => state.set_local(hub_jobs.downloads()),
            }
            let jobs = hub_jobs.jobs();
            let view = view::state(&state, &jobs, &version, &origin);
            match serde_json::to_string(&view) {
                Ok(json) => {
                    let _ = state_tx.send(Arc::from(json));
                }
                Err(e) => tracing::error!("cannot serialize the state: {e}"),
            }
        }
    });

    let ctx = api::Ctx {
        service,
        state: state_rx,
        default_dir: cfg.download_dir().display().to_string(),
    };
    let router = router(ctx, guard::Guard { token });
    (router, Background { hub, poller, jobs })
}

/// Run the web server until SIGINT/SIGTERM.
pub async fn serve(aria2: &Aria2, cfg: &Config, args: &ServeArgs) -> Result<()> {
    let token = cfg.web.token.clone().filter(|t| !t.is_empty());
    if !args.bind.is_loopback() && token.is_none() {
        bail!(
            "refusing to listen on {} without authentication: set `token` under [web] in the config",
            args.bind
        );
    }
    let addr = SocketAddr::new(args.bind, args.port);
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("cannot listen on {addr} (is the port already in use?)"))?;
    let (router, background) = app(aria2, cfg, token.clone());

    let shown = if args.bind.is_unspecified() {
        SocketAddr::new(std::net::Ipv4Addr::LOCALHOST.into(), args.port)
    } else {
        listener.local_addr()?
    };
    println!(
        "dlux {} · aria2 {} ({})",
        env!("CARGO_PKG_VERSION"),
        aria2.version().version,
        origin_text(aria2)
    );
    match &token {
        Some(_) => println!("web UI on http://{shown}/?token=<your web token>"),
        None => println!("web UI on http://{shown}/"),
    }
    println!("press Ctrl-C to stop");

    let result = axum::serve(listener, router)
        .with_graceful_shutdown(crate::signals::shutdown())
        .await
        .context("web server failed");
    background.stop().await;
    result
}

#[cfg(test)]
mod tests;
