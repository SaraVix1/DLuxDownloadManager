//! Human-readable formatting shared by the TUI and the web UI.

use std::time::Duration;

const UNITS: [&str; 7] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB", "EiB"];

/// `1536` → `"1.5 KiB"`.
pub fn bytes(n: u64) -> String {
    if n < 1024 {
        return format!("{n} B");
    }
    let mut value = n as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if value >= 100.0 {
        format!("{value:.0} {}", UNITS[unit])
    } else if value >= 10.0 {
        format!("{value:.1} {}", UNITS[unit])
    } else {
        format!("{value:.2} {}", UNITS[unit])
    }
}

/// `1536` → `"1.5 KiB/s"`.
pub fn speed(bytes_per_sec: u64) -> String {
    format!("{}/s", bytes(bytes_per_sec))
}

/// Compact duration: `"42s"`, `"3m 05s"`, `"2h 07m"`, `"3d 04h"`.
pub fn duration(d: Duration) -> String {
    let s = d.as_secs();
    let (days, hours, mins, secs) = (s / 86_400, (s / 3600) % 24, (s / 60) % 60, s % 60);
    if days > 0 {
        format!("{days}d {hours:02}h")
    } else if hours > 0 {
        format!("{hours}h {mins:02}m")
    } else if mins > 0 {
        format!("{mins}m {secs:02}s")
    } else {
        format!("{secs}s")
    }
}

/// ETA for display; `None` (unknown) renders as `"--"`.
pub fn eta(d: Option<Duration>) -> String {
    d.map_or_else(|| "--".to_owned(), duration)
}

/// `0.4567` → `"45.7%"`.
pub fn percent(ratio: f64) -> String {
    format!("{:.1}%", (ratio * 100.0).clamp(0.0, 100.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_bytes() {
        assert_eq!(bytes(0), "0 B");
        assert_eq!(bytes(1023), "1023 B");
        assert_eq!(bytes(1024), "1.00 KiB");
        assert_eq!(bytes(1536), "1.50 KiB");
        assert_eq!(bytes(15 * 1024 * 1024), "15.0 MiB");
        assert_eq!(bytes(700 * 1024 * 1024), "700 MiB");
        assert_eq!(bytes(u64::MAX), "16.0 EiB");
        assert_eq!(speed(2048), "2.00 KiB/s");
    }

    #[test]
    fn formats_durations() {
        assert_eq!(duration(Duration::from_secs(0)), "0s");
        assert_eq!(duration(Duration::from_secs(42)), "42s");
        assert_eq!(duration(Duration::from_secs(185)), "3m 05s");
        assert_eq!(duration(Duration::from_secs(2 * 3600 + 7 * 60)), "2h 07m");
        assert_eq!(
            duration(Duration::from_secs(3 * 86_400 + 4 * 3600)),
            "3d 04h"
        );
        assert_eq!(eta(None), "--");
    }

    #[test]
    fn formats_percent() {
        assert_eq!(percent(0.4567), "45.7%");
        assert_eq!(percent(1.2), "100.0%");
        assert_eq!(percent(-1.0), "0.0%");
    }
}
