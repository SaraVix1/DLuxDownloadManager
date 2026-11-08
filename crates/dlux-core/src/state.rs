//! Front-end independent application state: the latest aria2 [`Snapshot`], a rolling speed
//! history for the graph, and the filter tabs. Both the TUI and the web UI build on this.

use crate::rpc::{Client, Download, DownloadStatus, GlobalStat, Snapshot};
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Notify, mpsc};
use tokio::task::JoinHandle;

/// Number of samples kept for the speed graph (one per poll, i.e. the last 60 s).
pub const HISTORY_LEN: usize = 60;

/// Default interval between two aria2 polls.
pub const POLL_INTERVAL: Duration = Duration::from_secs(1);

/// The filter tabs shown above the download list.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum Filter {
    #[default]
    Active,
    Waiting,
    Done,
    Errors,
}

impl Filter {
    pub const ALL: [Filter; 4] = [Self::Active, Self::Waiting, Self::Done, Self::Errors];

    pub fn label(self) -> &'static str {
        match self {
            Self::Active => "Active",
            Self::Waiting => "Waiting",
            Self::Done => "Done",
            Self::Errors => "Errors",
        }
    }

    /// Whether `d` belongs under this tab.
    pub fn matches(self, d: &Download) -> bool {
        match self {
            Self::Active => d.status == DownloadStatus::Active,
            Self::Waiting => matches!(d.status, DownloadStatus::Waiting | DownloadStatus::Paused),
            Self::Done => matches!(d.status, DownloadStatus::Complete | DownloadStatus::Removed),
            Self::Errors => matches!(d.status, DownloadStatus::Error | DownloadStatus::Unknown),
        }
    }

    pub fn index(self) -> usize {
        Self::ALL.iter().position(|f| *f == self).unwrap_or(0)
    }

    pub fn next(self) -> Self {
        Self::ALL[(self.index() + 1) % Self::ALL.len()]
    }

    pub fn prev(self) -> Self {
        Self::ALL[(self.index() + Self::ALL.len() - 1) % Self::ALL.len()]
    }
}

/// Number of downloads under each tab.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counts {
    pub active: usize,
    pub waiting: usize,
    pub done: usize,
    pub errors: usize,
}

impl Counts {
    pub fn get(&self, filter: Filter) -> usize {
        match filter {
            Filter::Active => self.active,
            Filter::Waiting => self.waiting,
            Filter::Done => self.done,
            Filter::Errors => self.errors,
        }
    }

    pub fn total(&self) -> usize {
        self.active + self.waiting + self.done + self.errors
    }
}

/// Rolling window of global download/upload speeds, oldest first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpeedHistory {
    samples: VecDeque<(u64, u64)>,
    cap: usize,
}

impl Default for SpeedHistory {
    fn default() -> Self {
        Self::new(HISTORY_LEN)
    }
}

impl SpeedHistory {
    pub fn new(cap: usize) -> Self {
        Self {
            samples: VecDeque::with_capacity(cap),
            cap: cap.max(1),
        }
    }

    pub fn push(&mut self, download: u64, upload: u64) {
        if self.samples.len() == self.cap {
            self.samples.pop_front();
        }
        self.samples.push_back((download, upload));
    }

    pub fn len(&self) -> usize {
        self.samples.len()
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    pub fn capacity(&self) -> usize {
        self.cap
    }

    pub fn iter(&self) -> impl ExactSizeIterator<Item = (u64, u64)> + '_ {
        self.samples.iter().copied()
    }

    /// Highest download or upload speed in the window.
    pub fn peak(&self) -> u64 {
        self.samples
            .iter()
            .map(|&(d, u)| d.max(u))
            .max()
            .unwrap_or(0)
    }
}

/// The latest aria2 state plus derived data, updated from [`PollEvent`]s.
#[derive(Debug, Clone, Default)]
pub struct AppState {
    pub snapshot: Snapshot,
    pub history: SpeedHistory,
    /// Last poll error, cleared by the next successful poll.
    pub error: Option<String>,
    /// Whether at least one poll has succeeded.
    pub loaded: bool,
}

impl AppState {
    /// Apply a fresh snapshot and record the global speeds.
    pub fn apply(&mut self, snapshot: Snapshot) {
        self.history
            .push(snapshot.stat.download_speed, snapshot.stat.upload_speed);
        self.snapshot = snapshot;
        self.error = None;
        self.loaded = true;
    }

    /// Apply an out-of-schedule snapshot without adding a speed sample.
    pub fn apply_refresh(&mut self, snapshot: Snapshot) {
        self.snapshot = snapshot;
        self.error = None;
        self.loaded = true;
    }

    /// Record a failed poll. The graph keeps moving (with zero speed) so it stays in sync with
    /// wall-clock time.
    pub fn apply_error(&mut self, error: String) {
        self.history.push(0, 0);
        self.error = Some(error);
    }

    pub fn handle(&mut self, event: PollEvent) {
        match event {
            PollEvent::Snapshot(s) => self.apply(s),
            PollEvent::Refresh(s) => self.apply_refresh(s),
            PollEvent::Error(e) => self.apply_error(e),
        }
    }

    pub fn stat(&self) -> &GlobalStat {
        &self.snapshot.stat
    }

    /// Downloads under `filter`, in display order.
    pub fn downloads(&self, filter: Filter) -> Vec<&Download> {
        self.snapshot.iter().filter(|d| filter.matches(d)).collect()
    }

    pub fn counts(&self) -> Counts {
        let mut c = Counts::default();
        for d in self.snapshot.iter() {
            match Filter::ALL.into_iter().find(|f| f.matches(d)) {
                Some(Filter::Active) => c.active += 1,
                Some(Filter::Waiting) => c.waiting += 1,
                Some(Filter::Done) => c.done += 1,
                Some(Filter::Errors) | None => c.errors += 1,
            }
        }
        c
    }

    pub fn find(&self, gid: &str) -> Option<&Download> {
        self.snapshot.find(gid)
    }
}

/// Result of one poll.
#[derive(Debug, Clone)]
pub enum PollEvent {
    /// A scheduled poll: one per interval, recorded in the speed history.
    Snapshot(Snapshot),
    /// An extra poll requested with [`PollerHandle::refresh`] (e.g. right after pausing a
    /// download). Updates the list but not the speed history, which stays one sample per tick.
    Refresh(Snapshot),
    Error(String),
}

/// Controls a poller started with [`spawn_poller`]. Dropping it stops the poller.
#[derive(Debug)]
pub struct PollerHandle {
    refresh: Arc<Notify>,
    task: JoinHandle<()>,
}

impl PollerHandle {
    /// Poll again now instead of waiting for the next tick. Requests made while a poll is in
    /// flight are coalesced into one.
    pub fn refresh(&self) {
        self.refresh.notify_one();
    }

    /// A cheap handle for requesting refreshes from other tasks.
    pub fn refresher(&self) -> Arc<Notify> {
        Arc::clone(&self.refresh)
    }
}

impl Drop for PollerHandle {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Poll aria2 every `interval` on a background task, sending results to the returned channel.
/// The first poll happens immediately. The task ends when the receiver or the handle is dropped.
pub fn spawn_poller(
    client: Client,
    interval: Duration,
) -> (mpsc::Receiver<PollEvent>, PollerHandle) {
    let (tx, rx) = mpsc::channel(4);
    let refresh = Arc::new(Notify::new());
    let notified = Arc::clone(&refresh);
    let task = tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            let scheduled = tokio::select! {
                _ = ticker.tick() => true,
                () = notified.notified() => false,
            };
            let event = match client.snapshot().await {
                Ok(s) if scheduled => PollEvent::Snapshot(s),
                Ok(s) => PollEvent::Refresh(s),
                Err(e) if scheduled => PollEvent::Error(e.to_string()),
                // The next scheduled poll reports the failure; don't skew the graph twice.
                Err(_) => continue,
            };
            if tx.send(event).await.is_err() {
                break;
            }
        }
    });
    (rx, PollerHandle { refresh, task })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dl(gid: &str, status: DownloadStatus) -> Download {
        Download {
            gid: gid.into(),
            status,
            ..Default::default()
        }
    }

    fn snapshot() -> Snapshot {
        Snapshot {
            stat: GlobalStat {
                download_speed: 1000,
                upload_speed: 10,
                ..Default::default()
            },
            active: vec![dl("a", DownloadStatus::Active)],
            waiting: vec![
                dl("w", DownloadStatus::Waiting),
                dl("p", DownloadStatus::Paused),
            ],
            stopped: vec![
                dl("c", DownloadStatus::Complete),
                dl("e", DownloadStatus::Error),
                dl("r", DownloadStatus::Removed),
            ],
        }
    }

    #[test]
    fn filters_partition_downloads() {
        let mut state = AppState::default();
        state.apply(snapshot());
        let gids = |f| {
            state
                .downloads(f)
                .iter()
                .map(|d| d.gid.as_str())
                .collect::<Vec<_>>()
        };
        assert_eq!(gids(Filter::Active), ["a"]);
        assert_eq!(gids(Filter::Waiting), ["w", "p"]);
        assert_eq!(gids(Filter::Done), ["c", "r"]);
        assert_eq!(gids(Filter::Errors), ["e"]);
        let c = state.counts();
        assert_eq!(
            c,
            Counts {
                active: 1,
                waiting: 2,
                done: 2,
                errors: 1
            }
        );
        assert_eq!(c.total(), 6);
        assert_eq!(c.get(Filter::Waiting), 2);
    }

    #[test]
    fn filter_cycles() {
        assert_eq!(Filter::Active.next(), Filter::Waiting);
        assert_eq!(Filter::Errors.next(), Filter::Active);
        assert_eq!(Filter::Active.prev(), Filter::Errors);
        assert_eq!(Filter::Done.index(), 2);
    }

    #[test]
    fn history_is_bounded() {
        let mut h = SpeedHistory::new(3);
        assert!(h.is_empty());
        for i in 1..=5 {
            h.push(i * 10, i);
        }
        assert_eq!(h.len(), 3);
        assert_eq!(h.iter().collect::<Vec<_>>(), [(30, 3), (40, 4), (50, 5)]);
        assert_eq!(h.peak(), 50);
        assert_eq!(SpeedHistory::default().capacity(), HISTORY_LEN);
    }

    #[test]
    fn errors_keep_last_snapshot_and_advance_graph() {
        let mut state = AppState::default();
        state.handle(PollEvent::Snapshot(snapshot()));
        assert!(state.loaded);
        state.handle(PollEvent::Error("boom".into()));
        assert_eq!(state.error.as_deref(), Some("boom"));
        assert_eq!(state.snapshot.active.len(), 1, "last snapshot is kept");
        assert_eq!(
            state.history.iter().collect::<Vec<_>>(),
            [(1000, 10), (0, 0)]
        );
        state.handle(PollEvent::Snapshot(snapshot()));
        assert!(state.error.is_none());
    }

    #[test]
    fn refresh_updates_list_but_not_history() {
        let mut state = AppState::default();
        state.handle(PollEvent::Snapshot(snapshot()));
        state.handle(PollEvent::Refresh(Snapshot::default()));
        assert_eq!(state.history.len(), 1);
        assert_eq!(state.counts().total(), 0);
        assert!(state.loaded);
    }
}
