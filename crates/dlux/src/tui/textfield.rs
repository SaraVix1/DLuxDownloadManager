//! An editable text field (single- or multi-line) for dialogs: cursor movement, the usual
//! readline-style shortcuts, paste, and click-to-place-cursor.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthChar;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextField {
    lines: Vec<String>,
    /// Cursor line.
    row: usize,
    /// Cursor position in the line, in chars.
    col: usize,
    multiline: bool,
    /// First visible line (multi-line) — kept between frames so the view doesn't jump.
    scroll: usize,
    /// First visible char of the cursor line at the last render, for mouse clicks.
    hscroll: usize,
    /// Edited since it was created or last [`set_text`](Self::set_text).
    pub edited: bool,
}

impl TextField {
    pub fn new(multiline: bool) -> Self {
        Self {
            lines: vec![String::new()],
            row: 0,
            col: 0,
            multiline,
            scroll: 0,
            hscroll: 0,
            edited: false,
        }
    }

    pub fn with_text(multiline: bool, text: &str) -> Self {
        let mut f = Self::new(multiline);
        f.set_text(text);
        f
    }

    /// Replace the content and put the cursor at the end.
    pub fn set_text(&mut self, text: &str) {
        *self = Self::new(self.multiline);
        self.insert_str(text);
        self.edited = false;
    }

    pub fn text(&self) -> String {
        self.lines.join("\n")
    }

    pub fn is_empty(&self) -> bool {
        self.lines.iter().all(String::is_empty)
    }

    fn line(&self) -> &String {
        &self.lines[self.row]
    }

    fn line_len(&self) -> usize {
        self.line().chars().count()
    }

    /// Byte index of char `col` in the cursor line.
    fn byte(&self, col: usize) -> usize {
        self.line()
            .char_indices()
            .nth(col)
            .map_or(self.line().len(), |(i, _)| i)
    }

    pub fn insert_char(&mut self, c: char) {
        if c == '\n' {
            if self.multiline {
                let at = self.byte(self.col);
                let rest = self.lines[self.row].split_off(at);
                self.row += 1;
                self.lines.insert(self.row, rest);
                self.col = 0;
            } else {
                self.insert_char(' ');
                return;
            }
        } else if c.is_control() {
            return;
        } else {
            let at = self.byte(self.col);
            self.lines[self.row].insert(at, c);
            self.col += 1;
        }
        self.edited = true;
    }

    /// Insert pasted text; CRLF/CR become newlines (spaces in a single-line field).
    pub fn insert_str(&mut self, s: &str) {
        let s = s.replace("\r\n", "\n").replace('\r', "\n");
        let s = if self.multiline {
            s
        } else {
            s.trim_end_matches('\n').to_owned()
        };
        for c in s.chars() {
            self.insert_char(if c == '\t' { ' ' } else { c });
        }
    }

    fn backspace(&mut self) {
        if self.col > 0 {
            self.col -= 1;
            let at = self.byte(self.col);
            self.lines[self.row].remove(at);
        } else if self.row > 0 {
            let line = self.lines.remove(self.row);
            self.row -= 1;
            self.col = self.line_len();
            self.lines[self.row].push_str(&line);
        } else {
            return;
        }
        self.edited = true;
    }

    fn delete(&mut self) {
        if self.col < self.line_len() {
            let at = self.byte(self.col);
            self.lines[self.row].remove(at);
        } else if self.row + 1 < self.lines.len() {
            let next = self.lines.remove(self.row + 1);
            self.lines[self.row].push_str(&next);
        } else {
            return;
        }
        self.edited = true;
    }

    /// Delete the word before the cursor (Ctrl-W).
    fn delete_word(&mut self) {
        let chars: Vec<char> = self.line().chars().collect();
        let mut start = self.col;
        while start > 0 && chars[start - 1].is_whitespace() {
            start -= 1;
        }
        while start > 0 && !chars[start - 1].is_whitespace() {
            start -= 1;
        }
        if start == self.col {
            return self.backspace();
        }
        let (a, b) = (self.byte(start), self.byte(self.col));
        self.lines[self.row].replace_range(a..b, "");
        self.col = start;
        self.edited = true;
    }

    fn move_vertical(&mut self, delta: isize) -> bool {
        let target = self.row.saturating_add_signed(delta);
        if !self.multiline || target >= self.lines.len() || target == self.row {
            return false;
        }
        self.row = target;
        self.col = self.col.min(self.line_len());
        true
    }

    /// Handle an editing key. Returns false for keys the field does not use (so the dialog can
    /// handle them: Enter, Tab, Esc, and Up/Down at the edges of a multi-line field).
    pub fn on_key(&mut self, key: KeyEvent) -> bool {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        match key.code {
            KeyCode::Enter if alt && self.multiline => self.insert_char('\n'),
            KeyCode::Char('w') if ctrl => self.delete_word(),
            KeyCode::Backspace if ctrl || alt => self.delete_word(),
            KeyCode::Char('u') if ctrl => {
                let at = self.byte(self.col);
                self.lines[self.row].replace_range(..at, "");
                self.col = 0;
                self.edited = true;
            }
            KeyCode::Char('k') if ctrl => {
                let at = self.byte(self.col);
                self.lines[self.row].truncate(at);
                self.edited = true;
            }
            KeyCode::Char('a') if ctrl => self.col = 0,
            KeyCode::Char('e') if ctrl => self.col = self.line_len(),
            KeyCode::Char(_) if ctrl => return false,
            KeyCode::Char(c) => self.insert_char(c),
            KeyCode::Backspace => self.backspace(),
            KeyCode::Delete => self.delete(),
            KeyCode::Left if self.col > 0 => self.col -= 1,
            KeyCode::Left if self.row > 0 => {
                self.row -= 1;
                self.col = self.line_len();
            }
            KeyCode::Right if self.col < self.line_len() => self.col += 1,
            KeyCode::Right if self.row + 1 < self.lines.len() => {
                self.row += 1;
                self.col = 0;
            }
            KeyCode::Left | KeyCode::Right => {}
            KeyCode::Home => self.col = 0,
            KeyCode::End => self.col = self.line_len(),
            KeyCode::Up => return self.move_vertical(-1),
            KeyCode::Down => return self.move_vertical(1),
            _ => return false,
        }
        true
    }

    /// Place the cursor at a position relative to the field's area (as last rendered).
    pub fn click(&mut self, area: Rect, pos: Position) {
        if !area.contains(pos) {
            return;
        }
        let row = (self.scroll + usize::from(pos.y - area.y)).min(self.lines.len() - 1);
        // Only the cursor line is scrolled horizontally; the others start at column 0.
        let hscroll = if row == self.row { self.hscroll } else { 0 };
        self.row = row;
        let target = usize::from(pos.x - area.x);
        let mut width = 0;
        let mut col = hscroll;
        for c in self.line().chars().skip(hscroll) {
            let w = c.width().unwrap_or(0);
            if width + w > target {
                break;
            }
            width += w;
            col += 1;
        }
        self.col = col.min(self.line_len());
    }

    /// Draw the field into `area`. `placeholder` is shown (dimmed) when empty; the cursor is
    /// drawn as a reversed cell when `focused`.
    pub fn render(
        &mut self,
        buf: &mut Buffer,
        area: Rect,
        style: Style,
        placeholder: &str,
        focused: bool,
    ) {
        if area.is_empty() {
            return;
        }
        buf.set_style(area, style);
        let height = usize::from(area.height);
        let width = usize::from(area.width);
        if self.row < self.scroll {
            self.scroll = self.row;
        } else if self.row >= self.scroll + height {
            self.scroll = self.row + 1 - height;
        }
        self.scroll = self.scroll.min(self.lines.len().saturating_sub(1));

        if self.is_empty() && !placeholder.is_empty() {
            let dim = style.add_modifier(Modifier::DIM);
            buf.set_line(area.x, area.y, &Line::styled(placeholder, dim), area.width);
        }

        for (i, line) in self.lines.iter().enumerate().skip(self.scroll).take(height) {
            let y = area.y + (i - self.scroll) as u16;
            let chars: Vec<char> = line.chars().collect();
            if i == self.row {
                // Scroll horizontally so the cursor (one cell past the text at the end) fits.
                let mut start = self.hscroll.min(self.col);
                let span_w = |a: usize, b: usize| -> usize {
                    chars[a..b].iter().map(|c| c.width().unwrap_or(0)).sum()
                };
                while start < self.col && span_w(start, self.col) + 1 > width {
                    start += 1;
                }
                self.hscroll = start;
                let visible: String = chars[start..].iter().collect();
                buf.set_line(area.x, y, &Line::styled(visible, style), area.width);
                if focused {
                    let x = area.x + span_w(start, self.col) as u16;
                    if x < area.right()
                        && let Some(cell) = buf.cell_mut(Position::new(x, y))
                    {
                        cell.set_style(style.add_modifier(Modifier::REVERSED));
                    }
                }
            } else {
                let text: String = chars.iter().collect();
                let shown = super::ui::truncate(&text, width);
                buf.set_line(
                    area.x,
                    y,
                    &Line::from(Span::styled(shown, style)),
                    area.width,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    fn typed(f: &mut TextField, s: &str) {
        for c in s.chars() {
            f.on_key(key(KeyCode::Char(c)));
        }
    }

    #[test]
    fn typing_and_editing() {
        let mut f = TextField::new(false);
        typed(&mut f, "héllo wörld");
        assert_eq!(f.text(), "héllo wörld");
        assert!(f.edited);
        f.on_key(ctrl('w'));
        assert_eq!(f.text(), "héllo ");
        f.on_key(key(KeyCode::Home));
        f.on_key(key(KeyCode::Delete));
        f.on_key(key(KeyCode::Right));
        typed(&mut f, "E");
        assert_eq!(f.text(), "éEllo ");
        f.on_key(ctrl('k'));
        assert_eq!(f.text(), "éE");
        f.on_key(key(KeyCode::End));
        f.on_key(key(KeyCode::Backspace));
        assert_eq!(f.text(), "é");
        f.on_key(ctrl('u'));
        assert!(f.is_empty());
        assert!(
            !f.on_key(key(KeyCode::Enter)),
            "Enter is left to the dialog"
        );
        assert!(!f.on_key(key(KeyCode::Tab)));
    }

    #[test]
    fn multiline_and_paste() {
        let mut f = TextField::new(true);
        f.insert_str("a\r\nb\rc");
        assert_eq!(f.text(), "a\nb\nc");
        assert_eq!(f.lines.len(), 3);
        f.on_key(key(KeyCode::Up));
        f.on_key(key(KeyCode::Home));
        f.on_key(key(KeyCode::Backspace)); // joins "b" onto "a"
        assert_eq!(f.text(), "ab\nc");
        f.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT));
        assert_eq!(f.text(), "a\nb\nc");
        f.on_key(key(KeyCode::Up));
        f.on_key(key(KeyCode::Up));
        assert!(
            !f.on_key(key(KeyCode::Up)),
            "Up on the first line is left to the dialog"
        );

        let mut single = TextField::new(false);
        single.insert_str("one\ntwo\n");
        assert_eq!(single.text(), "one two");
    }

    #[test]
    fn set_text_resets_edited_and_moves_to_end() {
        let mut f = TextField::new(false);
        f.set_text("/home/me");
        assert!(!f.edited);
        typed(&mut f, "/x");
        assert_eq!(f.text(), "/home/me/x");
    }

    #[test]
    fn renders_with_horizontal_scroll_and_click() {
        let mut f = TextField::with_text(false, "0123456789abcdef");
        let area = Rect::new(2, 1, 8, 1);
        let mut buf = Buffer::empty(Rect::new(0, 0, 12, 3));
        f.render(&mut buf, area, Style::new(), "", true);
        let row: String = (2..10).map(|x| buf[(x, 1)].symbol().to_owned()).collect();
        assert_eq!(row, "9abcdef ", "end of the text plus the cursor cell");
        f.click(area, Position::new(3, 1));
        f.on_key(key(KeyCode::Char('_')));
        assert_eq!(f.text(), "0123456789_abcdef");

        let mut empty = TextField::new(false);
        let mut buf = Buffer::empty(Rect::new(0, 0, 12, 3));
        empty.render(&mut buf, area, Style::new(), "type here", false);
        let row: String = (2..10).map(|x| buf[(x, 1)].symbol().to_owned()).collect();
        assert_eq!(row, "type her");
    }
}
