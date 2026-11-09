//! The form dialogs: "Add downloads", "Speed limit" and the video format picker. State and
//! validation live here along with their rendering; key and mouse routing is in
//! [`super::input`].

use super::app::{App, DialogButton};
use super::overlay::{Tone, centered, dialog_button, popup_block};
use super::textfield::TextField;
use super::theme::Theme;
use super::ui::truncate;
use dlux_core::limits::{self, Limits, Target};
use dlux_core::sources::{self, AddOptions, Source};
use dlux_core::ytdlp::{self, DownloadSpec, VideoInfo};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Widget};
use unicode_width::UnicodeWidthStr;

/// A focusable control inside a form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Urls,
    Dir,
    Paused,
    Video,
    Download,
    Upload,
}

/// What the input code needs from a form: its fields, focus, text fields and error line.
pub trait Form {
    fn fields(&self) -> &'static [Field];
    fn focus(&mut self) -> &mut Field;
    fn text_field(&mut self, field: Field) -> Option<&mut TextField>;
    fn error(&mut self) -> &mut Option<String>;
    /// Toggle a checkbox field; returns false if `field` is not one.
    fn toggle(&mut self, _field: Field) -> bool {
        false
    }

    /// Move the focus by `delta` fields, wrapping around.
    fn cycle(&mut self, delta: isize) {
        let fields = self.fields();
        let n = fields.len() as isize;
        let i = fields.iter().position(|f| f == self.focus()).unwrap_or(0) as isize;
        *self.focus() = fields[(i + delta).rem_euclid(n) as usize];
    }
}

impl Form for AddForm {
    fn fields(&self) -> &'static [Field] {
        &Self::FIELDS
    }
    fn focus(&mut self) -> &mut Field {
        &mut self.focus
    }
    fn text_field(&mut self, field: Field) -> Option<&mut TextField> {
        self.field_mut(field)
    }
    fn error(&mut self) -> &mut Option<String> {
        &mut self.error
    }
    fn toggle(&mut self, field: Field) -> bool {
        match field {
            Field::Paused => self.paused = !self.paused,
            Field::Video => {
                self.video = Some(!self.video_mode());
            }
            _ => return false,
        }
        true
    }
}

impl Form for LimitsForm {
    fn fields(&self) -> &'static [Field] {
        &Self::FIELDS
    }
    fn focus(&mut self) -> &mut Field {
        &mut self.focus
    }
    fn text_field(&mut self, field: Field) -> Option<&mut TextField> {
        self.field_mut(field)
    }
    fn error(&mut self) -> &mut Option<String> {
        &mut self.error
    }
}

// ----- Add ------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddForm {
    pub urls: TextField,
    pub dir: TextField,
    pub paused: bool,
    /// Video mode chosen by the user; `None` = decided automatically from the input.
    pub video: Option<bool>,
    pub focus: Field,
    pub error: Option<String>,
}

/// What a valid Add form asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AddRequest {
    /// Hand the sources to aria2.
    Files(Vec<Source>, AddOptions),
    /// Fetch the page's formats with yt-dlp and show the picker.
    Video { url: String, dir: String },
}

impl AddForm {
    pub const FIELDS: [Field; 4] = [Field::Urls, Field::Dir, Field::Paused, Field::Video];

    pub fn new(default_dir: &str, text: &str) -> Self {
        Self {
            urls: TextField::with_text(true, text.trim_end()),
            dir: TextField::with_text(false, default_dir),
            paused: false,
            video: None,
            focus: Field::Urls,
            error: None,
        }
    }

    /// The single URL entered, if that is all there is.
    fn single_url(&self) -> Option<String> {
        let parsed = sources::parse(&self.urls.text());
        match (parsed.sources.as_slice(), parsed.invalid.is_empty()) {
            ([Source::Url(u)], true) => Some(u.clone()),
            _ => None,
        }
    }

    /// Whether Enter fetches formats with yt-dlp instead of adding to aria2: the user's choice,
    /// or else on for a single URL that looks like a video page rather than a file.
    pub fn video_mode(&self) -> bool {
        self.video.unwrap_or_else(|| {
            self.single_url()
                .is_some_and(|u| ytdlp::looks_like_video(&u))
        })
    }

    pub fn field_mut(&mut self, field: Field) -> Option<&mut TextField> {
        match field {
            Field::Urls => Some(&mut self.urls),
            Field::Dir => Some(&mut self.dir),
            _ => None,
        }
    }

    /// Validate the form: everything entered must be addable, and something must be entered.
    pub fn submit(&self) -> Result<AddRequest, String> {
        let dir = self.dir.text();
        if self.video_mode() {
            return match self.single_url() {
                Some(url) => Ok(AddRequest::Video { url, dir }),
                None => Err("Video mode takes a single http(s) URL".into()),
            };
        }
        let parsed = sources::parse(&self.urls.text());
        if let Some(bad) = parsed.invalid.first() {
            let more = match parsed.invalid.len() {
                1 => String::new(),
                n => format!(" (+{} more)", n - 1),
            };
            return Err(format!("{}: {}{more}", bad.input, bad.reason));
        }
        if parsed.sources.is_empty() {
            return Err("Enter a URL, magnet link or .torrent/.metalink path".into());
        }
        let options = AddOptions {
            dir: (!dir.trim().is_empty()).then_some(dir),
            paused: self.paused,
        };
        Ok(AddRequest::Files(parsed.sources, options))
    }
}

// ----- Video format picker --------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoForm {
    pub url: String,
    pub dir: String,
    /// The probe result, once it arrives.
    pub info: Option<VideoInfo>,
    pub error: Option<String>,
    /// Highlighted choice.
    pub cursor: usize,
    /// First visible choice (the list scrolls on small terminals).
    pub scroll: usize,
}

impl VideoForm {
    pub fn new(url: String, dir: String) -> Self {
        Self {
            url,
            dir,
            info: None,
            error: None,
            cursor: 0,
            scroll: 0,
        }
    }

    pub fn loading(&self) -> bool {
        self.info.is_none() && self.error.is_none()
    }

    /// yt-dlp did not recognise the page: offer a plain download instead.
    pub fn unsupported(&self) -> bool {
        self.error
            .as_deref()
            .is_some_and(|e| e.contains("doesn't recognise"))
    }

    pub fn move_cursor(&mut self, delta: isize) {
        let n = self.info.as_ref().map_or(0, |i| i.choices.len());
        if n > 0 {
            self.cursor = self.cursor.saturating_add_signed(delta).min(n - 1);
        }
    }

    /// What to download for the highlighted choice.
    pub fn spec(&self) -> Option<DownloadSpec> {
        let info = self.info.as_ref()?;
        let choice = info.choices.get(self.cursor)?;
        let dir = if self.dir.trim().is_empty() {
            dlux_core::paths::default_download_dir()
        } else {
            dlux_core::paths::expand_tilde(self.dir.trim())
        };
        Some(DownloadSpec {
            url: self.url.clone(),
            title: info.title.clone(),
            selector: choice.selector.clone(),
            dir,
            playlist: info.playlist,
            size: choice.size,
            needs_merge: choice.needs_merge,
        })
    }
}

// ----- Speed limit ----------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LimitsForm {
    pub target: Target,
    /// "All downloads" or the download's name.
    pub subject: String,
    pub download: TextField,
    pub upload: TextField,
    pub focus: Field,
    pub error: Option<String>,
    /// The current values arrived from aria2.
    pub loaded: bool,
}

impl LimitsForm {
    pub const FIELDS: [Field; 2] = [Field::Download, Field::Upload];

    pub fn new(target: Target, subject: String) -> Self {
        Self {
            target,
            subject,
            download: TextField::new(false),
            upload: TextField::new(false),
            focus: Field::Download,
            error: None,
            loaded: false,
        }
    }

    pub fn field_mut(&mut self, field: Field) -> Option<&mut TextField> {
        match field {
            Field::Download => Some(&mut self.download),
            Field::Upload => Some(&mut self.upload),
            _ => None,
        }
    }

    /// Fill in the current limits, unless the user already started typing.
    pub fn fill(&mut self, current: Limits) {
        if !self.download.edited {
            self.download
                .set_text(&limits::format_input(current.download));
        }
        if !self.upload.edited {
            self.upload.set_text(&limits::format_input(current.upload));
        }
        self.loaded = true;
    }

    pub fn submit(&self) -> Result<Limits, String> {
        let download =
            limits::parse(&self.download.text()).map_err(|e| format!("Download: {e}"))?;
        let upload = limits::parse(&self.upload.text()).map_err(|e| format!("Upload: {e}"))?;
        Ok(Limits { download, upload })
    }
}

// ----- rendering ------------------------------------------------------------------------------

/// A bordered input box; returns the inner area where the text goes.
fn input_box(buf: &mut Buffer, t: &Theme, area: Rect, focused: bool) -> Rect {
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(if focused { t.border_focus } else { t.border }))
        .style(Style::new().bg(t.bg));
    let inner = block.inner(area);
    block.render(area, buf);
    Rect {
        x: inner.x + 1,
        width: inner.width.saturating_sub(2),
        ..inner
    }
}

fn label(text: &str, t: &Theme, focused: bool) -> Line<'static> {
    Line::styled(
        text.to_owned(),
        Style::new()
            .fg(if focused { t.blue } else { t.fg_dark })
            .add_modifier(Modifier::BOLD),
    )
}

fn error_or(t: &Theme, error: Option<&str>, ok: Line<'static>, w: usize) -> Line<'static> {
    match error {
        Some(e) => Line::from(vec![
            Span::styled("✗ ", Style::new().fg(t.red)),
            Span::styled(truncate(e, w.saturating_sub(2)), Style::new().fg(t.red)),
        ]),
        None => ok,
    }
}

/// Draw the dialog's Confirm/Cancel buttons right-aligned on row `y`, with a key hint on the
/// left if there is room.
fn buttons(buf: &mut Buffer, app: &mut App, body: Rect, y: u16, confirm: &str, hint: &str) {
    let t = app.theme;
    let total = (confirm.width() + 2 + 1 + " Cancel ".width()) as u16;
    let mut x = body.right().saturating_sub(total);
    let room = usize::from(x.saturating_sub(body.x + 1));
    if hint.width() <= room {
        buf.set_line(
            body.x,
            y,
            &Line::styled(hint, Style::new().fg(t.comment)),
            room as u16,
        );
    }
    x += dialog_button(
        buf,
        app,
        x,
        y,
        confirm,
        DialogButton::Confirm,
        Tone::Primary,
    ) + 1;
    dialog_button(buf, app, x, y, "Cancel", DialogButton::Cancel, Tone::Normal);
}

pub fn render_add(buf: &mut Buffer, app: &mut App, form: &mut AddForm) {
    let t = app.theme;
    let screen = buf.area;
    // Fixed rows: borders 2, label 1, url box borders 2, status 1, label 1, dir box 3,
    // checkbox 1, gap 1, buttons 1.
    let fixed = 14;
    let url_rows = screen.height.saturating_sub(fixed + 2).clamp(1, 6);
    let area = centered(78, fixed + url_rows, screen);
    if area.width < 30 || area.height < fixed + 1 {
        return;
    }
    Clear.render(area, buf);
    let block = popup_block(&t, "Add downloads");
    let inner = block.inner(area);
    block.render(area, buf);
    app.hits.dialog = Some(area);
    let body = Rect {
        x: inner.x + 1,
        width: inner.width.saturating_sub(2),
        ..inner
    };
    let w = usize::from(body.width);
    let mut y = body.y;

    let focus = form.focus;
    buf.set_line(
        body.x,
        y,
        &label(
            "URLs, magnet links or .torrent / .metalink files",
            &t,
            focus == Field::Urls,
        ),
        body.width,
    );
    y += 1;
    let urls_box = Rect::new(body.x, y, body.width, url_rows + 2);
    let urls_area = input_box(buf, &t, urls_box, focus == Field::Urls);
    form.urls.render(
        buf,
        urls_area,
        Style::new().bg(t.bg).fg(t.fg),
        "one per line · paste or drop files here",
        focus == Field::Urls,
    );
    app.hits.fields.push((urls_box, Field::Urls, urls_area));
    y += url_rows + 2;

    let parsed = sources::parse(&form.urls.text());
    let status = if form.urls.is_empty() {
        Line::styled("Each line is one download", Style::new().fg(t.comment))
    } else if form.video_mode() && parsed.invalid.is_empty() && parsed.sources.len() == 1 {
        Line::from(vec![
            Span::styled("▶ ", Style::new().fg(t.magenta)),
            Span::styled(
                "Video page · Next fetches the available formats",
                Style::new().fg(t.magenta),
            ),
        ])
    } else if let Some(bad) = parsed.invalid.first() {
        Line::from(vec![
            Span::styled("! ", Style::new().fg(t.yellow)),
            Span::styled(
                truncate(
                    &format!("{}: {}", bad.input, bad.reason),
                    w.saturating_sub(2),
                ),
                Style::new().fg(t.yellow),
            ),
        ])
    } else {
        Line::from(vec![
            Span::styled("✓ ", Style::new().fg(t.green)),
            Span::styled(parsed.summary(), Style::new().fg(t.green)),
        ])
    };
    buf.set_line(
        body.x,
        y,
        &error_or(&t, form.error.as_deref(), status, w),
        body.width,
    );
    y += 1;

    buf.set_line(
        body.x,
        y,
        &label("Save to", &t, focus == Field::Dir),
        body.width,
    );
    y += 1;
    let dir_box = Rect::new(body.x, y, body.width, 3);
    let dir_area = input_box(buf, &t, dir_box, focus == Field::Dir);
    form.dir.render(
        buf,
        dir_area,
        Style::new().bg(t.bg).fg(t.fg),
        "aria2's default directory",
        focus == Field::Dir,
    );
    app.hits.fields.push((dir_box, Field::Dir, dir_area));
    y += 3;

    let video = form.video_mode();
    let mut x = body.x;
    for (field, on, text, dim) in [
        (Field::Paused, form.paused, "Start paused", video),
        (
            Field::Video,
            video,
            "Video: choose quality with yt-dlp",
            false,
        ),
    ] {
        let style = if focus == field {
            Style::new().fg(t.blue).add_modifier(Modifier::BOLD)
        } else if dim {
            Style::new().fg(t.comment)
        } else {
            Style::new().fg(t.fg_dark)
        };
        let line = Line::from(vec![
            Span::styled(if on { "[x] " } else { "[ ] " }, style),
            Span::styled(text, style),
        ]);
        let w = (line.width() as u16).min(body.right().saturating_sub(x));
        buf.set_line(x, y, &line, w);
        let rect = Rect::new(x, y, w, 1);
        app.hits.fields.push((rect, field, rect));
        x += w + 4;
    }

    let y = body.bottom().saturating_sub(1);
    if video {
        buttons(
            buf,
            app,
            body,
            y,
            "Next ›",
            "enter fetch formats · tab next",
        );
    } else {
        buttons(
            buf,
            app,
            body,
            y,
            "Add",
            "enter add · alt+enter new line · tab next",
        );
    }
}

pub fn render_limits(buf: &mut Buffer, app: &mut App, form: &mut LimitsForm) {
    let t = app.theme;
    let area = centered(60, 10, buf.area);
    if area.width < 30 || area.height < 10 {
        return;
    }
    Clear.render(area, buf);
    let title = match form.target {
        Target::Global => "Speed limit · all downloads".to_owned(),
        Target::Download(_) => "Speed limit".to_owned(),
    };
    let block = popup_block(&t, &title);
    let inner = block.inner(area);
    block.render(area, buf);
    app.hits.dialog = Some(area);
    let body = Rect {
        x: inner.x + 1,
        width: inner.width.saturating_sub(2),
        ..inner
    };
    let w = usize::from(body.width);
    buf.set_line(
        body.x,
        body.y,
        &Line::styled(
            truncate(&form.subject, w),
            Style::new().fg(t.fg).add_modifier(Modifier::BOLD),
        ),
        body.width,
    );

    let label_w = 10u16;
    let field_w = 14u16;
    let placeholder = if form.loaded { "0" } else { "loading…" };
    for (i, field) in LimitsForm::FIELDS.into_iter().enumerate() {
        let y = body.y + 2 + i as u16;
        let focused = form.focus == field;
        let (name, text) = match field {
            Field::Download => ("Download", &mut form.download),
            _ => ("Upload", &mut form.upload),
        };
        let name_line = Line::styled(
            name,
            Style::new()
                .fg(if focused { t.blue } else { t.fg_dark })
                .add_modifier(Modifier::BOLD),
        );
        buf.set_line(body.x, y, &name_line, label_w);
        let input = Rect::new(
            body.x + label_w,
            y,
            field_w.min(body.width.saturating_sub(label_w)),
            1,
        );
        let bg = if focused { t.bg_highlight } else { t.bg };
        text.render(
            buf,
            input,
            Style::new().bg(bg).fg(t.fg),
            placeholder,
            focused,
        );
        let rect = Rect::new(body.x, y, label_w + input.width, 1);
        app.hits.fields.push((rect, field, input));
        let preview = if text.is_empty() && !form.loaded {
            String::new()
        } else {
            limits::parse(&text.text())
                .map_or_else(|_| "?".to_owned(), |b| format!("= {}", limits::describe(b)))
        };
        let px = input.right() + 2;
        if px < body.right() {
            buf.set_line(
                px,
                y,
                &Line::styled(preview, Style::new().fg(t.comment)),
                body.right() - px,
            );
        }
    }

    let hint = Line::styled(
        truncate("e.g. 500K, 2M, 1.5M · 0 = unlimited", w),
        Style::new().fg(t.comment),
    );
    let msg = error_or(&t, form.error.as_deref(), hint, w);
    buf.set_line(body.x, body.y + 5, &msg, body.width);
    let y = body.bottom().saturating_sub(1);
    buttons(buf, app, body, y, "Apply", "enter apply · tab next");
}

/// Braille spinner frame for "loading" states.
fn spinner() -> &'static str {
    const FRAMES: [&str; 8] = ["⣾", "⣽", "⣻", "⢿", "⡿", "⣟", "⣯", "⣷"];
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    FRAMES[(ms / 120 % 8) as usize]
}

pub fn render_video(buf: &mut Buffer, app: &mut App, form: &mut VideoForm) {
    let t = app.theme;
    let screen = buf.area;
    let n = form.info.as_ref().map_or(0, |i| i.choices.len()) as u16;
    // Borders 2, title 1, subtitle 1, gap 1, list, gap 1, save-to 1, gap 1, buttons 1.
    let fixed = 9;
    let rows = n.clamp(2, screen.height.saturating_sub(fixed + 2).max(2));
    let area = centered(76, fixed + rows, screen);
    if area.width < 30 || area.height < fixed {
        return;
    }
    Clear.render(area, buf);
    let block = popup_block(&t, "Download video");
    let inner = block.inner(area);
    block.render(area, buf);
    app.hits.dialog = Some(area);
    let body = Rect {
        x: inner.x + 1,
        width: inner.width.saturating_sub(2),
        ..inner
    };
    let w = usize::from(body.width);
    let bold = Style::new().fg(t.fg).add_modifier(Modifier::BOLD);
    let dim = Style::new().fg(t.comment);
    let mut y = body.y;

    let title = form
        .info
        .as_ref()
        .map_or(form.url.as_str(), |i| i.title.as_str());
    buf.set_line(
        body.x,
        y,
        &Line::styled(truncate(title, w), bold),
        body.width,
    );
    y += 1;
    let subtitle = match (&form.info, &form.error) {
        (Some(info), _) => Line::styled(truncate(&info.subtitle, w), dim),
        (None, Some(e)) => Line::from(vec![
            Span::styled("✗ ", Style::new().fg(t.red)),
            Span::styled(truncate(e, w.saturating_sub(2)), Style::new().fg(t.red)),
        ]),
        _ if form.loading() => Line::from(vec![
            Span::styled(format!("{} ", spinner()), Style::new().fg(t.blue)),
            Span::styled("Fetching formats with yt-dlp…", dim),
        ]),
        _ => Line::raw(""),
    };
    buf.set_line(body.x, y, &subtitle, body.width);
    y += 2;

    let rows = rows.min(body.bottom().saturating_sub(y + 4));
    let list = Rect::new(body.x, y, body.width, rows);
    if let Some(info) = &form.info {
        let visible = usize::from(rows).max(1);
        if form.cursor < form.scroll {
            form.scroll = form.cursor;
        } else if form.cursor >= form.scroll + visible {
            form.scroll = form.cursor + 1 - visible;
        }
        for (i, choice) in info
            .choices
            .iter()
            .enumerate()
            .skip(form.scroll)
            .take(usize::from(rows))
        {
            let row = Rect::new(list.x, list.y + (i - form.scroll) as u16, list.width, 1);
            let selected = i == form.cursor;
            let hover = app.hover.is_some_and(|p| row.contains(p));
            let bg = if selected {
                t.bg_highlight
            } else if hover {
                t.bg_hover
            } else {
                t.bg_dark
            };
            buf.set_style(row, Style::new().bg(bg));
            let marker = if selected { "● " } else { "  " };
            let label_w = w.saturating_sub(choice.detail.width() + 4);
            let label = truncate(&choice.label, label_w);
            let pad = w.saturating_sub(2 + label.width() + choice.detail.width() + 1);
            let line = Line::from(vec![
                Span::styled(marker, Style::new().fg(t.blue)),
                Span::styled(
                    label,
                    if selected {
                        Style::new().fg(t.blue).add_modifier(Modifier::BOLD)
                    } else {
                        Style::new().fg(t.fg)
                    },
                ),
                Span::raw(" ".repeat(pad)),
                Span::styled(choice.detail.clone(), dim),
            ]);
            buf.set_line(row.x, row.y, &line, row.width);
            app.hits.choices.push((row, i));
        }
    } else if form.unsupported() && rows > 0 {
        buf.set_line(
            list.x,
            list.y,
            &Line::styled(
                truncate("It may still be a plain file: try “Download as file”.", w),
                dim,
            ),
            list.width,
        );
    }
    y += rows + 1;

    buf.set_line(
        body.x,
        y,
        &Line::from(vec![
            Span::styled("Save to ", dim),
            Span::styled(
                truncate(&form.dir, w.saturating_sub(8)),
                Style::new().fg(t.fg_dark),
            ),
        ]),
        body.width,
    );

    // Buttons, right-aligned: [primary] [Update yt-dlp] [Cancel].
    let y = body.bottom().saturating_sub(1);
    let primary = if form.unsupported() {
        Some(("Download as file", DialogButton::Fallback))
    } else if form.error.is_some() {
        Some(("Retry", DialogButton::Confirm))
    } else if form.info.is_some() {
        Some(("Download", DialogButton::Confirm))
    } else {
        None
    };
    let labels: Vec<&str> = primary
        .iter()
        .map(|(l, _)| *l)
        .chain(["Update yt-dlp", "Cancel"])
        .collect();
    let total: usize = labels.iter().map(|l| l.width() + 3).sum();
    let mut x = body.right().saturating_sub(total as u16 - 1).max(body.x);
    let hint = "↑/↓ choose · enter download · u update yt-dlp";
    let room = usize::from(x.saturating_sub(body.x + 1));
    if hint.width() <= room {
        buf.set_line(body.x, y, &Line::styled(hint, dim), room as u16);
    }
    if let Some((label, kind)) = primary {
        x += dialog_button(buf, app, x, y, label, kind, Tone::Primary) + 1;
    }
    x += dialog_button(
        buf,
        app,
        x,
        y,
        "Update yt-dlp",
        DialogButton::UpdateYtDlp,
        Tone::Normal,
    ) + 1;
    dialog_button(buf, app, x, y, "Cancel", DialogButton::Cancel, Tone::Normal);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_form_validation() {
        let mut form = AddForm::new("/dl", "");
        assert!(form.submit().unwrap_err().contains("Enter a URL"));
        form.urls.set_text("https://a.example/x.iso\nnope");
        assert_eq!(
            form.submit().unwrap_err(),
            "nope: not a URL, magnet link or file"
        );
        form.urls
            .set_text("https://a.example/x.iso\nmagnet:?xt=urn:btih:abc");
        form.paused = true;
        let Ok(AddRequest::Files(sources, options)) = form.submit() else {
            panic!("expected files");
        };
        assert_eq!(sources.len(), 2);
        assert_eq!(options.dir.as_deref(), Some("/dl"));
        assert!(options.paused);
        form.dir.set_text("  ");
        assert!(matches!(form.submit(), Ok(AddRequest::Files(_, o)) if o.dir.is_none()));
    }

    #[test]
    fn video_mode_is_automatic_until_toggled() {
        let mut form = AddForm::new("/dl", "https://www.youtube.com/watch?v=abc");
        assert!(form.video_mode());
        assert_eq!(
            form.submit(),
            Ok(AddRequest::Video {
                url: "https://www.youtube.com/watch?v=abc".into(),
                dir: "/dl".into()
            })
        );
        form.urls.set_text("https://example.com/file.iso");
        assert!(!form.video_mode(), "a file URL goes to aria2");
        form.toggle(Field::Video);
        assert!(form.video_mode(), "the user can force video mode");
        form.urls
            .set_text("https://a.example/x\nhttps://b.example/y");
        assert_eq!(
            form.submit(),
            Err("Video mode takes a single http(s) URL".into())
        );
        form.toggle(Field::Video);
        assert!(!form.video_mode());
        assert!(matches!(form.submit(), Ok(AddRequest::Files(s, _)) if s.len() == 2));
    }

    #[test]
    fn video_form_spec() {
        let mut form = VideoForm::new("https://youtu.be/x".into(), "/dl".into());
        assert!(form.loading());
        assert!(form.spec().is_none());
        form.info = Some(ytdlp::parse_probe("https://youtu.be/x", br#"{"title":"T"}"#).unwrap());
        form.move_cursor(5);
        assert_eq!(form.cursor, 0, "clamped to the only choice");
        let spec = form.spec().unwrap();
        assert_eq!(spec.title, "T");
        assert_eq!(spec.dir, std::path::PathBuf::from("/dl"));
        form.error = Some("yt-dlp doesn't recognise this page as a video".into());
        assert!(form.unsupported());
    }

    #[test]
    fn limits_form_fill_and_submit() {
        let mut form = LimitsForm::new(Target::Global, "All downloads".into());
        form.upload.insert_str("1M");
        form.fill(Limits {
            download: 512 * 1024,
            upload: 0,
        });
        assert_eq!(form.download.text(), "512K");
        assert_eq!(form.upload.text(), "1M", "typed value is kept");
        assert_eq!(
            form.submit(),
            Ok(Limits {
                download: 512 * 1024,
                upload: 1 << 20
            })
        );
        form.download.set_text("fast");
        assert!(form.submit().unwrap_err().starts_with("Download:"));
    }
}
