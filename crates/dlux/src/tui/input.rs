//! Keyboard and mouse handling. Open overlays get input first: a dialog, then the context menu,
//! then the main screen.

use super::app::{App, Command, Dialog, DialogButton};
use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::Position;
use std::time::{Duration, Instant};

/// Two clicks on the same card within this interval are a double-click.
pub const DOUBLE_CLICK: Duration = Duration::from_millis(450);

impl App {
    pub fn on_event(&mut self, event: Event) {
        match event {
            Event::Key(key) => self.on_key(key),
            Event::Mouse(mouse) => self.on_mouse(mouse, Instant::now()),
            Event::Paste(text) => self.on_paste(&text),
            _ => {}
        }
    }

    /// Bracketed paste (also what most terminals send for a dropped file): into the focused
    /// form field, or — on the main screen — straight into a new Add dialog.
    pub fn on_paste(&mut self, text: &str) {
        match self.dialog.as_mut() {
            None => self.open_add(text),
            Some(dialog) => {
                if let Some(form) = dialog.form() {
                    let focus = *form.focus();
                    if let Some(field) = form.text_field(focus) {
                        field.insert_str(text);
                        *form.error() = None;
                    }
                }
            }
        }
    }

    // ----- keyboard -----------------------------------------------------------------------

    pub fn on_key(&mut self, key: KeyEvent) {
        if key.kind == KeyEventKind::Release {
            return;
        }
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.should_quit = true;
        } else if self.dialog.is_some() {
            self.dialog_key(key);
        } else if self.menu.is_some() {
            self.menu_key(key);
        } else {
            self.main_key(key);
        }
    }

    fn dialog_key(&mut self, key: KeyEvent) {
        if self.dialog.as_mut().is_some_and(|d| d.form().is_some()) {
            return self.form_key(key);
        }
        let confirm = matches!(self.dialog, Some(Dialog::ConfirmRemove { .. }));
        let button = match key.code {
            KeyCode::Char('y') | KeyCode::Enter if confirm => DialogButton::Confirm,
            KeyCode::Char('n') if confirm => DialogButton::Cancel,
            KeyCode::Esc => DialogButton::Cancel,
            KeyCode::Char('q' | '?') | KeyCode::Enter if !confirm => DialogButton::Cancel,
            _ => return,
        };
        self.press_dialog(button);
    }

    fn form_key(&mut self, key: KeyEvent) {
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        match key.code {
            KeyCode::Esc => return self.press_dialog(DialogButton::Cancel),
            KeyCode::Enter if !alt => return self.press_dialog(DialogButton::Confirm),
            _ => {}
        }
        let Some(form) = self.dialog.as_mut().and_then(Dialog::form) else {
            return;
        };
        let focus = *form.focus();
        match key.code {
            KeyCode::Tab => form.cycle(1),
            KeyCode::BackTab => form.cycle(-1),
            KeyCode::Char(' ') if form.toggle(focus) => {}
            _ => {
                let used = form.text_field(focus).is_some_and(|f| f.on_key(key));
                if used {
                    *form.error() = None;
                } else if key.code == KeyCode::Down {
                    form.cycle(1);
                } else if key.code == KeyCode::Up {
                    form.cycle(-1);
                }
            }
        }
    }

    fn menu_key(&mut self, key: KeyEvent) {
        let Some(menu) = self.menu.as_mut() else {
            return;
        };
        let n = Command::MENU.len();
        match key.code {
            KeyCode::Esc | KeyCode::Char('q' | 'm') => self.menu = None,
            KeyCode::Char('j') | KeyCode::Down | KeyCode::Tab => {
                menu.cursor = (menu.cursor + 1) % n
            }
            KeyCode::Char('k') | KeyCode::Up | KeyCode::BackTab => {
                menu.cursor = (menu.cursor + n - 1) % n;
            }
            KeyCode::Enter => {
                let cmd = Command::MENU[menu.cursor];
                self.run_menu_item(cmd);
            }
            KeyCode::Char(c) => {
                if let Some(cmd) = Command::MENU
                    .into_iter()
                    .find(|cmd| cmd.key() == c.to_string())
                {
                    self.run_menu_item(cmd);
                }
            }
            _ => {}
        }
    }

    fn main_key(&mut self, key: KeyEvent) {
        let page = self.page_size.max(1) as isize;
        let cmd = match key.code {
            KeyCode::Char('q') | KeyCode::Esc => Command::Quit,
            KeyCode::Char('a') | KeyCode::Insert => Command::Add,
            KeyCode::Char('p') | KeyCode::Char(' ') => Command::Pause,
            KeyCode::Char('r') => Command::Resume,
            KeyCode::Char('d') | KeyCode::Delete => Command::Remove,
            KeyCode::Char('P') => Command::PauseAll,
            KeyCode::Char('R') => Command::ResumeAll,
            KeyCode::Char('X') => Command::Purge,
            KeyCode::Char('s') => Command::SpeedLimit,
            KeyCode::Char('S') => Command::GlobalLimit,
            KeyCode::Char('o') => Command::OpenFolder,
            KeyCode::Char('c') => Command::CopyUrl,
            KeyCode::Char('m') | KeyCode::Menu => Command::Menu,
            KeyCode::F(10) if key.modifiers.contains(KeyModifiers::SHIFT) => Command::Menu,
            KeyCode::Char('?') | KeyCode::F(1) => Command::Help,
            KeyCode::Enter => Command::Details,
            KeyCode::Tab => return self.set_filter(self.filter.next()),
            KeyCode::BackTab => return self.set_filter(self.filter.prev()),
            KeyCode::Char(c @ '1'..='4') => {
                let filter = dlux_core::state::Filter::ALL[(c as usize) - ('1' as usize)];
                return self.set_filter(filter);
            }
            KeyCode::Char('j') | KeyCode::Down => return self.move_selection(1),
            KeyCode::Char('k') | KeyCode::Up => return self.move_selection(-1),
            KeyCode::PageDown => return self.move_selection(page),
            KeyCode::PageUp => return self.move_selection(-page),
            KeyCode::Char('g') | KeyCode::Home => return self.select_first(),
            KeyCode::Char('G') | KeyCode::End => return self.select_last(),
            _ => return,
        };
        // Space toggles: pause an active download, resume a paused one.
        let cmd = if key.code == KeyCode::Char(' ')
            && !cmd.enabled(self.selected_download())
            && Command::Resume.enabled(self.selected_download())
        {
            Command::Resume
        } else {
            cmd
        };
        self.run(cmd);
    }

    // ----- mouse --------------------------------------------------------------------------

    pub fn on_mouse(&mut self, m: MouseEvent, now: Instant) {
        let pos = Position::new(m.column, m.row);
        self.hover = Some(pos);
        match m.kind {
            MouseEventKind::Moved => self.hover_menu(pos),
            MouseEventKind::Down(MouseButton::Left) => self.left_click(pos, now),
            MouseEventKind::Down(MouseButton::Right) => self.right_click(pos),
            MouseEventKind::Drag(MouseButton::Left) => {
                if self.dragging {
                    self.scroll_to_row(pos.y);
                } else {
                    self.hover_menu(pos);
                }
            }
            MouseEventKind::Up(MouseButton::Left) => self.dragging = false,
            MouseEventKind::ScrollDown => self.wheel(pos, 1),
            MouseEventKind::ScrollUp => self.wheel(pos, -1),
            _ => {}
        }
    }

    /// Keep the menu cursor under the pointer, so hover and keyboard highlight agree.
    fn hover_menu(&mut self, pos: Position) {
        if let (Some(cmd), Some(menu)) = (self.hits.menu_item_at(pos), self.menu.as_mut())
            && let Some(i) = Command::MENU.iter().position(|c| *c == cmd)
        {
            menu.cursor = i;
        }
    }

    fn left_click(&mut self, pos: Position, now: Instant) {
        if let Some(dialog) = self.dialog.as_mut() {
            let inside = self.hits.dialog.is_some_and(|r| r.contains(pos));
            if let Some(button) = self.hits.dialog_button_at(pos) {
                self.press_dialog(button);
            } else if let (Some(form), Some((field, text_area))) =
                (dialog.form(), self.hits.field_at(pos))
            {
                *form.focus() = field;
                if !form.toggle(field)
                    && let Some(f) = form.text_field(field)
                {
                    f.click(text_area, pos);
                }
            } else if !inside && dialog.form().is_none() {
                // A click outside dismisses Help or a confirmation, but never a form (that
                // would throw away what was typed).
                self.press_dialog(DialogButton::Cancel);
            }
            return;
        }
        if self.menu.is_some() {
            if let Some(cmd) = self.hits.menu_item_at(pos) {
                self.run_menu_item(cmd);
            } else if !self.hits.menu.is_some_and(|r| r.contains(pos)) {
                // A click outside the menu only closes it.
                self.menu = None;
            }
            return;
        }
        if let Some(filter) = self.hits.tab_at(pos) {
            self.set_filter(filter);
        } else if self.hits.speed.is_some_and(|r| r.contains(pos)) {
            self.run(Command::GlobalLimit);
        } else if let Some(gid) = self.hits.card_at(pos).cloned() {
            let double = self
                .last_click
                .as_ref()
                .is_some_and(|(t, g)| *g == gid && now.duration_since(*t) <= DOUBLE_CLICK);
            self.select_gid(&gid);
            if double {
                self.open_details();
                self.last_click = None;
            } else {
                self.last_click = Some((now, gid));
            }
        } else if let Some(cmd) = self.hits.button_at(pos).or_else(|| self.hits.hint_at(pos)) {
            self.run(cmd);
        } else if self.hits.scrollbar.is_some_and(|r| r.contains(pos)) {
            self.dragging = true;
            self.scroll_to_row(pos.y);
        }
    }

    fn right_click(&mut self, pos: Position) {
        if self.dialog.is_some() {
            return;
        }
        match self.hits.card_at(pos).cloned() {
            Some(gid) => {
                self.select_gid(&gid);
                self.open_menu(gid, pos);
            }
            None => self.menu = None,
        }
    }

    fn wheel(&mut self, pos: Position, delta: isize) {
        if self.dialog.is_some() {
            return;
        }
        if self.menu.is_some() {
            self.menu = None;
        }
        if self.hits.list.contains(pos) {
            self.scroll(delta);
        } else if self.hits.tabs.iter().any(|(r, _)| r.y == pos.y) {
            let filter = if delta > 0 {
                self.filter.next()
            } else {
                self.filter.prev()
            };
            self.set_filter(filter);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::app::Action;
    use crate::tui::app::tests::app_with;
    use dlux_core::state::Filter;
    use ratatui::layout::Rect;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn mouse(kind: MouseEventKind, x: u16, y: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column: x,
            row: y,
            modifiers: KeyModifiers::NONE,
        }
    }

    fn click(app: &mut App, x: u16, y: u16, now: Instant) {
        app.on_mouse(mouse(MouseEventKind::Down(MouseButton::Left), x, y), now);
        app.on_mouse(mouse(MouseEventKind::Up(MouseButton::Left), x, y), now);
    }

    /// Fake a rendered frame: cards for a, b, c stacked at y = 10, 14, 18.
    fn with_cards(app: &mut App) {
        app.hits.list = Rect::new(0, 9, 60, 14);
        app.hits.cards = ["a", "b", "c"]
            .iter()
            .enumerate()
            .map(|(i, g)| (Rect::new(1, 10 + 4 * i as u16, 58, 3), (*g).into()))
            .collect();
        app.hits.tabs = Filter::ALL
            .iter()
            .enumerate()
            .map(|(i, f)| (Rect::new(1 + 10 * i as u16, 8, 9, 1), *f))
            .collect();
        app.hits.buttons = Command::TOOLBAR
            .iter()
            .enumerate()
            .map(|(i, c)| (Rect::new(1 + 10 * i as u16, 24, 9, 1), *c))
            .collect();
    }

    #[test]
    fn selection_keys_move_and_clamp() {
        let mut app = app_with(&["a", "b", "c"]);
        assert_eq!(app.selected.as_deref(), Some("a"));
        app.on_key(key(KeyCode::Char('j')));
        app.on_key(key(KeyCode::Down));
        assert_eq!(app.selected.as_deref(), Some("c"));
        app.on_key(key(KeyCode::Char('j')));
        assert_eq!(app.selected.as_deref(), Some("c"), "clamped at the end");
        app.on_key(key(KeyCode::Char('g')));
        assert_eq!(app.selected.as_deref(), Some("a"));
        app.on_key(key(KeyCode::Char('k')));
        assert_eq!(app.selected.as_deref(), Some("a"), "clamped at the start");
        app.on_key(key(KeyCode::End));
        assert_eq!(app.selected.as_deref(), Some("c"));
    }

    #[test]
    fn tab_keys_switch_filters() {
        let mut app = app_with(&["a"]);
        app.on_key(key(KeyCode::Tab));
        assert_eq!(app.filter, Filter::Waiting);
        assert_eq!(app.selected.as_deref(), Some("w1"));
        app.on_key(key(KeyCode::BackTab));
        assert_eq!(app.filter, Filter::Active);
        app.on_key(key(KeyCode::Char('3')));
        assert_eq!(app.filter, Filter::Done);
        assert_eq!(app.selected, None);
    }

    #[test]
    fn action_keys() {
        let mut app = app_with(&["a"]);
        app.on_key(key(KeyCode::Char('p')));
        app.on_key(key(KeyCode::Char('o')));
        assert_eq!(
            app.take_actions(),
            vec![
                Action::Pause("a".into()),
                Action::OpenFolder("/downloads".into())
            ]
        );
        app.on_key(key(KeyCode::Char('?')));
        assert_eq!(app.dialog, Some(Dialog::Help));
        app.on_key(key(KeyCode::Char('p')));
        assert!(app.take_actions().is_empty(), "the dialog swallows keys");
        app.on_key(key(KeyCode::Esc));
        assert!(app.dialog.is_none());
        assert!(!app.should_quit, "Esc closed the dialog, not the app");
        app.on_key(key(KeyCode::Char('d')));
        app.on_key(key(KeyCode::Char('y')));
        assert_eq!(app.take_actions(), vec![Action::Remove("a".into())]);
    }

    #[test]
    fn space_toggles_pause() {
        let mut app = app_with(&["a"]);
        app.on_key(key(KeyCode::Char(' ')));
        assert_eq!(app.take_actions(), vec![Action::Pause("a".into())]);
        app.set_filter(Filter::Waiting);
        app.on_key(key(KeyCode::Char(' ')));
        assert_eq!(app.take_actions(), vec![Action::Resume("w1".into())]);
    }

    #[test]
    fn quit_keys() {
        for k in [
            key(KeyCode::Char('q')),
            key(KeyCode::Esc),
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
        ] {
            let mut app = app_with(&[]);
            app.on_key(k);
            assert!(app.should_quit);
        }
        // Ctrl-C quits even with a dialog open.
        let mut app = app_with(&[]);
        app.dialog = Some(Dialog::Help);
        app.on_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(app.should_quit);
    }

    #[test]
    fn clicks_select_cards_tabs_and_buttons() {
        let mut app = app_with(&["a", "b", "c"]);
        with_cards(&mut app);
        let t0 = Instant::now();
        click(&mut app, 20, 15, t0);
        assert_eq!(app.selected.as_deref(), Some("b"));
        click(&mut app, 12, 24, t0); // Pause button
        assert_eq!(app.take_actions(), vec![Action::Pause("b".into())]);
        click(&mut app, 12, 8, t0); // Waiting tab
        assert_eq!(app.filter, Filter::Waiting);
        click(&mut app, 59, 0, t0); // nowhere
        assert_eq!(app.filter, Filter::Waiting);
    }

    #[test]
    fn double_click_opens_details() {
        let mut app = app_with(&["a", "b", "c"]);
        with_cards(&mut app);
        let t0 = Instant::now();
        click(&mut app, 5, 10, t0);
        assert!(!app.details);
        click(&mut app, 5, 11, t0 + Duration::from_millis(200));
        assert!(app.details, "double-click shows details");

        app.details = false;
        click(&mut app, 5, 10, t0 + Duration::from_secs(5));
        click(&mut app, 5, 10, t0 + Duration::from_secs(6));
        assert!(!app.details, "too slow for a double-click");
        click(&mut app, 5, 14, t0 + Duration::from_millis(6100));
        assert!(!app.details, "different card");
    }

    #[test]
    fn right_click_menu() {
        let mut app = app_with(&["a", "b", "c"]);
        with_cards(&mut app);
        app.on_mouse(
            mouse(MouseEventKind::Down(MouseButton::Right), 30, 18),
            Instant::now(),
        );
        assert_eq!(app.selected.as_deref(), Some("c"));
        let menu = app.menu.clone().expect("menu open");
        assert_eq!(menu.gid, "c");
        assert_eq!(menu.anchor, Position::new(30, 18));

        // Fake the rendered menu and click "Open folder".
        app.hits.menu = Some(Rect::new(30, 18, 20, 8));
        app.hits.menu_items = Command::MENU
            .iter()
            .enumerate()
            .map(|(i, c)| (Rect::new(31, 19 + i as u16, 18, 1), *c))
            .collect();
        app.on_mouse(mouse(MouseEventKind::Moved, 35, 23), Instant::now());
        assert_eq!(
            app.menu.as_ref().unwrap().cursor,
            4,
            "hover moves the cursor"
        );
        click(&mut app, 35, 23, Instant::now());
        assert!(app.menu.is_none());
        assert_eq!(
            app.take_actions(),
            vec![Action::OpenFolder("/downloads".into())]
        );

        // A click outside closes the menu without selecting what is underneath.
        app.on_mouse(
            mouse(MouseEventKind::Down(MouseButton::Right), 30, 18),
            Instant::now(),
        );
        click(&mut app, 5, 10, Instant::now());
        assert!(app.menu.is_none());
        assert_eq!(app.selected.as_deref(), Some("c"));
    }

    #[test]
    fn menu_keyboard() {
        let mut app = app_with(&["a"]);
        app.on_key(key(KeyCode::Char('m')));
        assert!(app.menu.is_some());
        app.on_key(key(KeyCode::Down)); // Resume (disabled)
        app.on_key(key(KeyCode::Enter));
        assert!(app.take_actions().is_empty(), "disabled item does nothing");
        assert!(app.menu.is_none());
        app.on_key(key(KeyCode::Char('m')));
        app.on_key(key(KeyCode::Char('p')));
        assert_eq!(app.take_actions(), vec![Action::Pause("a".into())]);
    }

    #[test]
    fn dialog_buttons_and_outside_click() {
        let mut app = app_with(&["a"]);
        app.on_key(key(KeyCode::Char('d')));
        app.hits.dialog = Some(Rect::new(10, 5, 40, 8));
        app.hits.dialog_buttons = vec![
            (Rect::new(20, 11, 10, 1), DialogButton::Confirm),
            (Rect::new(32, 11, 10, 1), DialogButton::Cancel),
        ];
        click(&mut app, 15, 7, Instant::now());
        assert!(
            app.dialog.is_some(),
            "click inside the dialog body keeps it"
        );
        click(&mut app, 0, 0, Instant::now());
        assert!(app.dialog.is_none(), "click outside cancels");
        assert!(app.take_actions().is_empty());

        app.on_key(key(KeyCode::Char('d')));
        click(&mut app, 22, 11, Instant::now());
        assert_eq!(app.take_actions(), vec![Action::Remove("a".into())]);
    }

    #[test]
    fn wheel_and_scrollbar_drag() {
        let gids: Vec<String> = (0..20).map(|i| format!("g{i}")).collect();
        let refs: Vec<&str> = gids.iter().map(String::as_str).collect();
        let mut app = app_with(&refs);
        app.page_size = 3;
        app.hits.list = Rect::new(0, 5, 60, 13);
        app.hits.scrollbar = Some(Rect::new(59, 6, 1, 11));
        app.on_mouse(mouse(MouseEventKind::ScrollDown, 10, 8), Instant::now());
        app.on_mouse(mouse(MouseEventKind::ScrollDown, 10, 8), Instant::now());
        assert_eq!(app.offset, 2);
        app.on_mouse(mouse(MouseEventKind::ScrollUp, 10, 8), Instant::now());
        assert_eq!(app.offset, 1);
        app.on_mouse(mouse(MouseEventKind::ScrollDown, 10, 0), Instant::now());
        assert_eq!(app.offset, 1, "wheel outside the list does not scroll it");

        app.on_mouse(
            mouse(MouseEventKind::Down(MouseButton::Left), 59, 6),
            Instant::now(),
        );
        assert!(app.dragging);
        assert_eq!(app.offset, 0);
        app.on_mouse(
            mouse(MouseEventKind::Drag(MouseButton::Left), 30, 16),
            Instant::now(),
        );
        assert_eq!(
            app.offset, 17,
            "dragging follows the row even off the track"
        );
        app.on_mouse(
            mouse(MouseEventKind::Up(MouseButton::Left), 30, 16),
            Instant::now(),
        );
        assert!(!app.dragging);
        app.on_mouse(
            mouse(MouseEventKind::Drag(MouseButton::Left), 30, 6),
            Instant::now(),
        );
        assert_eq!(app.offset, 17, "no drag after release");
    }

    fn typed(app: &mut App, text: &str) {
        for c in text.chars() {
            app.on_key(key(KeyCode::Char(c)));
        }
    }

    #[test]
    fn paste_opens_add_dialog_and_enter_adds() {
        let mut app = app_with(&["a"]);
        app.default_dir = "/dl".into();
        app.on_event(Event::Paste("https://example.com/x.iso\r\n".into()));
        let Some(Dialog::Add(form)) = &app.dialog else {
            panic!("paste should open the Add dialog");
        };
        assert_eq!(form.urls.text(), "https://example.com/x.iso");
        // Keys go to the form, not the main screen.
        app.on_key(key(KeyCode::Char('q')));
        assert!(!app.should_quit);
        app.on_key(key(KeyCode::Backspace));
        app.on_key(key(KeyCode::Enter));
        assert!(app.dialog.is_none());
        match app.take_actions().as_slice() {
            [Action::Add { sources, options }] => {
                assert_eq!(sources.len(), 1);
                assert_eq!(options.dir.as_deref(), Some("/dl"));
            }
            other => panic!("unexpected actions {other:?}"),
        }
        assert_eq!(app.filter, Filter::Active);
    }

    #[test]
    fn add_form_keeps_errors_and_fields() {
        let mut app = app_with(&[]);
        app.on_key(key(KeyCode::Char('a')));
        app.on_key(key(KeyCode::Enter));
        let error = |app: &mut App| match &app.dialog {
            Some(Dialog::Add(f)) => f.error.clone(),
            _ => panic!("dialog closed"),
        };
        assert!(
            error(&mut app).is_some(),
            "empty form is rejected but stays open"
        );
        typed(&mut app, "not-a-url");
        assert!(error(&mut app).is_none(), "typing clears the error");
        app.on_key(key(KeyCode::Enter));
        assert!(error(&mut app).unwrap().contains("not-a-url"));

        // Tab to the checkbox, toggle it with space, tab around to the URLs again.
        app.on_key(key(KeyCode::Tab));
        app.on_key(key(KeyCode::Tab));
        app.on_key(key(KeyCode::Char(' ')));
        app.on_key(key(KeyCode::Tab));
        let Some(Dialog::Add(form)) = &app.dialog else {
            panic!()
        };
        assert!(form.paused);
        assert_eq!(form.focus, crate::tui::forms::Field::Urls);

        // A click outside a form does not throw it away; Esc does.
        app.hits.dialog = Some(Rect::new(10, 5, 40, 10));
        click(&mut app, 0, 0, Instant::now());
        assert!(app.dialog.is_some());
        app.on_key(key(KeyCode::Esc));
        assert!(app.dialog.is_none());
        assert!(!app.should_quit);
        assert!(app.take_actions().is_empty());
    }

    #[test]
    fn global_controls() {
        let mut app = app_with(&["a"]);
        app.on_key(key(KeyCode::Char('P')));
        app.on_key(key(KeyCode::Char('R'))); // w1 is paused
        app.on_key(key(KeyCode::Char('X'))); // e1 has stopped
        assert_eq!(
            app.take_actions(),
            vec![Action::PauseAll, Action::ResumeAll, Action::Purge]
        );
        let mut empty = app_with(&[]);
        empty.state.apply(dlux_core::rpc::Snapshot::default());
        empty.on_key(key(KeyCode::Char('P')));
        empty.on_key(key(KeyCode::Char('X')));
        assert!(empty.take_actions().is_empty(), "nothing to pause or clear");
    }

    #[test]
    fn speed_limit_dialog_round_trip() {
        use crate::tui::actions::Feedback;
        use dlux_core::limits::{Limits, Target};
        let mut app = app_with(&["a"]);
        app.on_key(key(KeyCode::Char('s')));
        assert_eq!(
            app.take_actions(),
            vec![Action::LoadLimits(Target::Download("a".into()))]
        );
        // Values from aria2 fill the form…
        app.on_feedback(Feedback::Limits(
            Target::Download("a".into()),
            Limits {
                download: 1 << 20,
                upload: 0,
            },
        ));
        // …and the user edits the download limit.
        app.on_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        typed(&mut app, "500K");
        app.on_key(key(KeyCode::Enter));
        assert_eq!(
            app.take_actions(),
            vec![Action::SetLimits(
                Target::Download("a".into()),
                Limits {
                    download: 500 * 1024,
                    upload: 0
                }
            )]
        );
        assert!(app.dialog.is_none());

        // The global limit from the header pill.
        app.hits.speed = Some(Rect::new(60, 1, 20, 1));
        click(&mut app, 65, 1, Instant::now());
        assert!(matches!(&app.dialog, Some(Dialog::Limits(f)) if f.target == Target::Global));
        app.on_feedback(Feedback::Limits(Target::Global, Limits::default()));
        assert_eq!(app.global_limits, Some(Limits::default()));
    }
}
