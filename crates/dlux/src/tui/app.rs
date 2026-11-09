//! TUI state: which tab is shown, which download is selected, scrolling, overlays (context menu,
//! dialogs, toast) and the queue of side effects for the event loop to perform.
//!
//! Input handling lives in [`super::input`]; this module holds the state it operates on and the
//! [`Command`]s that keys, toolbar buttons, menu items and hints all map to.

use super::actions::Feedback;
use super::forms::{AddForm, AddRequest, Field, Form, LimitsForm, VideoForm};
use super::theme::Theme;
use dlux_core::jobs::{self, Job};
use dlux_core::limits::{Limits, Target};
use dlux_core::rpc::{Download, DownloadStatus, Gid};
use dlux_core::sources::{AddOptions, Source};
use dlux_core::state::{AppState, Filter};
use dlux_core::ytdlp::DownloadSpec;
use ratatui::layout::{Position, Rect};
use std::time::{Duration, Instant};

/// How long a toast stays on screen.
pub const TOAST_DURATION: Duration = Duration::from_secs(4);

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
    pub buttons: Vec<(Rect, Command)>,
    pub hints: Vec<(Rect, Command)>,
    pub list: Rect,
    pub scrollbar: Option<Rect>,
    pub menu: Option<Rect>,
    pub menu_items: Vec<(Rect, Command)>,
    pub dialog: Option<Rect>,
    pub dialog_buttons: Vec<(Rect, DialogButton)>,
    /// Form controls: clickable area, the field, and where its text is drawn.
    pub fields: Vec<(Rect, Field, Rect)>,
    /// Rows of the video format picker.
    pub choices: Vec<(Rect, usize)>,
    /// The ↓/↑ speed pills in the header (click to set the global limit).
    pub speed: Option<Rect>,
    /// Whether the terminal is wide enough to show the details pane by default.
    pub wide: bool,
}

fn hit<T: Copy>(regions: &[(Rect, T)], pos: Position) -> Option<T> {
    regions
        .iter()
        .find(|(r, _)| r.contains(pos))
        .map(|(_, v)| *v)
}

impl HitMap {
    pub fn tab_at(&self, pos: Position) -> Option<Filter> {
        hit(&self.tabs, pos)
    }

    pub fn card_at(&self, pos: Position) -> Option<&Gid> {
        self.cards
            .iter()
            .find(|(r, _)| r.contains(pos))
            .map(|(_, g)| g)
    }

    pub fn button_at(&self, pos: Position) -> Option<Command> {
        hit(&self.buttons, pos)
    }

    pub fn hint_at(&self, pos: Position) -> Option<Command> {
        hit(&self.hints, pos)
    }

    pub fn menu_item_at(&self, pos: Position) -> Option<Command> {
        hit(&self.menu_items, pos)
    }

    pub fn dialog_button_at(&self, pos: Position) -> Option<DialogButton> {
        hit(&self.dialog_buttons, pos)
    }

    /// The form field under `pos`, with the area its text occupies.
    pub fn field_at(&self, pos: Position) -> Option<(Field, Rect)> {
        self.fields
            .iter()
            .find(|(r, _, _)| r.contains(pos))
            .map(|(_, f, text)| (*f, *text))
    }
}

/// Everything the user can ask for. Keys, toolbar buttons, context-menu items and the
/// clickable key hints all resolve to one of these, so they behave identically.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Add,
    Pause,
    Resume,
    Remove,
    PauseAll,
    ResumeAll,
    Purge,
    SpeedLimit,
    GlobalLimit,
    UpdateYtDlp,
    OpenFolder,
    CopyUrl,
    Details,
    Menu,
    Help,
    Quit,
}

impl Command {
    /// Buttons in the bottom toolbar, in order of importance (narrow terminals drop the last).
    pub const TOOLBAR: [Command; 7] = [
        Self::Add,
        Self::Pause,
        Self::Resume,
        Self::Remove,
        Self::PauseAll,
        Self::ResumeAll,
        Self::Purge,
    ];
    /// Items of a download's context menu.
    pub const MENU: [Command; 6] = [
        Self::Pause,
        Self::Resume,
        Self::Remove,
        Self::SpeedLimit,
        Self::OpenFolder,
        Self::CopyUrl,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Add => "+ Add",
            Self::Pause => "Pause",
            Self::Resume => "Resume",
            Self::Remove => "Remove",
            Self::PauseAll => "Pause all",
            Self::ResumeAll => "Resume all",
            Self::Purge => "Clear finished",
            Self::SpeedLimit => "Speed limit…",
            Self::GlobalLimit => "Global speed limit…",
            Self::UpdateYtDlp => "Update yt-dlp",
            Self::OpenFolder => "Open folder",
            Self::CopyUrl => "Copy URL",
            Self::Details => "Details",
            Self::Menu => "Menu",
            Self::Help => "Help",
            Self::Quit => "Quit",
        }
    }

    /// The keyboard shortcut, as shown in menus.
    pub fn key(self) -> &'static str {
        match self {
            Self::Add => "a",
            Self::Pause => "p",
            Self::Resume => "r",
            Self::Remove => "d",
            Self::PauseAll => "P",
            Self::ResumeAll => "R",
            Self::Purge => "X",
            Self::SpeedLimit => "s",
            Self::GlobalLimit => "S",
            Self::UpdateYtDlp => "U",
            Self::OpenFolder => "o",
            Self::CopyUrl => "c",
            Self::Details => "enter",
            Self::Menu => "m",
            Self::Help => "?",
            Self::Quit => "q",
        }
    }

    /// Whether the command applies, given the selected download. Commands acting on all
    /// downloads are refined by [`App::can`].
    pub fn enabled(self, selected: Option<&Download>) -> bool {
        match self {
            Self::Add | Self::Details | Self::Help | Self::Quit => true,
            Self::PauseAll | Self::ResumeAll | Self::Purge | Self::GlobalLimit => true,
            Self::UpdateYtDlp => true,
            // yt-dlp jobs run their own aria2c, which dlux can't limit per download.
            Self::SpeedLimit => {
                selected.is_some_and(|d| !d.status.is_stopped() && !jobs::is_local(&d.gid))
            }
            Self::Pause => selected.is_some_and(|d| {
                matches!(d.status, DownloadStatus::Active | DownloadStatus::Waiting)
            }),
            // A failed video can be retried: yt-dlp continues where it stopped.
            Self::Resume => selected.is_some_and(|d| {
                d.status == DownloadStatus::Paused
                    || (d.status == DownloadStatus::Error && jobs::is_local(&d.gid))
            }),
            Self::Remove | Self::Menu => selected.is_some(),
            Self::OpenFolder => selected.is_some_and(|d| !d.dir.is_empty()),
            Self::CopyUrl => selected.and_then(copy_text).is_some(),
        }
    }
}

/// What "Copy URL" copies: the first source URI, or a magnet link for torrents without one.
pub fn copy_text(d: &Download) -> Option<String> {
    if let Some(uri) = d.first_uri() {
        return Some(uri.to_owned());
    }
    d.info_hash
        .as_deref()
        .filter(|h| !h.is_empty())
        .map(|h| format!("magnet:?xt=urn:btih:{h}"))
}

/// A side effect requested by the UI, performed by the event loop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Pause(Gid),
    Resume(Gid),
    /// Stop an unfinished download (aria2 keeps it in the stopped list as "removed").
    Remove(Gid),
    /// Forget a finished, failed or removed download.
    RemoveResult(Gid),
    OpenFolder(String),
    Copy(String),
    Add {
        sources: Vec<Source>,
        options: AddOptions,
    },
    PauseAll,
    ResumeAll,
    /// Forget all finished, failed and removed downloads.
    Purge,
    LoadLimits(Target),
    SetLimits(Target, Limits),
    /// Ask yt-dlp for the page's formats.
    Probe(String),
    StartVideo(DownloadSpec),
    UpdateYtDlp,
}

/// A modal dialog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Dialog {
    Help,
    ConfirmRemove { gid: Gid, name: String },
    Add(AddForm),
    Limits(LimitsForm),
    Video(VideoForm),
}

impl Dialog {
    /// The dialog as a form, if it is one.
    pub fn form(&mut self) -> Option<&mut dyn Form> {
        match self {
            Self::Add(f) => Some(f),
            Self::Limits(f) => Some(f),
            Self::Help | Self::ConfirmRemove { .. } | Self::Video(_) => None,
        }
    }
}

/// Buttons inside a dialog. In the help dialog `Cancel` is the "Close" button.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialogButton {
    Confirm,
    Cancel,
    /// Video picker: add the URL to aria2 as a plain file instead.
    Fallback,
    /// Video picker: run `yt-dlp -U`.
    UpdateYtDlp,
}

/// An open context menu for one download.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Menu {
    pub gid: Gid,
    /// Where the menu opens (its top-left corner, moved to fit on screen).
    pub anchor: Position,
    /// Highlighted item index in [`Command::MENU`].
    pub cursor: usize,
}

/// A short message shown in the toolbar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Toast {
    pub text: String,
    pub error: bool,
    pub expires: Instant,
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
    /// Keep the selection scrolled into view. Cleared by wheel and scrollbar scrolling so the
    /// view can move away from the selection; set again by keyboard navigation.
    pub follow: bool,
    /// Number of cards that fit in the list (set while rendering).
    pub page_size: usize,
    /// Details pane toggled on (always shown on wide terminals).
    pub details: bool,
    pub theme: Theme,
    pub info: ConnectionInfo,
    pub hits: HitMap,
    /// Last known mouse position, for hover highlights.
    pub hover: Option<Position>,
    pub menu: Option<Menu>,
    pub dialog: Option<Dialog>,
    pub toast: Option<Toast>,
    /// The scrollbar thumb is being dragged.
    pub dragging: bool,
    /// Last left click on a card, for double-click detection.
    pub last_click: Option<(Instant, Gid)>,
    /// Side effects waiting for the event loop.
    pub actions: Vec<Action>,
    /// Pre-filled "Save to" directory of the Add dialog.
    pub default_dir: String,
    /// Global speed limits, once known.
    pub global_limits: Option<Limits>,
    /// Local jobs (yt-dlp videos, tool installs), for card tags and status notes.
    pub jobs: Vec<Job>,
    /// Last click on a format-picker row, for double-click.
    pub last_choice_click: Option<(Instant, usize)>,
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
            follow: true,
            page_size: 1,
            details: false,
            theme,
            info,
            hits: HitMap::default(),
            hover: None,
            menu: None,
            dialog: None,
            toast: None,
            dragging: false,
            last_click: None,
            actions: Vec::new(),
            default_dir: String::new(),
            global_limits: None,
            jobs: Vec::new(),
            last_choice_click: None,
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
    /// listed, otherwise select whatever now sits at the old index. Also drops a context menu
    /// whose download disappeared.
    pub fn sync_selection(&mut self) {
        if let Some(menu) = &self.menu
            && self.state.find(&menu.gid).is_none()
        {
            self.menu = None;
        }
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

    pub fn select_index(&mut self, index: usize) {
        let list = self.visible();
        if list.is_empty() {
            return;
        }
        let index = index.min(list.len() - 1);
        let gid = list[index].gid.clone();
        self.selected = Some(gid);
        self.selected_index = index;
        self.follow = true;
    }

    /// Select a download by GID if it is in the current list.
    pub fn select_gid(&mut self, gid: &str) {
        if let Some(index) = self.visible().iter().position(|d| d.gid == gid) {
            self.select_index(index);
        }
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
            self.follow = true;
            self.menu = None;
            self.sync_selection();
        }
    }

    /// Largest valid scroll offset for a list of `len` cards.
    fn max_offset(&self, len: usize) -> usize {
        len.saturating_sub(self.page_size.max(1))
    }

    /// Adjust the scroll offset so the selection is visible (when following it) and in range.
    pub fn ensure_visible(&mut self, len: usize) {
        let page = self.page_size.max(1);
        if self.follow {
            if self.selected_index < self.offset {
                self.offset = self.selected_index;
            } else if self.selected_index >= self.offset + page {
                self.offset = self.selected_index + 1 - page;
            }
        }
        self.offset = self.offset.min(self.max_offset(len));
    }

    /// Scroll the view by `delta` cards without moving the selection.
    pub fn scroll(&mut self, delta: isize) {
        let max = self.max_offset(self.visible().len());
        self.offset = self.offset.saturating_add_signed(delta).min(max);
        self.follow = false;
    }

    /// Scroll so the scrollbar thumb sits at row `y` (a click or drag on the track).
    pub fn scroll_to_row(&mut self, y: u16) {
        let Some(track) = self.hits.scrollbar.filter(|t| t.height > 0) else {
            return;
        };
        let max = self.max_offset(self.visible().len());
        let span = usize::from(track.height.saturating_sub(1));
        let rel = usize::from(y.clamp(track.top(), track.bottom().saturating_sub(1)) - track.y);
        // Round to the nearest offset; a one-row track can only show the top.
        self.offset = (rel * max + span / 2).checked_div(span).unwrap_or(0);
        self.follow = false;
    }

    /// Show the details pane, whatever the terminal width.
    pub fn open_details(&mut self) {
        // The pane is shown when `wide != details`.
        self.details = !self.hits.wide;
    }

    pub fn notify(&mut self, text: impl Into<String>, error: bool) {
        self.toast = Some(Toast {
            text: text.into(),
            error,
            expires: Instant::now() + TOAST_DURATION,
        });
    }

    /// The toast, unless it has expired.
    pub fn current_toast(&self, now: Instant) -> Option<&Toast> {
        self.toast.as_ref().filter(|t| t.expires > now)
    }

    /// Hand the queued side effects to the event loop.
    pub fn take_actions(&mut self) -> Vec<Action> {
        std::mem::take(&mut self.actions)
    }

    /// Open the context menu for `gid` at `anchor`.
    pub fn open_menu(&mut self, gid: Gid, anchor: Position) {
        let selected = self.state.find(&gid);
        let cursor = Command::MENU
            .iter()
            .position(|c| c.enabled(selected))
            .unwrap_or(0);
        self.menu = Some(Menu {
            gid,
            anchor,
            cursor,
        });
    }

    /// Whether `cmd` can run now.
    pub fn can(&self, cmd: Command) -> bool {
        let any = |f: fn(&Download) -> bool| self.state.snapshot.iter().any(f);
        match cmd {
            Command::PauseAll => {
                any(|d| matches!(d.status, DownloadStatus::Active | DownloadStatus::Waiting))
            }
            Command::ResumeAll => any(|d| d.status == DownloadStatus::Paused),
            Command::Purge => any(|d| d.status.is_stopped()),
            _ => cmd.enabled(self.selected_download()),
        }
    }

    /// Open the Add dialog, pre-filled with `text` (e.g. something pasted).
    pub fn open_add(&mut self, text: &str) {
        self.menu = None;
        self.dialog = Some(Dialog::Add(AddForm::new(&self.default_dir, text)));
    }

    fn open_limits(&mut self, target: Target, subject: String) {
        let mut form = LimitsForm::new(target.clone(), subject);
        if let (Target::Global, Some(current)) = (&target, self.global_limits) {
            form.fill(current);
        }
        self.dialog = Some(Dialog::Limits(form));
        self.actions.push(Action::LoadLimits(target));
    }

    /// Replace the local jobs (polled from the job manager).
    pub fn set_jobs(&mut self, jobs: Vec<Job>) {
        self.state
            .set_local(jobs.iter().rev().map(Job::to_download).collect());
        self.jobs = jobs;
    }

    /// The local job behind `gid`, if it is one.
    pub fn job(&self, gid: &str) -> Option<&Job> {
        let id = jobs::local_id(gid)?;
        self.jobs.iter().find(|j| j.id == id)
    }

    /// Apply a message from the action executor.
    pub fn on_feedback(&mut self, feedback: Feedback) {
        match feedback {
            Feedback::Probe { url, result } => {
                if let Some(Dialog::Video(form)) = &mut self.dialog
                    && form.url == url
                {
                    match result {
                        Ok(info) => {
                            form.info = Some(info);
                            form.error = None;
                            form.cursor = 0;
                        }
                        Err(e) => form.error = Some(e),
                    }
                }
            }
            Feedback::Toast { text, error } => self.notify(text, error),
            Feedback::Limits(target, limits) => {
                if target == Target::Global {
                    self.global_limits = Some(limits);
                }
                if let Some(Dialog::Limits(form)) = &mut self.dialog
                    && form.target == target
                {
                    form.fill(limits);
                }
            }
        }
    }

    /// Run `cmd` against the selected download. Disabled commands do nothing.
    pub fn run(&mut self, cmd: Command) {
        self.menu = None;
        if !self.can(cmd) {
            return;
        }
        let selected = self.selected_download().cloned();
        let gid = || selected.as_ref().map(|d| d.gid.clone()).unwrap_or_default();
        match cmd {
            Command::Add => self.open_add(""),
            Command::PauseAll => self.actions.push(Action::PauseAll),
            Command::ResumeAll => self.actions.push(Action::ResumeAll),
            Command::Purge => self.actions.push(Action::Purge),
            Command::SpeedLimit => {
                let name = selected.as_ref().map(Download::name).unwrap_or_default();
                self.open_limits(Target::Download(gid()), name);
            }
            Command::GlobalLimit => self.open_limits(Target::Global, "All downloads".into()),
            Command::UpdateYtDlp => {
                self.actions.push(Action::UpdateYtDlp);
                self.notify("Updating yt-dlp…", false);
            }
            Command::Pause => self.actions.push(Action::Pause(gid())),
            Command::Resume => self.actions.push(Action::Resume(gid())),
            Command::Remove => {
                let d = selected.as_ref().expect("enabled implies a selection");
                if d.status.is_stopped() {
                    self.actions.push(Action::RemoveResult(gid()));
                } else {
                    self.dialog = Some(Dialog::ConfirmRemove {
                        gid: gid(),
                        name: d.name(),
                    });
                }
            }
            Command::OpenFolder => {
                let dir = selected.map(|d| d.dir).unwrap_or_default();
                self.actions.push(Action::OpenFolder(dir));
            }
            Command::CopyUrl => {
                if let Some(text) = selected.as_ref().and_then(copy_text) {
                    self.actions.push(Action::Copy(text));
                }
            }
            // Shown when `wide != details`, so flipping always flips visibility.
            Command::Details => self.details = !self.details,
            Command::Menu => {
                let gid = gid();
                let anchor = self
                    .hits
                    .cards
                    .iter()
                    .find(|(_, g)| *g == gid)
                    .map(|(r, _)| Position::new(r.x + 2, r.y + 1))
                    .unwrap_or_else(|| Position::new(self.hits.list.x + 2, self.hits.list.y + 1));
                self.open_menu(gid, anchor);
            }
            Command::Help => self.dialog = Some(Dialog::Help),
            Command::Quit => self.should_quit = true,
        }
    }

    /// Press a dialog button. A form that fails validation stays open showing the error.
    pub fn press_dialog(&mut self, button: DialogButton) {
        let Some(dialog) = self.dialog.take() else {
            return;
        };
        if button == DialogButton::Cancel {
            return;
        }
        match dialog {
            Dialog::Help => {}
            Dialog::ConfirmRemove { gid, .. } => self.actions.push(Action::Remove(gid)),
            Dialog::Video(form) => self.press_video(form, button),
            Dialog::Add(mut form) => match form.submit() {
                Ok(AddRequest::Files(sources, options)) => {
                    let paused = options.paused;
                    self.actions.push(Action::Add { sources, options });
                    self.set_filter(if paused {
                        Filter::Waiting
                    } else {
                        Filter::Active
                    });
                }
                Ok(AddRequest::Video { url, dir }) => {
                    self.actions.push(Action::Probe(url.clone()));
                    self.dialog = Some(Dialog::Video(VideoForm::new(url, dir)));
                }
                Err(e) => {
                    form.error = Some(e);
                    self.dialog = Some(Dialog::Add(form));
                }
            },
            Dialog::Limits(mut form) => match form.submit() {
                Ok(limits) => self.actions.push(Action::SetLimits(form.target, limits)),
                Err(e) => {
                    form.error = Some(e);
                    self.dialog = Some(Dialog::Limits(form));
                }
            },
        }
    }

    fn press_video(&mut self, mut form: VideoForm, button: DialogButton) {
        match button {
            DialogButton::Cancel => {}
            DialogButton::UpdateYtDlp => {
                self.actions.push(Action::UpdateYtDlp);
                self.notify("Updating yt-dlp…", false);
                self.dialog = Some(Dialog::Video(form));
            }
            DialogButton::Fallback => {
                let dir = form.dir.trim().to_owned();
                self.actions.push(Action::Add {
                    sources: vec![Source::Url(form.url)],
                    options: AddOptions {
                        dir: (!dir.is_empty()).then_some(dir),
                        paused: false,
                    },
                });
                self.set_filter(Filter::Active);
            }
            DialogButton::Confirm if form.error.is_some() => {
                // Retry, e.g. after updating yt-dlp.
                form.error = None;
                self.actions.push(Action::Probe(form.url.clone()));
                self.dialog = Some(Dialog::Video(form));
            }
            DialogButton::Confirm => match form.spec() {
                Some(spec) => {
                    self.actions.push(Action::StartVideo(spec));
                    self.set_filter(Filter::Active);
                }
                // Still loading: keep waiting.
                None => self.dialog = Some(Dialog::Video(form)),
            },
        }
    }

    /// Run a context-menu item for the menu's download.
    pub fn run_menu_item(&mut self, cmd: Command) {
        if let Some(menu) = self.menu.take() {
            self.select_gid(&menu.gid);
            self.run(cmd);
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use dlux_core::rpc::Snapshot;

    pub fn dl(gid: &str, status: DownloadStatus) -> Download {
        Download {
            gid: gid.into(),
            status,
            dir: "/downloads".into(),
            ..Default::default()
        }
    }

    pub fn app_with(active: &[&str]) -> App {
        let mut app = App::new(Theme::TOKYO_NIGHT, ConnectionInfo::default());
        app.state.apply(Snapshot {
            active: active
                .iter()
                .map(|g| dl(g, DownloadStatus::Active))
                .collect(),
            waiting: vec![dl("w1", DownloadStatus::Paused)],
            stopped: vec![dl("e1", DownloadStatus::Error)],
            ..Default::default()
        });
        app.sync_selection();
        app
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
    fn wheel_scroll_leaves_selection_and_clamps() {
        let gids: Vec<String> = (0..10).map(|i| format!("g{i}")).collect();
        let refs: Vec<&str> = gids.iter().map(String::as_str).collect();
        let mut app = app_with(&refs);
        app.page_size = 3;
        app.scroll(4);
        app.ensure_visible(10);
        assert_eq!(app.offset, 4, "view moved away from the selection");
        assert_eq!(app.selected.as_deref(), Some("g0"));
        app.scroll(100);
        assert_eq!(app.offset, 7);
        app.scroll(-100);
        assert_eq!(app.offset, 0);
        // Keyboard navigation brings the selection back into view.
        app.scroll(5);
        app.move_selection(1);
        app.ensure_visible(10);
        assert_eq!(app.offset, 1);
    }

    #[test]
    fn scrollbar_track_maps_to_offset() {
        let gids: Vec<String> = (0..11).map(|i| format!("g{i}")).collect();
        let refs: Vec<&str> = gids.iter().map(String::as_str).collect();
        let mut app = app_with(&refs);
        app.page_size = 1;
        app.hits.scrollbar = Some(Rect::new(50, 10, 1, 11));
        app.scroll_to_row(10);
        assert_eq!(app.offset, 0);
        app.scroll_to_row(15);
        assert_eq!(app.offset, 5);
        app.scroll_to_row(20);
        assert_eq!(app.offset, 10);
        app.scroll_to_row(99);
        assert_eq!(app.offset, 10, "drag below the track clamps");
        app.scroll_to_row(0);
        assert_eq!(app.offset, 0, "drag above the track clamps");
    }

    #[test]
    fn commands_are_enabled_by_status() {
        let active = dl("a", DownloadStatus::Active);
        let paused = dl("p", DownloadStatus::Paused);
        assert!(Command::Pause.enabled(Some(&active)));
        assert!(!Command::Resume.enabled(Some(&active)));
        assert!(Command::Resume.enabled(Some(&paused)));
        assert!(!Command::Pause.enabled(Some(&paused)));
        assert!(!Command::Remove.enabled(None));
        assert!(Command::Add.enabled(None));
        assert!(!Command::CopyUrl.enabled(Some(&active)), "nothing to copy");
        let magnet = Download {
            info_hash: Some("abc".into()),
            ..active.clone()
        };
        assert_eq!(
            copy_text(&magnet).as_deref(),
            Some("magnet:?xt=urn:btih:abc")
        );
    }

    #[test]
    fn pause_resume_queue_actions() {
        let mut app = app_with(&["a"]);
        app.run(Command::Pause);
        app.run(Command::Resume); // disabled for an active download
        assert_eq!(app.take_actions(), vec![Action::Pause("a".into())]);
        app.set_filter(Filter::Waiting);
        app.run(Command::Resume);
        assert_eq!(app.take_actions(), vec![Action::Resume("w1".into())]);
    }

    #[test]
    fn remove_asks_for_unfinished_downloads_only() {
        let mut app = app_with(&["a"]);
        app.run(Command::Remove);
        assert!(app.take_actions().is_empty());
        assert!(matches!(app.dialog, Some(Dialog::ConfirmRemove { .. })));
        app.press_dialog(DialogButton::Cancel);
        assert!(app.dialog.is_none());
        assert!(app.take_actions().is_empty());

        app.run(Command::Remove);
        app.press_dialog(DialogButton::Confirm);
        assert_eq!(app.take_actions(), vec![Action::Remove("a".into())]);

        app.set_filter(Filter::Errors);
        app.run(Command::Remove);
        assert!(app.dialog.is_none(), "finished downloads go without asking");
        assert_eq!(app.take_actions(), vec![Action::RemoveResult("e1".into())]);
    }

    #[test]
    fn details_toggle_respects_width() {
        let mut app = app_with(&["a"]);
        app.hits.wide = false;
        app.run(Command::Details);
        assert!(app.details, "narrow: toggled on");
        app.run(Command::Details);
        assert!(!app.details);
        app.hits.wide = true;
        app.open_details();
        assert!(!app.details, "wide: already visible, stays visible");
    }

    #[test]
    fn menu_closes_when_its_download_disappears() {
        let mut app = app_with(&["a"]);
        app.open_menu("a".into(), Position::new(1, 1));
        assert_eq!(
            app.menu.as_ref().unwrap().cursor,
            0,
            "Pause is first enabled"
        );
        app.state.apply(Snapshot::default());
        app.sync_selection();
        assert!(app.menu.is_none());
    }

    #[test]
    fn toast_expires() {
        let mut app = app_with(&[]);
        app.notify("hi", false);
        let now = Instant::now();
        assert!(app.current_toast(now).is_some());
        assert!(app.current_toast(now + TOAST_DURATION * 2).is_none());
    }
}
