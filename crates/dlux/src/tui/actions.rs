//! Performs the side effects the UI asks for ([`Action`]s): aria2 calls, opening a folder and
//! copying to the clipboard. Each runs on its own task so the UI never blocks; the outcome comes
//! back as a [`Outcome`] message for a toast.

use super::app::Action;
use base64::Engine;
use dlux_core::paths;
use dlux_core::rpc::Client;
use std::io::Write;
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::sync::{Notify, mpsc};

/// Result of an action, shown as a toast.
#[derive(Debug, Clone)]
pub struct Outcome {
    pub text: String,
    pub error: bool,
}

impl Outcome {
    fn ok(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            error: false,
        }
    }

    fn err(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            error: true,
        }
    }
}

pub struct Executor {
    pub client: Client,
    /// Ask the poller for a fresh snapshot once an aria2 call succeeded.
    pub refresh: Arc<Notify>,
    pub outcomes: mpsc::UnboundedSender<Outcome>,
}

impl Executor {
    /// Start `action`. `terminal` is where the OSC 52 clipboard sequence is written.
    pub fn run(&self, action: Action, terminal: &mut impl Write) {
        let c = self.client.clone();
        match action {
            Action::Pause(gid) => self.rpc("pause", async move { c.pause(&gid).await.map(drop) }),
            Action::Resume(gid) => {
                self.rpc("resume", async move { c.unpause(&gid).await.map(drop) });
            }
            Action::Remove(gid) => {
                self.rpc("remove", async move { c.remove(&gid).await.map(drop) })
            }
            Action::RemoveResult(gid) => {
                self.rpc(
                    "remove",
                    async move { c.remove_download_result(&gid).await },
                );
            }
            Action::OpenFolder(dir) => {
                let tx = self.outcomes.clone();
                tokio::spawn(async move {
                    let _ = tx.send(open_folder(&dir).await);
                });
            }
            Action::Copy(text) => {
                // OSC 52 works in most modern terminals (also over SSH); a native clipboard
                // tool covers the ones that ignore it.
                let _ = write!(terminal, "{}", osc52(&text));
                let _ = terminal.flush();
                let tx = self.outcomes.clone();
                tokio::spawn(async move {
                    let msg = match copy_native(&text).await {
                        Some(tool) => format!("Copied URL to the clipboard ({tool})"),
                        None => "Copied URL to the clipboard".to_owned(),
                    };
                    let _ = tx.send(Outcome::ok(msg));
                });
            }
        }
    }

    fn rpc(
        &self,
        what: &'static str,
        call: impl Future<Output = dlux_core::rpc::Result<()>> + Send + 'static,
    ) {
        let refresh = Arc::clone(&self.refresh);
        let tx = self.outcomes.clone();
        tokio::spawn(async move {
            match call.await {
                Ok(()) => refresh.notify_one(),
                Err(e) => {
                    let _ = tx.send(Outcome::err(format!("Could not {what}: {e}")));
                }
            }
        });
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
async fn open_folder(dir: &str) -> Outcome {
    if dir.is_empty() {
        return Outcome::err("This download has no directory");
    }
    if !std::path::Path::new(dir).is_dir() {
        return Outcome::err(format!(
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
        Ok(s) if s.success() || cfg!(windows) => Outcome::ok(format!("Opened {dir}")),
        Ok(_) => Outcome::err(format!("{opener} could not open {dir}")),
        Err(e) => Outcome::err(format!("Cannot run {opener}: {e}")),
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
