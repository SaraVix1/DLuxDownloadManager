use super::error::{Error, Result, RpcError};
use super::types::{
    Download, File, Gid, GlobalStat, Options, PositionFrom, Snapshot, Uri, Version,
};
use base64::prelude::{BASE64_STANDARD, Engine as _};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use url::Url;

/// Default aria2 RPC endpoint.
pub const DEFAULT_RPC_URL: &str = "http://localhost:6800/jsonrpc";

/// Keys requested for list calls (`tellActive`/`tellWaiting`/`tellStopped`). Restricting keys keeps
/// the per-second poll small — notably it skips the potentially huge `bitfield`.
pub const DOWNLOAD_KEYS: &[&str] = &[
    "gid",
    "status",
    "totalLength",
    "completedLength",
    "uploadLength",
    "downloadSpeed",
    "uploadSpeed",
    "connections",
    "numSeeders",
    "seeder",
    "pieceLength",
    "numPieces",
    "dir",
    "files",
    "bittorrent",
    "infoHash",
    "errorCode",
    "errorMessage",
    "followedBy",
    "following",
    "belongsTo",
];

/// Maximum number of waiting/stopped downloads fetched per snapshot.
pub const SNAPSHOT_LIST_LIMIT: u32 = 1000;

/// One call inside a `system.multicall` batch.
#[derive(Debug, Clone)]
pub struct Call {
    pub method: &'static str,
    pub params: Vec<Value>,
}

impl Call {
    pub fn new(method: &'static str, params: Vec<Value>) -> Self {
        Self { method, params }
    }
}

/// Async aria2 JSON-RPC client over HTTP. Cheap to clone; clones share the connection pool.
#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    url: Url,
    secret: Option<Arc<str>>,
    next_id: Arc<AtomicU64>,
}

impl fmt::Debug for Client {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Never print the secret.
        f.debug_struct("Client")
            .field("url", &self.url.as_str())
            .field("secret", &self.secret.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

impl Client {
    /// Create a client for `url` (e.g. `http://localhost:6800/jsonrpc`). An empty secret is
    /// treated as no secret.
    pub fn new(url: &str, secret: Option<&str>) -> Result<Self> {
        let parsed = Url::parse(url).map_err(|e| Error::InvalidUrl {
            url: url.to_owned(),
            reason: e.to_string(),
        })?;
        match parsed.scheme() {
            "http" | "https" => {}
            "ws" | "wss" => {
                return Err(Error::InvalidUrl {
                    url: url.to_owned(),
                    reason: "WebSocket RPC is not supported, use the http:// endpoint".into(),
                });
            }
            other => {
                return Err(Error::InvalidUrl {
                    url: url.to_owned(),
                    reason: format!("unsupported scheme {other:?}"),
                });
            }
        }
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(3))
            .timeout(Duration::from_secs(30))
            .no_proxy() // aria2 is almost always local; never route RPC through a proxy.
            .user_agent(concat!("dlux/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(Error::Http)?;
        Ok(Self {
            http,
            url: parsed,
            secret: secret.filter(|s| !s.is_empty()).map(Arc::from),
            next_id: Arc::new(AtomicU64::new(1)),
        })
    }

    /// The RPC endpoint this client talks to.
    pub fn url(&self) -> &Url {
        &self.url
    }

    /// Whether this client sends an RPC secret.
    pub fn has_secret(&self) -> bool {
        self.secret.is_some()
    }

    /// Prepend the `token:<secret>` parameter when a secret is configured.
    fn with_token(&self, params: Vec<Value>) -> Vec<Value> {
        match &self.secret {
            Some(secret) => {
                let mut out = Vec::with_capacity(params.len() + 1);
                out.push(Value::String(format!("token:{secret}")));
                out.extend(params);
                out
            }
            None => params,
        }
    }

    /// Build a JSON-RPC 2.0 request body.
    pub(crate) fn request_body(&self, method: &str, params: Vec<Value>) -> Value {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let params = if method == "system.multicall" || method == "system.listMethods" {
            // system.* methods do not take a token; multicall carries tokens per call.
            params
        } else {
            self.with_token(params)
        };
        json!({
            "jsonrpc": "2.0",
            "id": format!("dlux-{id}"),
            "method": method,
            "params": params,
        })
    }

    async fn send(&self, body: &Value, timeout: Option<Duration>) -> Result<Vec<u8>> {
        let mut req = self.http.post(self.url.clone()).json(body);
        if let Some(t) = timeout {
            req = req.timeout(t);
        }
        let resp = req.send().await.map_err(|e| self.classify(e))?;
        // aria2 reports JSON-RPC errors with HTTP 400 and a JSON body, so read the body first and
        // only fall back to the HTTP status if it is not JSON.
        let status = resp.status();
        let bytes = resp.bytes().await.map_err(|e| self.classify(e))?;
        if !status.is_success() && serde_json::from_slice::<Value>(&bytes).is_err() {
            return Err(Error::Decode(format!(
                "HTTP {status}: {}",
                String::from_utf8_lossy(&bytes)
                    .chars()
                    .take(200)
                    .collect::<String>()
            )));
        }
        Ok(bytes.to_vec())
    }

    fn classify(&self, e: reqwest::Error) -> Error {
        if e.is_timeout() {
            Error::Timeout {
                url: self.url.to_string(),
                source: e,
            }
        } else if e.is_connect() {
            Error::Connect {
                url: self.url.to_string(),
                source: e,
            }
        } else {
            Error::Http(e)
        }
    }

    async fn call_inner<T: DeserializeOwned>(
        &self,
        method: &str,
        params: Vec<Value>,
        timeout: Option<Duration>,
    ) -> Result<T> {
        let body = self.request_body(method, params);
        let bytes = self.send(&body, timeout).await?;
        decode_response(&bytes)
    }

    /// Call an arbitrary aria2 method. The secret token is added automatically.
    pub async fn call<T: DeserializeOwned>(&self, method: &str, params: Vec<Value>) -> Result<T> {
        self.call_inner(method, params, None).await
    }

    /// Execute several calls in one round trip. Each entry is that call's result or error.
    pub async fn multicall(&self, calls: &[Call]) -> Result<Vec<Result<Value, RpcError>>> {
        let batch: Vec<Value> = calls
            .iter()
            .map(|c| json!({ "methodName": c.method, "params": self.with_token(c.params.clone()) }))
            .collect();
        let raw: Vec<Value> = self
            .call("system.multicall", vec![Value::Array(batch)])
            .await?;
        if raw.len() != calls.len() {
            return Err(Error::Decode(format!(
                "multicall returned {} results for {} calls",
                raw.len(),
                calls.len()
            )));
        }
        raw.into_iter().map(decode_multicall_entry).collect()
    }

    // ----- connection -------------------------------------------------------------------------

    /// `aria2.getVersion`.
    pub async fn get_version(&self) -> Result<Version> {
        self.call("aria2.getVersion", vec![]).await
    }

    /// `aria2.getVersion` with a short timeout, for probing whether aria2 is running.
    pub async fn ping(&self, timeout: Duration) -> Result<Version> {
        self.call_inner("aria2.getVersion", vec![], Some(timeout))
            .await
    }

    // ----- adding -----------------------------------------------------------------------------

    /// `aria2.addUri`: all `uris` must point to the *same* file (mirrors). Magnet links work too.
    pub async fn add_uri(&self, uris: &[String], options: &Options) -> Result<Gid> {
        self.call("aria2.addUri", vec![json!(uris), json!(options)])
            .await
    }

    /// `aria2.addTorrent` from raw .torrent bytes.
    pub async fn add_torrent(&self, torrent: &[u8], options: &Options) -> Result<Gid> {
        let encoded = BASE64_STANDARD.encode(torrent);
        self.call(
            "aria2.addTorrent",
            vec![json!(encoded), json!([]), json!(options)],
        )
        .await
    }

    /// `aria2.addMetalink` from raw .metalink/.meta4 bytes. Returns one GID per file.
    pub async fn add_metalink(&self, metalink: &[u8], options: &Options) -> Result<Vec<Gid>> {
        let encoded = BASE64_STANDARD.encode(metalink);
        self.call("aria2.addMetalink", vec![json!(encoded), json!(options)])
            .await
    }

    // ----- control ----------------------------------------------------------------------------

    async fn gid_call(&self, method: &str, gid: &str) -> Result<Gid> {
        self.call(method, vec![json!(gid)]).await
    }

    async fn ok_call(&self, method: &str, params: Vec<Value>) -> Result<()> {
        let s: String = self.call(method, params).await?;
        if s == "OK" {
            Ok(())
        } else {
            Err(Error::Decode(format!(
                "{method} returned {s:?}, expected \"OK\""
            )))
        }
    }

    pub async fn remove(&self, gid: &str) -> Result<Gid> {
        self.gid_call("aria2.remove", gid).await
    }

    pub async fn force_remove(&self, gid: &str) -> Result<Gid> {
        self.gid_call("aria2.forceRemove", gid).await
    }

    pub async fn pause(&self, gid: &str) -> Result<Gid> {
        self.gid_call("aria2.pause", gid).await
    }

    pub async fn force_pause(&self, gid: &str) -> Result<Gid> {
        self.gid_call("aria2.forcePause", gid).await
    }

    pub async fn unpause(&self, gid: &str) -> Result<Gid> {
        self.gid_call("aria2.unpause", gid).await
    }

    pub async fn pause_all(&self) -> Result<()> {
        self.ok_call("aria2.pauseAll", vec![]).await
    }

    pub async fn force_pause_all(&self) -> Result<()> {
        self.ok_call("aria2.forcePauseAll", vec![]).await
    }

    pub async fn unpause_all(&self) -> Result<()> {
        self.ok_call("aria2.unpauseAll", vec![]).await
    }

    /// Move a waiting download in the queue. Returns the new position.
    pub async fn change_position(&self, gid: &str, pos: i64, how: PositionFrom) -> Result<u64> {
        self.call(
            "aria2.changePosition",
            vec![json!(gid), json!(pos), json!(how.as_str())],
        )
        .await
    }

    /// Remove a finished/failed/removed download from aria2's memory.
    pub async fn remove_download_result(&self, gid: &str) -> Result<()> {
        self.ok_call("aria2.removeDownloadResult", vec![json!(gid)])
            .await
    }

    /// Remove all finished/failed/removed downloads from aria2's memory.
    pub async fn purge_download_result(&self) -> Result<()> {
        self.ok_call("aria2.purgeDownloadResult", vec![]).await
    }

    // ----- queries ----------------------------------------------------------------------------

    pub async fn tell_status(&self, gid: &str) -> Result<Download> {
        self.call("aria2.tellStatus", vec![json!(gid)]).await
    }

    pub async fn tell_active(&self) -> Result<Vec<Download>> {
        self.call("aria2.tellActive", vec![json!(DOWNLOAD_KEYS)])
            .await
    }

    /// Waiting/paused downloads in queue order.
    pub async fn tell_waiting(&self, offset: i64, num: u32) -> Result<Vec<Download>> {
        self.call(
            "aria2.tellWaiting",
            vec![json!(offset), json!(num), json!(DOWNLOAD_KEYS)],
        )
        .await
    }

    /// Stopped downloads. A negative `offset` counts from the most recently stopped one, so
    /// `tell_stopped(-1, n)` lists the newest first.
    pub async fn tell_stopped(&self, offset: i64, num: u32) -> Result<Vec<Download>> {
        self.call(
            "aria2.tellStopped",
            vec![json!(offset), json!(num), json!(DOWNLOAD_KEYS)],
        )
        .await
    }

    pub async fn get_files(&self, gid: &str) -> Result<Vec<File>> {
        self.call("aria2.getFiles", vec![json!(gid)]).await
    }

    pub async fn get_uris(&self, gid: &str) -> Result<Vec<Uri>> {
        self.call("aria2.getUris", vec![json!(gid)]).await
    }

    pub async fn get_global_stat(&self) -> Result<GlobalStat> {
        self.call("aria2.getGlobalStat", vec![]).await
    }

    // ----- options ----------------------------------------------------------------------------

    pub async fn get_option(&self, gid: &str) -> Result<Options> {
        self.call("aria2.getOption", vec![json!(gid)]).await
    }

    /// Change per-download options, e.g. `max-download-limit`.
    pub async fn change_option(&self, gid: &str, options: &Options) -> Result<()> {
        self.ok_call("aria2.changeOption", vec![json!(gid), json!(options)])
            .await
    }

    pub async fn get_global_option(&self) -> Result<Options> {
        self.call("aria2.getGlobalOption", vec![]).await
    }

    /// Change global options, e.g. `max-overall-download-limit`.
    pub async fn change_global_option(&self, options: &Options) -> Result<()> {
        self.ok_call("aria2.changeGlobalOption", vec![json!(options)])
            .await
    }

    // ----- session ----------------------------------------------------------------------------

    pub async fn save_session(&self) -> Result<()> {
        self.ok_call("aria2.saveSession", vec![]).await
    }

    /// Graceful shutdown (aria2 finishes in-flight actions and saves its session).
    pub async fn shutdown(&self) -> Result<()> {
        self.ok_call("aria2.shutdown", vec![]).await
    }

    pub async fn force_shutdown(&self) -> Result<()> {
        self.ok_call("aria2.forceShutdown", vec![]).await
    }

    // ----- aggregate --------------------------------------------------------------------------

    /// Fetch global stats and all download lists in one round trip.
    pub async fn snapshot(&self) -> Result<Snapshot> {
        let limit = SNAPSHOT_LIST_LIMIT;
        let calls = [
            Call::new("aria2.getGlobalStat", vec![]),
            Call::new("aria2.tellActive", vec![json!(DOWNLOAD_KEYS)]),
            Call::new(
                "aria2.tellWaiting",
                vec![json!(0), json!(limit), json!(DOWNLOAD_KEYS)],
            ),
            Call::new(
                "aria2.tellStopped",
                vec![json!(-1), json!(limit), json!(DOWNLOAD_KEYS)],
            ),
        ];
        let results = self.multicall(&calls).await?;
        decode_snapshot(results)
    }
}

/// Decode a JSON-RPC response body into the `result`, or the `error` if present.
pub(crate) fn decode_response<T: DeserializeOwned>(body: &[u8]) -> Result<T> {
    let mut obj: serde_json::Map<String, Value> = serde_json::from_slice(body).map_err(|e| {
        Error::Decode(format!(
            "invalid JSON ({e}): {}",
            String::from_utf8_lossy(body)
                .chars()
                .take(200)
                .collect::<String>()
        ))
    })?;
    if let Some(err) = obj.remove("error").filter(|v| !v.is_null()) {
        let err: RpcError = serde_json::from_value(err)
            .map_err(|e| Error::Decode(format!("malformed error object: {e}")))?;
        return Err(Error::Rpc(err));
    }
    let result = obj
        .remove("result")
        .ok_or_else(|| Error::Decode("response has neither result nor error".into()))?;
    serde_json::from_value(result).map_err(|e| Error::Decode(e.to_string()))
}

/// A multicall entry is either `[result]` on success or `{code, message}` on failure.
fn decode_multicall_entry(entry: Value) -> Result<Result<Value, RpcError>> {
    match entry {
        Value::Array(mut items) if items.len() == 1 => Ok(Ok(items.remove(0))),
        Value::Object(_) => serde_json::from_value::<RpcError>(entry)
            .map(Err)
            .map_err(|e| Error::Decode(format!("malformed multicall error: {e}"))),
        other => Err(Error::Decode(format!(
            "unexpected multicall entry: {other}"
        ))),
    }
}

fn decode_snapshot(results: Vec<Result<Value, RpcError>>) -> Result<Snapshot> {
    fn take<T: DeserializeOwned>(r: Option<Result<Value, RpcError>>) -> Result<T> {
        let v = r.ok_or_else(|| Error::Decode("missing multicall result".into()))??;
        serde_json::from_value(v).map_err(|e| Error::Decode(e.to_string()))
    }
    let mut it = results.into_iter();
    Ok(Snapshot {
        stat: take(it.next())?,
        active: take(it.next())?,
        waiting: take(it.next())?,
        stopped: take(it.next())?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rpc::DownloadStatus;

    fn client(secret: Option<&str>) -> Client {
        Client::new(DEFAULT_RPC_URL, secret).unwrap()
    }

    #[test]
    fn request_includes_token_when_secret_set() {
        let c = client(Some("s3cret"));
        let body = c.request_body("aria2.pause", vec![json!("abc")]);
        assert_eq!(body["jsonrpc"], "2.0");
        assert_eq!(body["method"], "aria2.pause");
        assert_eq!(body["params"], json!(["token:s3cret", "abc"]));
        assert!(body["id"].as_str().unwrap().starts_with("dlux-"));
    }

    #[test]
    fn request_without_secret_has_no_token() {
        let c = client(None);
        let body = c.request_body("aria2.getVersion", vec![]);
        assert_eq!(body["params"], json!([]));
        // Empty secrets are ignored.
        let c = client(Some(""));
        assert!(!c.has_secret());
    }

    #[test]
    fn multicall_does_not_take_top_level_token() {
        let c = client(Some("x"));
        let body = c.request_body("system.multicall", vec![json!([])]);
        assert_eq!(body["params"], json!([[]]));
    }

    #[test]
    fn ids_are_unique() {
        let c = client(None);
        let a = c.request_body("m", vec![]);
        let b = c.clone().request_body("m", vec![]);
        assert_ne!(a["id"], b["id"]);
    }

    #[test]
    fn debug_redacts_secret() {
        let c = client(Some("topsecret"));
        let dbg = format!("{c:?}");
        assert!(!dbg.contains("topsecret"));
        assert!(dbg.contains("redacted"));
    }

    #[test]
    fn rejects_bad_urls() {
        assert!(matches!(
            Client::new("not a url", None),
            Err(Error::InvalidUrl { .. })
        ));
        assert!(matches!(
            Client::new("ws://localhost:6800/jsonrpc", None),
            Err(Error::InvalidUrl { .. })
        ));
        assert!(Client::new("https://seedbox.example.com/jsonrpc", None).is_ok());
    }

    #[test]
    fn decodes_result() {
        let gid: Gid =
            decode_response(br#"{"id":"1","jsonrpc":"2.0","result":"2089b05ecca3d829"}"#).unwrap();
        assert_eq!(gid, "2089b05ecca3d829");
    }

    #[test]
    fn decodes_error() {
        let err = decode_response::<Gid>(
            br#"{"id":"1","jsonrpc":"2.0","error":{"code":1,"message":"Unauthorized"}}"#,
        )
        .unwrap_err();
        assert!(err.is_unauthorized());
        assert!(!err.is_connect());

        let err = decode_response::<Gid>(
            br#"{"id":"1","jsonrpc":"2.0","error":{"code":1,"message":"GID 0000000000000001 is not found"}}"#,
        )
        .unwrap_err();
        match err {
            Error::Rpc(e) => assert!(e.is_not_found()),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn decode_rejects_garbage() {
        assert!(matches!(
            decode_response::<Gid>(b"<html>"),
            Err(Error::Decode(_))
        ));
        assert!(matches!(
            decode_response::<Gid>(br#"{"id":"1","jsonrpc":"2.0"}"#),
            Err(Error::Decode(_))
        ));
        // Wrong result type.
        assert!(matches!(
            decode_response::<Vec<Download>>(br#"{"id":"1","result":"x"}"#),
            Err(Error::Decode(_))
        ));
    }

    #[test]
    fn decodes_multicall_snapshot() {
        let body = br#"{"id":"m","jsonrpc":"2.0","result":[
            [{"downloadSpeed":"100","uploadSpeed":"5","numActive":"1","numWaiting":"1","numStopped":"1","numStoppedTotal":"1"}],
            [[{"gid":"a1","status":"active","totalLength":"10","completedLength":"5","downloadSpeed":"100","files":[]}]],
            [[{"gid":"w1","status":"paused","files":[]}]],
            [[{"gid":"s1","status":"error","errorCode":"3","errorMessage":"Resource not found","files":[]}]]
        ]}"#;
        let raw: Vec<Value> = decode_response(body).unwrap();
        let entries = raw
            .into_iter()
            .map(decode_multicall_entry)
            .collect::<Result<Vec<_>>>()
            .unwrap();
        let snap = decode_snapshot(entries).unwrap();
        assert_eq!(snap.stat.download_speed, 100);
        assert_eq!(snap.active[0].gid, "a1");
        assert_eq!(snap.waiting[0].status, DownloadStatus::Paused);
        assert_eq!(snap.stopped[0].error_code, Some(3));
        assert_eq!(snap.iter().count(), 3);
        assert_eq!(snap.find("w1").unwrap().gid, "w1");
        assert!(snap.find("nope").is_none());
    }

    #[test]
    fn multicall_entry_errors_are_per_call() {
        let ok = decode_multicall_entry(json!(["OK"])).unwrap();
        assert_eq!(ok.unwrap(), json!("OK"));
        let err = decode_multicall_entry(json!({"code": 1, "message": "Unauthorized"}))
            .unwrap()
            .unwrap_err();
        assert!(err.is_unauthorized());
        assert!(decode_multicall_entry(json!(42)).is_err());

        // A failing call inside a snapshot surfaces as an RPC error.
        let entries = vec![
            Err(RpcError {
                code: 1,
                message: "Unauthorized".into(),
            }),
            Ok(json!([])),
            Ok(json!([])),
            Ok(json!([])),
        ];
        assert!(decode_snapshot(entries).unwrap_err().is_unauthorized());
    }
}
