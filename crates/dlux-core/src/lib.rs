//! Core library for **dlux** — *Download everything. Beautifully.*
//!
//! Front-end independent pieces shared by the dlux TUI and web UI:
//!
//! - [`rpc`] — async aria2 JSON-RPC client and typed results
//! - [`binaries`] — find or install yt-dlp and ffmpeg
//! - [`aria2`] — connect to a running aria2, or spawn and manage a private `aria2c`
//! - [`config`] — `~/.config/dlux/config.toml`
//! - [`paths`] — standard file locations and binary lookup
//! - [`fmt`] — human-readable sizes, speeds and durations
//! - [`sources`] — URLs, magnets and .torrent/.metalink files from user input, and adding them
//! - [`limits`] — parsing and applying global / per-download speed limits
//! - [`ytdlp`] — probing video pages, the format picker, download commands and progress parsing
//! - [`jobs`] — downloads dlux runs itself (yt-dlp videos, tool installs), listed like aria2's
//! - [`service`] — every operation the front ends offer, routed to aria2 or local jobs
//! - [`state`] — app state shared by the front ends: snapshot, filters, speed history, poller

pub mod aria2;
pub mod binaries;
pub mod config;
pub mod fmt;
mod fsutil;
pub mod jobs;
pub mod limits;
pub mod paths;
pub mod rpc;
pub mod service;
pub mod sources;
pub mod state;
pub mod ytdlp;

pub use aria2::{Aria2, Aria2Error, ConnectOptions, Origin};
pub use config::Config;
pub use rpc::Client;
