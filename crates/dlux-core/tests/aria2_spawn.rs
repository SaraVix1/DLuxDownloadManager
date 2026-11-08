//! Integration tests against a real `aria2c`. Skipped (with a note) when aria2c is not installed.

use dlux_core::aria2::{self, Aria2Error, ConnectOptions, Origin};
use dlux_core::rpc::{DownloadStatus, Options};
use dlux_core::{Config, paths};
use std::net::{Ipv4Addr, TcpListener};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn aria2c_available() -> bool {
    if paths::find_in_path("aria2c").is_some() {
        true
    } else {
        eprintln!("aria2c not found on PATH; skipping integration test");
        false
    }
}

fn free_port() -> u16 {
    TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn options(dir: &std::path::Path, port: u16) -> ConnectOptions {
    ConnectOptions {
        rpc_url: format!("http://127.0.0.1:{port}/jsonrpc"),
        data_dir: dir.join("data"),
        download_dir: dir.join("downloads"),
        probe_timeout: Duration::from_millis(300),
        ..ConnectOptions::from_config(&Config::default())
    }
}

/// Serve `body` over plain HTTP/1.1 for every request; returns the URL.
async fn serve_file(body: Vec<u8>) -> String {
    let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
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
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/octet-stream\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = sock.write_all(head.as_bytes()).await;
                let _ = sock.write_all(&body).await;
                let _ = sock.shutdown().await;
            });
        }
    });
    format!("http://127.0.0.1:{port}/files/hello.bin")
}

#[tokio::test]
async fn spawns_controls_and_stops_aria2() {
    if !aria2c_available() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let port = free_port();
    let opts = options(dir.path(), port);

    let aria2 = aria2::connect(&opts).await.expect("spawn aria2c");
    assert!(aria2.is_spawned());
    assert!(matches!(aria2.origin(), Origin::Spawned { pid: Some(_), port: p } if p == port));
    assert!(!aria2.version().version.is_empty());
    let client = aria2.client().clone();
    assert!(client.has_secret(), "a random secret is generated");

    // The temporary conf file holding the secret is gone once aria2 is up.
    let leftovers: Vec<_> = std::fs::read_dir(&opts.data_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().ends_with(".conf"))
        .collect();
    assert!(leftovers.is_empty(), "conf file left behind: {leftovers:?}");

    // A real download from a local HTTP server.
    let payload: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
    let url = serve_file(payload.clone()).await;
    let gid = client.add_uri(&[url], &Options::new()).await.unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    let done = loop {
        let d = client.tell_status(&gid).await.unwrap();
        if d.status == DownloadStatus::Complete || Instant::now() > deadline {
            break d;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    assert_eq!(done.status, DownloadStatus::Complete, "{done:?}");
    assert_eq!(done.name(), "hello.bin");
    assert_eq!(done.total_length, payload.len() as u64);
    let written = std::fs::read(opts.download_dir.join("hello.bin")).unwrap();
    assert_eq!(written, payload);

    // A paused download: pause/unpause/remove and the snapshot lists.
    let mut paused = Options::new();
    paused.insert("pause".into(), "true".into());
    let gid2 = client
        .add_uri(&["http://127.0.0.1:9/never.iso".into()], &paused)
        .await
        .unwrap();
    let snap = client.snapshot().await.unwrap();
    assert_eq!(snap.find(&gid2).unwrap().status, DownloadStatus::Paused);
    assert!(snap.stopped.iter().any(|d| d.gid == gid));
    assert_eq!(snap.stat.num_waiting, 1);

    // Per-download and global options.
    let mut limit = Options::new();
    limit.insert("max-download-limit".into(), "1M".into());
    client.change_option(&gid2, &limit).await.unwrap();
    assert_eq!(
        client.get_option(&gid2).await.unwrap()["max-download-limit"],
        "1048576"
    );
    let mut global = Options::new();
    global.insert("max-overall-download-limit".into(), "2M".into());
    client.change_global_option(&global).await.unwrap();
    assert_eq!(
        client.get_global_option().await.unwrap()["max-overall-download-limit"],
        "2097152"
    );

    client.remove(&gid2).await.unwrap();
    client.purge_download_result().await.unwrap();
    let snap = client.snapshot().await.unwrap();
    assert!(snap.stopped.is_empty() && snap.waiting.is_empty() && snap.active.is_empty());

    // Errors from aria2 come back as typed RPC errors.
    let err = client.pause("0000000000000001").await.unwrap_err();
    assert!(matches!(err, dlux_core::rpc::Error::Rpc(_)), "{err:?}");

    // A second dlux instance attaches to the running aria2 instead of spawning another one,
    // and a wrong secret is reported clearly.
    let mut ext = opts.clone();
    ext.rpc_secret = Some("wrong".into());
    assert!(matches!(
        aria2::connect(&ext).await,
        Err(Aria2Error::Unauthorized { .. })
    ));

    aria2.shutdown().await;
    assert!(
        client
            .ping(Duration::from_millis(300))
            .await
            .unwrap_err()
            .is_connect(),
        "aria2 is stopped after shutdown"
    );
    assert!(opts.data_dir.join("aria2.session").exists());
}

#[tokio::test]
async fn attaches_to_external_aria2() {
    if !aria2c_available() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let port = free_port();
    let mut opts = options(dir.path(), port);
    opts.rpc_secret = Some("shared-secret".into());

    // "External" aria2: one we spawn ourselves with a known secret.
    let owner = aria2::connect(&opts).await.unwrap();
    assert!(owner.is_spawned());

    let attached = aria2::connect(&opts).await.unwrap();
    assert_eq!(attached.origin(), Origin::External);
    // Shutting down an attached connection must not stop aria2.
    attached.shutdown().await;
    owner.client().get_version().await.unwrap();

    owner.shutdown().await;
}

#[tokio::test]
async fn no_spawn_when_disabled_or_remote() {
    let dir = tempfile::tempdir().unwrap();
    let mut opts = options(dir.path(), free_port());
    opts.auto_spawn = false;
    assert!(matches!(
        aria2::connect(&opts).await,
        Err(Aria2Error::NotRunning { .. })
    ));

    // 192.0.2.0/24 is TEST-NET-1: never routable, so the probe fails and spawn is refused.
    let mut remote = options(dir.path(), 6800);
    remote.rpc_url = "http://192.0.2.1:6800/jsonrpc".into();
    remote.probe_timeout = Duration::from_millis(200);
    assert!(matches!(
        aria2::connect(&remote).await,
        Err(Aria2Error::NotLocal { .. })
    ));
}
