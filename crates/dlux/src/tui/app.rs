//! TUI state: which tab is shown, which download is selected, scrolling, and input handling.

use super::theme::Theme;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use dlux_core::rpc::{Download, Gid};
use dlux_core::state::{AppState, Filter};
use ratatui::layout::Rect;

/// Static information about the aria2 connection, shown in the header.
#[derive(Debug, Clone, Default)]
pub struct ConnectionInfo {
    pub aria2_version: String,
    /// Short description, e.g. "started by dlux" or "localhost:6800".
    pub origin: String,
}

/// Clickable regions recorded while rendering, used for mouse hit-testing.
#[derive(Debug, Clone, Default)]
pub struct HitMap {
    pub tabs: Vec<(Rect, Filter)>,
    pub cards: Vec<(Rect, Gid)>,
    pub buttons: Vec<(Rect, Button)>,
    pub list: Rect,
    pub scrollbar: Option<Rect>,
}

/// Toolbar buttons.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    Add,
    Pause,
    Resume,
    Remove,
}

impl Button {
    pub const ALL: [Button; 4] = [Self::Add, Self::Pause, Self::Resume, Self::Remove];

    pub fn label(self) -> &'static str {
        match self {
            Self::Add => "+ Add",
            Self::Pause => "Pause",
            Self::Resume => "Resume",
            Self::Remove => "Remove",
        }
    }
}

#[derive(Debug)]
pub struct App {
    pub state: AppState,
    pub filter: Filter,
    /// GID of the selected download; kept stable across refreshes.
    pub selected: Option<Gid>,
    /// Index of the selection in the current list (used when the selected download leaves it).
    pub selected_index: usize,
    /// Index of the first visible card.
    pub offset: usize,
    /// Number of cards that fit in the list (set while rendering).
    pub page_size: usize,
    /// Details pane toggled on (always shown on wide terminals).
    pub details: bool,
    pub theme: Theme,
    pub info: ConnectionInfo,
    pub hits: HitMap,
    pub should_quit: bool,
}

impl App {
    pub fn new(theme: Theme, info: ConnectionInfo) -> Self {
        Self {
            state: AppState::default(),
            filter: Filter::default(),
            selected: None,
            selected_index: 0,
            offset: 0,
            page_size: 1,
            details: false,
            theme,
            info,
            hits: HitMap::default(),
            should_quit: false,
        }
    }

    /// Downloads under the current tab.
    pub fn visible(&self) -> Vec<&Download> {
        self.state.downloads(self.filter)
    }

    pub fn selected_download(&self) -> Option<&Download> {
        self.selected
            .as_deref()
            .and_then(|gid| self.visible().into_iter().find(|d| d.gid == gid))
    }

    /// Re-resolve the selection against the current list: keep the same GID if it is still
    /// listed, otherwise select whatever now sits at the old index.
    pub fn sync_selection(&mut self) {
        let list = self.visible();
        if list.is_empty() {
            self.selected = None;
            self.selected_index = 0;
            self.offset = 0;
            return;
        }
        let index = self
            .selected
            .as_deref()
            .and_then(|gid| list.iter().position(|d| d.gid == gid))
            .unwrap_or_else(|| self.selected_index.min(list.len() - 1));
        let gid = list[index].gid.clone();
        let len = list.len();
        self.selected = Some(gid);
        self.selected_index = index;
        self.offset = self.offset.min(len.saturating_sub(1));
    }

    fn select_index(&mut self, index: usize) {
        let list = self.visible();
        if list.is_empty() {
            return;
        }
        let index = index.min(list.len() - 1);
        let gid = list[index].gid.clone();
        self.selected = Some(gid);
        self.selected_index = index;
    }

    pub fn move_selection(&mut self, delta: isize) {
        self.sync_selection();
        let target = self.selected_index.saturating_add_signed(delta);
        self.select_index(target);
    }

    pub fn select_first(&mut self) {
        self.select_index(0);
    }

    pub fn select_last(&mut self) {
        self.select_index(usize::MAX);
    }

    pub fn set_filter(&mut self, filter: Filter) {
        if filter != self.filter {
            self.filter = filter;
            self.selected = None;
            self.selected_index = 0;
            self.offset = 0;
            self.sync_selection();
        }
    }

    /// Adjust the scroll offset so the selection is visible.
    pub fn ensure_visible(&mut self, len: usize) {
        let page = self.page_size.max(1);
        if self.selected_index < self.offset {
            self.offset = self.selected_index;
        } else if self.selected_index >= self.offset + page {
            self.offset = self.selected_index + 1 - page;
        }
        self.offset = self.offset.min(len.saturating_sub(page));
    }

    pub fn on_event(&mut self, event: Event) {
        if let Event::Key(key) = event {
            self.on_key(key);
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        if key.kind == KeyEventKind::Release {
            return;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let page = self.page_size.max(1) as isize;
        match key.code {
            KeyCode::Char('c') if ctrl => self.should_quit = true,
            KeyCode::Char('q') | KeyCode::Esc => self.should_quit = true,
            KeyCode::Tab => self.set_filter(self.filter.next()),
            KeyCode::BackTab => self.set_filter(self.filter.prev()),
            KeyCode::Char(c @ '1'..='4') => {
                self.set_filter(Filter::ALL[(c as usize) - ('1' as usize)]);
            }
            KeyCode::Char('j') | KeyCode::Down => self.move_selection(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_selection(-1),
            KeyCode::PageDown => self.move_selection(page),
            KeyCode::PageUp => self.move_selection(-page),
            KeyCode::Char('g') | KeyCode::Home => self.select_first(),
            KeyCode::Char('G') | KeyCode::End => self.select_last(),
            KeyCode::Enter => self.details = !self.details,
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dlux_core::rpc::{DownloadStatus, Snapshot};

    fn dl(gid: &str, status: DownloadStatus) -> Download {
        Download {
            gid: gid.into(),
            status,
            ..Default::default()
        }
    }

    fn app_with(active: &[&str]) -> App {
        let mut app = App::new(Theme::TOKYO_NIGHT, ConnectionInfo::default());
        app.state.apply(Snapshot {
            active: active
                .iter()
                .map(|g| dl(g, DownloadStatus::Active))
                .collect(),
            waiting: vec![dl("w1", DownloadStatus::Paused)],
            ..Default::default()
        });
        app.sync_selection();
        app
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn selection_moves_and_clamps() {
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
    fn selection_follows_gid_across_refreshes() {
        let mut app = app_with(&["a", "b", "c"]);
        app.move_selection(1);
        assert_eq!(app.selected.as_deref(), Some("b"));
        // "a" finished: "b" moves to index 0 but stays selected.
        app.state.apply(Snapshot {
            active: vec![
                dl("b", DownloadStatus::Active),
                dl("c", DownloadStatus::Active),
            ],
            ..Default::default()
        });
        app.sync_selection();
        assert_eq!(app.selected.as_deref(), Some("b"));
        assert_eq!(app.selected_index, 0);
        // "b" finished too: the selection falls to what is now at its index.
        app.state.apply(Snapshot {
            active: vec![dl("c", DownloadStatus::Active)],
            ..Default::default()
        });
        app.sync_selection();
        assert_eq!(app.selected.as_deref(), Some("c"));
    }

    #[test]
    fn tabs_switch_and_reset_selection() {
        let mut app = app_with(&["a"]);
        app.on_key(key(KeyCode::Tab));
        assert_eq!(app.filter, Filter::Waiting);
        assert_eq!(app.selected.as_deref(), Some("w1"));
        app.on_key(key(KeyCode::BackTab));
        assert_eq!(app.filter, Filter::Active);
        app.on_key(key(KeyCode::Char('4')));
        assert_eq!(app.filter, Filter::Errors);
        assert_eq!(app.selected, None);
    }

    #[test]
    fn scrolling_keeps_selection_visible() {
        let gids: Vec<String> = (0..10).map(|i| format!("g{i}")).collect();
        let refs: Vec<&str> = gids.iter().map(String::as_str).collect();
        let mut app = app_with(&refs);
        app.page_size = 3;
        app.select_index(5);
        app.ensure_visible(10);
        assert_eq!(app.offset, 3);
        app.select_index(1);
        app.ensure_visible(10);
        assert_eq!(app.offset, 1);
        app.select_last();
        app.ensure_visible(10);
        assert_eq!(app.offset, 7);
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
    }
}
