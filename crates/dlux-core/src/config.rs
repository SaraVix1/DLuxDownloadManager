//! `~/.config/dlux/config.toml`.
//!
//! Every field has a default, so a missing file or a partial file is fine.

use crate::paths;
use crate::rpc::DEFAULT_RPC_URL;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("cannot read config file {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid config file {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: Box<toml::de::Error>,
    },
    #[error("cannot write config file {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub aria2: Aria2Config,
    pub downloads: DownloadsConfig,
    pub ui: UiConfig,
    pub web: WebConfig,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Aria2Config {
    /// JSON-RPC endpoint of aria2.
    pub rpc_url: String,
    /// `--rpc-secret` of an already-running aria2. When dlux spawns aria2 itself and this is
    /// unset, a random secret is generated for that session.
    pub rpc_secret: Option<String>,
    /// Start `aria2c` automatically when nothing answers at a local `rpc_url`.
    pub auto_spawn: bool,
    /// Explicit path to `aria2c`; otherwise it is looked up on `PATH`.
    pub aria2c_path: Option<PathBuf>,
    /// Extra command-line arguments for a spawned `aria2c`.
    pub extra_args: Vec<String>,
}

impl Default for Aria2Config {
    fn default() -> Self {
        Self {
            rpc_url: DEFAULT_RPC_URL.to_owned(),
            rpc_secret: None,
            auto_spawn: true,
            aria2c_path: None,
            extra_args: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DownloadsConfig {
    /// Default download directory. `~` is expanded.
    pub dir: PathBuf,
    /// `--max-concurrent-downloads` for a spawned aria2.
    pub max_concurrent: u32,
}

impl Default for DownloadsConfig {
    fn default() -> Self {
        Self {
            dir: paths::default_download_dir(),
            max_concurrent: 5,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiConfig {
    pub theme: String,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            theme: "tokyo-night".to_owned(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WebConfig {
    /// Access token for the web UI. Required when `dlux serve` binds to a non-loopback address.
    pub token: Option<String>,
}

impl Config {
    /// Load from the default location, falling back to defaults if the file does not exist.
    pub fn load() -> Result<Self, ConfigError> {
        Self::load_from(&paths::config_file())
    }

    /// Load from `path`, falling back to defaults if the file does not exist.
    pub fn load_from(path: &Path) -> Result<Self, ConfigError> {
        match std::fs::read_to_string(path) {
            Ok(text) => Self::from_toml(&text).map_err(|source| ConfigError::Parse {
                path: path.to_path_buf(),
                source: Box::new(source),
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(source) => Err(ConfigError::Read {
                path: path.to_path_buf(),
                source,
            }),
        }
    }

    pub fn from_toml(text: &str) -> Result<Self, toml::de::Error> {
        let mut cfg: Self = toml::from_str(text)?;
        cfg.normalize();
        Ok(cfg)
    }

    pub fn to_toml(&self) -> String {
        toml::to_string_pretty(self).expect("config is always serializable")
    }

    /// Write the config to `path` (creating parent directories). On Unix the file is created
    /// with mode `0600` because it can contain secrets.
    pub fn save_to(&self, path: &Path) -> Result<(), ConfigError> {
        let err = |source| ConfigError::Write {
            path: path.to_path_buf(),
            source,
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(err)?;
        }
        crate::fsutil::write_private(path, self.to_toml().as_bytes()).map_err(err)
    }

    /// Download directory with `~` expanded.
    pub fn download_dir(&self) -> PathBuf {
        paths::expand_tilde(&self.downloads.dir)
    }

    /// The local `aria2c` binary (configured path, else PATH), e.g. for yt-dlp to use as its
    /// downloader. `None` if there is none on this machine.
    pub fn aria2c_binary(&self) -> Option<PathBuf> {
        self.aria2
            .aria2c_path
            .as_ref()
            .map(paths::expand_tilde)
            .filter(|p| p.is_file())
            .or_else(|| paths::find_binary("aria2c"))
    }

    /// Treat empty strings as "unset" so `rpc_secret = ""` behaves like no secret.
    fn normalize(&mut self) {
        fn clear_empty(s: &mut Option<String>) {
            if s.as_deref().is_some_and(|v| v.trim().is_empty()) {
                *s = None;
            }
        }
        clear_empty(&mut self.aria2.rpc_secret);
        clear_empty(&mut self.web.token);
        if self
            .aria2
            .aria2c_path
            .as_ref()
            .is_some_and(|p| p.as_os_str().is_empty())
        {
            self.aria2.aria2c_path = None;
        }
        if self.aria2.rpc_url.trim().is_empty() {
            self.aria2.rpc_url = DEFAULT_RPC_URL.to_owned();
        }
        if self.downloads.dir.as_os_str().is_empty() {
            self.downloads.dir = paths::default_download_dir();
        }
        if self.downloads.max_concurrent == 0 {
            self.downloads.max_concurrent = 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_file_gives_defaults() {
        let cfg = Config::from_toml("").unwrap();
        assert_eq!(cfg, Config::default());
        assert_eq!(cfg.aria2.rpc_url, DEFAULT_RPC_URL);
        assert!(cfg.aria2.auto_spawn);
        assert_eq!(cfg.ui.theme, "tokyo-night");
    }

    #[test]
    fn partial_file_keeps_other_defaults() {
        let cfg = Config::from_toml(
            r#"
            [aria2]
            rpc_url = "http://127.0.0.1:6801/jsonrpc"
            rpc_secret = "hunter2"

            [downloads]
            dir = "~/Videos"

            [web]
            token = ""
            "#,
        )
        .unwrap();
        assert_eq!(cfg.aria2.rpc_url, "http://127.0.0.1:6801/jsonrpc");
        assert_eq!(cfg.aria2.rpc_secret.as_deref(), Some("hunter2"));
        assert!(cfg.aria2.auto_spawn);
        assert_eq!(cfg.downloads.max_concurrent, 5);
        assert_eq!(cfg.web.token, None, "empty token is treated as unset");
        if let Some(home) = dirs::home_dir() {
            assert_eq!(cfg.download_dir(), home.join("Videos"));
        }
    }

    #[test]
    fn empty_secret_is_none() {
        let cfg = Config::from_toml("[aria2]\nrpc_secret = \"\"\n").unwrap();
        assert_eq!(cfg.aria2.rpc_secret, None);
    }

    #[test]
    fn invalid_toml_is_an_error() {
        assert!(Config::from_toml("[aria2\n").is_err());
        assert!(Config::from_toml("[aria2]\nauto_spawn = \"yes\"\n").is_err());
    }

    #[test]
    fn roundtrip_through_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/config.toml");
        assert_eq!(Config::load_from(&path).unwrap(), Config::default());

        let mut cfg = Config::default();
        cfg.aria2.rpc_secret = Some("abc".into());
        cfg.aria2.extra_args = vec!["--seed-time=0".into()];
        cfg.web.token = Some("tok".into());
        cfg.save_to(&path).unwrap();
        assert_eq!(Config::load_from(&path).unwrap(), cfg);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }
}
