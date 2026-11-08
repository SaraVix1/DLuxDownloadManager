//! Standard locations for dlux files.
//!
//! On Unix-like systems (including macOS) dlux follows the XDG base directory layout so the
//! documented paths (`~/.config/dlux`, `~/.local/share/dlux`) hold everywhere. On Windows the
//! platform's roaming config / local data directories are used.

use std::path::{Path, PathBuf};

const APP: &str = "dlux";

fn home() -> Option<PathBuf> {
    dirs::home_dir()
}

#[cfg(unix)]
fn xdg(var: &str, fallback: &str) -> Option<PathBuf> {
    std::env::var_os(var)
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| home().map(|h| h.join(fallback)))
}

/// Base config directory, e.g. `~/.config`.
fn config_base() -> Option<PathBuf> {
    #[cfg(unix)]
    {
        xdg("XDG_CONFIG_HOME", ".config")
    }
    #[cfg(not(unix))]
    {
        dirs::config_dir()
    }
}

/// Base data directory, e.g. `~/.local/share`.
fn data_base() -> Option<PathBuf> {
    #[cfg(unix)]
    {
        xdg("XDG_DATA_HOME", ".local/share")
    }
    #[cfg(not(unix))]
    {
        dirs::data_local_dir()
    }
}

/// `~/.config/dlux`
pub fn config_dir() -> PathBuf {
    config_base()
        .unwrap_or_else(|| PathBuf::from(".config"))
        .join(APP)
}

/// `~/.config/dlux/config.toml`
pub fn config_file() -> PathBuf {
    config_dir().join("config.toml")
}

/// `~/.local/share/dlux`
pub fn data_dir() -> PathBuf {
    data_base()
        .unwrap_or_else(|| PathBuf::from(".local/share"))
        .join(APP)
}

/// `~/.local/share/dlux/bin` — where auto-downloaded helper binaries (yt-dlp, ffmpeg) live.
pub fn bin_dir() -> PathBuf {
    data_dir().join("bin")
}

/// The user's download directory (`~/Downloads` or the platform equivalent).
pub fn default_download_dir() -> PathBuf {
    dirs::download_dir()
        .or_else(|| home().map(|h| h.join("Downloads")))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Expand a leading `~` / `~/` to the home directory.
pub fn expand_tilde(path: impl AsRef<Path>) -> PathBuf {
    let path = path.as_ref();
    let Some(s) = path.to_str() else {
        return path.to_path_buf();
    };
    if s == "~" {
        return home().unwrap_or_else(|| path.to_path_buf());
    }
    if let Some(rest) = s.strip_prefix("~/").or_else(|| s.strip_prefix("~\\")) {
        if let Some(h) = home() {
            return h.join(rest);
        }
    }
    path.to_path_buf()
}

/// Search `PATH` for an executable called `name` (adds `.exe` on Windows).
pub fn find_in_path(name: &str) -> Option<PathBuf> {
    let file = if cfg!(windows) && Path::new(name).extension().is_none() {
        format!("{name}.exe")
    } else {
        name.to_owned()
    };
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(&file))
        .find(|candidate| is_executable(candidate))
}

/// Find a helper binary: first on `PATH`, then in [`bin_dir`].
pub fn find_binary(name: &str) -> Option<PathBuf> {
    find_in_path(name).or_else(|| {
        let file = if cfg!(windows) {
            format!("{name}.exe")
        } else {
            name.to_owned()
        };
        let candidate = bin_dir().join(file);
        is_executable(&candidate).then_some(candidate)
    })
}

fn is_executable(path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_ends_with_app_name() {
        assert!(config_dir().ends_with("dlux"));
        assert!(config_file().ends_with("dlux/config.toml"));
        assert!(data_dir().ends_with("dlux"));
        assert!(bin_dir().ends_with("dlux/bin"));
    }

    #[test]
    fn tilde_expansion() {
        if let Some(h) = dirs::home_dir() {
            assert_eq!(expand_tilde("~"), h);
            assert_eq!(expand_tilde("~/Downloads"), h.join("Downloads"));
        }
        assert_eq!(expand_tilde("/abs/path"), PathBuf::from("/abs/path"));
        assert_eq!(expand_tilde("rel/~x"), PathBuf::from("rel/~x"));
    }

    #[cfg(unix)]
    #[test]
    fn finds_sh_in_path() {
        assert!(find_in_path("sh").is_some());
        assert!(find_in_path("definitely-not-a-real-binary-dlux").is_none());
    }
}
