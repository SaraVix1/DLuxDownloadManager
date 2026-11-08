//! aria2 JSON-RPC client.
//!
//! See the [aria2 RPC interface](https://aria2.github.io/manual/en/html/aria2c.html#rpc-interface).

mod client;
pub mod de;
mod error;
mod types;

pub use client::{Call, Client, DEFAULT_RPC_URL, DOWNLOAD_KEYS, SNAPSHOT_LIST_LIMIT};
pub use error::{Error, Result, RpcError};
pub use types::{
    BitTorrent, BitTorrentInfo, Download, DownloadStatus, File, Gid, GlobalStat, Options,
    PositionFrom, Snapshot, Uri, Version,
};
