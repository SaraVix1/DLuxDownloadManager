//! yt-dlp integration: probing a page for its formats (`yt-dlp -J`), turning them into a short
//! list of choices ("1080p", "Audio only", …), building the download command (with aria2c as the
//! downloader) and parsing its progress output.

use serde::Deserialize;
use std::ffi::OsString;
use std::path::PathBuf;
use url::Url;

/// Marker that starts our `--progress-template` lines.
pub const PROGRESS_MARK: &str = "DLUX|";

/// File extensions of direct downloads: URLs ending in these go straight to aria2.
const FILE_EXTENSIONS: &[&str] = &[
    "7z", "aac", "apk", "appimage", "avi", "bin", "bz2", "csv", "deb", "dmg", "doc", "docx",
    "epub", "exe", "flac", "flv", "gz", "img", "iso", "jar", "jpeg", "jpg", "json", "m4a", "m4v",
    "meta4", "metalink", "mkv", "mov", "mp3", "mp4", "msi", "ogg", "opus", "pdf", "pkg", "png",
    "rar", "rpm", "svg", "tar", "tgz", "torrent", "txt", "wav", "webm", "webp", "xls", "xlsx",
    "xz", "zip", "zst",
];

/// Sites that are always video pages.
const VIDEO_HOSTS: &[&str] = &[
    "youtube.com",
    "youtu.be",
    "vimeo.com",
    "dailymotion.com",
    "twitch.tv",
    "tiktok.com",
    "soundcloud.com",
    "bandcamp.com",
    "bilibili.com",
    "rumble.com",
    "odysee.com",
    "x.com",
    "twitter.com",
    "instagram.com",
    "facebook.com",
    "reddit.com",
    "streamable.com",
    "nicovideo.jp",
];

/// Whether `url` is probably a page yt-dlp should handle rather than a file for aria2: known
/// video sites, and http(s) URLs whose path does not end in a file extension.
pub fn looks_like_video(url: &str) -> bool {
    let Ok(u) = Url::parse(url) else {
        return false;
    };
    if !matches!(u.scheme(), "http" | "https") {
        return false;
    }
    let host = u.host_str().unwrap_or_default().to_ascii_lowercase();
    if VIDEO_HOSTS
        .iter()
        .any(|h| host == *h || host.ends_with(&format!(".{h}")))
    {
        return true;
    }
    let last = u
        .path_segments()
        .and_then(|mut s| s.next_back())
        .unwrap_or("");
    match last.rsplit_once('.') {
        Some((_, ext)) => !FILE_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str()),
        None => true,
    }
}

// ----- probing --------------------------------------------------------------------------------

/// The parts of `yt-dlp -J` output dlux uses.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct RawInfo {
    #[serde(rename = "_type")]
    pub kind: Option<String>,
    pub title: Option<String>,
    pub uploader: Option<String>,
    pub channel: Option<String>,
    pub duration: Option<f64>,
    pub extractor_key: Option<String>,
    pub webpage_url: Option<String>,
    #[serde(default)]
    pub formats: Vec<RawFormat>,
    /// Playlists (with `--flat-playlist`).
    #[serde(default)]
    pub entries: Vec<serde_json::Value>,
    pub playlist_count: Option<u64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct RawFormat {
    pub format_id: String,
    pub ext: Option<String>,
    pub height: Option<u32>,
    pub fps: Option<f64>,
    pub vcodec: Option<String>,
    pub acodec: Option<String>,
    pub filesize: Option<u64>,
    pub filesize_approx: Option<u64>,
    /// Total bitrate in kbit/s, used to estimate sizes when yt-dlp gives none.
    pub tbr: Option<f64>,
}

impl RawFormat {
    fn has_video(&self) -> bool {
        self.vcodec.as_deref().is_some_and(|c| c != "none")
            || (self.height.is_some() && self.vcodec.is_none())
    }

    fn has_audio(&self) -> bool {
        self.acodec.as_deref().is_some_and(|c| c != "none")
    }

    fn size(&self, duration: Option<f64>) -> Option<u64> {
        self.filesize.or(self.filesize_approx).or_else(|| {
            let (tbr, secs) = (self.tbr?, duration?);
            Some((tbr * 1000.0 / 8.0 * secs) as u64)
        })
    }
}

/// One choice in the format picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    /// "Best quality", "1080p60", "Audio only".
    pub label: String,
    /// Extra detail: "mp4 · ~245 MiB".
    pub detail: String,
    /// yt-dlp `-f` selector.
    pub selector: String,
    /// Estimated total size, if known.
    pub size: Option<u64>,
    /// Needs ffmpeg to merge separate video and audio streams.
    pub needs_merge: bool,
}

/// A probed page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoInfo {
    pub url: String,
    pub title: String,
    /// "YouTube · Some Channel · 12:34", or "Playlist · 23 videos".
    pub subtitle: String,
    pub playlist: bool,
    pub choices: Vec<Choice>,
}

/// Standard heights offered in the picker.
const HEIGHTS: &[u32] = &[4320, 2160, 1440, 1080, 720, 480, 360, 240, 144];

fn duration_text(secs: f64) -> String {
    let s = secs.round() as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

fn detail(ext: Option<&str>, size: Option<u64>) -> String {
    let mut parts = Vec::new();
    if let Some(ext) = ext {
        parts.push(ext.to_owned());
    }
    if let Some(size) = size {
        parts.push(format!("~{}", crate::fmt::bytes(size)));
    }
    parts.join(" · ")
}

impl VideoInfo {
    /// Build the picker from `yt-dlp -J` output for `url`.
    pub fn from_raw(url: &str, raw: RawInfo) -> Self {
        let playlist = raw.kind.as_deref() == Some("playlist");
        let title = raw
            .title
            .clone()
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| url.to_owned());
        let subtitle = if playlist {
            let n = raw.playlist_count.unwrap_or(raw.entries.len() as u64);
            format!("Playlist · {n} {}", if n == 1 { "video" } else { "videos" })
        } else {
            [
                raw.extractor_key.clone(),
                raw.channel.clone().or(raw.uploader.clone()),
                raw.duration.map(duration_text),
            ]
            .into_iter()
            .flatten()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" · ")
        };
        let choices = if playlist {
            playlist_choices()
        } else {
            video_choices(&raw)
        };
        Self {
            url: url.to_owned(),
            title,
            subtitle,
            playlist,
            choices,
        }
    }
}

fn height_selector(h: u32) -> String {
    format!("bv*[height<={h}]+ba/b[height<={h}]")
}

fn playlist_choices() -> Vec<Choice> {
    let mut v = vec![Choice {
        label: "Best quality".into(),
        detail: "every video".into(),
        selector: "bv*+ba/b".into(),
        size: None,
        needs_merge: true,
    }];
    for h in [1080, 720, 480] {
        v.push(Choice {
            label: format!("{h}p"),
            detail: "or the best below".into(),
            selector: height_selector(h),
            size: None,
            needs_merge: true,
        });
    }
    v.push(Choice {
        label: "Audio only".into(),
        detail: "best audio".into(),
        selector: "ba/b".into(),
        size: None,
        needs_merge: false,
    });
    v
}

fn video_choices(raw: &RawInfo) -> Vec<Choice> {
    let dur = raw.duration;
    let videos: Vec<&RawFormat> = raw.formats.iter().filter(|f| f.has_video()).collect();
    let best_audio = raw
        .formats
        .iter()
        .filter(|f| f.has_audio() && !f.has_video())
        .max_by_key(|f| f.size(dur).unwrap_or(0));
    // Best video stream at or below `h`: highest, then largest.
    let best_at = |h: u32| {
        videos
            .iter()
            .filter(|f| f.height.is_some_and(|fh| fh <= h))
            .max_by_key(|f| (f.height.unwrap_or(0), f.size(dur).unwrap_or(0)))
            .copied()
    };
    let total = |v: &RawFormat| -> Option<u64> {
        let audio = if v.has_audio() {
            Some(0)
        } else {
            best_audio.map(|a| a.size(dur).unwrap_or(0))
        };
        Some(v.size(dur)? + audio.unwrap_or(0))
    };
    let label = |v: &RawFormat| {
        let h = v.height.unwrap_or(0);
        match v.fps {
            Some(fps) if fps > 30.5 => format!("{h}p{}", fps.round() as u32),
            _ => format!("{h}p"),
        }
    };

    let mut choices = Vec::new();
    if let Some(top) = best_at(u32::MAX) {
        choices.push(Choice {
            label: format!("Best quality ({})", label(top)),
            detail: detail(top.ext.as_deref(), total(top)),
            selector: "bv*+ba/b".into(),
            size: total(top),
            needs_merge: !top.has_audio(),
        });
        let top_h = top.height.unwrap_or(0);
        for &h in HEIGHTS.iter().filter(|&&h| h < top_h) {
            let Some(v) = best_at(h) else { continue };
            // Only offer heights that really exist (e.g. no "1440p" made of a 1080p stream).
            if v.height != Some(h) {
                continue;
            }
            choices.push(Choice {
                label: label(v),
                detail: detail(v.ext.as_deref(), total(v)),
                selector: height_selector(h),
                size: total(v),
                needs_merge: !v.has_audio(),
            });
        }
    } else {
        choices.push(Choice {
            label: "Best quality".into(),
            detail: String::new(),
            selector: "bv*+ba/b".into(),
            size: None,
            needs_merge: false,
        });
    }
    if let Some(a) = best_audio {
        let size = a.size(dur);
        choices.push(Choice {
            label: "Audio only".into(),
            detail: detail(a.ext.as_deref(), size),
            selector: "ba/b".into(),
            size,
            needs_merge: false,
        });
    }
    choices
}

/// Arguments for probing `url`.
pub fn probe_args(url: &str) -> Vec<String> {
    [
        "-J",
        "--no-playlist",
        "--flat-playlist",
        "--no-warnings",
        "--no-colors",
        "--",
        url,
    ]
    .map(String::from)
    .to_vec()
}

/// Parse `yt-dlp -J` stdout.
pub fn parse_probe(url: &str, stdout: &[u8]) -> Result<VideoInfo, String> {
    let raw: RawInfo = serde_json::from_slice(stdout)
        .map_err(|e| format!("could not read yt-dlp's output ({e})"))?;
    Ok(VideoInfo::from_raw(url, raw))
}

// ----- downloading ----------------------------------------------------------------------------

/// Everything needed to (re)start a yt-dlp download.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadSpec {
    pub url: String,
    pub title: String,
    pub selector: String,
    pub dir: PathBuf,
    pub playlist: bool,
    /// Estimated total size, for the progress bar before yt-dlp knows.
    pub size: Option<u64>,
    /// The selector merges separate video and audio streams (needs ffmpeg).
    pub needs_merge: bool,
}

/// A selector that never needs merging: keeps only the single-file alternatives of `selector`
/// (`bv*[height<=720]+ba/b[height<=720]` → `b[height<=720]/b`). Used when ffmpeg is missing.
pub fn progressive(selector: &str) -> String {
    let mut parts: Vec<&str> = selector.split('/').filter(|p| !p.contains('+')).collect();
    if !parts.contains(&"b") {
        parts.push("b");
    }
    parts.join("/")
}

/// Paths of the helper programs passed to yt-dlp.
#[derive(Debug, Clone, Default)]
pub struct Tools {
    /// aria2c to use as the downloader; yt-dlp's own downloader when `None`.
    pub aria2c: Option<PathBuf>,
    pub ffmpeg: Option<PathBuf>,
}

/// Arguments for downloading `spec`.
pub fn download_args(spec: &DownloadSpec, tools: &Tools) -> Vec<OsString> {
    let mut a: Vec<OsString> = Vec::new();
    let mut push = |s: &str| a.push(s.into());
    push("--newline");
    push("--no-colors");
    push("--progress-template");
    push(&format!(
        "download:{PROGRESS_MARK}%(progress.status)s|%(progress.downloaded_bytes)s|\
         %(progress.total_bytes)s|%(progress.total_bytes_estimate)s|%(progress.speed)s|\
         %(progress.eta)s"
    ));
    push("-f");
    push(&spec.selector);
    push(if spec.playlist {
        "--yes-playlist"
    } else {
        "--no-playlist"
    });
    push("-o");
    // The quality is part of the name ("… 1080p.mp4", "… audio.webm") so downloading another
    // quality of the same video doesn't collide with — and get skipped because of — the first.
    push(if spec.playlist {
        "%(playlist_title|playlist)s/%(playlist_index)03d - %(title)s [%(id)s] %(height&{}p|audio)s.%(ext)s"
    } else {
        "%(title)s [%(id)s] %(height&{}p|audio)s.%(ext)s"
    });
    a.push("-P".into());
    a.push(spec.dir.clone().into());
    if let Some(aria2c) = &tools.aria2c {
        a.push("--downloader".into());
        a.push(aria2c.clone().into());
        a.push("--downloader-args".into());
        a.push("aria2c:-x 8 -s 8 -k 1M --summary-interval=1".into());
    }
    if let Some(ffmpeg) = &tools.ffmpeg {
        a.push("--ffmpeg-location".into());
        a.push(ffmpeg.clone().into());
    }
    a.push("--".into());
    a.push(spec.url.clone().into());
    a
}

/// A recognised line of yt-dlp download output.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// From our progress template.
    Progress {
        finished: bool,
        downloaded: u64,
        total: Option<u64>,
        speed: Option<u64>,
        eta: Option<u64>,
    },
    /// A new file starts downloading (a stream, or a playlist item's stream).
    Destination(PathBuf),
    /// The file is already complete on disk.
    AlreadyDownloaded(PathBuf),
    /// Playlist position: item `n` of `of`.
    Item {
        n: u64,
        of: u64,
    },
    /// ffmpeg merges the streams into this file.
    Merging(PathBuf),
    /// Another post-processing step ("ExtractAudio", "FixupM3u8", …).
    PostProcess(String),
    Error(String),
}

fn num(s: &str) -> Option<u64> {
    let s = s.trim();
    if s.is_empty() || s == "NA" || s == "None" {
        return None;
    }
    s.parse::<f64>()
        .ok()
        .filter(|v| v.is_finite() && *v >= 0.0)
        .map(|v| v as u64)
}

fn unquote(s: &str) -> &str {
    s.trim().trim_matches('"')
}

/// Parse one line of `yt-dlp` output (stdout or stderr).
pub fn parse_line(line: &str) -> Option<Event> {
    let line = line.trim_end_matches(['\r', '\n']).trim_start();
    if let Some(rest) = line.strip_prefix(PROGRESS_MARK) {
        let f: Vec<&str> = rest.split('|').collect();
        if f.len() < 6 {
            return None;
        }
        return Some(Event::Progress {
            finished: f[0].trim() == "finished",
            downloaded: num(f[1]).unwrap_or(0),
            total: num(f[2]).or_else(|| num(f[3])),
            speed: num(f[4]),
            eta: num(f[5]),
        });
    }
    if let Some(event) = parse_aria2c_readout(line) {
        return Some(event);
    }
    if let Some(msg) = line.strip_prefix("ERROR:") {
        return Some(Event::Error(msg.trim().to_owned()));
    }
    if let Some(rest) = line.strip_prefix("[download] ") {
        if let Some(path) = rest.strip_prefix("Destination: ") {
            return Some(Event::Destination(PathBuf::from(path.trim())));
        }
        if let Some(path) = rest.strip_suffix(" has already been downloaded") {
            return Some(Event::AlreadyDownloaded(PathBuf::from(unquote(path))));
        }
        if let Some(pos) = rest.strip_prefix("Downloading item ") {
            let (n, of) = pos.split_once(" of ")?;
            return Some(Event::Item {
                n: n.trim().parse().ok()?,
                of: of.trim().parse().ok()?,
            });
        }
        return None;
    }
    if let Some(rest) = line.strip_prefix("[Merger] Merging formats into ") {
        return Some(Event::Merging(PathBuf::from(unquote(rest))));
    }
    // "[ExtractAudio] Destination: …", "[FixupM3u8] Fixing …", "[VideoConvertor] …"
    if let Some(rest) = line.strip_prefix('[') {
        let (name, _) = rest.split_once(']')?;
        const STEPS: &[&str] = &[
            "ExtractAudio",
            "FixupM3u8",
            "FixupM4a",
            "FixupStretched",
            "FixupDuplicateMoov",
            "FixupTimestamp",
            "VideoConvertor",
            "VideoRemuxer",
            "EmbedThumbnail",
            "EmbedSubtitle",
            "Metadata",
            "MoveFiles",
        ];
        if STEPS.contains(&name) {
            return Some(Event::PostProcess(name.to_owned()));
        }
    }
    None
}

/// aria2c's size notation: "0B", "32KiB", "1.5MiB", "2GiB".
fn aria2c_size(s: &str) -> Option<u64> {
    let s = s.trim();
    let (num, mult) = [
        ("GiB", 1u64 << 30),
        ("MiB", 1 << 20),
        ("KiB", 1 << 10),
        ("B", 1),
    ]
    .iter()
    .find_map(|(unit, m)| s.strip_suffix(unit).map(|n| (n, *m)))?;
    let v: f64 = num.parse().ok()?;
    Some((v * mult as f64) as u64)
}

/// aria2c's ETA notation: "7s", "1m5s", "2h3m4s".
fn aria2c_eta(s: &str) -> Option<u64> {
    let mut total = 0u64;
    let mut digits = String::new();
    for c in s.trim().chars() {
        match c {
            '0'..='9' => digits.push(c),
            'h' | 'm' | 's' => {
                let n: u64 = digits.parse().ok()?;
                digits.clear();
                total += n * match c {
                    'h' => 3600,
                    'm' => 60,
                    _ => 1,
                };
            }
            _ => return None,
        }
    }
    digits.is_empty().then_some(total)
}

/// aria2c's console readout, which yt-dlp passes through when aria2c is the downloader:
/// `[#bc2021 32KiB/246KiB(12%) CN:1 DL:27KiB ETA:7s]`.
fn parse_aria2c_readout(line: &str) -> Option<Event> {
    let inner = line.strip_prefix("[#")?.strip_suffix(']')?;
    let mut parts = inner.split_whitespace();
    let _gid = parts.next()?;
    let sizes = parts.next()?;
    let sizes = sizes.split_once('(').map_or(sizes, |(s, _)| s);
    let (done, total) = sizes.split_once('/')?;
    let (mut speed, mut eta) = (None, None);
    for p in parts {
        if let Some(v) = p.strip_prefix("DL:") {
            speed = aria2c_size(v);
        } else if let Some(v) = p.strip_prefix("ETA:") {
            eta = aria2c_eta(v);
        }
    }
    let total = aria2c_size(total).filter(|t| *t > 0);
    Some(Event::Progress {
        finished: false,
        downloaded: aria2c_size(done)?,
        total,
        speed,
        eta,
    })
}

/// Turn a yt-dlp error message into something friendlier.
pub fn friendly_error(msg: &str) -> String {
    let lower = msg.to_ascii_lowercase();
    // Drop the "[youtube] dQw4w9WgXcQ: " prefix.
    let core = match msg.strip_prefix('[').and_then(|r| r.split_once("] ")) {
        Some((_, rest)) => rest.split_once(": ").map_or(rest, |(_, m)| m),
        None => msg,
    };
    if lower.contains("unsupported url") {
        "yt-dlp doesn't recognise this page as a video".into()
    } else if lower.contains("private video") {
        "This video is private".into()
    } else if lower.contains("sign in to confirm your age") || lower.contains("age-restricted") {
        "This video is age-restricted and needs a signed-in account".into()
    } else if lower.contains("sign in to confirm") {
        "The site asks to sign in (try updating yt-dlp)".into()
    } else if lower.contains("video unavailable") || lower.contains("this video is not available") {
        "This video is unavailable".into()
    } else if lower.contains("requested format is not available") {
        "That quality is no longer offered; pick another".into()
    } else if lower.contains("ffmpeg") && lower.contains("not installed") {
        "ffmpeg is needed to merge video and audio".into()
    } else if lower.contains("unable to download webpage")
        || lower.contains("name or service not known")
        || lower.contains("temporary failure in name resolution")
        || lower.contains("connection refused")
        || lower.contains("timed out")
    {
        format!("Network error: {core}")
    } else {
        core.to_owned()
    }
}

/// The most informative error line in yt-dlp's stderr, made friendly.
pub fn error_from_output(stderr: &str) -> Option<String> {
    stderr
        .lines()
        .rev()
        .find_map(|l| l.trim().strip_prefix("ERROR:").map(str::trim))
        .map(friendly_error)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn video_url_heuristic() {
        for (url, video) in [
            ("https://www.youtube.com/watch?v=dQw4w9WgXcQ", true),
            ("https://youtu.be/dQw4w9WgXcQ", true),
            ("https://m.youtube.com/shorts/abc", true),
            ("https://vimeo.com/123", true),
            ("https://example.com/videos/my-talk", true),
            ("https://example.com/", true),
            ("https://example.com/watch.php?id=4", true),
            ("https://example.com/file.ISO", false),
            ("https://example.com/a/b/movie.mp4?sig=x", false),
            ("https://example.com/pkg.tar.gz", false),
            ("ftp://example.com/stream", false),
            ("magnet:?xt=urn:btih:abc", false),
            ("not a url", false),
        ] {
            assert_eq!(looks_like_video(url), video, "{url}");
        }
    }

    fn fmt(id: &str, h: Option<u32>, v: &str, a: &str, size: u64, ext: &str) -> RawFormat {
        RawFormat {
            format_id: id.into(),
            ext: Some(ext.into()),
            height: h,
            fps: Some(30.0),
            vcodec: Some(v.into()),
            acodec: Some(a.into()),
            filesize: Some(size),
            ..Default::default()
        }
    }

    #[test]
    fn choices_from_formats() {
        let mb = 1 << 20;
        let raw = RawInfo {
            title: Some("Talk".into()),
            extractor_key: Some("Youtube".into()),
            channel: Some("Conf".into()),
            duration: Some(3725.0),
            formats: vec![
                fmt("140", None, "none", "mp4a", 50 * mb, "m4a"),
                fmt("251", None, "none", "opus", 40 * mb, "webm"),
                fmt("18", Some(360), "avc1", "mp4a", 80 * mb, "mp4"),
                fmt("136", Some(720), "avc1", "none", 200 * mb, "mp4"),
                RawFormat {
                    fps: Some(60.0),
                    ..fmt("299", Some(1080), "avc1", "none", 500 * mb, "mp4")
                },
                fmt("sb0", None, "none", "none", 0, "mhtml"),
            ],
            ..Default::default()
        };
        let info = VideoInfo::from_raw("u", raw);
        assert_eq!(info.title, "Talk");
        assert_eq!(info.subtitle, "Youtube · Conf · 1:02:05");
        let labels: Vec<_> = info.choices.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(
            labels,
            vec!["Best quality (1080p60)", "720p", "360p", "Audio only"]
        );
        let best = &info.choices[0];
        assert_eq!(best.size, Some(550 * mb), "video + best audio");
        assert!(best.needs_merge);
        assert_eq!(best.detail, "mp4 · ~550 MiB");
        assert_eq!(
            info.choices[1].selector,
            "bv*[height<=720]+ba/b[height<=720]"
        );
        assert!(!info.choices[2].needs_merge, "360p has audio built in");
        assert_eq!(info.choices[2].size, Some(80 * mb));
        assert_eq!(info.choices[3].selector, "ba/b");
        assert_eq!(info.choices[3].size, Some(50 * mb));
    }

    #[test]
    fn sizes_estimated_from_bitrate() {
        let raw = RawInfo {
            duration: Some(100.0),
            formats: vec![RawFormat {
                format_id: "hls-1".into(),
                height: Some(720),
                tbr: Some(800.0),
                vcodec: None,
                ..Default::default()
            }],
            ..Default::default()
        };
        let info = VideoInfo::from_raw("https://e.x/v", raw);
        assert_eq!(info.title, "https://e.x/v", "falls back to the URL");
        assert_eq!(info.choices.len(), 1);
        assert_eq!(info.choices[0].size, Some(10_000_000));
    }

    #[test]
    fn playlists_and_bare_pages() {
        let p = parse_probe(
            "u",
            br#"{"_type":"playlist","title":"Mix","entries":[{},{},{}]}"#,
        )
        .unwrap();
        assert!(p.playlist);
        assert_eq!(p.subtitle, "Playlist · 3 videos");
        assert_eq!(p.choices.last().unwrap().label, "Audio only");
        let bare = parse_probe("u", br#"{"title":"Clip"}"#).unwrap();
        assert_eq!(bare.choices.len(), 1);
        assert_eq!(bare.choices[0].label, "Best quality");
        assert!(parse_probe("u", b"<html>").is_err());
    }

    #[test]
    fn parses_progress_output() {
        assert_eq!(
            parse_line("DLUX|downloading|1048576|10485760|NA|524288.5|18\r"),
            Some(Event::Progress {
                finished: false,
                downloaded: 1 << 20,
                total: Some(10 << 20),
                speed: Some(524_288),
                eta: Some(18),
            })
        );
        assert_eq!(
            parse_line("DLUX|downloading|5|NA|900|NA|NA"),
            Some(Event::Progress {
                finished: false,
                downloaded: 5,
                total: Some(900),
                speed: None,
                eta: None,
            }),
            "estimate used when the total is unknown"
        );
        assert!(matches!(
            parse_line("DLUX|finished|10|10|NA|NA|NA"),
            Some(Event::Progress { finished: true, .. })
        ));
        assert_eq!(parse_line("DLUX|bad"), None);
        assert_eq!(
            parse_line("[download] Destination: /dl/Talk [abc].f299.mp4"),
            Some(Event::Destination("/dl/Talk [abc].f299.mp4".into()))
        );
        assert_eq!(
            parse_line("[download] /dl/Talk [abc].mp4 has already been downloaded"),
            Some(Event::AlreadyDownloaded("/dl/Talk [abc].mp4".into()))
        );
        assert_eq!(
            parse_line("[download] Downloading item 3 of 23"),
            Some(Event::Item { n: 3, of: 23 })
        );
        assert_eq!(
            parse_line(r#"[Merger] Merging formats into "/dl/Talk [abc].mp4""#),
            Some(Event::Merging("/dl/Talk [abc].mp4".into()))
        );
        assert_eq!(
            parse_line("[ExtractAudio] Destination: x.mp3"),
            Some(Event::PostProcess("ExtractAudio".into()))
        );
        assert_eq!(
            parse_line("ERROR: [youtube] abc: Private video"),
            Some(Event::Error("[youtube] abc: Private video".into()))
        );
        assert_eq!(parse_line("[youtube] abc: Downloading webpage"), None);
        // aria2c's readout (yt-dlp passes it through when aria2c is the downloader).
        assert_eq!(
            parse_line("[#bc2021 32KiB/246KiB(12%) CN:1 DL:27KiB ETA:7s]"),
            Some(Event::Progress {
                finished: false,
                downloaded: 32 << 10,
                total: Some(246 << 10),
                speed: Some(27 << 10),
                eta: Some(7),
            })
        );
        assert_eq!(
            parse_line("[#2089b0 1.5MiB/2.0GiB(0%) CN:8 DL:3.2MiB ETA:10m38s]"),
            Some(Event::Progress {
                finished: false,
                downloaded: 3 << 19,
                total: Some(2 << 30),
                speed: Some((3.2 * (1u64 << 20) as f64) as u64),
                eta: Some(638),
            })
        );
        assert_eq!(
            parse_line("[#2089b0 0B/0B CN:1 DL:0B]"),
            Some(Event::Progress {
                finished: false,
                downloaded: 0,
                total: None,
                speed: Some(0),
                eta: None,
            }),
            "unknown size"
        );
        assert_eq!(parse_line("[#nope]"), None);
        assert_eq!(aria2c_eta("1h2m3s"), Some(3723));
        assert_eq!(aria2c_eta("3x"), None);
        assert_eq!(parse_line("[download]  12.5% of 10MiB"), None);
    }

    #[test]
    fn friendly_errors() {
        assert_eq!(
            friendly_error("Unsupported URL: https://example.com/"),
            "yt-dlp doesn't recognise this page as a video"
        );
        assert_eq!(
            friendly_error("[youtube] abc: Private video. Sign in"),
            "This video is private"
        );
        assert_eq!(
            friendly_error("[generic] x: Something odd happened"),
            "Something odd happened"
        );
        assert!(
            friendly_error("[youtube] abc: Unable to download webpage: timed out")
                .starts_with("Network error")
        );
        assert_eq!(
            error_from_output("WARNING: x\nERROR: [vimeo] 1: Video unavailable\n"),
            Some("This video is unavailable".into())
        );
        assert_eq!(error_from_output("all good"), None);
    }

    #[test]
    fn download_command() {
        let spec = DownloadSpec {
            url: "https://youtu.be/x".into(),
            title: "T".into(),
            selector: "ba/b".into(),
            dir: "/dl".into(),
            playlist: false,
            size: None,
            needs_merge: false,
        };
        assert_eq!(
            progressive("bv*[height<=720]+ba/b[height<=720]"),
            "b[height<=720]/b"
        );
        assert_eq!(progressive("bv*+ba/b"), "b");
        assert_eq!(progressive("ba/b"), "ba/b");
        let tools = Tools {
            aria2c: Some("/usr/bin/aria2c".into()),
            ffmpeg: Some("/bin/ffmpeg".into()),
        };
        let args: Vec<String> = download_args(&spec, &tools)
            .into_iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        let after = |flag: &str| {
            let i = args.iter().position(|a| a == flag).unwrap();
            args[i + 1].clone()
        };
        assert_eq!(after("-f"), "ba/b");
        assert!(
            after("-o").contains("%(height&{}p|audio)s"),
            "quality in the file name"
        );
        assert_eq!(after("-P"), "/dl");
        assert_eq!(after("--downloader"), "/usr/bin/aria2c");
        assert_eq!(after("--ffmpeg-location"), "/bin/ffmpeg");
        assert!(after("--progress-template").starts_with("download:DLUX|"));
        assert!(args.contains(&"--no-playlist".into()));
        assert_eq!(args.last().unwrap(), "https://youtu.be/x");
        assert_eq!(
            args[args.len() - 2],
            "--",
            "URL can't be mistaken for an option"
        );

        let plain = download_args(&spec, &Tools::default());
        assert!(!plain.iter().any(|a| a == "--downloader"));
        assert_eq!(probe_args("u").last().unwrap(), "u");
    }
}
