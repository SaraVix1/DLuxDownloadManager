//! Rendering. Everything is drawn from [`App`]; clickable regions are recorded in
//! [`App::hits`] for mouse handling.

use super::app::{App, Command, HitMap};
use super::overlay;
use super::theme::Theme;
use dlux_core::jobs::{Job, JobKind};
use dlux_core::rpc::{Download, DownloadStatus};
use dlux_core::state::Filter;
use dlux_core::{fmt, limits};
use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols::{self, Marker};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Axis, Block, BorderType, Chart, Dataset, GraphType, Paragraph, Scrollbar, ScrollbarOrientation,
    ScrollbarState, StatefulWidget, Widget,
};
use unicode_width::UnicodeWidthStr;

/// Rows per download card, and blank rows between cards.
pub const CARD_HEIGHT: u16 = 3;
pub const CARD_GAP: u16 = 1;
/// Terminals at least this wide show the details pane beside the list.
pub const WIDE: u16 = 110;

/// Screen regions for one frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AppLayout {
    pub header: Rect,
    pub graph: Option<Rect>,
    pub tabs: Rect,
    pub list: Rect,
    pub details: Option<Rect>,
    pub toolbar: Rect,
    pub hints: Option<Rect>,
}

/// Responsive layout. The details pane starts visible on wide terminals and hidden on narrow
/// ones; `details_toggled` flips that.
pub fn compute_layout(area: Rect, details_toggled: bool) -> AppLayout {
    let h = area.height;
    let header_h = if h >= 16 { 3 } else { 1 };
    let graph_h = match h {
        44.. => 12,
        30.. => 9,
        24.. => 7,
        _ => 0,
    };
    let hints_h = u16::from(h >= 14);
    let [header, graph, tabs, body, toolbar, hints] = Layout::vertical([
        Constraint::Length(header_h),
        Constraint::Length(graph_h),
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(1),
        Constraint::Length(hints_h),
    ])
    .areas(area);

    let wide = area.width >= WIDE;
    let show_details = wide != details_toggled;
    let (list, details) = if !show_details {
        (body, None)
    } else if wide {
        let w = (area.width * 2 / 5).clamp(40, 64);
        let [l, d] = Layout::horizontal([Constraint::Min(30), Constraint::Length(w)]).areas(body);
        (l, Some(d))
    } else if body.height >= 16 {
        let dh = (body.height / 2).clamp(8, 14);
        let [l, d] = Layout::vertical([Constraint::Min(5), Constraint::Length(dh)]).areas(body);
        (l, Some(d))
    } else {
        // Not enough room for both: the details replace the list.
        (Rect::new(body.x, body.y, 0, 0), Some(body))
    };

    AppLayout {
        header,
        graph: (graph_h > 0).then_some(graph),
        tabs,
        list,
        details,
        toolbar,
        hints: (hints_h > 0).then_some(hints),
    }
}

pub fn render(frame: &mut Frame, app: &mut App) {
    app.sync_selection();
    app.hits = HitMap::default();
    let t = app.theme;
    let area = frame.area();
    let buf = frame.buffer_mut();
    Block::new()
        .style(Style::new().bg(t.bg).fg(t.fg))
        .render(area, buf);

    let layout = compute_layout(area, app.details);
    app.hits.wide = area.width >= WIDE;
    render_header(buf, app, layout.header);
    if let Some(r) = layout.graph {
        render_graph(buf, app, r);
    }
    render_tabs(buf, app, layout.tabs);
    render_list(buf, app, layout.list);
    if let Some(r) = layout.details {
        render_details(buf, app, r);
    }
    render_toolbar(buf, app, layout.toolbar);
    if let Some(r) = layout.hints {
        render_hints(buf, app, r);
    }
    overlay::render(buf, app);
}

/// Whether the mouse is over `rect`.
fn hovered(app: &App, rect: Rect) -> bool {
    app.hover.is_some_and(|p| rect.contains(p))
}

// ----- helpers ------------------------------------------------------------------------------

fn panel<'a>(t: &Theme, title: &'a str, focused: bool) -> Block<'a> {
    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(if focused { t.border_focus } else { t.border }))
        .title(Line::from(vec![
            Span::raw(" "),
            Span::styled(
                title,
                Style::new()
                    .fg(if focused { t.blue } else { t.fg_dark })
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
        ]))
}

/// A pill: text on a coloured background with half-block caps for rounded ends.
fn pill(text: impl Into<String>, bg: Color, fg: Color) -> Vec<Span<'static>> {
    vec![
        Span::styled("▐", Style::new().fg(bg)),
        Span::styled(
            text.into(),
            Style::new().bg(bg).fg(fg).add_modifier(Modifier::BOLD),
        ),
        Span::styled("▌", Style::new().fg(bg)),
    ]
}

fn spans_width(spans: &[Span]) -> u16 {
    spans.iter().map(|s| s.width() as u16).sum()
}

/// Truncate to `max` display columns, adding an ellipsis when shortened.
pub fn truncate(s: &str, max: usize) -> String {
    if s.width() <= max {
        return s.to_owned();
    }
    if max == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut w = 0;
    for c in s.chars() {
        let cw = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
        if w + cw + 1 > max {
            break;
        }
        out.push(c);
        w += cw;
    }
    out.push('…');
    out
}

fn put_line(buf: &mut Buffer, x: u16, y: u16, spans: Vec<Span<'_>>, max: u16) -> u16 {
    let (end, _) = buf.set_line(x, y, &Line::from(spans), max);
    end
}

fn put_right(buf: &mut Buffer, area: Rect, y: u16, spans: Vec<Span<'_>>) -> u16 {
    let w = spans_width(&spans).min(area.width);
    let x = area.right().saturating_sub(w);
    buf.set_line(x, y, &Line::from(spans), w);
    x
}

fn centered_message(buf: &mut Buffer, area: Rect, lines: Vec<Line<'_>>) {
    if area.is_empty() {
        return;
    }
    let h = (lines.len() as u16).min(area.height);
    let y = area.y + area.height.saturating_sub(h) / 2;
    Paragraph::new(lines)
        .alignment(Alignment::Center)
        .render(Rect::new(area.x, y, area.width, h), buf);
}

/// Thin, half-cell-smooth progress bar.
fn progress_bar(ratio: f64, width: u16, fg: Color, track: Color) -> Vec<Span<'static>> {
    let width = usize::from(width);
    let halves = (ratio.clamp(0.0, 1.0) * width as f64 * 2.0).round() as usize;
    let full = halves / 2;
    let half = halves % 2 == 1 && full < width;
    let rest = width.saturating_sub(full + usize::from(half));
    let mut spans = vec![Span::styled("━".repeat(full), Style::new().fg(fg))];
    if half {
        spans.push(Span::styled("╸", Style::new().fg(fg)));
    }
    spans.push(Span::styled("─".repeat(rest), Style::new().fg(track)));
    spans
}

/// Round up to a "nice" step (1, 1.5, 2, 3, 5, 7.5 × 10ⁿ) so the graph's scale stays stable
/// while leaving little empty headroom.
fn nice_ceiling(v: u64) -> u64 {
    let v = v.max(1);
    if let Some(&c) = [1, 2, 3, 5, 10].iter().find(|&&c| c >= v) {
        return c;
    }
    let mut step = 10u64;
    loop {
        for m in [10, 15, 20, 30, 50, 75] {
            let candidate = (step / 10).saturating_mul(m);
            if candidate >= v {
                return candidate;
            }
        }
        if step > u64::MAX / 10 {
            return u64::MAX;
        }
        step *= 10;
    }
}

/// Round a speed up to a nice step *within its binary unit*, so axis labels read
/// "10 MiB/s" rather than "9.54 MiB/s".
fn nice_speed_ceiling(bytes: u64) -> u64 {
    let mut unit = 1u64;
    while bytes / unit >= 1024 && unit < 1 << 50 {
        unit <<= 10;
    }
    nice_ceiling(bytes.div_ceil(unit)).saturating_mul(unit)
}

/// Compact axis label: no decimals for whole numbers ("10 MiB/s", "512 KiB/s").
fn axis_label(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
    let mut unit = 0;
    let mut value = bytes as f64;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if (value - value.round()).abs() < 0.05 {
        format!("{:.0} {}/s", value.round(), UNITS[unit])
    } else {
        format!("{value:.1} {}/s", UNITS[unit])
    }
}

fn status_text(d: &Download) -> String {
    match d.status {
        DownloadStatus::Active if d.is_metadata() => "fetching metadata".into(),
        DownloadStatus::Active if d.seeder => "seeding".into(),
        DownloadStatus::Active => "downloading".into(),
        DownloadStatus::Waiting => "queued".into(),
        DownloadStatus::Paused => "paused".into(),
        DownloadStatus::Complete => "complete".into(),
        DownloadStatus::Removed => "removed".into(),
        DownloadStatus::Error => match d.error_message.as_deref().filter(|m| !m.is_empty()) {
            Some(m) => format!("error: {m}"),
            None => "error".into(),
        },
        DownloadStatus::Unknown => "unknown".into(),
    }
}

fn size_text(d: &Download) -> String {
    if d.total_length > 0 {
        format!(
            "{} / {}",
            fmt::bytes(d.completed_length),
            fmt::bytes(d.total_length)
        )
    } else if d.completed_length > 0 {
        fmt::bytes(d.completed_length)
    } else {
        "size unknown".into()
    }
}

// ----- header -------------------------------------------------------------------------------

fn logo(t: &Theme) -> Vec<Span<'static>> {
    let bold = Modifier::BOLD;
    vec![
        Span::styled("◆ ", Style::new().fg(t.purple)),
        Span::styled("d", Style::new().fg(t.blue).add_modifier(bold)),
        Span::styled("l", Style::new().fg(t.cyan).add_modifier(bold)),
        Span::styled("u", Style::new().fg(t.teal).add_modifier(bold)),
        Span::styled("x", Style::new().fg(t.magenta).add_modifier(bold)),
    ]
}

fn render_header(buf: &mut Buffer, app: &mut App, area: Rect) {
    let t = &app.theme.clone();
    let inner = if area.height >= 3 {
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(Style::new().fg(t.border));
        let inner = block.inner(area);
        block.render(area, buf);
        Rect {
            x: inner.x + 1,
            width: inner.width.saturating_sub(2),
            ..inner
        }
    } else {
        area
    };
    if inner.is_empty() {
        return;
    }
    let stat = app.state.stat();
    // A set global limit is shown in its pill: "↓ 1.2 MiB/s ≤ 2M".
    let limit = |l: u64| {
        if l > 0 {
            format!(" ≤ {}", limits::format_input(l))
        } else {
            String::new()
        }
    };
    let gl = app.global_limits.unwrap_or_default();
    let down = format!(
        "↓ {}{}",
        fmt::speed(stat.download_speed),
        limit(gl.download)
    );
    let up = format!("↑ {}{}", fmt::speed(stat.upload_speed), limit(gl.upload));
    // Two pills (text + 2 caps each) and a space between them.
    let pills_w = (down.width() + up.width() + 5) as u16;
    let speed_rect = Rect::new(inner.right().saturating_sub(pills_w), inner.y, pills_w, 1);
    let (down_bg, up_bg) = if hovered(app, speed_rect) {
        (t.teal, t.purple)
    } else {
        (t.green, t.magenta)
    };
    let mut right = Vec::new();
    right.extend(pill(down, down_bg, t.bg_dark));
    right.push(Span::raw(" "));
    right.extend(pill(up, up_bg, t.bg_dark));

    let status = if app.state.error.is_some() {
        vec![
            Span::styled("● ", Style::new().fg(t.red)),
            Span::styled("aria2 offline", Style::new().fg(t.red)),
            Span::raw("  "),
        ]
    } else if app.info.aria2_version.is_empty() {
        Vec::new()
    } else {
        vec![
            Span::styled("● ", Style::new().fg(t.green)),
            Span::styled(
                format!("aria2 {} · {}", app.info.aria2_version, app.info.origin),
                Style::new().fg(t.comment),
            ),
            Span::raw("  "),
        ]
    };

    let logo = logo(t);
    let tagline = vec![
        Span::raw("  "),
        Span::styled(
            "Download everything. Beautifully.",
            Style::new().fg(t.comment).add_modifier(Modifier::ITALIC),
        ),
    ];
    let avail = inner.width;
    let mut used = spans_width(&logo) + spans_width(&right);
    let mut right_all = right;
    if used + spans_width(&status) < avail {
        used += spans_width(&status);
        let mut s = status;
        s.extend(right_all);
        right_all = s;
    }
    let mut left = logo;
    if used + spans_width(&tagline) < avail {
        left.extend(tagline);
    }
    put_line(buf, inner.x, inner.y, left, inner.width);
    if spans_width(&right_all) < avail {
        put_right(buf, inner, inner.y, right_all);
        app.hits.speed = Some(speed_rect);
    }
}

// ----- speed graph --------------------------------------------------------------------------

fn render_graph(buf: &mut Buffer, app: &App, area: Rect) {
    let t = &app.theme;
    let history = &app.state.history;
    let peak = history.peak();
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(t.border))
        .title(Line::from(vec![
            Span::raw(" "),
            Span::styled(
                "Speed",
                Style::new().fg(t.fg_dark).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" · last 60s ", Style::new().fg(t.comment)),
        ]))
        .title_top(
            Line::from(vec![
                Span::styled(" ━ ", Style::new().fg(t.green)),
                Span::styled("down ", Style::new().fg(t.fg_dark)),
                Span::styled("━ ", Style::new().fg(t.magenta)),
                Span::styled("up ", Style::new().fg(t.fg_dark)),
                Span::styled(
                    format!("· peak {} ", fmt::speed(peak)),
                    Style::new().fg(t.comment),
                ),
            ])
            .right_aligned(),
        );
    if history.is_empty() {
        let inner = block.inner(area);
        block.render(area, buf);
        centered_message(
            buf,
            inner,
            vec![Line::styled(
                "waiting for data…",
                Style::new().fg(t.comment),
            )],
        );
        return;
    }
    let cap = history.capacity();
    let start = cap - history.len();
    let down: Vec<(f64, f64)> = history
        .iter()
        .enumerate()
        .map(|(i, (d, _))| ((start + i) as f64, d as f64))
        .collect();
    let up: Vec<(f64, f64)> = history
        .iter()
        .enumerate()
        .map(|(i, (_, u))| ((start + i) as f64, u as f64))
        .collect();
    let top = nice_speed_ceiling(peak.max(64 * 1024));
    let label = |v: u64| Span::styled(axis_label(v), Style::new().fg(t.comment));
    let y_labels = if area.width >= 50 && area.height >= 6 {
        vec![
            Span::styled("0", Style::new().fg(t.comment)),
            label(top / 2),
            label(top),
        ]
    } else {
        Vec::new()
    };
    let chart = Chart::new(vec![
        Dataset::default()
            .marker(Marker::Braille)
            .graph_type(GraphType::Line)
            .style(Style::new().fg(t.magenta))
            .data(&up),
        Dataset::default()
            .marker(Marker::Braille)
            .graph_type(GraphType::Line)
            .style(Style::new().fg(t.green))
            .data(&down),
    ])
    .block(block)
    .style(Style::new().bg(t.bg))
    .x_axis(
        Axis::default()
            .bounds([0.0, (cap - 1) as f64])
            .style(Style::new().fg(t.border)),
    )
    .y_axis(
        Axis::default()
            .bounds([0.0, top as f64])
            .labels(y_labels)
            .style(Style::new().fg(t.border)),
    );
    chart.render(area, buf);
}

// ----- tabs ---------------------------------------------------------------------------------

fn render_tabs(buf: &mut Buffer, app: &mut App, area: Rect) {
    let t = app.theme;
    let counts = app.state.counts();
    let mut x = area.x + 1;
    for filter in Filter::ALL {
        let n = counts.get(filter);
        let selected = filter == app.filter;
        let label_w = (filter.label().width() + n.to_string().width() + 3) as u16;
        let rect = Rect::new(x, area.y, label_w, 1);
        let hover = !selected && hovered(app, rect);
        let spans: Vec<Span> = if selected {
            pill(format!("{} {n}", filter.label()), t.blue, t.bg_dark)
        } else if hover {
            pill(format!("{} {n}", filter.label()), t.bg_highlight, t.fg)
        } else {
            let count_color = if filter == Filter::Errors && n > 0 {
                t.red
            } else {
                t.comment
            };
            vec![
                Span::raw(" "),
                Span::styled(filter.label(), Style::new().fg(t.fg_dark)),
                Span::styled(format!(" {n}"), Style::new().fg(count_color)),
                Span::raw(" "),
            ]
        };
        let w = spans_width(&spans);
        if x + w > area.right() {
            break;
        }
        put_line(buf, x, area.y, spans, w);
        app.hits.tabs.push((Rect::new(x, area.y, w, 1), filter));
        x += w + 1;
    }
}

// ----- download list ------------------------------------------------------------------------

fn empty_text(filter: Filter) -> &'static str {
    match filter {
        Filter::Active => "No active downloads",
        Filter::Waiting => "Nothing waiting",
        Filter::Done => "Nothing finished yet",
        Filter::Errors => "No errors",
    }
}

fn render_list(buf: &mut Buffer, app: &mut App, area: Rect) {
    app.hits.list = area;
    if area.width < 4 || area.height < 3 {
        return;
    }
    let t = app.theme;
    let len = app.visible().len();
    let counter = if len > 0 {
        format!(" {}/{len} ", app.selected_index + 1)
    } else {
        String::new()
    };
    let block = panel(&t, "Downloads", true)
        .title_top(Line::from(Span::styled(counter, Style::new().fg(t.comment))).right_aligned());
    let inner = block.inner(area);
    block.render(area, buf);
    let inner = Rect {
        x: inner.x + 1,
        width: inner.width.saturating_sub(2),
        ..inner
    };

    if !app.state.loaded {
        let msg = match &app.state.error {
            Some(e) => Line::styled(e.clone(), Style::new().fg(t.red)),
            None => Line::styled("Connecting to aria2…", Style::new().fg(t.comment)),
        };
        centered_message(buf, inner, vec![msg]);
        return;
    }
    if len == 0 {
        let mut lines = vec![Line::styled(
            empty_text(app.filter),
            Style::new().fg(t.fg_dark),
        )];
        if app.filter == Filter::Active {
            lines.push(Line::styled(
                "Press a or click + Add to start a download",
                Style::new().fg(t.comment),
            ));
        }
        centered_message(buf, inner, lines);
        return;
    }

    let row = CARD_HEIGHT + CARD_GAP;
    app.page_size = usize::from((inner.height + CARD_GAP) / row).max(1);
    app.ensure_visible(len);
    let offset = app.offset;
    let page = app.page_size;
    let selected = app.selected.clone();
    let list = app.visible();
    let mut cards = Vec::new();
    for (slot, d) in list.iter().skip(offset).take(page).enumerate() {
        let y = inner.y + slot as u16 * row;
        let rect = Rect::new(inner.x, y, inner.width, CARD_HEIGHT).intersection(inner);
        let is_selected = selected.as_deref() == Some(d.gid.as_str());
        let hover = !is_selected && hovered(app, rect);
        render_card(buf, rect, d, app.job(&d.gid), is_selected, hover, &t);
        cards.push((rect, d.gid.clone()));
    }
    app.hits.cards = cards;

    if len > page {
        let track = Rect::new(area.right() - 1, inner.y, 1, inner.height);
        let mut state = ScrollbarState::new(len.saturating_sub(page) + 1)
            .position(offset)
            .viewport_content_length(1);
        Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .symbols(symbols::scrollbar::Set {
                track: "│",
                thumb: "┃",
                begin: "",
                end: "",
            })
            .begin_symbol(None)
            .end_symbol(None)
            .track_style(Style::new().fg(t.border))
            .thumb_style(Style::new().fg(if app.dragging || hovered(app, track) {
                t.cyan
            } else {
                t.blue
            }))
            .render(track, buf, &mut state);
        app.hits.scrollbar = Some(track);
    }
}

fn render_card(
    buf: &mut Buffer,
    area: Rect,
    d: &Download,
    job: Option<&Job>,
    selected: bool,
    hover: bool,
    t: &Theme,
) {
    if area.is_empty() {
        return;
    }
    if hover {
        buf.set_style(area, Style::new().bg(t.bg_hover));
    }
    if selected {
        buf.set_style(area, Style::new().bg(t.bg_highlight));
        for y in area.top()..area.bottom() {
            if let Some(cell) = buf.cell_mut(Position::new(area.x, y)) {
                cell.set_symbol("▎").set_fg(t.blue);
            }
        }
    }
    let content = Rect {
        x: area.x + 2,
        width: area.width.saturating_sub(3),
        ..area
    };
    if content.width == 0 {
        return;
    }
    let color = t.progress_color(d);
    let w = usize::from(content.width);

    // Line 1: name, tags, percent.
    let percent = fmt::percent(d.progress());
    let mut tags: Vec<Span> = Vec::new();
    if d.is_torrent() {
        tags.push(Span::styled(" torrent", Style::new().fg(t.purple)));
    }
    if d.is_metadata() {
        tags.push(Span::styled(" magnet", Style::new().fg(t.purple)));
    }
    match job.map(|j| j.kind) {
        Some(JobKind::Video) => tags.push(Span::styled(" video", Style::new().fg(t.magenta))),
        Some(JobKind::Install(_)) => tags.push(Span::styled(" tool", Style::new().fg(t.cyan))),
        None => {}
    }
    let right_w = percent.width() + 1;
    let tags_w = usize::from(spans_width(&tags));
    let name_max = w.saturating_sub(right_w + tags_w);
    let name_style = Style::new().fg(t.fg).add_modifier(Modifier::BOLD);
    let mut line1 = vec![Span::styled(truncate(&d.name(), name_max), name_style)];
    if name_max > 8 {
        line1.extend(tags);
    }
    put_line(buf, content.x, content.y, line1, content.width);
    put_right(
        buf,
        content,
        content.y,
        vec![Span::styled(
            percent,
            Style::new().fg(color).add_modifier(Modifier::BOLD),
        )],
    );

    // Line 2: progress bar.
    if content.height >= 2 {
        put_line(
            buf,
            content.x,
            content.y + 1,
            progress_bar(d.progress(), content.width, color, t.gutter),
            content.width,
        );
    }

    // Line 3: size · speed · ETA, status on the right.
    if content.height >= 3 {
        let dim = Style::new().fg(t.comment);
        let mut meta = vec![Span::styled(size_text(d), Style::new().fg(t.fg_dark))];
        if d.status == DownloadStatus::Active {
            meta.push(Span::styled(" · ", dim));
            meta.push(Span::styled(
                format!("↓ {}", fmt::speed(d.download_speed)),
                Style::new().fg(t.green),
            ));
            if d.upload_speed > 0 {
                meta.push(Span::styled(" · ", dim));
                meta.push(Span::styled(
                    format!("↑ {}", fmt::speed(d.upload_speed)),
                    Style::new().fg(t.magenta),
                ));
            }
            if !d.seeder {
                meta.push(Span::styled(" · ETA ", dim));
                meta.push(Span::styled(fmt::eta(d.eta()), Style::new().fg(t.fg_dark)));
            }
        }
        let meta_w = usize::from(spans_width(&meta));
        let status_max = w.saturating_sub(meta_w + 2);
        let y = content.y + 2;
        put_line(buf, content.x, y, meta, content.width);
        if status_max >= 6 {
            // Jobs explain what yt-dlp is doing ("merging", "item 3/23").
            let text = job.and_then(Job::note).unwrap_or_else(|| status_text(d));
            let status = truncate(&text, status_max);
            put_right(
                buf,
                content,
                y,
                vec![Span::styled(status, Style::new().fg(t.status_color(d)))],
            );
        }
    }
}

// ----- details ------------------------------------------------------------------------------

fn render_details(buf: &mut Buffer, app: &App, area: Rect) {
    let t = &app.theme;
    let block = panel(t, "Details", false);
    let inner = block.inner(area);
    block.render(area, buf);
    let inner = Rect {
        x: inner.x + 1,
        width: inner.width.saturating_sub(2),
        ..inner
    };
    let Some(d) = app.selected_download() else {
        centered_message(
            buf,
            inner,
            vec![Line::styled(
                "Select a download to see its details",
                Style::new().fg(t.comment),
            )],
        );
        return;
    };
    let w = usize::from(inner.width);
    let label_w = 12;
    let value_w = w.saturating_sub(label_w);
    let label = |s: &str| Span::styled(format!("{s:<label_w$}"), Style::new().fg(t.comment));
    let value = |s: &str| Span::styled(truncate(s, value_w), Style::new().fg(t.fg));
    let row = |l: &str, v: &str| Line::from(vec![label(l), value(v)]);

    let mut lines = vec![
        Line::styled(
            truncate(&d.name(), w),
            Style::new().fg(t.fg).add_modifier(Modifier::BOLD),
        ),
        Line::from(vec![
            label("Status"),
            Span::styled(
                truncate(&status_text(d), value_w),
                Style::new().fg(t.status_color(d)),
            ),
        ]),
        row(
            "Progress",
            &format!("{} · {}", fmt::percent(d.progress()), size_text(d)),
        ),
        row(
            "Speed",
            &format!(
                "↓ {}  ↑ {}",
                fmt::speed(d.download_speed),
                fmt::speed(d.upload_speed)
            ),
        ),
    ];
    if d.status == DownloadStatus::Active && !d.seeder {
        lines.push(row("ETA", &fmt::eta(d.eta())));
    }
    match app.job(&d.gid).map(|j| j.kind) {
        Some(JobKind::Video) => lines.push(row("Via", "yt-dlp (with aria2c)")),
        Some(JobKind::Install(_)) => lines.push(row("Via", "dlux tool installer")),
        None => lines.push(row("Connections", &d.connections.to_string())),
    }
    if d.is_torrent() {
        let seeds = if d.seeder {
            format!("{} (seeding)", d.num_seeders)
        } else {
            d.num_seeders.to_string()
        };
        lines.push(row("Seeds", &seeds));
        if d.upload_length > 0 {
            lines.push(row("Uploaded", &fmt::bytes(d.upload_length)));
        }
    }
    lines.push(row("Directory", &d.dir));
    lines.push(row("GID", &d.gid));
    if let Some(hash) = &d.info_hash {
        lines.push(row("Info hash", hash));
    }
    if let Some(uri) = d.first_uri() {
        lines.push(row("URL", uri));
    }

    let files: Vec<_> = d.files.iter().filter(|f| !f.path.is_empty()).collect();
    let room = usize::from(inner.height).saturating_sub(lines.len() + 2);
    if !files.is_empty() && room > 0 {
        lines.push(Line::raw(""));
        lines.push(Line::styled(
            format!("Files ({})", files.len()),
            Style::new().fg(t.blue).add_modifier(Modifier::BOLD),
        ));
        let show = if files.len() > room {
            room.saturating_sub(1)
        } else {
            files.len()
        };
        for f in files.iter().take(show) {
            let pct = if f.length > 0 {
                f.completed_length as f64 / f.length as f64
            } else {
                0.0
            };
            let size = fmt::bytes(f.length);
            let pct_s = format!("{:>4.0}% ", pct * 100.0);
            let name_w = w.saturating_sub(pct_s.width() + size.width() + 1);
            let name_style = if f.selected {
                Style::new().fg(t.fg_dark)
            } else {
                Style::new()
                    .fg(t.comment)
                    .add_modifier(Modifier::CROSSED_OUT)
            };
            let name = truncate(f.file_name(), name_w);
            let pad = name_w.saturating_sub(name.width());
            lines.push(Line::from(vec![
                Span::styled(pct_s, Style::new().fg(t.comment)),
                Span::styled(name, name_style),
                Span::raw(" ".repeat(pad + 1)),
                Span::styled(size, Style::new().fg(t.comment)),
            ]));
        }
        if show < files.len() {
            lines.push(Line::styled(
                format!("… and {} more", files.len() - show),
                Style::new().fg(t.comment),
            ));
        }
    }
    Paragraph::new(lines).render(inner, buf);
}

// ----- toolbar & hints ----------------------------------------------------------------------

fn render_toolbar(buf: &mut Buffer, app: &mut App, area: Rect) {
    let t = app.theme;
    let mut x = area.x + 1;
    for cmd in Command::TOOLBAR {
        let enabled = app.can(cmd);
        let w = cmd.label().width() as u16 + 4;
        let rect = Rect::new(x, area.y, w, 1);
        let hover = enabled && hovered(app, rect);
        let (bg, fg) = match (cmd, enabled, hover) {
            (Command::Add, _, false) => (t.blue, t.bg_dark),
            (Command::Add, _, true) => (t.cyan, t.bg_dark),
            (_, true, false) => (t.bg_highlight, t.fg),
            (_, true, true) => (t.border, t.fg),
            (_, false, _) => (t.bg_highlight, t.comment),
        };
        let spans = pill(format!(" {} ", cmd.label()), bg, fg);
        if x + w > area.right() {
            break;
        }
        put_line(buf, x, area.y, spans, w);
        app.hits.buttons.push((rect, cmd));
        x += w + 1;
    }
    let room = usize::from(area.right().saturating_sub(x + 2));
    if let Some(toast) = app.current_toast(std::time::Instant::now()) {
        let (icon, color) = if toast.error {
            ("✗ ", t.red)
        } else {
            ("✓ ", t.green)
        };
        let text = truncate(&toast.text, room.saturating_sub(icon.width() + 1));
        if room > 8 {
            put_right(
                buf,
                area,
                area.y,
                vec![
                    Span::styled(icon, Style::new().fg(color)),
                    Span::styled(format!("{text} "), Style::new().fg(color)),
                ],
            );
        }
        return;
    }
    let version = format!("dlux {} ", env!("CARGO_PKG_VERSION"));
    if version.width() <= room {
        put_right(
            buf,
            area,
            area.y,
            vec![Span::styled(version, Style::new().fg(t.comment))],
        );
    }
}

/// Key hints; the ones with a command are clickable.
const HINTS: &[(&str, &str, Option<Command>)] = &[
    ("j/k", "move", None),
    ("tab", "filter", None),
    ("enter", "details", Some(Command::Details)),
    ("a", "add", Some(Command::Add)),
    ("p", "pause", Some(Command::Pause)),
    ("r", "resume", Some(Command::Resume)),
    ("d", "remove", Some(Command::Remove)),
    ("m", "menu", Some(Command::Menu)),
    ("?", "help", Some(Command::Help)),
    ("q", "quit", Some(Command::Quit)),
];

fn render_hints(buf: &mut Buffer, app: &mut App, area: Rect) {
    let t = app.theme;
    let mut x = area.x + 1;
    for (key, desc, cmd) in HINTS {
        let w = (key.width() + desc.width() + 1) as u16;
        if x + w > area.right() {
            break;
        }
        let rect = Rect::new(x, area.y, w, 1);
        let hover = cmd.is_some() && hovered(app, rect);
        let desc_style = if hover {
            Style::new().fg(t.fg).add_modifier(Modifier::UNDERLINED)
        } else {
            Style::new().fg(t.comment)
        };
        let item = vec![
            Span::styled(*key, Style::new().fg(t.blue).add_modifier(Modifier::BOLD)),
            Span::styled(format!(" {desc}"), desc_style),
        ];
        put_line(buf, x, area.y, item, w);
        if let Some(cmd) = cmd {
            app.hits.hints.push((rect, *cmd));
        }
        x += w + 2;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::app::ConnectionInfo;
    use dlux_core::limits::Limits;
    use dlux_core::rpc::{BitTorrent, BitTorrentInfo, File, GlobalStat, Snapshot, Uri};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn sample_app() -> App {
        let mut app = App::new(
            Theme::TOKYO_NIGHT,
            ConnectionInfo {
                aria2_version: "1.37.0".into(),
                origin: "started by dlux".into(),
            },
        );
        let http = Download {
            gid: "2089b05ecca3d829".into(),
            status: DownloadStatus::Active,
            total_length: 2_000_000_000,
            completed_length: 900_000_000,
            download_speed: 12 * 1024 * 1024,
            connections: 8,
            dir: "/home/user/Downloads".into(),
            files: vec![File {
                index: 1,
                path: "/home/user/Downloads/ubuntu-24.04-desktop-amd64.iso".into(),
                length: 2_000_000_000,
                completed_length: 900_000_000,
                selected: true,
                uris: vec![Uri {
                    uri: "https://releases.example.com/ubuntu.iso".into(),
                    status: "used".into(),
                }],
            }],
            ..Default::default()
        };
        let torrent = Download {
            gid: "aaaabbbbccccdddd".into(),
            status: DownloadStatus::Active,
            total_length: 4_000_000_000,
            completed_length: 100_000_000,
            download_speed: 30 * 1024,
            upload_speed: 4 * 1024,
            num_seeders: 9,
            bittorrent: Some(BitTorrent {
                info: Some(BitTorrentInfo {
                    name: "debian-12-dvd".into(),
                }),
                ..Default::default()
            }),
            info_hash: Some("0123456789abcdef0123456789abcdef01234567".into()),
            ..Default::default()
        };
        let paused = Download {
            gid: "1111222233334444".into(),
            status: DownloadStatus::Paused,
            total_length: 10_000_000,
            completed_length: 2_500_000,
            ..Default::default()
        };
        let failed = Download {
            gid: "5555666677778888".into(),
            status: DownloadStatus::Error,
            error_message: Some("Resource not found".into()),
            ..Default::default()
        };
        for i in 0..20u64 {
            app.state.history.push(i * 512 * 1024, i * 1024);
        }
        app.state.apply(Snapshot {
            stat: GlobalStat {
                download_speed: 12 * 1024 * 1024 + 30 * 1024,
                upload_speed: 4096,
                num_active: 2,
                ..Default::default()
            },
            active: vec![http, torrent],
            waiting: vec![paused],
            stopped: vec![failed],
        });
        app
    }

    fn draw(app: &mut App, w: u16, h: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        terminal.draw(|f| render(f, app)).unwrap();
        let buf = terminal.backend().buffer();
        (0..h)
            .map(|y| {
                (0..w)
                    .map(|x| buf[(x, y)].symbol().to_owned())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn renders_wide_layout() {
        let mut app = sample_app();
        let screen = draw(&mut app, 140, 45);
        for needle in [
            "dlux",
            "Download everything. Beautifully.",
            "↓ 12.0 MiB/s",
            "Speed",
            "Active 2",
            "Waiting 1",
            "Errors 1",
            "ubuntu-24.04-desktop-amd64.iso",
            "debian-12-dvd",
            "torrent",
            "Details",
            "2089b05ecca3d829",
            "Connections",
            "+ Add",
            "Resume",
            "q quit",
            "╭",
            "━",
        ] {
            assert!(screen.contains(needle), "missing {needle:?}:\n{screen}");
        }
        assert_eq!(app.hits.tabs.len(), 4);
        assert_eq!(app.hits.cards.len(), 2);
        assert_eq!(
            app.hits.buttons.len(),
            7,
            "all toolbar buttons fit at 140 columns"
        );
        assert!(app.hits.hints.len() >= 6, "hints are clickable");
    }

    #[test]
    fn renders_standard_80x24() {
        let mut app = sample_app();
        let screen = draw(&mut app, 80, 24);
        assert!(screen.contains("ubuntu-24.04"));
        assert!(screen.contains("Active 2"));
        assert!(
            !screen.contains("Details"),
            "details hidden on narrow screens"
        );
        app.details = true;
        let screen = draw(&mut app, 80, 24);
        assert!(screen.contains("Details"), "enter shows details:\n{screen}");
    }

    #[test]
    fn renders_tiny_terminals_without_panicking() {
        for (w, h) in [(20, 6), (40, 10), (1, 1), (0, 0), (60, 15), (200, 60)] {
            let mut app = sample_app();
            draw(&mut app, w, h);
            app.details = true;
            draw(&mut app, w, h);
        }
    }

    #[test]
    fn empty_and_loading_states() {
        let mut app = App::new(Theme::TOKYO_NIGHT, ConnectionInfo::default());
        assert!(draw(&mut app, 80, 24).contains("Connecting to aria2"));
        app.state.apply(Snapshot::default());
        assert!(draw(&mut app, 80, 24).contains("No active downloads"));
        app.state.apply_error("cannot reach aria2".into());
        assert!(draw(&mut app, 120, 30).contains("aria2 offline"));
    }

    #[test]
    fn scrollbar_appears_when_list_overflows() {
        let mut app = sample_app();
        let many: Vec<Download> = (0..30)
            .map(|i| Download {
                gid: format!("{i:016x}"),
                status: DownloadStatus::Active,
                ..Default::default()
            })
            .collect();
        app.state.apply(Snapshot {
            active: many,
            ..Default::default()
        });
        app.sync_selection();
        let screen = draw(&mut app, 100, 30);
        assert!(app.hits.scrollbar.is_some());
        assert!(screen.contains('┃'));
        assert!(app.page_size < 30);
        app.select_last();
        draw(&mut app, 100, 30);
        assert!(
            app.hits
                .cards
                .iter()
                .any(|(_, g)| *g == format!("{:016x}", 29))
        );
    }

    #[test]
    fn layout_is_responsive() {
        let wide = compute_layout(Rect::new(0, 0, 150, 50), false);
        assert!(wide.graph.is_some() && wide.details.is_some() && wide.hints.is_some());
        assert_eq!(
            wide.list.y,
            wide.details.unwrap().y,
            "details beside the list"
        );

        let narrow = compute_layout(Rect::new(0, 0, 80, 24), false);
        assert!(narrow.graph.is_some());
        assert!(narrow.details.is_none());

        let short = compute_layout(Rect::new(0, 0, 80, 18), false);
        assert!(short.graph.is_none(), "graph is dropped on short terminals");
        assert_eq!(short.header.height, 3);

        let tiny = compute_layout(Rect::new(0, 0, 50, 12), true);
        assert_eq!(tiny.header.height, 1);
        assert!(tiny.hints.is_none());
        assert_eq!(tiny.list.area(), 0, "details replace the list");
    }

    #[test]
    fn progress_bar_is_smooth_and_exact_width() {
        let w = |spans: Vec<Span>| spans_width(&spans);
        let t = Theme::TOKYO_NIGHT;
        for ratio in [0.0, 0.01, 0.25, 0.5, 0.51, 0.99, 1.0, 2.0, -1.0] {
            assert_eq!(w(progress_bar(ratio, 37, t.green, t.gutter)), 37, "{ratio}");
        }
        let text: String = progress_bar(0.25, 10, t.green, t.gutter)
            .iter()
            .map(|s| s.content.to_string())
            .collect();
        assert_eq!(text, "━━╸───────");
        assert!(w(progress_bar(0.5, 0, t.green, t.gutter)) == 0);
    }

    #[test]
    fn helpers() {
        assert_eq!(truncate("hello", 10), "hello");
        assert_eq!(truncate("hello world", 6), "hello…");
        assert_eq!(truncate("hello", 0), "");
        assert_eq!(nice_ceiling(0), 1);
        assert_eq!(nice_ceiling(3), 3);
        assert_eq!(nice_ceiling(4), 5);
        assert_eq!(nice_ceiling(11), 15);
        assert_eq!(nice_ceiling(150), 150);
        assert_eq!(nice_ceiling(151), 200);
        assert_eq!(nice_ceiling(501), 750);
        assert_eq!(nice_ceiling(751), 1000);
        let mib = 1024 * 1024;
        assert_eq!(nice_speed_ceiling(9 * mib + mib / 50), 10 * mib);
        assert_eq!(nice_speed_ceiling(11 * mib), 15 * mib);
        assert_eq!(nice_speed_ceiling(300 * 1024), 300 * 1024);
        assert_eq!(nice_speed_ceiling(1500 * 1024), 2 * mib);
        assert_eq!(nice_speed_ceiling(0), 1);
        assert_eq!(axis_label(10 * mib), "10 MiB/s");
        assert_eq!(axis_label(5 * mib), "5 MiB/s");
        assert_eq!(axis_label(1536 * 1024), "1.5 MiB/s");
        assert_eq!(axis_label(0), "0 B/s");
    }

    #[test]
    fn renders_add_and_limit_dialogs() {
        let mut app = sample_app();
        app.default_dir = "/home/user/Downloads".into();
        app.open_add("https://example.com/a.iso\nmagnet:?xt=urn:btih:abc");
        let screen = draw(&mut app, 100, 30);
        for needle in [
            "Add downloads",
            "https://example.com/a.iso",
            "✓ 1 URL, 1 magnet",
            "Save to",
            "/home/user/Downloads",
            "[ ] Start paused",
            " Add ",
            " Cancel ",
        ] {
            assert!(screen.contains(needle), "missing {needle:?}:\n{screen}");
        }
        assert_eq!(app.hits.fields.len(), 4, "URLs, dir and two checkboxes");
        assert_eq!(app.hits.dialog_buttons.len(), 2);

        app.dialog = None;
        app.global_limits = Some(Limits {
            download: 2 << 20,
            upload: 0,
        });
        app.run(Command::GlobalLimit);
        let screen = draw(&mut app, 100, 30);
        for needle in [
            "Speed limit · all downloads",
            "2M",
            "= 2.00 MiB/s",
            "= unlimited",
            "≤ 2M",
        ] {
            assert!(screen.contains(needle), "missing {needle:?}:\n{screen}");
        }
        // Small terminals don't panic with a form open.
        for (w, h) in [(30, 10), (50, 15), (80, 24)] {
            draw(&mut app, w, h);
            app.open_add("x");
            draw(&mut app, w, h);
        }
    }

    #[test]
    fn renders_video_picker_and_video_cards() {
        use crate::tui::app::Dialog;
        use crate::tui::forms::VideoForm;
        let mut app = sample_app();
        let mut form = VideoForm::new("https://youtu.be/x".into(), "/home/user/Videos".into());
        let screen = {
            app.dialog = Some(Dialog::Video(form.clone()));
            draw(&mut app, 100, 30)
        };
        assert!(screen.contains("Fetching formats with yt-dlp"), "{screen}");
        assert!(screen.contains(" Update yt-dlp "));
        form.info = Some(
            dlux_core::ytdlp::parse_probe(
                "https://youtu.be/x",
                br#"{"title":"Me at the zoo","extractor_key":"Youtube","duration":19,
                     "formats":[{"format_id":"18","ext":"mp4","height":360,"vcodec":"avc1","acodec":"mp4a","filesize":800000}]}"#,
            )
            .unwrap(),
        );
        app.dialog = Some(Dialog::Video(form));
        let screen = draw(&mut app, 100, 30);
        for needle in [
            "Download video",
            "Me at the zoo",
            "Youtube · 0:19",
            "● Best quality (360p)",
            "mp4 · ~781 KiB",
            "/home/user/Videos",
            " Download ",
        ] {
            assert!(screen.contains(needle), "missing {needle:?}:\n{screen}");
        }
        assert_eq!(app.hits.choices.len(), 1);
        for (w, h) in [(30, 8), (60, 14)] {
            draw(&mut app, w, h);
        }

        // A running yt-dlp job is listed with a video tag and its stage.
        app.dialog = None;
        let mut job_download = Download {
            gid: "dlux-1".into(),
            status: DownloadStatus::Active,
            ..Default::default()
        };
        job_download.files = vec![File {
            path: "/home/user/Videos/Me at the zoo".into(),
            ..Default::default()
        }];
        app.state.set_local(vec![job_download]);
        let screen = draw(&mut app, 140, 45);
        assert!(screen.contains("Me at the zoo"), "{screen}");
    }
}
