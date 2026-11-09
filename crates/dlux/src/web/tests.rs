//! End-to-end tests of the web API against a real, private `aria2c` (skipped when aria2c is
//! not installed): the real router on a random port, driven with HTTP and WebSocket clients.

use super::app;
use dlux_core::aria2::{self, Aria2, ConnectOptions};
use dlux_core::{Config, paths};
use futures_util::StreamExt;
use serde_json::{Value, json};
use std::net::{Ipv4Addr, TcpListener};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn aria2c_available() -> bool {
    let found = paths::find_in_path("aria2c").is_some();
    if !found {
        eprintln!("aria2c not found on PATH; skipping web integration test");
    }
    found
}

fn free_port() -> u16 {
    TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// Serve `body` slowly over HTTP/1.1 (so downloads stay active long enough to control). Honours
/// `Range: bytes=N-` like a real server, so aria2 can resume.
async fn slow_file_server(body: Vec<u8>) -> String {
    let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((mut sock, _)) = listener.accept().await {
            let body = body.clone();
            tokio::spawn(async move {
                let mut buf = vec![0u8; 4096];
                let mut req = Vec::new();
                while !req.windows(4).any(|w| w == b"\r\n\r\n") {
                    match sock.read(&mut buf).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => req.extend_from_slice(&buf[..n]),
                    }
                }
                let req = String::from_utf8_lossy(&req).to_ascii_lowercase();
                let start = req
                    .lines()
                    .find_map(|l| l.strip_prefix("range: bytes="))
                    .and_then(|r| r.split('-').next()?.trim().parse::<usize>().ok())
                    .filter(|s| *s < body.len());
                let head = match start {
                    Some(s) => format!(
                        "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {s}-{}/{}\r\n\
                         Content-Length: {}\r\nAccept-Ranges: bytes\r\nConnection: close\r\n\r\n",
                        body.len() - 1,
                        body.len(),
                        body.len() - s
                    ),
                    None => format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nAccept-Ranges: bytes\r\n\
                         Connection: close\r\n\r\n",
                        body.len()
                    ),
                };
                if sock.write_all(head.as_bytes()).await.is_err() {
                    return;
                }
                for chunk in body[start.unwrap_or(0)..].chunks(16 * 1024) {
                    if sock.write_all(chunk).await.is_err() {
                        return;
                    }
                    tokio::time::sleep(Duration::from_millis(200)).await;
                }
            });
        }
    });
    format!("http://127.0.0.1:{port}/file.bin")
}

struct Server {
    base: String,
    http: reqwest::Client,
    aria2: Aria2,
    background: Option<super::Background>,
    _dir: tempfile::TempDir,
}

impl Server {
    async fn start(token: Option<&str>) -> Server {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = Config::default();
        cfg.downloads.dir = dir.path().join("downloads");
        let opts = ConnectOptions {
            rpc_url: format!("http://127.0.0.1:{}/jsonrpc", free_port()),
            data_dir: dir.path().join("data"),
            download_dir: dir.path().join("downloads"),
            probe_timeout: Duration::from_millis(300),
            ..ConnectOptions::from_config(&cfg)
        };
        let aria2 = aria2::connect(&opts).await.expect("spawn aria2c");
        let (router, background) = app(&aria2, &cfg, token.map(str::to_owned));
        let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        Server {
            base,
            http: reqwest::Client::new(),
            aria2,
            background: Some(background),
            _dir: dir,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    async fn get(&self, path: &str) -> (u16, Value) {
        let r = self.http.get(self.url(path)).send().await.unwrap();
        (r.status().as_u16(), r.json().await.unwrap_or(Value::Null))
    }

    async fn send(&self, method: reqwest::Method, path: &str, body: Option<Value>) -> (u16, Value) {
        let mut req = self.http.request(method, self.url(path));
        if let Some(b) = body {
            req = req.json(&b);
        }
        let r = req.send().await.unwrap();
        (r.status().as_u16(), r.json().await.unwrap_or(Value::Null))
    }

    async fn post(&self, path: &str, body: Value) -> (u16, Value) {
        self.send(reqwest::Method::POST, path, Some(body)).await
    }

    /// Poll /api/state until `pred` holds.
    async fn wait_state(&self, what: &str, pred: impl Fn(&Value) -> bool) -> Value {
        for _ in 0..50 {
            let (_, state) = self.get("/api/state").await;
            if pred(&state) {
                return state;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        panic!("timed out waiting for {what}");
    }

    async fn stop(mut self) {
        if let Some(bg) = self.background.take() {
            bg.stop().await;
        }
        self.aria2.shutdown().await;
    }
}

fn download<'a>(state: &'a Value, gid: &str) -> Option<&'a Value> {
    state["downloads"]
        .as_array()?
        .iter()
        .find(|d| d["gid"] == gid)
}

#[tokio::test]
async fn rest_api_end_to_end() {
    if !aria2c_available() {
        return;
    }
    let srv = Server::start(None).await;
    let state = srv
        .wait_state("the first poll", |s| s["aria2"]["connected"] == true)
        .await;
    assert_eq!(state["aria2"]["origin"], "started by dlux");
    assert!(state["downloads"].as_array().unwrap().is_empty());
    assert_eq!(state["version"], env!("CARGO_PKG_VERSION"));

    // Add a slow download; it shows up as active.
    let url = slow_file_server(vec![7u8; 4 << 20]).await;
    let (status, added) = srv.post("/api/downloads", json!({ "text": url })).await;
    assert_eq!(status, 200, "{added}");
    let gid = added["added"][0]["gids"][0].as_str().unwrap().to_owned();
    assert!(added["message"].as_str().unwrap().starts_with("Added"));
    let state = srv
        .wait_state("the download to be active", |s| {
            download(s, &gid).is_some_and(|d| d["status"] == "active")
        })
        .await;
    let d = download(&state, &gid).unwrap();
    assert_eq!(d["kind"], "http");
    assert_eq!(d["tab"], "active");
    assert_eq!(d["url"], url);
    assert_eq!(state["counts"]["active"], 1);

    // Pause and resume.
    let (status, _) = srv
        .post(&format!("/api/downloads/{gid}/pause"), json!({}))
        .await;
    assert_eq!(status, 200);
    srv.wait_state("paused", |s| {
        download(s, &gid).is_some_and(|d| d["status"] == "paused")
    })
    .await;
    let (status, _) = srv
        .post(&format!("/api/downloads/{gid}/resume"), json!({}))
        .await;
    assert_eq!(status, 200);
    srv.wait_state("resumed", |s| {
        download(s, &gid).is_some_and(|d| d["status"] == "active")
    })
    .await;
    // It keeps going (resumes from where it stopped instead of failing).
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let (_, state) = srv.get("/api/state").await;
    assert_eq!(
        download(&state, &gid).unwrap()["status"],
        "active",
        "{state}"
    );

    // Per-download and global limits, as text or bytes.
    let (status, l) = srv
        .send(
            reqwest::Method::PUT,
            &format!("/api/downloads/{gid}/limits"),
            Some(json!({ "download": "300K" })),
        )
        .await;
    assert_eq!(status, 200, "{l}");
    assert_eq!(l["download"], 300 * 1024);
    assert_eq!(l["download_text"], "300K");
    let (_, l) = srv.get(&format!("/api/downloads/{gid}/limits")).await;
    assert_eq!(l["download"], 300 * 1024);
    let (status, l) = srv
        .send(
            reqwest::Method::PUT,
            "/api/limits",
            Some(json!({ "download": 2097152, "upload": "0" })),
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(l["download_text"], "2M");
    let (status, err) = srv
        .send(
            reqwest::Method::PUT,
            "/api/limits",
            Some(json!({ "download": "fast" })),
        )
        .await;
    assert_eq!(status, 400);
    assert!(err["error"].as_str().unwrap().contains("not a speed"));

    // Remove: an active download is stopped, a stopped one is forgotten.
    let (status, _) = srv
        .send(
            reqwest::Method::DELETE,
            &format!("/api/downloads/{gid}"),
            None,
        )
        .await;
    assert_eq!(status, 200);
    srv.wait_state("removed", |s| {
        download(s, &gid).is_some_and(|d| d["status"] == "removed")
    })
    .await;
    let (status, _) = srv
        .send(
            reqwest::Method::DELETE,
            &format!("/api/downloads/{gid}"),
            None,
        )
        .await;
    assert_eq!(status, 200);
    srv.wait_state("forgotten", |s| download(s, &gid).is_none())
        .await;

    // Errors map to statuses with a message.
    let (status, err) = srv
        .post("/api/downloads/0123456789abcdef/pause", json!({}))
        .await;
    assert_eq!(status, 404, "{err}");
    let (status, err) = srv
        .post("/api/downloads", json!({ "text": "not a url" }))
        .await;
    assert_eq!(status, 400);
    assert!(err["error"].as_str().unwrap().contains("not a URL"));
    let (status, _) = srv.post("/api/downloads/dlux-42/pause", json!({})).await;
    assert_eq!(status, 404);
    let (status, _) = srv.get("/api/nope").await;
    assert_eq!(status, 404);
    let (status, _) = srv
        .post("/api/video/probe", json!({ "url": "ftp://x/y" }))
        .await;
    assert_eq!(status, 400);

    // Upload a metalink as the raw body (browser drag-and-drop).
    let metalink = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<metalink xmlns="urn:ietf:params:xml:ns:metalink"><file name="m.bin"><url>{url}</url></file></metalink>"#
    );
    let r = srv
        .http
        .post(srv.url("/api/upload?name=x.meta4&paused=true"))
        .header("content-type", "application/metalink4+xml")
        .body(metalink)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let up: Value = r.json().await.unwrap();
    let mgid = up["gids"][0].as_str().unwrap().to_owned();
    srv.wait_state("the metalink download", |s| {
        download(s, &mgid).is_some_and(|d| d["status"] == "paused" && d["name"] == "m.bin")
    })
    .await;
    let r = srv
        .http
        .post(srv.url("/api/upload"))
        .header("content-type", "application/octet-stream")
        .body("x")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 400, "unknown file type");

    // Global controls.
    for path in ["/api/pause-all", "/api/resume-all", "/api/purge"] {
        let (status, body) = srv.post(path, json!({})).await;
        assert_eq!(status, 200, "{path}: {body}");
    }

    srv.stop().await;
}

#[tokio::test]
async fn websocket_pushes_state() {
    if !aria2c_available() {
        return;
    }
    let srv = Server::start(None).await;
    let ws_url = srv.url("/api/ws").replacen("http", "ws", 1);
    let (mut ws, _) = tokio_tungstenite::connect_async(&ws_url)
        .await
        .expect("connect");
    let mut ticks = 0;
    while ticks < 2 {
        let msg = tokio::time::timeout(Duration::from_secs(5), ws.next())
            .await
            .expect("a state message within 5 s")
            .unwrap()
            .unwrap();
        let state: Value = serde_json::from_str(msg.to_text().unwrap()).unwrap();
        if state.get("aria2").is_some() {
            assert!(state["history"].is_array());
            ticks += 1;
        }
    }

    // A cross-site page can't open the socket.
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let mut req = ws_url.into_client_request().unwrap();
    req.headers_mut()
        .insert("origin", "https://evil.example".parse().unwrap());
    assert!(tokio_tungstenite::connect_async(req).await.is_err());
    srv.stop().await;
}

#[tokio::test]
async fn security_checks() {
    if !aria2c_available() {
        return;
    }
    let srv = Server::start(None).await;
    // Cross-origin and form posts are refused; same-origin JSON is fine.
    let r = srv
        .http
        .post(srv.url("/api/pause-all"))
        .header("origin", "https://evil.example")
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403);
    let r = srv
        .http
        .post(srv.url("/api/downloads"))
        .header("content-type", "text/plain")
        .body(r#"{"text":"http://x/y"}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 415);
    let r = srv
        .http
        .post(srv.url("/api/pause-all"))
        .header("origin", srv.base.clone())
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    // DNS rebinding: a foreign Host name is refused without a token.
    let r = srv
        .http
        .get(srv.url("/api/state"))
        .header("host", "evil.example")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403);
    // The spawned aria2 has an RPC secret, and nothing the browser can read carries it
    // (aria2 secrets travel as "token:<secret>").
    assert!(srv.aria2.client().has_secret());
    let body = srv
        .http
        .get(srv.url("/api/state"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(!body.contains("token:") && !body.to_ascii_lowercase().contains("secret"));
    let page = srv
        .http
        .get(srv.url("/"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(page.contains("dlux") && !page.contains("token:"));
    srv.stop().await;

    // With a token every request needs it; ?token= sets a cookie.
    let srv = Server::start(Some("s3cret")).await;
    assert_eq!(srv.get("/api/state").await.0, 401);
    let r = srv
        .http
        .get(srv.url("/api/state"))
        .bearer_auth("s3cret")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let r = srv
        .http
        .get(srv.url("/?token=s3cret"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let cookie = r.headers()["set-cookie"].to_str().unwrap();
    assert!(cookie.starts_with("dlux_token=s3cret") && cookie.contains("HttpOnly"));
    let r = srv
        .http
        .get(srv.url("/api/state"))
        .header("cookie", "dlux_token=s3cret")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    srv.stop().await;
}
