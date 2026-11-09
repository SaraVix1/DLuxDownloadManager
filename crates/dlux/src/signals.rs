//! Shutdown signals shared by the TUI and the web server.

/// Resolves on SIGTERM/SIGHUP (Unix) or Ctrl-C delivered as a signal (e.g. outside raw mode).
pub async fn shutdown() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut term = signal(SignalKind::terminate()).ok();
        let mut hup = signal(SignalKind::hangup()).ok();
        tokio::select! {
            _ = async { if let Some(s) = term.as_mut() { s.recv().await } else { std::future::pending().await } } => {}
            _ = async { if let Some(s) = hup.as_mut() { s.recv().await } else { std::future::pending().await } } => {}
            _ = tokio::signal::ctrl_c() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
