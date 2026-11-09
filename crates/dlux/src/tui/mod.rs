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
use dlux_core::jobs::JobManager;
use dlux_core::service::Service;
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
    // yt-dlp uses aria2c as its downloader too (its own process, not our RPC instance).
    let jobs = JobManager::new(cfg.aria2c_binary());
    let jobs_changed = jobs.changed();
    let executor = Executor {
        service: Service::new(aria2.client().clone(), jobs.clone(), poller.refresher()),
        feedback: feedback_tx,
    };
    // Show the global speed limits in the header from the start.
    app.actions
        .push(app::Action::LoadLimits(dlux_core::limits::Target::Global));

    let mut guard = terminal::init().context("cannot initialise the terminal")?;
    let mut events = EventStream::new();
    let mut shutdown = std::pin::pin!(crate::signals::shutdown());

    let result: Result<()> = async {
        loop {
            guard.terminal.draw(|frame| ui::render(frame, &mut app))?;
            tokio::select! {
                poll = polls.recv() => match poll {
                    Some(event) => {
                        app.set_jobs(jobs.jobs());
                        app.state.handle(event);
                    }
                    None => bail!("the aria2 poller stopped unexpectedly"),
                },
                Some(message) = feedback.recv() => app.on_feedback(message),
                () = jobs_changed.notified() => app.set_jobs(jobs.jobs()),
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
    // Stop yt-dlp (and the aria2c it runs) after the terminal is restored.
    jobs.shutdown().await;
    result
}
