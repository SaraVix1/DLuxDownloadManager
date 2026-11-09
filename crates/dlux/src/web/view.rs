//! The JSON the web UI sees: a stable, front-end friendly shape built from [`AppState`] (not
//! aria2's raw field names). It never contains the aria2 RPC secret or the web token.

use dlux_core::jobs::{Job, JobKind};
use dlux_core::rpc::Download;
use dlux_core::state::{AppState, Filter};
use serde::Serialize;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct StateView {
    pub version: &'static str,
    pub aria2: Aria2View,
    pub speed: Speed,
    /// Last 60 s of (download, upload) speeds, oldest first.
    pub history: Vec<[u64; 2]>,
    pub counts: Counts,
    pub downloads: Vec<DownloadView>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Aria2View {
    pub version: String,
    /// "started by dlux" or "host:port".
    pub origin: String,
    /// At least one poll succeeded and the last one did not fail.
    pub connected: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
pub struct Speed {
    pub download: u64,
    pub upload: u64,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
pub struct Counts {
    pub active: usize,
    pub waiting: usize,
    pub done: usize,
    pub errors: usize,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct DownloadView {
    pub gid: String,
    pub name: String,
    /// aria2's status: active, waiting, paused, complete, removed, error.
    pub status: &'static str,
    /// The filter tab it belongs to: active, waiting, done, errors.
    pub tab: &'static str,
    /// http, torrent, magnet, video or tool.
    pub kind: &'static str,
    pub total: u64,
    pub completed: u64,
    /// 0.0 – 1.0.
    pub progress: f64,
    pub download_speed: u64,
    pub upload_speed: u64,
    /// Seconds left, when it can be estimated.
    pub eta: Option<u64>,
    pub connections: u32,
    pub seeders: u32,
    pub seeding: bool,
    pub dir: String,
    pub url: Option<String>,
    pub error: Option<String>,
    /// What yt-dlp is doing ("merging", "item 3/23"), for videos.
    pub note: Option<String>,
    pub files: Vec<FileView>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct FileView {
    pub path: String,
    pub length: u64,
    pub completed: u64,
    pub selected: bool,
}

fn tab(d: &Download) -> &'static str {
    match Filter::ALL.into_iter().find(|f| f.matches(d)) {
        Some(Filter::Active) => "active",
        Some(Filter::Waiting) => "waiting",
        Some(Filter::Done) => "done",
        Some(Filter::Errors) | None => "errors",
    }
}

pub fn download(d: &Download, job: Option<&Job>) -> DownloadView {
    let kind = match job.map(|j| j.kind) {
        Some(JobKind::Video) => "video",
        Some(JobKind::Install(_)) => "tool",
        None if d.is_metadata() => "magnet",
        None if d.is_torrent() => "torrent",
        None => "http",
    };
    DownloadView {
        gid: d.gid.clone(),
        name: d.name(),
        status: d.status.as_str(),
        tab: tab(d),
        kind,
        total: d.total_length,
        completed: d.completed_length,
        progress: d.progress(),
        download_speed: d.download_speed,
        upload_speed: d.upload_speed,
        eta: d.eta().map(|e| e.as_secs()),
        connections: d.connections,
        seeders: d.num_seeders,
        seeding: d.seeder,
        dir: d.dir.clone(),
        url: d.first_uri().map(str::to_owned).or_else(|| {
            d.info_hash
                .as_ref()
                .map(|h| format!("magnet:?xt=urn:btih:{h}"))
        }),
        error: d.error_message.clone().filter(|e| !e.is_empty()),
        note: job.and_then(Job::note),
        files: d
            .files
            .iter()
            .filter(|f| !f.path.is_empty())
            .map(|f| FileView {
                path: f.path.clone(),
                length: f.length,
                completed: f.completed_length,
                selected: f.selected,
            })
            .collect(),
    }
}

/// Build the view. `jobs` are the local jobs behind `state.local`.
pub fn state(state: &AppState, jobs: &[Job], aria2_version: &str, origin: &str) -> StateView {
    let counts = state.counts();
    let job =
        |gid: &str| dlux_core::jobs::local_id(gid).and_then(|id| jobs.iter().find(|j| j.id == id));
    StateView {
        version: env!("CARGO_PKG_VERSION"),
        aria2: Aria2View {
            version: aria2_version.to_owned(),
            origin: origin.to_owned(),
            connected: state.loaded && state.error.is_none(),
            error: state.error.clone(),
        },
        speed: Speed {
            download: state.download_speed(),
            upload: state.upload_speed(),
        },
        history: state.history.iter().map(|(d, u)| [d, u]).collect(),
        counts: Counts {
            active: counts.active,
            waiting: counts.waiting,
            done: counts.done,
            errors: counts.errors,
        },
        downloads: state.all().map(|d| download(d, job(&d.gid))).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dlux_core::rpc::{DownloadStatus, File, GlobalStat, Snapshot, Uri};

    #[test]
    fn builds_a_view() {
        let mut st = AppState::default();
        st.apply(Snapshot {
            stat: GlobalStat {
                download_speed: 100,
                upload_speed: 5,
                ..Default::default()
            },
            active: vec![Download {
                gid: "2089b05ecca3d829".into(),
                status: DownloadStatus::Active,
                total_length: 1000,
                completed_length: 250,
                download_speed: 100,
                dir: "/dl".into(),
                files: vec![File {
                    path: "/dl/a.iso".into(),
                    length: 1000,
                    completed_length: 250,
                    selected: true,
                    uris: vec![Uri {
                        uri: "https://x/a.iso".into(),
                        status: "used".into(),
                    }],
                    ..Default::default()
                }],
                ..Default::default()
            }],
            stopped: vec![Download {
                gid: "5555666677778888".into(),
                status: DownloadStatus::Error,
                error_message: Some("Resource not found".into()),
                info_hash: Some("abc".into()),
                ..Default::default()
            }],
            ..Default::default()
        });
        let v = state(&st, &[], "1.37.0", "started by dlux");
        assert!(v.aria2.connected);
        assert_eq!(
            v.speed,
            Speed {
                download: 100,
                upload: 5
            }
        );
        assert_eq!(v.history, vec![[100, 5]]);
        assert_eq!((v.counts.active, v.counts.errors), (1, 1));
        let a = &v.downloads[0];
        assert_eq!(
            (a.name.as_str(), a.status, a.tab, a.kind),
            ("a.iso", "active", "active", "http")
        );
        assert_eq!(a.progress, 0.25);
        assert_eq!(a.url.as_deref(), Some("https://x/a.iso"));
        assert_eq!(a.files.len(), 1);
        let e = &v.downloads[1];
        assert_eq!((e.status, e.tab), ("error", "errors"));
        assert_eq!(e.error.as_deref(), Some("Resource not found"));
        assert_eq!(e.url.as_deref(), Some("magnet:?xt=urn:btih:abc"));

        let json = serde_json::to_value(&v).unwrap();
        assert_eq!(json["downloads"][0]["download_speed"], 100);
        assert!(json.get("secret").is_none());
    }
}
