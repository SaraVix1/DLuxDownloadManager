//! Performs the side effects the UI asks for ([`Action`]s): aria2 calls, opening a folder and
//! copying to the clipboard. Each runs on its own task so the UI never blocks; results come back
//! as [`Feedback`] messages.

use super::app::Action;
use base64::Engine;
use dlux_core::limits::{self, Limits, Target};
use dlux_core::paths;
use dlux_core::service::{self, Service};
use dlux_core::ytdlp::VideoInfo;
use std::io::Write;
use std::process::Stdio;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::sync::mpsc;

/// A message from a finished action to the UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Feedback {
    /// Show a toast.
    Toast { text: String, error: bool },
    /// Current speed limits, after loading or changing them.
    Limits(Target, Limits),
    /// The formats of a video page (or why they could not be read).
    Probe {
        url: String,
        result: Result<VideoInfo, String>,
    },
}

impl Feedback {
    fn ok(text: impl Into<String>) -> Self {
        Self::Toast {
            text: text.into(),
            error: false,
        }
    }

    fn err(text: impl Into<String>) -> Self {
        Self::Toast {
            text: text.into(),
            error: true,
        }
    }
}

pub struct Executor {
    /// The operations shared with the web UI.
    pub service: Service,
    pub feedback: mpsc::UnboundedSender<Feedback>,
}

impl Executor {
    /// Run `fut` in the background; toast its error, or `success` when given.
    fn spawn<T: Send + 'static>(
        &self,
        success: Option<&'static str>,
        fut: impl Future<Output = service::Result<T>> + Send + 'static,
    ) {
        let tx = self.feedback.clone();
        tokio::spawn(async move {
            match fut.await {
                Ok(_) => {
                    if let Some(msg) = success {
                        let _ = tx.send(Feedback::ok(msg));
                    }
                }
                Err(e) => {
                    let _ = tx.send(Feedback::err(e.to_string()));
                }
            }
        });
    }

    /// Start `action`. `terminal` is where the OSC 52 clipboard sequence is written.
    pub fn run(&self, action: Action, terminal: &mut impl Write) {
        let svc = self.service.clone();
        let tx = self.feedback.clone();
        match action {
            Action::Pause(gid) => self.spawn(None, async move { svc.pause(&gid).await }),
            Action::Resume(gid) => self.spawn(None, async move { svc.resume(&gid).await }),
            Action::Remove(gid) | Action::RemoveResult(gid) => {
                self.spawn(None, async move { svc.remove(&gid).await });
            }
            Action::PauseAll => {
                self.spawn(Some("Paused all downloads"), async move {
                    svc.pause_all().await
                });
            }
            Action::ResumeAll => {
                self.spawn(Some("Resumed all downloads"), async move {
                    svc.resume_all().await
                });
            }
            Action::Purge => {
                self.spawn(Some("Cleared finished downloads"), async move {
                    svc.purge().await
                });
            }
            Action::Add { sources, options } => {
                tokio::spawn(async move {
                    let summary = svc.add(&sources, &options).await;
                    let _ = tx.send(Feedback::Toast {
                        text: summary.message(),
                        error: !summary.ok(),
                    });
                });
            }
            Action::LoadLimits(target) => {
                tokio::spawn(async move {
                    let _ = tx.send(match svc.limits(&target).await {
                        Ok(l) => Feedback::Limits(target, l),
                        Err(e) => Feedback::err(e.to_string()),
                    });
                });
            }
            Action::SetLimits(target, new) => {
                tokio::spawn(async move {
                    match svc.set_limits(&target, new).await {
                        Ok(()) => {
                            let scope = match target {
                                Target::Global => "Global limit",
                                Target::Download(_) => "Limit",
                            };
                            let _ = tx.send(Feedback::Limits(target, new));
                            let _ = tx.send(Feedback::ok(format!(
                                "{scope}: ↓ {}, ↑ {}",
                                limits::describe(new.download),
                                limits::describe(new.upload)
                            )));
                        }
                        Err(e) => {
                            let _ = tx.send(Feedback::err(e.to_string()));
                        }
                    }
                });
            }
            Action::Probe(url) => {
                tokio::spawn(async move {
                    let result = svc.probe(&url).await.map_err(|e| e.to_string());
                    let _ = tx.send(Feedback::Probe { url, result });
                });
            }
            Action::StartVideo(spec) => {
                if let Err(e) = svc.start_video(spec) {
                    let _ = tx.send(Feedback::err(e.to_string()));
                }
            }
            Action::UpdateYtDlp => {
                tokio::spawn(async move {
                    let _ = tx.send(match svc.update_ytdlp().await {
                        Ok(msg) => Feedback::ok(msg),
                        Err(e) => Feedback::err(e.to_string()),
                    });
                });
            }
            Action::OpenFolder(dir) => {
                tokio::spawn(async move {
                    let _ = tx.send(open_folder(&dir).await);
                });
            }
            Action::Copy(text) => {
                // OSC 52 works in most modern terminals (also over SSH); a native clipboard
                // tool covers the ones that ignore it.
                let _ = write!(terminal, "{}", osc52(&text));
                let _ = terminal.flush();
                tokio::spawn(async move {
                    let msg = match copy_native(&text).await {
                        Some(tool) => format!("Copied URL to the clipboard ({tool})"),
                        None => "Copied URL to the clipboard".to_owned(),
                    };
                    let _ = tx.send(Feedback::ok(msg));
                });
            }
        }
    }
}

/// The OSC 52 "set clipboard" escape sequence.
pub fn osc52(text: &str) -> String {
    let b64 = base64::engine::general_purpose::STANDARD.encode(text);
    // Inside tmux, wrap it so tmux passes it through to the outer terminal.
    if std::env::var_os("TMUX").is_some() {
        format!("\x1bPtmux;\x1b\x1b]52;c;{b64}\x07\x1b\\")
    } else {
        format!("\x1b]52;c;{b64}\x07")
    }
}

/// Clipboard tools to try, in order, with their arguments.
fn clipboard_tools() -> Vec<(&'static str, &'static [&'static str])> {
    if cfg!(target_os = "macos") {
        vec![("pbcopy", &[])]
    } else if cfg!(windows) {
        vec![("clip", &[])]
    } else {
        let mut tools: Vec<(&'static str, &'static [&'static str])> = Vec::new();
        if std::env::var_os("WAYLAND_DISPLAY").is_some() {
            tools.push(("wl-copy", &[]));
        }
        if std::env::var_os("DISPLAY").is_some() {
            tools.push(("xclip", &["-selection", "clipboard"]));
            tools.push(("xsel", &["--clipboard", "--input"]));
        }
        tools
    }
}

/// Copy with the first available native clipboard tool; returns its name on success.
async fn copy_native(text: &str) -> Option<&'static str> {
    for (tool, args) in clipboard_tools() {
        let Some(path) = paths::find_in_path(tool) else {
            continue;
        };
        let child = Command::new(path)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        let Ok(mut child) = child else {
            continue;
        };
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(text.as_bytes()).await;
        }
        // wl-copy and xclip fork to keep serving the selection; the parent exits right away.
        if child.wait().await.is_ok_and(|s| s.success()) {
            return Some(tool);
        }
    }
    None
}

/// Open `dir` in the platform file manager.
async fn open_folder(dir: &str) -> Feedback {
    if dir.is_empty() {
        return Feedback::err("This download has no directory");
    }
    if !std::path::Path::new(dir).is_dir() {
        return Feedback::err(format!(
            "{dir} does not exist (is aria2 on another machine?)"
        ));
    }
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(windows) {
        "explorer"
    } else {
        "xdg-open"
    };
    let status = Command::new(opener)
        .arg(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await;
    match status {
        // explorer.exe exits with 1 even on success.
        Ok(s) if s.success() || cfg!(windows) => Feedback::ok(format!("Opened {dir}")),
        Ok(_) => Feedback::err(format!("{opener} could not open {dir}")),
        Err(e) => Feedback::err(format!("Cannot run {opener}: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn osc52_encodes_text() {
        // Plain or tmux-wrapped depending on $TMUX; both carry the same payload.
        let seq = osc52("https://example.com/a");
        assert!(seq.contains("52;c;aHR0cHM6Ly9leGFtcGxlLmNvbS9h"), "{seq:?}");
        assert!(seq.ends_with('\x07') || seq.ends_with("\x1b\\"));
    }
}
