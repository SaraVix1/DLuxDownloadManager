//! Tokyo Night colour palettes, with a 256-colour fallback for terminals without truecolor.

use dlux_core::rpc::{Download, DownloadStatus};
use ratatui::style::Color;

/// Downloads slower than this are drawn with the "slow" (amber) colour.
pub const SLOW_SPEED: u64 = 256 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    /// Main background.
    pub bg: Color,
    /// Darker background (pill text, header).
    pub bg_dark: Color,
    /// Selection background.
    pub bg_highlight: Color,
    /// Mouse-hover background: between `bg` and `bg_highlight`.
    pub bg_hover: Color,
    /// Panel borders.
    pub border: Color,
    /// Borders of the focused panel.
    pub border_focus: Color,
    pub fg: Color,
    pub fg_dark: Color,
    /// Muted text (labels, hints).
    pub comment: Color,
    /// Empty part of progress bars, scrollbar track.
    pub gutter: Color,
    pub blue: Color,
    pub cyan: Color,
    pub magenta: Color,
    pub purple: Color,
    pub orange: Color,
    pub yellow: Color,
    pub green: Color,
    pub teal: Color,
    pub red: Color,
}

const fn rgb(hex: u32) -> Color {
    Color::Rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

impl Theme {
    /// Tokyo Night ("night" variant).
    pub const TOKYO_NIGHT: Theme = Theme {
        bg: rgb(0x1a1b26),
        bg_dark: rgb(0x16161e),
        bg_highlight: rgb(0x292e42),
        bg_hover: rgb(0x21232f),
        border: rgb(0x3b4261),
        border_focus: rgb(0x7aa2f7),
        fg: rgb(0xc0caf5),
        fg_dark: rgb(0xa9b1d6),
        comment: rgb(0x565f89),
        gutter: rgb(0x3b4261),
        blue: rgb(0x7aa2f7),
        cyan: rgb(0x7dcfff),
        magenta: rgb(0xbb9af7),
        purple: rgb(0x9d7cd8),
        orange: rgb(0xff9e64),
        yellow: rgb(0xe0af68),
        green: rgb(0x9ece6a),
        teal: rgb(0x73daca),
        red: rgb(0xf7768e),
    };

    /// Tokyo Night Storm (slightly lighter background).
    pub const TOKYO_NIGHT_STORM: Theme = Theme {
        bg: rgb(0x24283b),
        bg_dark: rgb(0x1f2335),
        bg_highlight: rgb(0x2f344d),
        bg_hover: rgb(0x292e42),
        ..Self::TOKYO_NIGHT
    };

    /// Look up a theme by its config name; unknown names fall back to Tokyo Night.
    pub fn by_name(name: &str) -> Theme {
        match name.trim().to_ascii_lowercase().as_str() {
            "tokyo-night-storm" | "storm" => Self::TOKYO_NIGHT_STORM,
            _ => Self::TOKYO_NIGHT,
        }
    }

    /// Pick the theme for the current terminal, degrading to the 256-colour palette when the
    /// terminal does not advertise truecolor support.
    pub fn for_terminal(name: &str) -> Theme {
        let theme = Self::by_name(name);
        if supports_truecolor() {
            theme
        } else {
            theme.to_256()
        }
    }

    /// Map every RGB colour to the nearest xterm-256 colour.
    pub fn to_256(self) -> Theme {
        let m = to_indexed;
        Theme {
            bg: m(self.bg),
            bg_dark: m(self.bg_dark),
            bg_highlight: m(self.bg_highlight),
            bg_hover: m(self.bg_hover),
            border: m(self.border),
            border_focus: m(self.border_focus),
            fg: m(self.fg),
            fg_dark: m(self.fg_dark),
            comment: m(self.comment),
            gutter: m(self.gutter),
            blue: m(self.blue),
            cyan: m(self.cyan),
            magenta: m(self.magenta),
            purple: m(self.purple),
            orange: m(self.orange),
            yellow: m(self.yellow),
            green: m(self.green),
            teal: m(self.teal),
            red: m(self.red),
        }
    }

    /// Progress bar colour: green when fast, amber when slow, gray when paused, red on error.
    pub fn progress_color(&self, d: &Download) -> Color {
        match d.status {
            DownloadStatus::Active if d.download_speed >= SLOW_SPEED || d.seeder => self.green,
            DownloadStatus::Active => self.yellow,
            DownloadStatus::Complete => self.teal,
            DownloadStatus::Error | DownloadStatus::Unknown => self.red,
            DownloadStatus::Waiting | DownloadStatus::Paused | DownloadStatus::Removed => {
                self.comment
            }
        }
    }

    /// Colour for a status label.
    pub fn status_color(&self, d: &Download) -> Color {
        match d.status {
            DownloadStatus::Active => self.green,
            DownloadStatus::Waiting => self.blue,
            DownloadStatus::Paused | DownloadStatus::Removed => self.comment,
            DownloadStatus::Complete => self.teal,
            DownloadStatus::Error | DownloadStatus::Unknown => self.red,
        }
    }
}

fn supports_truecolor() -> bool {
    let colorterm = std::env::var("COLORTERM").unwrap_or_default();
    if matches!(colorterm.as_str(), "truecolor" | "24bit") {
        return true;
    }
    let term = std::env::var("TERM").unwrap_or_default();
    if term.contains("truecolor") || term.contains("direct") {
        return true;
    }
    // Windows Terminal and modern conhost support truecolor but do not set COLORTERM.
    cfg!(windows) || std::env::var_os("WT_SESSION").is_some()
}

/// Nearest xterm-256 colour (6×6×6 cube or grayscale ramp) for an RGB colour.
fn to_indexed(c: Color) -> Color {
    let Color::Rgb(r, g, b) = c else {
        return c;
    };
    const LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];
    let nearest_level = |v: u8| {
        LEVELS
            .iter()
            .enumerate()
            .min_by_key(|&(_, &l)| (i32::from(l) - i32::from(v)).abs())
            .map_or(0, |(i, _)| i)
    };
    let (ri, gi, bi) = (nearest_level(r), nearest_level(g), nearest_level(b));
    let cube = (LEVELS[ri], LEVELS[gi], LEVELS[bi]);
    let cube_idx = 16 + 36 * ri + 6 * gi + bi;

    let avg = (u32::from(r) + u32::from(g) + u32::from(b)) / 3;
    let gray_i = ((avg.saturating_sub(8)) / 10).min(23) as u8;
    let gray_v = 8 + 10 * gray_i;
    let gray = (gray_v, gray_v, gray_v);

    let dist = |(cr, cg, cb): (u8, u8, u8)| {
        let d = |a: u8, b: u8| (i32::from(a) - i32::from(b)).pow(2);
        d(cr, r) + d(cg, g) + d(cb, b)
    };
    if dist(gray) < dist(cube) {
        Color::Indexed(232 + gray_i)
    } else {
        Color::Indexed(cube_idx as u8)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indexed_fallback() {
        assert_eq!(to_indexed(Color::Rgb(0, 0, 0)), Color::Indexed(16));
        assert_eq!(to_indexed(Color::Rgb(255, 255, 255)), Color::Indexed(231));
        assert_eq!(to_indexed(Color::Rgb(255, 0, 0)), Color::Indexed(196));
        // Dark Tokyo Night background becomes a dark gray.
        assert!(matches!(
            to_indexed(rgb(0x1a1b26)),
            Color::Indexed(232..=236)
        ));
        assert_eq!(to_indexed(Color::Red), Color::Red);
        let t = Theme::TOKYO_NIGHT.to_256();
        assert!(!matches!(t.blue, Color::Rgb(..)));
    }

    #[test]
    fn theme_lookup() {
        assert_eq!(Theme::by_name("tokyo-night"), Theme::TOKYO_NIGHT);
        assert_eq!(
            Theme::by_name("Tokyo-Night-Storm"),
            Theme::TOKYO_NIGHT_STORM
        );
        assert_eq!(Theme::by_name("nope"), Theme::TOKYO_NIGHT);
    }

    #[test]
    fn progress_colors() {
        let t = Theme::TOKYO_NIGHT;
        let mut d = Download {
            status: DownloadStatus::Active,
            download_speed: SLOW_SPEED,
            ..Default::default()
        };
        assert_eq!(t.progress_color(&d), t.green);
        d.download_speed = 10;
        assert_eq!(t.progress_color(&d), t.yellow);
        d.status = DownloadStatus::Paused;
        assert_eq!(t.progress_color(&d), t.comment);
        d.status = DownloadStatus::Error;
        assert_eq!(t.progress_color(&d), t.red);
    }
}
