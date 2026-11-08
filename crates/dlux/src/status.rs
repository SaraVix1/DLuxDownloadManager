//! `dlux status`: a plain-text (or JSON) overview of aria2 and its downloads.

use anyhow::Result;
use dlux_core::aria2::{Aria2, Origin};
use dlux_core::fmt;
use dlux_core::rpc::{Download, Snapshot};
use std::io::Write;

pub async fn print(aria2: &Aria2, json: bool) -> Result<()> {
    let snap = aria2.client().snapshot().await?;
    let mut out = std::io::stdout().lock();
    if json {
        serde_json::to_writer_pretty(&mut out, &snap)?;
        writeln!(out)?;
        return Ok(());
    }
    write_report(&mut out, aria2, &snap)?;
    Ok(())
}

fn write_report(out: &mut impl Write, aria2: &Aria2, snap: &Snapshot) -> std::io::Result<()> {
    let origin = match aria2.origin() {
        Origin::External => format!("connected to {}", aria2.client().url()),
        Origin::Spawned { port, .. } => format!("started by dlux on port {port}"),
    };
    writeln!(
        out,
        "dlux {} · aria2 {} ({origin})",
        env!("CARGO_PKG_VERSION"),
        aria2.version().version
    )?;
    let s = &snap.stat;
    writeln!(
        out,
        "↓ {}  ↑ {}   active {} · waiting {} · stopped {}",
        fmt::speed(s.download_speed),
        fmt::speed(s.upload_speed),
        s.num_active,
        s.num_waiting,
        s.num_stopped
    )?;
    if snap.iter().next().is_none() {
        writeln!(out, "\nNo downloads.")?;
        return Ok(());
    }
    writeln!(
        out,
        "\n{:<9} {:>7} {:>10} {:>11} {:>8}  {:<16} NAME",
        "STATUS", "DONE", "SIZE", "SPEED", "ETA", "GID"
    )?;
    for d in snap.iter() {
        writeln!(out, "{}", row(d))?;
    }
    Ok(())
}

fn row(d: &Download) -> String {
    let size = if d.total_length > 0 {
        fmt::bytes(d.total_length)
    } else {
        "?".to_owned()
    };
    let mut line = format!(
        "{:<9} {:>7} {:>10} {:>11} {:>8}  {:<16} {}",
        d.status.as_str(),
        fmt::percent(d.progress()),
        size,
        fmt::speed(d.download_speed),
        fmt::eta(d.eta()),
        d.gid,
        d.name()
    );
    if let Some(msg) = d.error_message.as_deref().filter(|m| !m.is_empty()) {
        line.push_str(&format!("  ({msg})"));
    }
    line
}
