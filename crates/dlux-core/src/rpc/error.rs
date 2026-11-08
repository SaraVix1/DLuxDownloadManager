use serde::{Deserialize, Serialize};

/// An error object returned by aria2 (`{"code": 1, "message": "..."}`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[error("aria2 error {code}: {message}")]
pub struct RpcError {
    pub code: i64,
    pub message: String,
}

impl RpcError {
    /// aria2 rejected the RPC secret (or none was sent but one is required).
    pub fn is_unauthorized(&self) -> bool {
        self.message.eq_ignore_ascii_case("unauthorized")
    }

    /// The GID does not exist (e.g. it was purged).
    pub fn is_not_found(&self) -> bool {
        self.message.contains("is not found")
    }
}

/// Errors produced by the aria2 RPC client.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Nothing accepted the connection (aria2 not running, wrong port, unreachable host).
    #[error("cannot reach aria2 at {url}: {source}")]
    Connect {
        url: String,
        #[source]
        source: reqwest::Error,
    },
    /// The request timed out. Something may be listening but not answering in time (aria2
    /// deliberately delays its reply to a wrong secret).
    #[error("aria2 at {url} did not answer in time")]
    Timeout {
        url: String,
        #[source]
        source: reqwest::Error,
    },
    /// The HTTP request failed after the connection was established.
    #[error("aria2 HTTP request failed: {0}")]
    Http(#[source] reqwest::Error),
    /// aria2 answered with a JSON-RPC error.
    #[error(transparent)]
    Rpc(#[from] RpcError),
    /// aria2 answered with something that is not a valid JSON-RPC response for the call.
    #[error("unexpected aria2 response: {0}")]
    Decode(String),
    /// The configured RPC URL is not usable.
    #[error("invalid aria2 RPC URL {url:?}: {reason}")]
    InvalidUrl { url: String, reason: String },
}

impl Error {
    /// `true` when aria2 is reachable but rejected the secret.
    pub fn is_unauthorized(&self) -> bool {
        matches!(self, Self::Rpc(e) if e.is_unauthorized())
    }

    /// `true` when nothing accepted the connection at the RPC URL.
    pub fn is_connect(&self) -> bool {
        matches!(self, Self::Connect { .. })
    }

    /// `true` when the request timed out.
    pub fn is_timeout(&self) -> bool {
        matches!(self, Self::Timeout { .. })
    }
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
