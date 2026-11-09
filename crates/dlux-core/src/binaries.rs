//! External tools dlux needs for video downloads: yt-dlp and ffmpeg.
//!
//! [`find`] prefers the user's own installation (PATH) and falls back to dlux's private
//! `~/.local/share/dlux/bin/`. [`install`] downloads the official standalone builds into that
//! directory: yt-dlp from its GitHub releases, ffmpeg from yt-dlp's FFmpeg-Builds. Every
//! download is checked against the SHA-256 checksum file published with the release, and is
//! written to a temporary file that is only moved into place once complete.

use crate::paths;
use sha2::{Digest, Sha256};
use std::fmt;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use tokio::io::AsyncWriteExt;

const YTDLP_RELEASES: &str = "https://github.com/yt-dlp/yt-dlp/releases/latest/download";
const YTDLP_SUMS: &str = "SHA2-256SUMS";
const FFMPEG_RELEASES: &str = "https://github.com/yt-dlp/FFmpeg-Builds/releases/latest/download";
const FFMPEG_SUMS: &str = "checksums.sha256";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tool {
    YtDlp,
    Ffmpeg,
}

impl Tool {
    pub fn name(self) -> &'static str {
        match self {
            Self::YtDlp => "yt-dlp",
            Self::Ffmpeg => "ffmpeg",
        }
    }

    fn exe(name: &str) -> String {
        if cfg!(windows) {
            format!("{name}.exe")
        } else {
            name.to_owned()
        }
    }

    /// Where dlux installs this tool.
    pub fn private_path(self) -> PathBuf {
        paths::bin_dir().join(Self::exe(self.name()))
    }
}

impl fmt::Display for Tool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// Locate `tool`: on PATH first (the user's own, possibly newer or package-managed install),
/// then in dlux's bin directory.
pub fn find(tool: Tool) -> Option<PathBuf> {
    paths::find_in_path(tool.name()).or_else(|| {
        let p = tool.private_path();
        p.is_file().then_some(p)
    })
}

#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    #[error("no prebuilt {tool} for this platform; please install it with your package manager")]
    Unsupported { tool: Tool },
    #[error("download failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("download failed: {url} returned HTTP {status}")]
    Status { url: String, status: u16 },
    #[error("checksum mismatch for {asset} (download corrupted or tampered with)")]
    Checksum { asset: String },
    #[error("no checksum published for {asset}")]
    MissingChecksum { asset: String },
    #[error("{0} not found in the downloaded archive")]
    NotInArchive(String),
    #[error("cannot write {path}: {source}")]
    Io {
        path: String,
        source: std::io::Error,
    },
}

fn io_err(path: &Path) -> impl FnOnce(std::io::Error) -> InstallError + '_ {
    move |source| InstallError::Io {
        path: path.display().to_string(),
        source,
    }
}

/// What to download for `tool` on this platform: (release base URL, asset, checksum file).
fn asset(tool: Tool) -> Option<(&'static str, &'static str, &'static str)> {
    let (os, arch) = (std::env::consts::OS, std::env::consts::ARCH);
    match tool {
        Tool::YtDlp => {
            let name = match (os, arch) {
                ("linux", "x86_64") => "yt-dlp_linux",
                ("linux", "aarch64") => "yt-dlp_linux_aarch64",
                ("macos", _) => "yt-dlp_macos",
                ("windows", "x86_64") => "yt-dlp.exe",
                ("windows", "x86") => "yt-dlp_x86.exe",
                _ => return None,
            };
            Some((YTDLP_RELEASES, name, YTDLP_SUMS))
        }
        Tool::Ffmpeg => {
            let name = match (os, arch) {
                ("linux", "x86_64") => "ffmpeg-master-latest-linux64-gpl.tar.xz",
                ("linux", "aarch64") => "ffmpeg-master-latest-linuxarm64-gpl.tar.xz",
                ("windows", "x86_64") => "ffmpeg-master-latest-win64-gpl.zip",
                _ => return None,
            };
            Some((FFMPEG_RELEASES, name, FFMPEG_SUMS))
        }
    }
}

/// Whether [`install`] can provide `tool` on this platform.
pub fn installable(tool: Tool) -> bool {
    asset(tool).is_some()
}

/// Find `asset`'s hash in a `sha256sum`-style file ("<hex>  <name>" or "<hex> *<name>").
pub fn checksum_for(sums: &str, asset: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        let hash = parts.next()?;
        let name = parts.next()?.trim_start_matches('*');
        (name == asset && hash.len() == 64).then(|| hash.to_ascii_lowercase())
    })
}

fn client() -> Result<reqwest::Client, InstallError> {
    Ok(reqwest::Client::builder()
        .user_agent(concat!("dlux/", env!("CARGO_PKG_VERSION")))
        .build()?)
}

async fn get(client: &reqwest::Client, url: &str) -> Result<reqwest::Response, InstallError> {
    let resp = client.get(url).send().await?;
    if !resp.status().is_success() {
        return Err(InstallError::Status {
            url: url.to_owned(),
            status: resp.status().as_u16(),
        });
    }
    Ok(resp)
}

/// Download and install `tool` into dlux's bin directory, reporting `(downloaded, total)` bytes
/// to `progress`. Returns the installed path.
pub async fn install(
    tool: Tool,
    mut progress: impl FnMut(u64, Option<u64>) + Send,
) -> Result<PathBuf, InstallError> {
    let (base, asset, sums_file) = asset(tool).ok_or(InstallError::Unsupported { tool })?;
    let client = client()?;
    let sums = get(&client, &format!("{base}/{sums_file}"))
        .await?
        .text()
        .await?;
    let expected = checksum_for(&sums, asset).ok_or_else(|| InstallError::MissingChecksum {
        asset: asset.to_owned(),
    })?;

    let dir = paths::bin_dir();
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(io_err(&dir))?;
    let download = dir.join(format!(".{asset}.part"));
    let mut resp = get(&client, &format!("{base}/{asset}")).await?;
    let total = resp.content_length();
    let mut file = tokio::fs::File::create(&download)
        .await
        .map_err(io_err(&download))?;
    let mut hasher = Sha256::new();
    let mut done = 0u64;
    progress(0, total);
    while let Some(chunk) = resp.chunk().await? {
        hasher.update(&chunk);
        file.write_all(&chunk).await.map_err(io_err(&download))?;
        done += chunk.len() as u64;
        progress(done, total);
    }
    file.flush().await.map_err(io_err(&download))?;
    drop(file);

    let actual = hex(&hasher.finalize());
    if actual != expected {
        let _ = tokio::fs::remove_file(&download).await;
        return Err(InstallError::Checksum {
            asset: asset.to_owned(),
        });
    }

    let target = tool.private_path();
    let result = match tool {
        Tool::YtDlp => place(&download, &target),
        Tool::Ffmpeg => {
            let (archive, bin) = (download.clone(), dir.clone());
            let asset = asset.to_owned();
            tokio::task::spawn_blocking(move || extract_ffmpeg(&archive, &asset, &bin))
                .await
                .unwrap_or_else(|e| Err(InstallError::NotInArchive(format!("ffmpeg ({e})"))))
        }
    };
    let _ = tokio::fs::remove_file(&download).await;
    result.map(|()| target)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Make `from` executable and atomically move it to `to`.
fn place(from: &Path, to: &Path) -> Result<(), InstallError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(from, std::fs::Permissions::from_mode(0o755))
            .map_err(io_err(from))?;
    }
    std::fs::rename(from, to).map_err(io_err(to))
}

/// Write one archive member to `dir/name` via a temporary file.
fn write_member(mut reader: impl Read, dir: &Path, name: &str) -> Result<(), InstallError> {
    let tmp = dir.join(format!(".{name}.part"));
    let mut out = std::fs::File::create(&tmp).map_err(io_err(&tmp))?;
    std::io::copy(&mut reader, &mut out).map_err(io_err(&tmp))?;
    out.flush().map_err(io_err(&tmp))?;
    drop(out);
    place(&tmp, &dir.join(name))
}

/// Extract `ffmpeg` and `ffprobe` from a FFmpeg-Builds archive (`<root>/bin/<name>`).
fn extract_ffmpeg(archive: &Path, asset: &str, dir: &Path) -> Result<(), InstallError> {
    let wanted = [Tool::exe("ffmpeg"), Tool::exe("ffprobe")];
    let mut found = Vec::new();
    let file = std::fs::File::open(archive).map_err(io_err(archive))?;
    let member_name = |path: &str| -> Option<String> {
        let mut parts = path.rsplit('/');
        let name = parts.next()?;
        (parts.next() == Some("bin") && wanted.iter().any(|w| w == name)).then(|| name.to_owned())
    };
    if asset.ends_with(".zip") {
        let mut zip = zip::ZipArchive::new(std::io::BufReader::new(file))
            .map_err(|e| InstallError::NotInArchive(format!("ffmpeg (bad zip: {e})")))?;
        for i in 0..zip.len() {
            let entry = zip
                .by_index(i)
                .map_err(|e| InstallError::NotInArchive(format!("ffmpeg (bad zip: {e})")))?;
            // Skip entries whose names aren't valid text; the binaries always are.
            let Ok(raw) = entry.name() else { continue };
            if let Some(name) = member_name(&raw.replace('\\', "/")) {
                write_member(entry, dir, &name)?;
                found.push(name);
            }
        }
    } else {
        let xz = liblzma::read::XzDecoder::new(std::io::BufReader::new(file));
        let mut tar = tar::Archive::new(xz);
        for entry in tar.entries().map_err(io_err(archive))? {
            let entry = entry.map_err(io_err(archive))?;
            let path = entry.path().map_err(io_err(archive))?;
            if let Some(name) = member_name(&path.to_string_lossy()) {
                write_member(entry, dir, &name)?;
                found.push(name);
            }
        }
    }
    if found.iter().any(|f| *f == wanted[0]) {
        Ok(())
    } else {
        Err(InstallError::NotInArchive(wanted[0].clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksum_lookup() {
        let sums = "\
0000000000000000000000000000000000000000000000000000000000000001  yt-dlp
ABCDEF0000000000000000000000000000000000000000000000000000000002 *yt-dlp_linux
short  yt-dlp_macos
";
        assert_eq!(
            checksum_for(sums, "yt-dlp_linux").as_deref(),
            Some("abcdef0000000000000000000000000000000000000000000000000000000002")
        );
        assert!(checksum_for(sums, "yt-dlp_linux_aarch64").is_none());
        assert!(
            checksum_for(sums, "yt-dlp_macos").is_none(),
            "malformed hash"
        );
        assert!(
            checksum_for(sums, "yt-dlp_lin").is_none(),
            "exact name only"
        );
    }

    #[test]
    fn assets_for_this_platform() {
        if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
            assert_eq!(asset(Tool::YtDlp).unwrap().1, "yt-dlp_linux");
            assert!(
                asset(Tool::Ffmpeg)
                    .unwrap()
                    .1
                    .ends_with("linux64-gpl.tar.xz")
            );
        }
        if cfg!(target_os = "macos") {
            assert!(!installable(Tool::Ffmpeg));
        }
    }

    #[test]
    fn extracts_ffmpeg_from_tar_xz() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("ffmpeg.tar.xz");
        {
            let file = std::fs::File::create(&archive).unwrap();
            let xz = liblzma::write::XzEncoder::new(file, 1);
            let mut tar = tar::Builder::new(xz);
            for (path, body) in [
                ("ffmpeg-master/bin/ffmpeg", &b"#!ffmpeg"[..]),
                ("ffmpeg-master/bin/ffprobe", b"#!ffprobe"),
                ("ffmpeg-master/doc/ffmpeg", b"not this one"),
                ("ffmpeg-master/LICENSE.txt", b"GPL"),
            ] {
                let mut header = tar::Header::new_gnu();
                header.set_size(body.len() as u64);
                header.set_mode(0o644);
                header.set_cksum();
                tar.append_data(&mut header, path, body).unwrap();
            }
            tar.into_inner().unwrap().finish().unwrap();
        }
        let bin = dir.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        extract_ffmpeg(&archive, "x.tar.xz", &bin).unwrap();
        assert_eq!(std::fs::read(bin.join("ffmpeg")).unwrap(), b"#!ffmpeg");
        assert_eq!(std::fs::read(bin.join("ffprobe")).unwrap(), b"#!ffprobe");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(bin.join("ffmpeg"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o111, 0o111, "executable");
        }
        assert!(
            extract_ffmpeg(&archive, "x.tar.xz", &dir.path().join("missing")).is_err(),
            "unwritable target is an error"
        );
    }
}
