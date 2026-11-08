//! Connecting to aria2, and starting/stopping a private `aria2c` when none is running.
//!
//! [`connect`] first probes the configured RPC URL. If aria2 answers, dlux uses it as-is
//! ([`Origin::External`]) and never stops it. If nothing answers and the URL points at this
//! machine, dlux spawns `aria2c --enable-rpc` with a random secret ([`Origin::Spawned`]) and stops it
//! again in [`Aria2::shutdown`].
//!
//! The spawned process is protected against leaks three ways: a graceful `aria2.shutdown` on
//! exit, `kill_on_drop`, and aria2's own `--stop-with-process=<dlux pid>` which makes it exit even
//! if dlux is killed with `SIGKILL` or aborts on panic.
//!
//! The secret is passed through a `0600` conf file rather than argv so it does not show up in
//! `ps`; the file is deleted as soon as aria2 has read it.

use crate::config::Config;
use crate::rpc::{self, Client, Version};
use crate::{fsutil, paths};
use std::net::{Ipv4Addr, TcpListener};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};
use tokio::process::{Child, Command};
use url::{Host, Url};

/// Errors from [`connect`].
#[derive(Debug, thiserror::Error)]
pub enum Aria2Error {
    #[error(
        "aria2 at {url} rejected the RPC secret. Set `rpc_secret` under [aria2] in the dlux \
         config (or pass --rpc-secret) to match aria2's --rpc-secret"
    )]
    Unauthorized { url: String },
    #[error("aria2 is not running at {url} and auto_spawn is disabled")]
    NotRunning {
        url: String,
        #[source]
        source: rpc::Error,
    },
    #[error(
        "aria2 is not reachable at {url}. dlux only starts aria2 automatically for local \
         addresses (localhost / 127.0.0.1)"
    )]
    NotLocal {
        url: String,
        #[source]
        source: rpc::Error,
    },
    #[error(
        "aria2c was not found on PATH. Install aria2 (e.g. `sudo apt install aria2`, \
         `brew install aria2`, `winget install aria2.aria2`) or set `aria2c_path` under [aria2] \
         in the dlux config"
    )]
    NotFound,
    #[error("failed to start {path}: {source}")]
    Spawn {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("aria2c exited during startup ({status}){}", fmt_log(.log))]
    EarlyExit { status: String, log: String },
    #[error("aria2c did not answer on port {port} within {}s", .timeout.as_secs())]
    StartTimeout { port: u16, timeout: Duration },
    #[error("cannot prepare {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error(transparent)]
    Rpc(#[from] rpc::Error),
}

fn fmt_log(log: &str) -> String {
    if log.trim().is_empty() {
        String::new()
    } else {
        format!(":\n{}", log.trim_end())
    }
}

/// Second, longer probe used when the first one timed out against a local port.
const SLOW_PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// Everything [`connect`] needs. Usually built with [`ConnectOptions::from_config`].
#[derive(Debug, Clone)]
pub struct ConnectOptions {
    pub rpc_url: String,
    pub rpc_secret: Option<String>,
    pub auto_spawn: bool,
    pub aria2c_path: Option<PathBuf>,
    pub extra_args: Vec<String>,
    pub download_dir: PathBuf,
    pub max_concurrent: u32,
    /// Where the session file, log and temporary conf file of a spawned aria2 go.
    pub data_dir: PathBuf,
    /// How long to wait for a running aria2 to answer the initial probe.
    pub probe_timeout: Duration,
    /// How long to wait for a spawned aria2 to become ready.
    pub startup_timeout: Duration,
}

impl ConnectOptions {
    pub fn from_config(cfg: &Config) -> Self {
        Self {
            rpc_url: cfg.aria2.rpc_url.clone(),
            rpc_secret: cfg.aria2.rpc_secret.clone(),
            auto_spawn: cfg.aria2.auto_spawn,
            aria2c_path: cfg.aria2.aria2c_path.as_ref().map(paths::expand_tilde),
            extra_args: cfg.aria2.extra_args.clone(),
            download_dir: cfg.download_dir(),
            max_concurrent: cfg.downloads.max_concurrent.max(1),
            data_dir: paths::data_dir(),
            probe_timeout: Duration::from_millis(1500),
            startup_timeout: Duration::from_secs(10),
        }
    }
}

/// Where the aria2 instance came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// An aria2 that was already running. dlux never stops it.
    External,
    /// An `aria2c` started by dlux; stopped by [`Aria2::shutdown`].
    Spawned { pid: Option<u32>, port: u16 },
}

/// A live connection to aria2, owning the child process if dlux started it.
#[derive(Debug)]
pub struct Aria2 {
    client: Client,
    version: Version,
    origin: Origin,
    child: Option<Child>,
}

impl Aria2 {
    pub fn client(&self) -> &Client {
        &self.client
    }

    pub fn version(&self) -> &Version {
        &self.version
    }

    pub fn origin(&self) -> Origin {
        self.origin
    }

    pub fn is_spawned(&self) -> bool {
        matches!(self.origin, Origin::Spawned { .. })
    }

    /// Stop the spawned aria2 (if any) gracefully so the session is saved (SIGTERM on Unix,
    /// `aria2.shutdown` elsewhere), then kill it if it does not exit in time. A no-op for an
    /// external aria2.
    pub async fn shutdown(mut self) {
        let Some(mut child) = self.child.take() else {
            return;
        };
        // On Unix, SIGTERM makes aria2 save its session and exit in about a second, whereas the
        // `aria2.shutdown` RPC always lingers for ~4 s before the process exits.
        if !request_terminate(&child) {
            let graceful = tokio::time::timeout(Duration::from_secs(2), self.client.shutdown());
            match graceful.await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => tracing::debug!("graceful aria2 shutdown failed: {e}"),
                Err(_) => tracing::debug!("timed out sending aria2.shutdown"),
            }
        }
        match tokio::time::timeout(Duration::from_secs(5), child.wait()).await {
            Ok(Ok(status)) => tracing::debug!("aria2c exited with {status}"),
            Ok(Err(e)) => tracing::warn!("waiting for aria2c failed: {e}"),
            Err(_) => {
                tracing::warn!("aria2c did not exit in time, killing it");
                let _ = child.kill().await;
            }
        }
    }
}

/// Ask the child to terminate gracefully with SIGTERM. Returns `false` if that is not possible
/// (non-Unix platform, or the process already exited), in which case the RPC is used instead.
#[cfg(unix)]
fn request_terminate(child: &Child) -> bool {
    use nix::sys::signal::{Signal, kill};
    use nix::unistd::Pid;
    let Some(pid) = child.id().and_then(|id| i32::try_from(id).ok()) else {
        return false;
    };
    kill(Pid::from_raw(pid), Signal::SIGTERM).is_ok()
}

#[cfg(not(unix))]
fn request_terminate(_child: &Child) -> bool {
    false
}

/// Connect to aria2, spawning a private `aria2c` if needed and allowed.
pub async fn connect(opts: &ConnectOptions) -> Result<Aria2, Aria2Error> {
    let client = Client::new(&opts.rpc_url, opts.rpc_secret.as_deref())?;
    let url = client.url().clone();
    let local = is_local(&url);
    let mut probe = client.ping(opts.probe_timeout).await;
    if local && matches!(&probe, Err(e) if e.is_timeout()) {
        // Something is listening but slow to answer. aria2 delays its reply to a wrong secret by
        // a couple of seconds, so give it time rather than spawning a second instance.
        probe = client.ping(SLOW_PROBE_TIMEOUT).await;
    }
    let probe_err = match probe {
        Ok(version) => {
            tracing::info!(url = %opts.rpc_url, version = %version.version, "connected to running aria2");
            return Ok(Aria2 {
                client,
                version,
                origin: Origin::External,
                child: None,
            });
        }
        Err(e) if e.is_unauthorized() => {
            return Err(Aria2Error::Unauthorized {
                url: opts.rpc_url.clone(),
            });
        }
        Err(e) if e.is_connect() || e.is_timeout() => e,
        Err(e) => return Err(e.into()),
    };

    if !local {
        return Err(Aria2Error::NotLocal {
            url: opts.rpc_url.clone(),
            source: probe_err,
        });
    }
    if probe_err.is_timeout() {
        // A local process holds the port but does not speak aria2 RPC in time.
        return Err(probe_err.into());
    }
    if !opts.auto_spawn {
        return Err(Aria2Error::NotRunning {
            url: opts.rpc_url.clone(),
            source: probe_err,
        });
    }
    spawn(opts, &url).await
}

/// `true` if the URL's host is this machine.
pub fn is_local(url: &Url) -> bool {
    match url.host() {
        Some(Host::Domain(d)) => d.eq_ignore_ascii_case("localhost"),
        Some(Host::Ipv4(ip)) => ip.is_loopback(),
        Some(Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    }
}

/// Generate a random 128-bit secret as 32 hex characters.
pub fn random_secret() -> String {
    let bytes: [u8; 16] = rand::random();
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn port_is_free(port: u16) -> bool {
    TcpListener::bind((Ipv4Addr::LOCALHOST, port)).is_ok()
}

fn free_port() -> std::io::Result<u16> {
    Ok(TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?
        .local_addr()?
        .port())
}

fn locate_aria2c(opts: &ConnectOptions) -> Result<PathBuf, Aria2Error> {
    match &opts.aria2c_path {
        Some(p) if p.is_file() => Ok(p.clone()),
        Some(_) => Err(Aria2Error::NotFound),
        None => paths::find_binary("aria2c").ok_or(Aria2Error::NotFound),
    }
}

/// Contents of the conf file handed to a spawned aria2.
fn conf_contents(port: u16, secret: &str, opts: &ConnectOptions, session: &Path) -> String {
    let mut lines = vec![
        "enable-rpc=true".to_owned(),
        format!("rpc-listen-port={port}"),
        "rpc-listen-all=false".to_owned(),
        format!("rpc-secret={secret}"),
        format!("dir={}", opts.download_dir.display()),
        format!("max-concurrent-downloads={}", opts.max_concurrent),
        "continue=true".to_owned(),
        format!("input-file={}", session.display()),
        format!("save-session={}", session.display()),
        "save-session-interval=30".to_owned(),
        format!("stop-with-process={}", std::process::id()),
        "console-log-level=warn".to_owned(),
        "summary-interval=0".to_owned(),
        "show-console-readout=false".to_owned(),
    ];
    lines.push(String::new());
    lines.join("\n")
}

async fn spawn(opts: &ConnectOptions, url: &Url) -> Result<Aria2, Aria2Error> {
    let aria2c = locate_aria2c(opts)?;
    let io_err = |path: &Path| {
        let path = path.to_path_buf();
        move |source| Aria2Error::Io { path, source }
    };

    // Prefer the configured port; fall back to a free one if something else holds it.
    let wanted = url.port_or_known_default().unwrap_or(6800);
    let port = if port_is_free(wanted) {
        wanted
    } else {
        free_port().map_err(io_err(Path::new("127.0.0.1")))?
    };
    let secret = opts.rpc_secret.clone().unwrap_or_else(random_secret);

    std::fs::create_dir_all(&opts.data_dir).map_err(io_err(&opts.data_dir))?;
    std::fs::create_dir_all(&opts.download_dir).map_err(io_err(&opts.download_dir))?;
    let session = opts.data_dir.join("aria2.session");
    if !session.exists() {
        std::fs::write(&session, b"").map_err(io_err(&session))?;
    }
    let conf = ConfFile::create(
        opts.data_dir
            .join(format!("aria2-rpc-{}.conf", std::process::id())),
        &conf_contents(port, &secret, opts, &session),
    )
    .map_err(|(path, source)| Aria2Error::Io { path, source })?;
    let log_path = opts.data_dir.join("aria2.log");
    let log = std::fs::File::create(&log_path).map_err(io_err(&log_path))?;
    let log_err = log.try_clone().map_err(io_err(&log_path))?;

    let mut cmd = Command::new(&aria2c);
    cmd.arg(format!("--conf-path={}", conf.path.display()))
        .args(&opts.extra_args)
        .stdin(Stdio::null())
        .stdout(log)
        .stderr(log_err)
        .kill_on_drop(true);
    // Own process group: a Ctrl-C in the terminal goes to dlux, which then shuts aria2 down
    // gracefully, instead of hitting both at once.
    #[cfg(unix)]
    cmd.process_group(0);
    tracing::info!(path = %aria2c.display(), port, "spawning aria2c");
    let mut child = cmd.spawn().map_err(|source| Aria2Error::Spawn {
        path: aria2c.clone(),
        source,
    })?;

    let rpc_url = format!("http://127.0.0.1:{port}/jsonrpc");
    let client = Client::new(&rpc_url, Some(&secret))?;
    let deadline = Instant::now() + opts.startup_timeout;
    let version = loop {
        if let Ok(Some(status)) = child.try_wait() {
            return Err(Aria2Error::EarlyExit {
                status: status.to_string(),
                log: tail(&log_path, 20),
            });
        }
        match client.ping(Duration::from_millis(500)).await {
            Ok(v) => break v,
            Err(e) if (e.is_connect() || e.is_timeout()) && Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            Err(e) if e.is_connect() || e.is_timeout() => {
                let _ = child.kill().await;
                return Err(Aria2Error::StartTimeout {
                    port,
                    timeout: opts.startup_timeout,
                });
            }
            Err(e) => {
                let _ = child.kill().await;
                return Err(e.into());
            }
        }
    };
    // aria2 has read its configuration; the secret no longer needs to exist on disk.
    drop(conf);

    let pid = child.id();
    tracing::info!(?pid, port, version = %version.version, "aria2c ready");
    Ok(Aria2 {
        client,
        version,
        origin: Origin::Spawned { pid, port },
        child: Some(child),
    })
}

/// A private conf file that is deleted on drop.
struct ConfFile {
    path: PathBuf,
}

impl ConfFile {
    fn create(path: PathBuf, contents: &str) -> Result<Self, (PathBuf, std::io::Error)> {
        match fsutil::write_private(&path, contents.as_bytes()) {
            Ok(()) => Ok(Self { path }),
            Err(e) => Err((path, e)),
        }
    }
}

impl Drop for ConfFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Last `n` lines of a file, for error messages.
fn tail(path: &Path, n: usize) -> String {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_hosts() {
        let local = |s: &str| is_local(&Url::parse(s).unwrap());
        assert!(local("http://localhost:6800/jsonrpc"));
        assert!(local("http://LOCALHOST:6800/jsonrpc"));
        assert!(local("http://127.0.0.1:6800/jsonrpc"));
        assert!(local("http://127.1.2.3:6800/jsonrpc"));
        assert!(local("http://[::1]:6800/jsonrpc"));
        assert!(!local("http://192.168.1.10:6800/jsonrpc"));
        assert!(!local("https://seedbox.example.com/jsonrpc"));
    }

    #[test]
    fn secrets_are_random_hex() {
        let a = random_secret();
        let b = random_secret();
        assert_eq!(a.len(), 32);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }

    #[test]
    fn conf_file_has_rpc_settings_and_is_removed_on_drop() {
        let dir = tempfile::tempdir().unwrap();
        let opts = ConnectOptions {
            data_dir: dir.path().to_path_buf(),
            download_dir: dir.path().join("dl"),
            ..ConnectOptions::from_config(&Config::default())
        };
        let text = conf_contents(6801, "abc123", &opts, &dir.path().join("s"));
        assert!(text.contains("enable-rpc=true\n"));
        assert!(text.contains("rpc-listen-port=6801\n"));
        assert!(text.contains("rpc-listen-all=false\n"));
        assert!(text.contains("rpc-secret=abc123\n"));
        assert!(text.contains(&format!("stop-with-process={}\n", std::process::id())));

        let path = dir.path().join("x.conf");
        let conf = ConfFile::create(path.clone(), &text).unwrap();
        assert!(path.exists());
        drop(conf);
        assert!(!path.exists());
    }

    #[test]
    fn missing_explicit_aria2c_is_not_found() {
        let opts = ConnectOptions {
            aria2c_path: Some(PathBuf::from("/nonexistent/aria2c")),
            ..ConnectOptions::from_config(&Config::default())
        };
        assert!(matches!(locate_aria2c(&opts), Err(Aria2Error::NotFound)));
    }

    #[test]
    fn tail_returns_last_lines() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("log");
        std::fs::write(&p, "1\n2\n3\n4\n").unwrap();
        assert_eq!(tail(&p, 2), "3\n4");
        assert_eq!(tail(&dir.path().join("missing"), 2), "");
    }
}
