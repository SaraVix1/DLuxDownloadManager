//! Speed limits: parsing what the user types ("500K", "1.5 MiB/s", "0") and reading/writing the
//! global and per-download limits in aria2. A limit of 0 means unlimited.

use crate::rpc::{self, Client, Options};

/// Which limits to read or change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Global,
    Download(rpc::Gid),
}

impl Target {
    /// aria2 option names for the (download, upload) limits.
    fn keys(&self) -> (&'static str, &'static str) {
        match self {
            Self::Global => ("max-overall-download-limit", "max-overall-upload-limit"),
            Self::Download(_) => ("max-download-limit", "max-upload-limit"),
        }
    }
}

/// Download and upload limits in bytes per second; 0 = unlimited.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Limits {
    pub download: u64,
    pub upload: u64,
}

/// Parse a speed limit. Accepts a number with an optional binary unit (`K`, `M`, `G`, also
/// written `KiB`, `KB`, `k`…, optionally followed by `/s`). A bare number is bytes per second,
/// as in aria2. Empty, `0`, `none`, `off`, `unlimited` and `∞` mean unlimited.
pub fn parse(input: &str) -> Result<u64, String> {
    let s = input.trim().to_ascii_lowercase();
    let s = s.strip_suffix("/s").unwrap_or(&s).trim();
    if s.is_empty() || matches!(s, "0" | "none" | "off" | "unlimited" | "∞" | "-") {
        return Ok(0);
    }
    let split = s
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(s.len());
    let (number, unit) = s.split_at(split);
    let value: f64 = number
        .parse()
        .map_err(|_| format!("{:?} is not a speed (try 500K or 2M)", input.trim()))?;
    let unit = unit.trim();
    let unit = unit.strip_suffix('b').unwrap_or(unit);
    let unit = unit.strip_suffix('i').unwrap_or(unit);
    let multiplier: u64 = match unit {
        "" => 1,
        "k" => 1 << 10,
        "m" => 1 << 20,
        "g" => 1 << 30,
        _ => return Err(format!("unknown unit {unit:?} (use K, M or G)")),
    };
    Ok((value * multiplier as f64).round() as u64)
}

/// The shortest exact way to write a limit for editing: "0", "512K", "2M", "1536".
pub fn format_input(bytes: u64) -> String {
    match bytes {
        0 => "0".into(),
        b if b % (1 << 30) == 0 => format!("{}G", b >> 30),
        b if b % (1 << 20) == 0 => format!("{}M", b >> 20),
        b if b % (1 << 10) == 0 => format!("{}K", b >> 10),
        b => b.to_string(),
    }
}

/// Human-readable limit: "unlimited" or e.g. "2.00 MiB/s".
pub fn describe(bytes: u64) -> String {
    if bytes == 0 {
        "unlimited".into()
    } else {
        crate::fmt::speed(bytes)
    }
}

fn option_value(options: &Options, key: &str) -> u64 {
    options.get(key).and_then(|v| parse(v).ok()).unwrap_or(0)
}

/// Read the current limits.
pub async fn get(client: &Client, target: &Target) -> rpc::Result<Limits> {
    let options = match target {
        Target::Global => client.get_global_option().await?,
        Target::Download(gid) => client.get_option(gid).await?,
    };
    let (down, up) = target.keys();
    Ok(Limits {
        download: option_value(&options, down),
        upload: option_value(&options, up),
    })
}

/// Change the limits.
pub async fn set(client: &Client, target: &Target, limits: Limits) -> rpc::Result<()> {
    let (down, up) = target.keys();
    let mut options = Options::new();
    options.insert(down.into(), limits.download.to_string());
    options.insert(up.into(), limits.upload.to_string());
    match target {
        Target::Global => client.change_global_option(&options).await,
        Target::Download(gid) => client.change_option(gid, &options).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_limits() {
        for (input, want) in [
            ("", 0),
            ("0", 0),
            (" Unlimited ", 0),
            ("1024", 1024),
            ("500K", 500 * 1024),
            ("500k", 500 * 1024),
            ("500 KiB/s", 500 * 1024),
            ("2M", 2 << 20),
            ("1.5MB", 3 << 19),
            ("1g", 1 << 30),
        ] {
            assert_eq!(parse(input), Ok(want), "{input:?}");
        }
        assert!(parse("fast").is_err());
        assert!(parse("5X").unwrap_err().contains("unknown unit"));
    }

    #[test]
    fn formats_for_editing() {
        assert_eq!(format_input(0), "0");
        assert_eq!(format_input(512 * 1024), "512K");
        assert_eq!(format_input(2 << 20), "2M");
        assert_eq!(format_input(1 << 30), "1G");
        assert_eq!(format_input(1500), "1500");
        for b in [0, 1500, 512 * 1024, 2 << 20] {
            assert_eq!(parse(&format_input(b)), Ok(b));
        }
        assert_eq!(describe(0), "unlimited");
    }

    #[test]
    fn option_values_from_aria2() {
        let mut o = Options::new();
        o.insert("max-download-limit".into(), "1048576".into());
        o.insert("max-upload-limit".into(), "0".into());
        assert_eq!(option_value(&o, "max-download-limit"), 1 << 20);
        assert_eq!(option_value(&o, "max-upload-limit"), 0);
        assert_eq!(option_value(&o, "missing"), 0);
    }
}
