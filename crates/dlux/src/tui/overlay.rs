//! Things drawn on top of the main screen: the context menu and modal dialogs. Their clickable
//! regions are recorded in [`App::hits`] like the rest of the UI.

use super::app::{App, Command, Dialog, DialogButton};
use super::theme::Theme;
use super::ui::truncate;
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph, Widget};
use unicode_width::UnicodeWidthStr;

/// Keyboard shortcuts listed in the help dialog.
const HELP_KEYS: &[(&str, &str)] = &[
    ("j/k  ↓/↑", "move selection"),
    ("g/G  Home/End", "first / last"),
    ("PgUp/PgDn", "page up / down"),
    ("tab  1-4", "switch filter tab"),
    ("enter", "toggle details"),
    ("a", "add download"),
    ("p  r  space", "pause / resume / toggle"),
    ("d  Del", "remove"),
    ("o", "open folder"),
    ("c", "copy URL"),
    ("m", "context menu"),
    ("?", "this help"),
    ("q  Esc", "quit (Esc closes popups)"),
];

/// Mouse actions listed in the help dialog.
const HELP_MOUSE: &[(&str, &str)] = &[
    ("click", "select card, tab or button"),
    ("double-click", "open details"),
    ("right-click", "context menu"),
    ("wheel", "scroll list / switch tabs"),
    ("drag scrollbar", "scroll"),
];

pub fn render(buf: &mut Buffer, app: &mut App) {
    if app.menu.is_some() {
        render_menu(buf, app);
    }
    if app.dialog.is_some() {
        render_dialog(buf, app);
    }
}

/// Place a `w`×`h` box at `anchor`, moved left/up as needed to stay inside `screen`.
pub fn fit(anchor: Position, w: u16, h: u16, screen: Rect) -> Rect {
    let w = w.min(screen.width);
    let h = h.min(screen.height);
    let x = anchor.x.min(screen.right().saturating_sub(w)).max(screen.x);
    let y = anchor
        .y
        .min(screen.bottom().saturating_sub(h))
        .max(screen.y);
    Rect::new(x, y, w, h)
}

fn centered(w: u16, h: u16, screen: Rect) -> Rect {
    let w = w.min(screen.width);
    let h = h.min(screen.height);
    Rect::new(
        screen.x + (screen.width - w) / 2,
        screen.y + (screen.height - h) / 2,
        w,
        h,
    )
}

fn popup_block<'a>(t: &Theme, title: &'a str) -> Block<'a> {
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(t.border_focus))
        .style(Style::new().bg(t.bg_dark).fg(t.fg));
    if title.is_empty() {
        block
    } else {
        block.title(Line::from(vec![
            Span::raw(" "),
            Span::styled(title, Style::new().fg(t.blue).add_modifier(Modifier::BOLD)),
            Span::raw(" "),
        ]))
    }
}

fn render_menu(buf: &mut Buffer, app: &mut App) {
    let Some(menu) = app.menu.clone() else {
        return;
    };
    let t = app.theme;
    let target = app.state.find(&menu.gid).cloned();
    let label_w = Command::MENU
        .iter()
        .map(|c| c.label().width() + c.key().width() + 4)
        .max()
        .unwrap_or(0) as u16;
    let screen = buf.area;
    let area = fit(
        menu.anchor,
        label_w + 2,
        Command::MENU.len() as u16 + 2,
        screen,
    );
    if area.width < 6 || area.height < 3 {
        return;
    }
    Clear.render(area, buf);
    let block = popup_block(&t, "");
    let inner = block.inner(area);
    block.render(area, buf);
    app.hits.menu = Some(area);

    for (i, cmd) in Command::MENU.into_iter().enumerate() {
        let y = inner.y + i as u16;
        if y >= inner.bottom() {
            break;
        }
        let row = Rect::new(inner.x, y, inner.width, 1);
        let enabled = cmd.enabled(target.as_ref());
        let active = i == menu.cursor && enabled;
        let bg = if active { t.bg_highlight } else { t.bg_dark };
        let fg = match (enabled, active) {
            (false, _) => t.comment,
            (true, true) => t.blue,
            (true, false) => t.fg,
        };
        buf.set_style(row, Style::new().bg(bg));
        let w = usize::from(inner.width);
        let key = cmd.key();
        let label = truncate(cmd.label(), w.saturating_sub(key.width() + 3));
        let pad = w.saturating_sub(label.width() + key.width() + 2);
        let line = Line::from(vec![
            Span::styled(format!(" {label}"), Style::new().fg(fg)),
            Span::raw(" ".repeat(pad)),
            Span::styled(key, Style::new().fg(t.comment)),
            Span::raw(" "),
        ]);
        buf.set_line(row.x, row.y, &line, row.width);
        app.hits.menu_items.push((row, cmd));
    }
}

/// A clickable dialog button: `[ label ]`, highlighted when hovered.
fn button(
    buf: &mut Buffer,
    app: &mut App,
    x: u16,
    y: u16,
    label: &str,
    kind: DialogButton,
    primary: bool,
) -> u16 {
    let t = app.theme;
    let text = format!(" {label} ");
    let w = text.width() as u16;
    let rect = Rect::new(x, y, w, 1).intersection(buf.area);
    let hovered = app.hover.is_some_and(|p| rect.contains(p));
    let (bg, fg) = match (primary, hovered) {
        (true, false) => (t.red, t.bg_dark),
        (true, true) => (t.orange, t.bg_dark),
        (false, false) => (t.bg_highlight, t.fg),
        (false, true) => (t.border, t.fg),
    };
    buf.set_line(
        rect.x,
        rect.y,
        &Line::styled(
            text,
            Style::new().bg(bg).fg(fg).add_modifier(Modifier::BOLD),
        ),
        rect.width,
    );
    app.hits.dialog_buttons.push((rect, kind));
    w
}

fn render_dialog(buf: &mut Buffer, app: &mut App) {
    let Some(dialog) = app.dialog.clone() else {
        return;
    };
    let screen = buf.area;
    let t = app.theme;
    match dialog {
        Dialog::ConfirmRemove { name, .. } => {
            let area = centered(56, 8, screen);
            if area.width < 20 || area.height < 6 {
                // Too small to draw; y/n still work.
                return;
            }
            Clear.render(area, buf);
            let block = popup_block(&t, "Remove download");
            let inner = block.inner(area);
            block.render(area, buf);
            app.hits.dialog = Some(area);
            let body = Rect {
                x: inner.x + 1,
                width: inner.width.saturating_sub(2),
                ..inner
            };
            let w = usize::from(body.width);
            Paragraph::new(vec![
                Line::raw(""),
                Line::from(vec![
                    Span::raw("Remove "),
                    Span::styled(
                        truncate(&name, w.saturating_sub(9)),
                        Style::new().add_modifier(Modifier::BOLD),
                    ),
                    Span::raw("?"),
                ]),
                Line::styled(
                    truncate("Already downloaded data stays on disk.", w),
                    Style::new().fg(t.comment),
                ),
            ])
            .render(body, buf);
            let y = inner.bottom().saturating_sub(1);
            let total = " Remove ".width() as u16 + 1 + " Cancel ".width() as u16;
            let mut x = body.right().saturating_sub(total);
            x += button(buf, app, x, y, "Remove", DialogButton::Confirm, true) + 1;
            button(buf, app, x, y, "Cancel", DialogButton::Cancel, false);
        }
        Dialog::Help => {
            let rows = HELP_KEYS.len() + HELP_MOUSE.len() + 6;
            let area = centered(58, rows as u16, screen);
            if area.width < 20 || area.height < 5 {
                return;
            }
            Clear.render(area, buf);
            let block = popup_block(&t, "Help");
            let inner = block.inner(area);
            block.render(area, buf);
            app.hits.dialog = Some(area);
            let key_w = 16;
            let item = |k: &str, d: &str| {
                Line::from(vec![
                    Span::styled(
                        format!(" {k:<key_w$}"),
                        Style::new().fg(t.blue).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(d.to_owned(), Style::new().fg(t.fg_dark)),
                ])
            };
            let heading = |s: &'static str| {
                Line::styled(
                    format!(" {s}"),
                    Style::new().fg(t.magenta).add_modifier(Modifier::BOLD),
                )
            };
            let mut lines = vec![heading("Keyboard")];
            lines.extend(HELP_KEYS.iter().map(|(k, d)| item(k, d)));
            lines.push(Line::raw(""));
            lines.push(heading("Mouse"));
            lines.extend(HELP_MOUSE.iter().map(|(k, d)| item(k, d)));
            let text = Rect {
                height: inner.height.saturating_sub(1),
                ..inner
            };
            Paragraph::new(lines).render(text, buf);
            let y = inner.bottom().saturating_sub(1);
            let x = inner.right().saturating_sub(" Close ".width() as u16 + 1);
            button(buf, app, x, y, "Close", DialogButton::Cancel, false);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn popups_stay_on_screen() {
        let screen = Rect::new(0, 0, 80, 24);
        assert_eq!(
            fit(Position::new(5, 5), 20, 7, screen),
            Rect::new(5, 5, 20, 7)
        );
        assert_eq!(
            fit(Position::new(75, 20), 20, 7, screen),
            Rect::new(60, 17, 20, 7)
        );
        assert_eq!(fit(Position::new(0, 0), 100, 30, screen), screen);
        assert_eq!(centered(40, 10, screen), Rect::new(20, 7, 40, 10));
        assert_eq!(centered(100, 10, screen).width, 80);
    }
}
