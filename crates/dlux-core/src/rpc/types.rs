//! Typed representations of aria2 RPC results.
//!
//! Field names follow the aria2 manual (camelCase on the wire). All numeric and boolean fields go
//! through the lenient helpers in [`super::de`] because aria2 sends them as strings.

use super::de::{bool_lenient, opt_u32_lenient, u32_lenient, u64_lenient};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::time::Duration;

/// aria2 download identifier (16 hex characters).
pub type Gid = String;

/// aria2 option map (`{"dir": "/tmp", "max-download-limit": "1M"}`). All values are strings.
pub type Options = BTreeMap<String, String>;

/// Lifecycle state of a download as reported by aria2.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DownloadStatus {
    Active,
    #[default]
    Waiting,
    Paused,
    Error,
    Complete,
    Removed,
    /// A status this version of dlux does not know about.
    #[serde(other)]
    Unknown,
}

impl DownloadStatus {
    /// `true` for downloads in aria2's "stopped" list (finished, failed or removed).
    pub fn is_stopped(self) -> bool {
        matches!(self, Self::Complete | Self::Error | Self::Removed)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Waiting => "waiting",
            Self::Paused => "paused",
            Self::Error => "error",
            Self::Complete => "complete",
            Self::Removed => "removed",
            Self::Unknown => "unknown",
        }
    }
}

impl std::fmt::Display for DownloadStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One URI of a file, as returned by `aria2.getUris` / inside `files[].uris`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Uri {
    pub uri: String,
    /// `"used"` or `"waiting"`.
    #[serde(default)]
    pub status: String,
}

/// One file of a download (`aria2.getFiles`, `files[]` in `tellStatus`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct File {
    #[serde(default, deserialize_with = "u32_lenient")]
    pub index: u32,
    #[serde(default)]
    pub path: String,
    #[serde(default, deserialize_with = "u64_lenient")]
    pub length: u64,
    #[serde(default, deserialize_with = "u64_lenient")]
    pub completed_length: u64,
    #[serde(default, deserialize_with = "bool_lenient")]
    pub selected: bool,
    #[serde(default)]
    pub uris: Vec<Uri>,
}

impl File {
    /// The last path component, falling back to the full path.
    pub fn file_name(&self) -> &str {
        file_name_of(&self.path)
    }
}

/// `info` dictionary of a BitTorrent download.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BitTorrentInfo {
    #[serde(default)]
    pub name: String,
}

/// BitTorrent-specific metadata (`bittorrent` key in `tellStatus`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BitTorrent {
    #[serde(default)]
    pub announce_list: Vec<Vec<String>>,
    #[serde(default)]
    pub comment: Option<String>,
    #[serde(default, deserialize_with = "u64_lenient")]
    pub creation_date: u64,
    /// `"single"` or `"multi"`.
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub info: Option<BitTorrentInfo>,
}

/// A download, as returned by `aria2.tellStatus`, `tellActive`, `tellWaiting` and `tellStopped`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Download {
    pub gid: Gid,
    #[serde(default)]
    pub status: DownloadStatus,
    #[serde(default, deserialize_with = "u64_lenient")]
    pub total_length: u64,
    #[serde(default, deserialize_with = "u64_lenient")]
    pub completed_length: u64,
    #[serde(default, deserialize_with = "u64_lenient")]
    pub upload_length: u64,
    /// Bytes per second.
    #[serde(default, deserialize_with = "u64_lenient")]
    pub download_speed: u64,
    /// Bytes per second.
    #[serde(default, deserialize_with = "u64_lenient")]
    pub upload_speed: u64,
    #[serde(default, deserialize_with = "u32_lenient")]
    pub connections: u32,
    #[serde(default, deserialize_with = "u32_lenient")]
    pub num_seeders: u32,
    #[serde(default, deserialize_with = "bool_lenient")]
    pub seeder: bool,
    #[serde(default, deserialize_with = "u64_lenient")]
    pub piece_length: u64,
    #[serde(default, deserialize_with = "u64_lenient")]
    pub num_pieces: u64,
    #[serde(default)]
    pub dir: String,
    #[serde(default)]
    pub files: Vec<File>,
    #[serde(default)]
    pub bittorrent: Option<BitTorrent>,
    #[serde(default)]
    pub info_hash: Option<String>,
    #[serde(default, deserialize_with = "opt_u32_lenient")]
    pub error_code: Option<u32>,
    #[serde(default)]
    pub error_message: Option<String>,
    /// GIDs of downloads started as a result of this one (e.g. torrent after a magnet's metadata).
    #[serde(default)]
    pub followed_by: Vec<Gid>,
    /// Reverse of `followed_by`.
    #[serde(default)]
    pub following: Option<Gid>,
    /// GID of the parent download, for downloads that belong to a metalink/torrent group.
    #[serde(default)]
    pub belongs_to: Option<Gid>,
}

impl Download {
    /// A human-friendly name: torrent name, first file name, or first URI, falling back to the GID.
    pub fn name(&self) -> String {
        if let Some(name) = self
            .bittorrent
            .as_ref()
            .and_then(|bt| bt.info.as_ref())
            .map(|info| info.name.as_str())
            .filter(|n| !n.is_empty())
        {
            return name.to_owned();
        }
        if let Some(file) = self.files.first() {
            if !file.path.is_empty() {
                // Magnet links report a pseudo path while fetching metadata.
                if let Some(hash) = file.path.strip_prefix("[METADATA]") {
                    return format!("[metadata] {hash}");
                }
                return file.file_name().to_owned();
            }
            if let Some(uri) = file.uris.first() {
                let name = uri_file_name(&uri.uri);
                if !name.is_empty() {
                    return name.to_owned();
                }
            }
        }
        self.gid.clone()
    }

    /// `true` while a magnet link is still fetching torrent metadata.
    pub fn is_metadata(&self) -> bool {
        self.files
            .first()
            .is_some_and(|f| f.path.starts_with("[METADATA]"))
    }

    pub fn is_torrent(&self) -> bool {
        self.bittorrent.is_some() || self.info_hash.is_some()
    }

    /// Completion ratio in `0.0..=1.0`. Unknown sizes report `0.0` (or `1.0` once complete).
    pub fn progress(&self) -> f64 {
        if self.status == DownloadStatus::Complete {
            return 1.0;
        }
        if self.total_length == 0 {
            return 0.0;
        }
        (self.completed_length as f64 / self.total_length as f64).clamp(0.0, 1.0)
    }

    /// Estimated time remaining at the current speed, if it can be computed.
    pub fn eta(&self) -> Option<Duration> {
        if self.download_speed == 0 || self.total_length == 0 {
            return None;
        }
        let remaining = self.total_length.saturating_sub(self.completed_length);
        Some(Duration::from_secs(remaining.div_ceil(self.download_speed)))
    }

    /// The first URI of the first file (for "copy URL").
    pub fn first_uri(&self) -> Option<&str> {
        self.files
            .iter()
            .flat_map(|f| f.uris.iter())
            .map(|u| u.uri.as_str())
            .next()
    }
}

/// `aria2.getGlobalStat` result.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GlobalStat {
    #[serde(default, deserialize_with = "u64_lenient")]
    pub download_speed: u64,
    #[serde(default, deserialize_with = "u64_lenient")]
    pub upload_speed: u64,
    #[serde(default, deserialize_with = "u32_lenient")]
    pub num_active: u32,
    #[serde(default, deserialize_with = "u32_lenient")]
    pub num_waiting: u32,
    #[serde(default, deserialize_with = "u32_lenient")]
    pub num_stopped: u32,
    #[serde(default, deserialize_with = "u32_lenient")]
    pub num_stopped_total: u32,
}

/// `aria2.getVersion` result.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Version {
    pub version: String,
    #[serde(default)]
    pub enabled_features: Vec<String>,
}

/// `how` argument of `aria2.changePosition`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PositionFrom {
    /// Relative to the beginning of the queue.
    Set,
    /// Relative to the current position.
    Cur,
    /// Relative to the end of the queue.
    End,
}

impl PositionFrom {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Set => "POS_SET",
            Self::Cur => "POS_CUR",
            Self::End => "POS_END",
        }
    }
}

/// Everything a front end needs to render one frame, fetched in a single `system.multicall`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Snapshot {
    pub stat: GlobalStat,
    pub active: Vec<Download>,
    pub waiting: Vec<Download>,
    pub stopped: Vec<Download>,
}

impl Snapshot {
    /// All downloads in display order: active, then waiting/paused, then stopped.
    pub fn iter(&self) -> impl Iterator<Item = &Download> {
        self.active
            .iter()
            .chain(self.waiting.iter())
            .chain(self.stopped.iter())
    }

    pub fn find(&self, gid: &str) -> Option<&Download> {
        self.iter().find(|d| d.gid == gid)
    }
}

fn file_name_of(path: &str) -> &str {
    path.rsplit(['/', '\\'])
        .find(|s| !s.is_empty())
        .unwrap_or(path)
}

/// File name portion of a URI, without query string or fragment.
fn uri_file_name(uri: &str) -> &str {
    let without_query = uri.split(['?', '#']).next().unwrap_or(uri);
    let after_scheme = without_query
        .split_once("://")
        .map_or(without_query, |(_, rest)| rest);
    match after_scheme.split_once('/') {
        Some((_, path)) => file_name_of(path),
        None => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TELL_STATUS_HTTP: &str = r#"{
        "bitfield": "ffff",
        "completedLength": "524288",
        "connections": "4",
        "dir": "/home/user/Downloads",
        "downloadSpeed": "131072",
        "errorCode": "0",
        "errorMessage": "",
        "files": [{
            "completedLength": "524288",
            "index": "1",
            "length": "1048576",
            "path": "/home/user/Downloads/ubuntu.iso",
            "selected": "true",
            "uris": [
                {"status": "used", "uri": "https://example.com/ubuntu.iso"},
                {"status": "waiting", "uri": "https://mirror.example.com/ubuntu.iso"}
            ]
        }],
        "gid": "2089b05ecca3d829",
        "numPieces": "1",
        "pieceLength": "1048576",
        "status": "active",
        "totalLength": "1048576",
        "uploadLength": "0",
        "uploadSpeed": "0"
    }"#;

    const TELL_STATUS_TORRENT: &str = r#"{
        "bittorrent": {
            "announceList": [["udp://tracker.example.org:1337/announce"]],
            "comment": "Debian CD",
            "creationDate": "1700000000",
            "info": {"name": "debian-12.iso"},
            "mode": "single"
        },
        "completedLength": "0",
        "connections": "12",
        "dir": "/tmp",
        "downloadSpeed": "0",
        "files": [],
        "gid": "aaaabbbbccccdddd",
        "infoHash": "0123456789abcdef0123456789abcdef01234567",
        "numSeeders": "9",
        "seeder": "false",
        "status": "paused",
        "totalLength": "0",
        "uploadLength": "0",
        "uploadSpeed": "1024"
    }"#;

    #[test]
    fn parses_http_download() {
        let d: Download = serde_json::from_str(TELL_STATUS_HTTP).unwrap();
        assert_eq!(d.gid, "2089b05ecca3d829");
        assert_eq!(d.status, DownloadStatus::Active);
        assert_eq!(d.total_length, 1_048_576);
        assert_eq!(d.completed_length, 524_288);
        assert_eq!(d.download_speed, 131_072);
        assert_eq!(d.connections, 4);
        assert_eq!(d.error_code, Some(0));
        assert_eq!(d.files.len(), 1);
        assert!(d.files[0].selected);
        assert_eq!(d.files[0].uris[1].status, "waiting");
        assert_eq!(d.name(), "ubuntu.iso");
        assert_eq!(d.progress(), 0.5);
        assert_eq!(d.eta(), Some(Duration::from_secs(4)));
        assert_eq!(d.first_uri(), Some("https://example.com/ubuntu.iso"));
        assert!(!d.is_torrent());
    }

    #[test]
    fn parses_torrent_download() {
        let d: Download = serde_json::from_str(TELL_STATUS_TORRENT).unwrap();
        assert_eq!(d.status, DownloadStatus::Paused);
        assert_eq!(d.num_seeders, 9);
        assert!(!d.seeder);
        assert!(d.is_torrent());
        assert_eq!(d.name(), "debian-12.iso");
        assert_eq!(d.progress(), 0.0);
        assert_eq!(d.eta(), None);
        let bt = d.bittorrent.unwrap();
        assert_eq!(bt.creation_date, 1_700_000_000);
        assert_eq!(
            bt.announce_list[0][0],
            "udp://tracker.example.org:1337/announce"
        );
    }

    #[test]
    fn name_falls_back_to_uri_then_gid() {
        let d: Download = serde_json::from_str(
            r#"{"gid":"g1","files":[{"path":"","uris":[{"uri":"http://h/a/b/file.zip?x=1","status":"used"}]}]}"#,
        )
        .unwrap();
        assert_eq!(d.name(), "file.zip");

        let d: Download = serde_json::from_str(r#"{"gid":"g2","files":[]}"#).unwrap();
        assert_eq!(d.name(), "g2");

        let d: Download =
            serde_json::from_str(r#"{"gid":"g3","files":[{"path":"[METADATA]abcdef","uris":[]}]}"#)
                .unwrap();
        assert!(d.is_metadata());
        assert_eq!(d.name(), "[metadata] abcdef");
    }

    #[test]
    fn unknown_status_does_not_fail() {
        let d: Download = serde_json::from_str(r#"{"gid":"g","status":"hibernating"}"#).unwrap();
        assert_eq!(d.status, DownloadStatus::Unknown);
    }

    #[test]
    fn complete_progress_is_full_even_without_length() {
        let d: Download = serde_json::from_str(r#"{"gid":"g","status":"complete"}"#).unwrap();
        assert_eq!(d.progress(), 1.0);
        assert!(d.status.is_stopped());
    }

    #[test]
    fn parses_global_stat_and_version() {
        let s: GlobalStat = serde_json::from_str(
            r#"{"downloadSpeed":"21846","numActive":"2","numStopped":"1","numStoppedTotal":"5","numWaiting":"3","uploadSpeed":"0"}"#,
        )
        .unwrap();
        assert_eq!(s.download_speed, 21_846);
        assert_eq!(s.num_active, 2);
        assert_eq!(s.num_waiting, 3);
        assert_eq!(s.num_stopped_total, 5);

        let v: Version = serde_json::from_str(
            r#"{"enabledFeatures":["BitTorrent","Metalink"],"version":"1.37.0"}"#,
        )
        .unwrap();
        assert_eq!(v.version, "1.37.0");
        assert_eq!(v.enabled_features, ["BitTorrent", "Metalink"]);
    }

    #[test]
    fn uri_file_name_edge_cases() {
        assert_eq!(uri_file_name("https://example.com/"), "");
        assert_eq!(uri_file_name("https://example.com"), "");
        assert_eq!(uri_file_name("https://example.com/dir/"), "dir");
        assert_eq!(uri_file_name("ftp://h/x.tar.gz#frag"), "x.tar.gz");
    }
}
