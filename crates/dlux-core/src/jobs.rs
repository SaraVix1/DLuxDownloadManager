//! Downloads that dlux runs itself rather than through the aria2 RPC: yt-dlp videos and tool
//! installs (yt-dlp, ffmpeg). The front ends show them next to aria2's downloads: each job is
//! presented as a [`Download`] whose GID starts with [`GID_PREFIX`].
//!
//! yt-dlp is started in its own process group (Unix) so that pausing also stops the aria2c it
//! spawns as its downloader. Resuming restarts yt-dlp, which continues the partial files.

use crate::binaries::{self, Tool};
use crate::rpc::{Download, DownloadStatus, File, Uri};
use crate::ytdlp::{self, DownloadSpec, Event, Tools, VideoInfo};
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::process::Command;
use tokio::sync::{Notify, mpsc};
use tokio::task::JoinHandle;

/// GIDs of local jobs look like `dlux-3`; aria2 GIDs are 16 hex digits, so they never collide.
pub const GID_PREFIX: &str = "dlux-";

/// How long probing a page may take.
const PROBE_TIMEOUT: Duration = Duration::from_secs(90);

pub type JobId = u64;

/// The job id behind a local GID.
pub fn local_id(gid: &str) -> Option<JobId> {
    gid.strip_prefix(GID_PREFIX)?.parse().ok()
}

pub fn is_local(gid: &str) -> bool {
    local_id(gid).is_some()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobKind {
    Video,
    Install(Tool),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobState {
    Starting,
    Downloading,
    /// Post-processing (merging streams, fixing up the container…).
    Processing(String),
    Paused,
    Done,
    Failed(String),
}

#[derive(Debug, Clone)]
pub struct Job {
    pub id: JobId,
    pub kind: JobKind,
    pub title: String,
    pub url: String,
    pub dir: PathBuf,
    pub state: JobState,
    /// Bytes of streams that finished, plus the current stream's progress.
    pub done_bytes: u64,
    pub current: u64,
    pub current_total: Option<u64>,
    /// Estimated total from the format picker.
    pub estimate: Option<u64>,
    pub speed: u64,
    pub eta: Option<u64>,
    /// Number of files (streams) started so far.
    pub streams: u32,
    /// Playlist position.
    pub item: Option<(u64, u64)>,
    /// The output file, once known.
    pub file: Option<PathBuf>,
    /// yt-dlp found the file already complete and skipped the download.
    pub already_downloaded: bool,
    spec: Option<DownloadSpec>,
}

impl Job {
    pub fn gid(&self) -> String {
        format!("{GID_PREFIX}{}", self.id)
    }

    pub fn completed(&self) -> u64 {
        self.done_bytes + self.current
    }

    pub fn total(&self) -> Option<u64> {
        // Finished streams plus the current one's size, once either is known.
        let known = (self.done_bytes > 0 || self.current_total.is_some())
            .then(|| self.done_bytes + self.current_total.unwrap_or(0));
        match (known, self.estimate) {
            (Some(k), Some(e)) => Some(k.max(e)),
            (k, e) => k.or(e),
        }
    }

    /// Short status text for the card ("merging", "item 3/23", "installing").
    pub fn note(&self) -> Option<String> {
        let running = matches!(self.state, JobState::Starting | JobState::Downloading);
        match (&self.state, self.kind) {
            (JobState::Done, JobKind::Video) if self.already_downloaded => {
                Some("already on disk".into())
            }
            (JobState::Starting, JobKind::Video) => Some("starting yt-dlp".into()),
            (JobState::Processing(step), _) => Some(step.clone()),
            (_, JobKind::Install(_)) if running => Some("installing".into()),
            _ if running => match self.item {
                Some((n, of)) => Some(format!("item {n}/{of}")),
                None if self.streams > 1 => Some(format!("stream {}", self.streams)),
                None => None,
            },
            _ => None,
        }
    }

    /// The job as a [`Download`], so it can be listed and rendered like aria2's.
    pub fn to_download(&self) -> Download {
        let (status, error_message) = match &self.state {
            JobState::Starting | JobState::Downloading | JobState::Processing(_) => {
                (DownloadStatus::Active, None)
            }
            JobState::Paused => (DownloadStatus::Paused, None),
            JobState::Done => (DownloadStatus::Complete, None),
            JobState::Failed(e) => (DownloadStatus::Error, Some(e.clone())),
        };
        // `Download::name()` uses the file name, so the title stands in until yt-dlp names it.
        let path = self
            .file
            .clone()
            .unwrap_or_else(|| self.dir.join(&self.title));
        let total = self.total().unwrap_or(0);
        let completed = if status == DownloadStatus::Complete {
            total.max(self.completed())
        } else {
            self.completed()
        };
        Download {
            gid: self.gid(),
            status,
            total_length: total,
            completed_length: completed,
            download_speed: if status == DownloadStatus::Active {
                self.speed
            } else {
                0
            },
            dir: self.dir.display().to_string(),
            error_message,
            files: vec![File {
                index: 1,
                path: path.display().to_string(),
                length: total,
                completed_length: completed,
                selected: true,
                uris: vec![Uri {
                    uri: self.url.clone(),
                    status: "used".into(),
                }],
            }],
            ..Default::default()
        }
    }

    fn apply(&mut self, event: Event) {
        match event {
            Event::Progress {
                finished,
                downloaded,
                total,
                speed,
                eta,
            } => {
                self.state = JobState::Downloading;
                self.current = downloaded;
                self.current_total = total.or(self.current_total);
                self.speed = speed.unwrap_or(0);
                self.eta = eta;
                if finished {
                    self.finish_stream();
                }
            }
            Event::Destination(path) => {
                self.finish_stream();
                self.streams += 1;
                self.state = JobState::Downloading;
                self.file = Some(path);
            }
            Event::AlreadyDownloaded(path) => {
                self.file = Some(path);
                self.already_downloaded = true;
            }
            Event::Item { n, of } => self.item = Some((n, of)),
            Event::Merging(path) => {
                self.finish_stream();
                self.file = Some(path);
                self.state = JobState::Processing("merging".into());
            }
            Event::PostProcess(step) => {
                self.finish_stream();
                self.state = JobState::Processing(post_process_label(&step).into());
            }
            Event::Error(_) => {}
        }
    }

    /// The job finished: show the real size of the result instead of estimates.
    fn complete(&mut self, file_size: Option<u64>) {
        self.finish_stream();
        if let Some(size) = file_size {
            self.done_bytes = size;
            self.estimate = None;
        }
    }

    /// Fold the current stream into `done_bytes`.
    fn finish_stream(&mut self) {
        let size = self.current_total.unwrap_or(self.current).max(self.current);
        self.done_bytes += size;
        self.current = 0;
        self.current_total = None;
        self.speed = 0;
        self.eta = None;
    }
}

fn post_process_label(step: &str) -> &'static str {
    match step {
        "ExtractAudio" => "extracting audio",
        "VideoConvertor" | "VideoRemuxer" => "converting",
        "MoveFiles" => "finishing",
        _ => "post-processing",
    }
}

/// How a running job can be stopped.
struct Running {
    stop: Arc<Notify>,
    task: JoinHandle<()>,
}

#[derive(Default)]
struct Inner {
    next_id: JobId,
    jobs: Vec<Job>,
    running: HashMap<JobId, Running>,
}

/// Owns the local jobs. Cheap to clone; all clones share the same jobs.
#[derive(Clone)]
pub struct JobManager {
    inner: Arc<Mutex<Inner>>,
    changed: Arc<Notify>,
    install_locks: Arc<tokio::sync::Mutex<()>>,
    aria2c: Option<PathBuf>,
}

impl std::fmt::Debug for JobManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JobManager").finish_non_exhaustive()
    }
}

impl JobManager {
    /// `aria2c` is handed to yt-dlp as its downloader (yt-dlp's own downloader if `None`).
    pub fn new(aria2c: Option<PathBuf>) -> Self {
        Self {
            inner: Arc::default(),
            changed: Arc::new(Notify::new()),
            install_locks: Arc::default(),
            aria2c,
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Notified when a job is added, removed or changes state (not on every progress tick).
    pub fn changed(&self) -> Arc<Notify> {
        Arc::clone(&self.changed)
    }

    pub fn jobs(&self) -> Vec<Job> {
        self.lock().jobs.clone()
    }

    pub fn job(&self, id: JobId) -> Option<Job> {
        self.lock().jobs.iter().find(|j| j.id == id).cloned()
    }

    /// All jobs as downloads, newest first.
    pub fn downloads(&self) -> Vec<Download> {
        self.lock()
            .jobs
            .iter()
            .rev()
            .map(Job::to_download)
            .collect()
    }

    fn add(
        &self,
        kind: JobKind,
        title: String,
        url: String,
        dir: PathBuf,
        spec: Option<DownloadSpec>,
    ) -> JobId {
        let mut inner = self.lock();
        inner.next_id += 1;
        let id = inner.next_id;
        inner.jobs.push(Job {
            id,
            kind,
            title,
            url,
            dir,
            state: JobState::Starting,
            done_bytes: 0,
            current: 0,
            current_total: None,
            estimate: spec.as_ref().and_then(|s| s.size),
            speed: 0,
            eta: None,
            streams: 0,
            item: None,
            file: None,
            already_downloaded: false,
            spec,
        });
        drop(inner);
        self.changed.notify_one();
        id
    }

    fn update(&self, id: JobId, f: impl FnOnce(&mut Job)) {
        let mut inner = self.lock();
        if let Some(job) = inner.jobs.iter_mut().find(|j| j.id == id) {
            let before = std::mem::discriminant(&job.state);
            f(job);
            let transition = before != std::mem::discriminant(&job.state);
            drop(inner);
            if transition {
                self.changed.notify_one();
            }
        }
    }

    fn set_state(&self, id: JobId, state: JobState) {
        self.update(id, |j| {
            j.state = state;
            j.speed = 0;
        });
    }

    // ----- tools ------------------------------------------------------------------------------

    /// Find `tool`, installing it (shown as a job with a progress bar) if it is missing.
    pub async fn ensure_tool(&self, tool: Tool) -> Result<PathBuf, String> {
        if let Some(path) = binaries::find(tool) {
            return Ok(path);
        }
        if !binaries::installable(tool) {
            return Err(format!(
                "{tool} is not installed, and dlux has no prebuilt {tool} for this system — \
                 please install it with your package manager"
            ));
        }
        // One install at a time; whoever waited finds the tool already there.
        let _guard = self.install_locks.lock().await;
        if let Some(path) = binaries::find(tool) {
            return Ok(path);
        }
        let id = self.add(
            JobKind::Install(tool),
            format!("Installing {tool}"),
            String::new(),
            crate::paths::bin_dir(),
            None,
        );
        let this = self.clone();
        let result = binaries::install(tool, move |done, total| {
            this.update(id, |j| {
                j.state = JobState::Downloading;
                j.current = done;
                j.current_total = total;
            });
        })
        .await;
        match result {
            Ok(path) => {
                self.update(id, |j| {
                    j.title = format!("{tool} installed");
                    j.file = Some(path.clone());
                    j.state = JobState::Done;
                });
                Ok(path)
            }
            Err(e) => {
                let msg = format!("Could not install {tool}: {e}");
                self.set_state(id, JobState::Failed(msg.clone()));
                Err(msg)
            }
        }
    }

    /// Run `yt-dlp -U`; returns its result as a message.
    pub async fn update_ytdlp(&self) -> Result<String, String> {
        let Some(path) = binaries::find(Tool::YtDlp) else {
            return Err("yt-dlp is not installed yet; it is installed on first use".into());
        };
        let out = Command::new(&path)
            .args(["-U", "--no-colors"])
            .stdin(Stdio::null())
            .kill_on_drop(true)
            .output()
            .await
            .map_err(|e| format!("Cannot run yt-dlp: {e}"))?;
        let text = format!(
            "{}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        let lower = text.to_ascii_lowercase();
        if lower.contains("pip")
            || lower.contains("package manager")
            || lower.contains("not supported")
        {
            return Err(format!(
                "This yt-dlp ({}) was installed by another tool; update it there",
                path.display()
            ));
        }
        if let Some(e) = ytdlp::error_from_output(&text) {
            return Err(format!("yt-dlp update failed: {e}"));
        }
        let summary = text
            .lines()
            .map(str::trim)
            .rev()
            .find(|l| {
                l.contains("up to date") || l.contains("Updated") || l.contains("Updating to")
            })
            .unwrap_or("yt-dlp updated")
            .to_owned();
        Ok(summary)
    }

    // ----- probing ----------------------------------------------------------------------------

    /// Ask yt-dlp what `url` offers.
    pub async fn probe(&self, url: &str) -> Result<VideoInfo, String> {
        let ytdlp = self.ensure_tool(Tool::YtDlp).await?;
        let run = Command::new(ytdlp)
            .args(ytdlp::probe_args(url))
            .stdin(Stdio::null())
            .kill_on_drop(true)
            .output();
        let out = tokio::time::timeout(PROBE_TIMEOUT, run)
            .await
            .map_err(|_| "yt-dlp took too long to read the page".to_owned())?
            .map_err(|e| format!("Cannot run yt-dlp: {e}"))?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            return Err(ytdlp::error_from_output(&stderr)
                .unwrap_or_else(|| format!("yt-dlp failed ({})", out.status)));
        }
        ytdlp::parse_probe(url, &out.stdout)
    }

    // ----- video jobs -------------------------------------------------------------------------

    /// Start downloading a video; returns the job's id.
    pub fn start_video(&self, spec: DownloadSpec) -> JobId {
        let id = self.add(
            JobKind::Video,
            spec.title.clone(),
            spec.url.clone(),
            spec.dir.clone(),
            Some(spec),
        );
        self.spawn(id);
        id
    }

    fn spawn(&self, id: JobId) {
        let stop = Arc::new(Notify::new());
        let this = self.clone();
        let stop2 = Arc::clone(&stop);
        let task = tokio::spawn(async move {
            let result = this.run_video(id, &stop2).await;
            let mut inner = this.lock();
            // Only remove our own entry: after a quick pause + resume a newer run owns the slot.
            if inner
                .running
                .get(&id)
                .is_some_and(|r| Arc::ptr_eq(&r.stop, &stop2))
            {
                inner.running.remove(&id);
            }
            drop(inner);
            if let Some(state) = result {
                this.set_state(id, state);
            }
        });
        self.lock().running.insert(id, Running { stop, task });
    }

    /// Run yt-dlp for job `id`. Returns the final state, or `None` if it was stopped on purpose
    /// (the stopper sets the state).
    async fn run_video(&self, id: JobId, stop: &Notify) -> Option<JobState> {
        let spec = self.job(id)?.spec?;
        let ytdlp = match self.ensure_tool(Tool::YtDlp).await {
            Ok(p) => p,
            Err(e) => return Some(JobState::Failed(e)),
        };
        let mut spec = spec;
        let ffmpeg = if spec.needs_merge {
            match self.ensure_tool(Tool::Ffmpeg).await {
                Ok(p) => Some(p),
                Err(_) => {
                    // Without ffmpeg, settle for a single-file format.
                    spec.selector = ytdlp::progressive(&spec.selector);
                    None
                }
            }
        } else {
            binaries::find(Tool::Ffmpeg)
        };
        let tools = Tools {
            aria2c: self.aria2c.clone(),
            ffmpeg,
        };
        if let Err(e) = tokio::fs::create_dir_all(&spec.dir).await {
            return Some(JobState::Failed(format!(
                "Cannot create {}: {e}",
                spec.dir.display()
            )));
        }

        let mut cmd = Command::new(ytdlp);
        cmd.args(ytdlp::download_args(&spec, &tools))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(unix)]
        cmd.process_group(0);
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => return Some(JobState::Failed(format!("Cannot run yt-dlp: {e}"))),
        };
        let (tx, mut lines) = mpsc::unbounded_channel();
        let readers = [
            child.stdout.take().map(|s| read_lines(s, tx.clone())),
            child.stderr.take().map(|s| read_lines(s, tx.clone())),
        ];
        drop(tx);

        let mut last_error = None;
        let stopped = loop {
            tokio::select! {
                line = lines.recv() => match line {
                    Some(line) => {
                        if let Some(event) = ytdlp::parse_line(&line) {
                            if let Event::Error(e) = &event {
                                last_error = Some(ytdlp::friendly_error(e));
                            }
                            self.update(id, |j| j.apply(event));
                        }
                    }
                    None => break false,
                },
                () = stop.notified() => break true,
            }
        };
        if stopped {
            terminate(&mut child).await;
            for r in readers.into_iter().flatten() {
                r.abort();
            }
            return None;
        }
        let status = child.wait().await;
        match status {
            Ok(s) if s.success() => {
                let file = self.job(id).and_then(|j| j.file);
                let size = match file {
                    Some(f) => tokio::fs::metadata(f).await.ok().map(|m| m.len()),
                    None => None,
                };
                self.update(id, |j| j.complete(size));
                Some(JobState::Done)
            }
            Ok(s) => Some(JobState::Failed(
                last_error.unwrap_or_else(|| format!("yt-dlp failed ({s})")),
            )),
            Err(e) => Some(JobState::Failed(format!("yt-dlp: {e}"))),
        }
    }

    /// Stop a running job, leaving it in `state`.
    fn stop(&self, id: JobId, state: Option<JobState>) {
        let running = self.lock().running.remove(&id);
        if let Some(r) = running {
            r.stop.notify_one();
            // The task exits on its own once yt-dlp is down; nothing to await here.
            drop(r.task);
        }
        if let Some(state) = state {
            self.set_state(id, state);
        }
    }

    pub fn pause(&self, id: JobId) -> bool {
        let can = self.job(id).is_some_and(|j| {
            j.kind == JobKind::Video
                && matches!(j.state, JobState::Starting | JobState::Downloading)
        });
        if can {
            self.stop(id, Some(JobState::Paused));
        }
        can
    }

    /// Resume a paused job or retry a failed one.
    pub fn resume(&self, id: JobId) -> bool {
        let can = self.job(id).is_some_and(|j| {
            j.kind == JobKind::Video && matches!(j.state, JobState::Paused | JobState::Failed(_))
        });
        if can {
            self.update(id, |j| {
                j.state = JobState::Starting;
                // yt-dlp reports the whole file again when it continues a stream.
                j.already_downloaded = false;
                j.done_bytes = 0;
                j.current = 0;
                j.current_total = None;
                j.streams = 0;
            });
            self.spawn(id);
        }
        can
    }

    /// Stop (if needed) and forget a job. Downloaded files stay on disk.
    pub fn remove(&self, id: JobId) {
        self.stop(id, None);
        self.lock().jobs.retain(|j| j.id != id);
        self.changed.notify_one();
    }

    pub fn pause_all(&self) {
        for job in self.jobs() {
            self.pause(job.id);
        }
    }

    pub fn resume_all(&self) {
        for job in self.jobs() {
            if job.state == JobState::Paused {
                self.resume(job.id);
            }
        }
    }

    /// Forget finished and failed jobs.
    pub fn purge(&self) {
        self.lock()
            .jobs
            .retain(|j| !matches!(j.state, JobState::Done | JobState::Failed(_)));
        self.changed.notify_one();
    }

    /// Stop every running job and wait (up to a few seconds) for yt-dlp and its aria2c to exit,
    /// so nothing keeps downloading after dlux quits.
    pub async fn shutdown(&self) {
        let running: Vec<(JobId, Running)> = self.lock().running.drain().collect();
        let mut tasks = Vec::new();
        for (id, r) in running {
            r.stop.notify_one();
            tasks.push(r.task);
            self.set_state(id, JobState::Paused);
        }
        let _ = tokio::time::timeout(Duration::from_secs(5), async {
            for t in tasks {
                let _ = t.await;
            }
        })
        .await;
    }
}

fn read_lines(
    stream: impl AsyncRead + Unpin + Send + 'static,
    tx: mpsc::UnboundedSender<String>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut lines = BufReader::new(stream).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            // aria2c's console readout uses \r without \n; keep the last segment.
            let line = line.rsplit('\r').next().unwrap_or("").to_owned();
            if tx.send(line).is_err() {
                break;
            }
        }
    })
}

/// Stop yt-dlp and its aria2c: SIGTERM to the process group (aria2c saves its control file so
/// the download can continue), then kill after a grace period.
async fn terminate(child: &mut tokio::process::Child) {
    #[cfg(unix)]
    if let Some(pid) = child.id() {
        use nix::sys::signal::{Signal, killpg};
        use nix::unistd::Pid;
        let _ = killpg(Pid::from_raw(pid as i32), Signal::SIGTERM);
        if tokio::time::timeout(Duration::from_secs(3), child.wait())
            .await
            .is_ok()
        {
            return;
        }
        let _ = killpg(Pid::from_raw(pid as i32), Signal::SIGKILL);
    }
    let _ = child.kill().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn video_job() -> Job {
        Job {
            id: 7,
            kind: JobKind::Video,
            title: "My Talk".into(),
            url: "https://youtu.be/x".into(),
            dir: "/dl".into(),
            state: JobState::Starting,
            done_bytes: 0,
            current: 0,
            current_total: None,
            estimate: Some(1000),
            speed: 0,
            eta: None,
            streams: 0,
            item: None,
            file: None,
            already_downloaded: false,
            spec: None,
        }
    }

    fn progress(downloaded: u64, total: u64, finished: bool) -> Event {
        Event::Progress {
            finished,
            downloaded,
            total: Some(total),
            speed: Some(50),
            eta: Some(3),
        }
    }

    #[test]
    fn gids() {
        assert_eq!(video_job().gid(), "dlux-7");
        assert_eq!(local_id("dlux-7"), Some(7));
        assert!(is_local("dlux-12"));
        assert!(!is_local("2089b05ecca3d829"));
        assert!(!is_local("dlux-x"));
    }

    #[test]
    fn progress_across_two_streams_and_merge() {
        let mut j = video_job();
        let d = j.to_download();
        assert_eq!(d.name(), "My Talk", "title until yt-dlp names the file");
        assert_eq!(d.status, DownloadStatus::Active);
        assert_eq!(j.note().as_deref(), Some("starting yt-dlp"));

        j.apply(Event::Destination("/dl/My Talk [x].f137.mp4".into()));
        j.apply(progress(300, 800, false));
        assert_eq!(
            (j.completed(), j.total()),
            (300, Some(1000)),
            "estimate is larger"
        );
        j.apply(progress(800, 800, true));
        j.apply(Event::Destination("/dl/My Talk [x].f140.m4a".into()));
        assert_eq!(j.note().as_deref(), Some("stream 2"));
        j.apply(progress(100, 400, false));
        assert_eq!((j.completed(), j.total()), (900, Some(1200)));
        j.apply(Event::Merging("/dl/My Talk [x].mp4".into()));
        assert_eq!(j.state, JobState::Processing("merging".into()));
        assert_eq!(j.note().as_deref(), Some("merging"));
        assert_eq!(j.completed(), 1200);
        let d = j.to_download();
        assert_eq!(d.name(), "My Talk [x].mp4");
        assert_eq!(d.first_uri(), Some("https://youtu.be/x"));
        assert_eq!(d.download_speed, 0);
    }

    #[test]
    fn states_map_to_download_status() {
        let mut j = video_job();
        j.state = JobState::Failed("This video is private".into());
        let d = j.to_download();
        assert_eq!(d.status, DownloadStatus::Error);
        assert_eq!(d.error_message.as_deref(), Some("This video is private"));
        j.state = JobState::Paused;
        assert_eq!(j.to_download().status, DownloadStatus::Paused);
        j.state = JobState::Done;
        let d = j.to_download();
        assert_eq!(d.status, DownloadStatus::Complete);
        assert_eq!(d.progress(), 1.0, "a finished job shows as full");
        j.state = JobState::Downloading;
        j.item = Some((3, 23));
        assert_eq!(j.note().as_deref(), Some("item 3/23"));
        j.kind = JobKind::Install(Tool::Ffmpeg);
        assert_eq!(j.note().as_deref(), Some("installing"));
    }

    #[tokio::test]
    async fn manager_lifecycle_without_processes() {
        let m = JobManager::new(None);
        let id = m.add(JobKind::Video, "t".into(), "u".into(), "/dl".into(), None);
        assert_eq!(m.downloads().len(), 1);
        m.set_state(id, JobState::Done);
        assert!(!m.pause(id), "only running videos pause");
        assert!(!m.resume(id), "finished jobs don't resume");
        m.purge();
        assert!(m.jobs().is_empty());
        let id = m.add(JobKind::Video, "t".into(), "u".into(), "/dl".into(), None);
        m.remove(id);
        assert!(m.jobs().is_empty());
    }

    #[test]
    fn finished_jobs_show_the_real_size() {
        let mut j = video_job();
        j.estimate = Some(2 << 30);
        j.apply(Event::AlreadyDownloaded("/dl/T [x] audio.webm".into()));
        j.complete(Some(10_202_210));
        j.state = JobState::Done;
        let d = j.to_download();
        assert_eq!(d.total_length, 10_202_210, "not the 2 GiB estimate");
        assert_eq!(d.completed_length, 10_202_210);
        assert_eq!(j.note().as_deref(), Some("already on disk"));
    }
}
