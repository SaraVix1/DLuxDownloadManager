//! Everything a front end can ask dlux to do, in one place: the TUI and the web API both call
//! these methods. Operations on a GID are routed to the [`JobManager`] for local jobs
//! (`dlux-N`) and to aria2 otherwise. After a change that aria2 should reflect, the poller is
//! asked to refresh so the UI updates right away.

use crate::jobs::{self, JobId, JobManager};
use crate::limits::{self, Limits, Target};
use crate::rpc::{self, Client, Gid};
use crate::sources::{self, AddOptions, Source};
use crate::ytdlp::{DownloadSpec, VideoInfo};
use std::sync::Arc;
use tokio::sync::Notify;

/// Why an operation failed. The message is meant for users.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    /// The request itself is wrong (bad input, wrong state).
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    NotFound(String),
    /// aria2, yt-dlp or the network failed.
    #[error("{0}")]
    Failed(String),
}

pub type Result<T> = std::result::Result<T, Error>;

fn rpc_err(what: &str) -> impl FnOnce(rpc::Error) -> Error + '_ {
    move |e| match &e {
        rpc::Error::Rpc(r) if r.message.contains("is not found") => {
            Error::NotFound(format!("No download with that GID ({what})"))
        }
        rpc::Error::Rpc(r) => Error::Invalid(format!("Could not {what}: {}", r.message)),
        _ => Error::Failed(format!("Could not {what}: {e}")),
    }
}

/// The outcome of adding several sources.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AddSummary {
    pub added: Vec<(Source, Vec<Gid>)>,
    pub failed: Vec<(Source, String)>,
}

impl AddSummary {
    /// One-line summary: "Added 3 downloads", "Added 2 of 3 · x: reason".
    pub fn message(&self) -> String {
        let total = self.added.len() + self.failed.len();
        let noun = if total == 1 { "download" } else { "downloads" };
        match (self.added.first(), self.failed.first()) {
            (Some((s, _)), None) if total == 1 => format!("Added {s}"),
            (_, None) => format!("Added {total} {noun}"),
            (None, Some((s, e))) => format!("Could not add {s}: {e}"),
            (Some(_), Some((s, e))) => format!("Added {} of {total} · {s}: {e}", self.added.len()),
        }
    }

    pub fn ok(&self) -> bool {
        self.failed.is_empty()
    }
}

/// An uploaded `.torrent` / `.metalink` (the web UI cannot pass a local path).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    Torrent,
    Metalink,
}

#[derive(Clone)]
pub struct Service {
    client: Client,
    jobs: JobManager,
    refresh: Arc<Notify>,
}

impl std::fmt::Debug for Service {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Service").finish_non_exhaustive()
    }
}

impl Service {
    /// `refresh` is the poller's refresher ([`crate::state::PollerHandle::refresher`]).
    pub fn new(client: Client, jobs: JobManager, refresh: Arc<Notify>) -> Self {
        Self {
            client,
            jobs,
            refresh,
        }
    }

    pub fn client(&self) -> &Client {
        &self.client
    }

    pub fn jobs(&self) -> &JobManager {
        &self.jobs
    }

    fn changed(&self) {
        self.refresh.notify_one();
    }

    fn job_id(&self, gid: &str) -> Option<Result<JobId>> {
        let id = jobs::local_id(gid)?;
        Some(match self.jobs.job(id) {
            Some(_) => Ok(id),
            None => Err(Error::NotFound(format!("No download {gid}"))),
        })
    }

    // ----- per download -----------------------------------------------------------------------

    pub async fn pause(&self, gid: &str) -> Result<()> {
        if let Some(id) = self.job_id(gid) {
            return if self.jobs.pause(id?) {
                Ok(())
            } else {
                Err(Error::Invalid("Only running videos can be paused".into()))
            };
        }
        self.client.pause(gid).await.map_err(rpc_err("pause"))?;
        self.changed();
        Ok(())
    }

    /// Resume a paused download (or retry a failed video).
    pub async fn resume(&self, gid: &str) -> Result<()> {
        if let Some(id) = self.job_id(gid) {
            return if self.jobs.resume(id?) {
                Ok(())
            } else {
                Err(Error::Invalid(
                    "Only paused or failed videos can be resumed".into(),
                ))
            };
        }
        self.client.unpause(gid).await.map_err(rpc_err("resume"))?;
        self.changed();
        Ok(())
    }

    /// Remove a download: stop it if it is running, or forget it if it already stopped.
    /// Downloaded data stays on disk.
    pub async fn remove(&self, gid: &str) -> Result<()> {
        if let Some(id) = self.job_id(gid) {
            self.jobs.remove(id?);
            return Ok(());
        }
        let d = self
            .client
            .tell_status(gid)
            .await
            .map_err(rpc_err("remove"))?;
        if d.status.is_stopped() {
            self.client
                .remove_download_result(gid)
                .await
                .map_err(rpc_err("remove"))?;
        } else {
            self.client.remove(gid).await.map_err(rpc_err("remove"))?;
        }
        self.changed();
        Ok(())
    }

    // ----- all downloads ----------------------------------------------------------------------

    pub async fn pause_all(&self) -> Result<()> {
        self.jobs.pause_all();
        self.client
            .pause_all()
            .await
            .map_err(rpc_err("pause all"))?;
        self.changed();
        Ok(())
    }

    pub async fn resume_all(&self) -> Result<()> {
        self.jobs.resume_all();
        self.client
            .unpause_all()
            .await
            .map_err(rpc_err("resume all"))?;
        self.changed();
        Ok(())
    }

    /// Forget all finished, failed and removed downloads.
    pub async fn purge(&self) -> Result<()> {
        self.jobs.purge();
        self.client
            .purge_download_result()
            .await
            .map_err(rpc_err("clear finished"))?;
        self.changed();
        Ok(())
    }

    // ----- adding -----------------------------------------------------------------------------

    pub async fn add(&self, sources: &[Source], options: &AddOptions) -> AddSummary {
        let results = sources::add(&self.client, sources, options).await;
        let mut summary = AddSummary::default();
        for (source, result) in sources.iter().zip(results) {
            match result {
                Ok(gids) => summary.added.push((source.clone(), gids)),
                Err(e) => summary.failed.push((source.clone(), e.to_string())),
            }
        }
        self.changed();
        summary
    }

    /// Parse free-form input (as in the Add dialog) and add it. Nothing is added if any part
    /// is invalid.
    pub async fn add_text(&self, text: &str, options: &AddOptions) -> Result<AddSummary> {
        let parsed = sources::parse(text);
        if let Some(bad) = parsed.invalid.first() {
            return Err(Error::Invalid(format!("{}: {}", bad.input, bad.reason)));
        }
        if parsed.sources.is_empty() {
            return Err(Error::Invalid(
                "Nothing to add: give a URL, magnet link or .torrent/.metalink path".into(),
            ));
        }
        Ok(self.add(&parsed.sources, options).await)
    }

    /// Add an uploaded `.torrent` or `.metalink` file.
    pub async fn add_file(
        &self,
        kind: FileKind,
        bytes: &[u8],
        options: &AddOptions,
    ) -> Result<Vec<Gid>> {
        if bytes.is_empty() {
            return Err(Error::Invalid("The file is empty".into()));
        }
        let opts = options.to_aria2();
        let gids = match kind {
            FileKind::Torrent => vec![
                self.client
                    .add_torrent(bytes, &opts)
                    .await
                    .map_err(rpc_err("add the torrent"))?,
            ],
            FileKind::Metalink => self
                .client
                .add_metalink(bytes, &opts)
                .await
                .map_err(rpc_err("add the metalink"))?,
        };
        self.changed();
        Ok(gids)
    }

    // ----- speed limits -----------------------------------------------------------------------

    fn check_target(&self, target: &Target) -> Result<()> {
        match target {
            Target::Download(gid) if jobs::is_local(gid) => Err(Error::Invalid(
                "Videos are downloaded by yt-dlp; only the global limit applies".into(),
            )),
            _ => Ok(()),
        }
    }

    pub async fn limits(&self, target: &Target) -> Result<Limits> {
        self.check_target(target)?;
        limits::get(&self.client, target)
            .await
            .map_err(rpc_err("read the speed limit"))
    }

    pub async fn set_limits(&self, target: &Target, new: Limits) -> Result<()> {
        self.check_target(target)?;
        limits::set(&self.client, target, new)
            .await
            .map_err(rpc_err("set the speed limit"))
    }

    // ----- video ------------------------------------------------------------------------------

    pub async fn probe(&self, url: &str) -> Result<VideoInfo> {
        if !matches!(
            url::Url::parse(url)
                .map(|u| u.scheme().to_owned())
                .as_deref(),
            Ok("http" | "https")
        ) {
            return Err(Error::Invalid("Give an http(s) URL of a video page".into()));
        }
        self.jobs.probe(url).await.map_err(Error::Failed)
    }

    /// Start a yt-dlp download; returns its GID.
    pub fn start_video(&self, spec: DownloadSpec) -> Result<Gid> {
        if spec.selector.trim().is_empty() {
            return Err(Error::Invalid("Choose a format".into()));
        }
        let id = self.jobs.start_video(spec);
        Ok(format!("{}{id}", jobs::GID_PREFIX))
    }

    pub async fn update_ytdlp(&self) -> Result<String> {
        self.jobs.update_ytdlp().await.map_err(Error::Failed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_summary_messages() {
        let url = |u: &str| Source::Url(u.into());
        let mut s = AddSummary {
            added: vec![(url("http://a/x"), vec!["g".into()])],
            ..Default::default()
        };
        assert_eq!(s.message(), "Added http://a/x");
        s.added.push((url("http://a/y"), vec![]));
        assert_eq!(s.message(), "Added 2 downloads");
        s.failed.push((url("http://a/z"), "boom".into()));
        assert_eq!(s.message(), "Added 2 of 3 · http://a/z: boom");
        assert!(!s.ok());
        s.added.clear();
        assert_eq!(s.message(), "Could not add http://a/z: boom");
    }

    #[tokio::test]
    async fn local_gids_never_reach_aria2() {
        // An aria2 URL nothing listens on: any RPC would fail with a connection error.
        let client = Client::new("http://127.0.0.1:9/jsonrpc", None).unwrap();
        let svc = Service::new(client, JobManager::new(None), Arc::new(Notify::new()));
        assert!(matches!(
            svc.pause("dlux-99").await,
            Err(Error::NotFound(_))
        ));
        assert!(matches!(
            svc.limits(&Target::Download("dlux-1".into())).await,
            Err(Error::Invalid(_))
        ));
        assert!(matches!(
            svc.probe("ftp://x/y").await,
            Err(Error::Invalid(_))
        ));
        assert!(matches!(
            svc.add_text("nope", &AddOptions::default()).await,
            Err(Error::Invalid(_))
        ));
        assert!(matches!(
            svc.add_file(FileKind::Torrent, b"", &AddOptions::default())
                .await,
            Err(Error::Invalid(_))
        ));
        assert!(matches!(
            svc.pause("2089b05ecca3d829").await,
            Err(Error::Failed(_))
        ));
    }
}
