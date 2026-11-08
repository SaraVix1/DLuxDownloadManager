//! Performs the side effects the UI asks for ([`Action`]s): aria2 calls, opening a folder and
//! copying to the clipboard. Each runs on its own task so the UI never blocks; results come back
//! as [`Feedback`] messages.

use super::app::Action;
use base64::Engine;
use dlux_core::limits::{self, Limits, Target};
use dlux_core::paths;
use dlux_core::rpc::Client;
use dlux_core::sources::{self, AddOptions, Source};
use std::io::Write;
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::sync::{Notify, mpsc};

/// A message from a finished action to the UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Feedback {
    /// Show a toast.
    Toast { text: String, error: bool },
    /// Current speed limits, after loading or changing them.
    Limits(Target, Limits),
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
    pub client: Client,
    /// Ask the poller for a fresh snapshot once an aria2 call succeeded.
    pub refresh: Arc<Notify>,
    pub feedback: mpsc::UnboundedSender<Feedback>,
}

impl Executor {
    /// Start `action`. `terminal` is where the OSC 52 clipboard sequence is written.
    pub fn run(&self, action: Action, terminal: &mut impl Write) {
        let c = self.client.clone();
        let tx = self.feedback.clone();
        match action {
            Action::Pause(gid) => {
                self.rpc("pause", None, async move { c.pause(&gid).await.map(drop) })
            }
            Action::Resume(gid) => {
                self.rpc(
                    "resume",
                    None,
                    async move { c.unpause(&gid).await.map(drop) },
                );
            }
            Action::Remove(gid) => {
                self.rpc(
                    "remove",
                    None,
                    async move { c.remove(&gid).await.map(drop) },
                );
            }
            Action::RemoveResult(gid) => {
                self.rpc("remove", None, async move {
                    c.remove_download_result(&gid).await
                });
            }
            Action::PauseAll => {
                self.rpc("pause all", Some("Paused all downloads"), async move {
                    c.pause_all().await
                });
            }
            Action::ResumeAll => {
                self.rpc("resume all", Some("Resumed all downloads"), async move {
                    c.unpause_all().await
                });
            }
            Action::Purge => {
                self.rpc(
                    "clear finished",
                    Some("Cleared finished downloads"),
                    async move { c.purge_download_result().await },
                );
            }
            Action::Add { sources, options } => {
                let refresh = Arc::clone(&self.refresh);
                tokio::spawn(async move {
                    let feedback = add(&c, &sources, &options).await;
                    refresh.notify_one();
                    let _ = tx.send(feedback);
                });
            }
            Action::LoadLimits(target) => {
                tokio::spawn(async move {
                    let _ = tx.send(match limits::get(&c, &target).await {
                        Ok(l) => Feedback::Limits(target, l),
                        Err(e) => Feedback::err(format!("Could not read the speed limit: {e}")),
                    });
                });
            }
            Action::SetLimits(target, new) => {
                tokio::spawn(async move {
                    match limits::set(&c, &target, new).await {
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
                            let _ = tx.send(Feedback::err(format!("Could not set the limit: {e}")));
                        }
                    }
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

    /// Run an aria2 call; refresh on success (with an optional toast), toast on failure.
    fn rpc(
        &self,
        what: &'static str,
        success: Option<&'static str>,
        call: impl Future<Output = dlux_core::rpc::Result<()>> + Send + 'static,
    ) {
        let refresh = Arc::clone(&self.refresh);
        let tx = self.feedback.clone();
        tokio::spawn(async move {
            match call.await {
                Ok(()) => {
                    refresh.notify_one();
                    if let Some(msg) = success {
                        let _ = tx.send(Feedback::ok(msg));
                    }
                }
                Err(e) => {
                    let _ = tx.send(Feedback::err(format!("Could not {what}: {e}")));
                }
            }
        });
    }
}

/// Add the sources and summarise the result in one toast.
async fn add(client: &Client, sources: &[Source], options: &AddOptions) -> Feedback {
    let results = sources::add(client, sources, options).await;
    let total = results.len();
    let failed: Vec<_> = sources
        .iter()
        .zip(&results)
        .filter_map(|(s, r)| r.as_ref().err().map(|e| (s, e)))
        .collect();
    let added = total - failed.len();
    let noun = |n: usize| if n == 1 { "download" } else { "downloads" };
    match failed.first() {
        None if total == 1 => Feedback::ok(format!("Added {}", sources[0])),
        None => Feedback::ok(format!("Added {total} {}", noun(total))),
        Some((s, e)) if added == 0 => Feedback::err(format!("Could not add {s}: {e}")),
        Some((s, e)) => Feedback::err(format!("Added {added} of {total} · {s}: {e}")),
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
