//! The interactive terminal UI (the default `dlux` command).

mod actions;
mod app;
mod forms;
mod input;
mod overlay;
mod terminal;
mod textfield;
mod theme;
mod ui;

use actions::Executor;
use anyhow::{Context, Result, bail};
use app::{App, ConnectionInfo};
use crossterm::event::EventStream;
use dlux_core::Config;
use dlux_core::aria2::{Aria2, Origin};
use dlux_core::state::{POLL_INTERVAL, spawn_poller};
use futures_util::StreamExt;
use theme::Theme;
use tokio::sync::mpsc;

/// Run the TUI until the user quits.
pub async fn run(aria2: &Aria2, cfg: &Config) -> Result<()> {
    let info = ConnectionInfo {
        aria2_version: aria2.version().version.clone(),
        origin: match aria2.origin() {
            Origin::Spawned { .. } => "started by dlux".to_owned(),
            Origin::External => {
                let url = aria2.client().url();
                let host = url.host_str().unwrap_or("?");
                match url.port() {
                    Some(port) => format!("{host}:{port}"),
                    None => host.to_owned(),
                }
            }
        },
    };
    let mut app = App::new(Theme::for_terminal(&cfg.ui.theme), info);
    app.default_dir = cfg.download_dir().display().to_string();
    let (mut polls, poller) = spawn_poller(aria2.client().clone(), POLL_INTERVAL);
    let (feedback_tx, mut feedback) = mpsc::unbounded_channel();
    let executor = Executor {
        client: aria2.client().clone(),
        refresh: poller.refresher(),
        feedback: feedback_tx,
    };
    // Show the global speed limits in the header from the start.
    app.actions
        .push(app::Action::LoadLimits(dlux_core::limits::Target::Global));

    let mut guard = terminal::init().context("cannot initialise the terminal")?;
    let mut events = EventStream::new();
    let mut shutdown = std::pin::pin!(shutdown_signal());

    let result: Result<()> = async {
        loop {
            guard.terminal.draw(|frame| ui::render(frame, &mut app))?;
            tokio::select! {
                poll = polls.recv() => match poll {
                    Some(event) => app.state.handle(event),
                    None => bail!("the aria2 poller stopped unexpectedly"),
                },
                Some(message) = feedback.recv() => app.on_feedback(message),
                event = events.next() => match event {
                    Some(Ok(event)) => app.on_event(event),
                    Some(Err(e)) => return Err(e).context("reading terminal input"),
                    None => break,
                },
                () = &mut shutdown => break,
            }
            if app.should_quit {
                break;
            }
            for action in app.take_actions() {
                executor.run(action, guard.terminal.backend_mut());
            }
        }
        Ok(())
    }
    .await;

    drop(poller);
    drop(guard);
    result
}

/// Resolves on SIGTERM/SIGHUP (Unix) or Ctrl-C delivered as a signal (e.g. outside raw mode).
async fn shutdown_signal() {
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
