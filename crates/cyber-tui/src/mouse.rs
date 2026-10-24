//! Mouse input in fullscreen mode (`tui` → Rendering modes and terminal integration,
//! Mouse clicks): the wheel scrolls the transcript, or the open panel, instead of the
//! terminal's own scrollback, and left clicks act on the targets the last frame drew.
//! `tui.mouse: off` leaves the mouse to the terminal, for native selection.
//!
//! While drawing, views register what they draw as [`Target`]s. A click looks up the
//! topmost one under the pointer and presses the key that already does the same thing,
//! so every click goes through a key handler.

use std::collections::HashMap;
use std::ops::Range;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};

use crate::app::{Action, App, Overlay, PermStep};
use crate::model::Request;

/// Transcript lines one wheel notch scrolls.
pub const WHEEL_LINES: u16 = 3;

/// A second press on the same control within this long ends a double click.
pub const DOUBLE_CLICK: Duration = Duration::from_millis(500);

/// What a click on a rectangle of the last frame does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// A row of a list; see [`Act`].
    Row { list: List, index: usize, act: Act },
    /// A chip, a tab or a collapsed row: press this key.
    Key(KeyEvent),
    /// A pane that takes focus.
    Pane(Pane),
    /// Inside a modal panel, on no target: nothing.
    Inside,
}

/// The lists whose selection a click moves, through [`App::select_row`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum List {
    Picker,
    Palette,
    /// The options of the permission prompt's choice step.
    Permission,
    /// The options of the open question tab.
    Question,
    /// The file explorer's tree rows.
    Explorer,
    /// The theme switcher's themes.
    Themes,
    /// The Workflow monitor's runs.
    Runs,
    /// The agents of the run open in the Workflow monitor.
    Agents,
    /// `/messages`.
    Messages,
    /// The members of `/team`.
    Team,
    /// The rows of `/tree`.
    Tree,
    /// The Settings Config rows.
    Config,
    /// The sidebar's focusable rows.
    Sidebar,
}

/// When a row acts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Act {
    /// A control: select and act at once.
    First,
    /// A list row: select, then act on a second click.
    Second,
}

impl Target {
    /// A list row: the first click selects it, a second one activates it.
    pub fn row(list: List, index: usize) -> Self {
        Self::Row {
            list,
            index,
            act: Act::Second,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    Transcript,
    Composer,
    Sidebar,
    ExplorerTree,
    ExplorerPreview,
}

/// A press: what it hit, where, in which panel and request, and when. A row press also
/// keeps the item it selected, so a second click confirms that item only.
#[derive(Debug, Clone, PartialEq)]
struct Press {
    target: Target,
    cell: (u16, u16),
    context: Context,
    at: Instant,
    item: Option<String>,
}

impl Press {
    /// Whether `next` comes within a double click of this press but in another panel
    /// or request: the tail of a multi-click whose earlier press opened it.
    fn tail(&self, next: &Press) -> bool {
        self.context != next.context && next.at.saturating_duration_since(self.at) < DOUBLE_CLICK
    }
}

/// The open panel and the pending request: a click armed in one never acts in another.
type Context = (std::mem::Discriminant<Overlay>, Option<String>);

/// The click targets of a frame, in draw order.
type Hits = Vec<(Rect, Target)>;

/// The click targets of the last frame, the last press, and where each list's window
/// starts.
#[derive(Debug, Default)]
pub struct Mouse {
    hits: Mutex<Hits>,
    last: Option<Press>,
    windows: Mutex<HashMap<List, usize>>,
}

impl Mouse {
    fn hits(&self) -> std::sync::MutexGuard<'_, Hits> {
        self.hits
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// A new frame starts with no targets.
    pub fn clear(&self) {
        self.hits().clear();
    }

    /// Register `target` over `rect`; later targets are drawn over earlier ones.
    pub fn push(&self, rect: Rect, target: Target) {
        if !rect.is_empty() {
            self.hits().push((rect, target));
        }
    }

    /// A modal panel over `rect`: nothing drawn before it takes clicks, and a click on
    /// it hits only what it registers next, or [`Target::Inside`].
    pub fn modal(&self, rect: Rect) {
        let mut hits = self.hits();
        hits.clear();
        hits.push((rect, Target::Inside));
    }

    /// The topmost target at a cell.
    fn hit(&self, column: u16, row: u16) -> Option<Target> {
        let at = Position::new(column, row);
        self.hits()
            .iter()
            .rev()
            .find(|(rect, _)| rect.contains(at))
            .map(|&(_, target)| target)
    }

    /// The topmost pane at a cell, under any target drawn on it.
    fn pane(&self, column: u16, row: u16) -> Option<Pane> {
        let at = Position::new(column, row);
        self.hits()
            .iter()
            .rev()
            .find_map(|&(rect, target)| match target {
                Target::Pane(pane) if rect.contains(at) => Some(pane),
                _ => None,
            })
    }

    /// Forget the armed row: a key went in between.
    pub fn disarm(&mut self) {
        self.last = None;
    }

    /// The first of `len` lines that `list` shows in `room` lines, so that the selected
    /// row's `selected` lines show. The window stays where the last frame left it until
    /// the selection leaves it, so a clicked row stays under the pointer.
    pub fn window(&self, list: List, selected: Range<usize>, room: usize, len: usize) -> usize {
        let mut windows = self
            .windows
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let low = selected.end.saturating_sub(room);
        let high = selected.start.min(len.saturating_sub(room)).max(low);
        let first = windows.get(&list).copied().unwrap_or(0).clamp(low, high);
        windows.insert(list, first);
        first
    }
}

impl App {
    /// Whether the TUI captures the mouse: on unless `tui.mouse` is `off`.
    pub fn mouse_capture(&self) -> bool {
        self.prefs.get("mouse") != "off"
    }

    /// A wheel notch scrolls the transcript, or moves Up/Down in a panel that scrolls
    /// or lists. Prompts and confirmations ignore the wheel, so a stray notch never
    /// answers or closes them. An unmodified left press clicks; other buttons, releases
    /// and drags are not used. With `tui.mouse: off` nothing is handled, even an event
    /// the terminal still sends after capture was turned off.
    pub fn on_mouse(&mut self, event: MouseEvent) -> Vec<Action> {
        if !self.mouse_capture() {
            return Vec::new();
        }
        match event.kind {
            MouseEventKind::ScrollUp => self.wheel(true, event.column, event.row),
            MouseEventKind::ScrollDown => self.wheel(false, event.column, event.row),
            MouseEventKind::Down(MouseButton::Left) if self.takes_clicks(event.modifiers) => {
                self.click(event.column, event.row)
            }
            _ => Vec::new(),
        }
    }

    fn wheel(&mut self, up: bool, column: u16, row: u16) -> Vec<Action> {
        let code = if up { KeyCode::Up } else { KeyCode::Down };
        if let Overlay::Files = self.overlay
            && let Some(pane) = self.mouse.pane(column, row)
        {
            self.explorer_wheel(pane, code);
            return Vec::new();
        }
        if let Overlay::None = self.overlay {
            // The sidebar does not scroll, and the transcript beside it stays put.
            if self.mouse.pane(column, row) == Some(Pane::Sidebar) {
                return Vec::new();
            }
            self.scroll = if up {
                self.scroll.saturating_add(WHEEL_LINES)
            } else {
                self.scroll.saturating_sub(WHEEL_LINES)
            };
            return Vec::new();
        }
        if !wheel_moves(&self.overlay) {
            return Vec::new();
        }
        self.on_key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    /// In the explorer the wheel acts on the pane under the pointer, whichever has
    /// focus: it scrolls the preview, or moves the tree's selection.
    fn explorer_wheel(&mut self, pane: Pane, code: KeyCode) {
        match pane {
            Pane::ExplorerPreview => {
                for _ in 0..WHEEL_LINES {
                    self.scroll_preview(code);
                }
            }
            Pane::ExplorerTree => self.move_key(code),
            Pane::Transcript | Pane::Composer | Pane::Sidebar => {}
        }
    }

    /// A modifier leaves the press to the terminal's own selection, and screen-reader
    /// rows are sentences the targets do not follow.
    fn takes_clicks(&self, modifiers: KeyModifiers) -> bool {
        modifiers.is_empty() && !self.screen_reader
    }

    fn click(&mut self, column: u16, row: u16) -> Vec<Action> {
        let Some(target) = self.mouse.hit(column, row) else {
            return Vec::new();
        };
        let press = Press {
            target,
            cell: (column, row),
            context: self.click_context(),
            at: self.now(),
            item: None,
        };
        let previous = self.mouse.last.take();
        if let Some(last) = previous.as_ref().filter(|last| last.tail(&press)) {
            // The rest of a multi-click that opened this panel or request: it never
            // selects or arms here, and it carries the burst on to the next press.
            self.mouse.last = Some(Press {
                at: press.at,
                ..last.clone()
            });
            return Vec::new();
        }
        // A click on the transcript or the composer takes focus back from the sidebar
        // first, also on a row drawn over them.
        if self.sidebar.focus.is_some()
            && matches!(
                self.mouse.pane(column, row),
                Some(Pane::Transcript | Pane::Composer)
            )
        {
            self.leave_sidebar();
        }
        match target {
            Target::Row {
                list,
                index,
                act: Act::Second,
            } => self.press_row(list, index, press, previous),
            Target::Row {
                act: Act::First, ..
            }
            | Target::Key(_) => self.press_control(press, previous),
            Target::Pane(pane) => self.focus_pane(pane),
            Target::Inside => Vec::new(),
        }
    }

    fn click_context(&self) -> Context {
        (
            std::mem::discriminant(&self.overlay),
            self.active_request().map(|request| request.id().to_owned()),
        )
    }

    /// The first press selects and arms the row; a press on the armed row activates it,
    /// when it still selects the same item: a list that changed in between only re-arms.
    fn press_row(
        &mut self,
        list: List,
        index: usize,
        mut press: Press,
        previous: Option<Press>,
    ) -> Vec<Action> {
        if !self.select_row(list, index) {
            return Vec::new();
        }
        press.item = self.selected_item(list);
        let armed = previous.is_some_and(|last| {
            last.target == press.target
                && last.context == press.context
                && last.item.is_some()
                && last.item == press.item
        });
        if armed {
            let actions = self.activate(list);
            // Kept after the key, which disarms, and with no item, so it never arms:
            // only to absorb the rest of the burst.
            self.mouse.last = Some(Press {
                item: None,
                ..press
            });
            return actions;
        }
        self.mouse.last = Some(press);
        Vec::new()
    }

    /// A control acts at once; the second press of a double click is absorbed, so a
    /// double click toggles once, also when the first press moved what is under the
    /// pointer.
    fn press_control(&mut self, press: Press, previous: Option<Press>) -> Vec<Action> {
        let double = previous.is_some_and(|last| {
            (last.target == press.target || last.cell == press.cell)
                && last.context == press.context
                && press.at.saturating_duration_since(last.at) < DOUBLE_CLICK
        });
        if double {
            return Vec::new();
        }
        let actions = match press.target {
            Target::Row { list, index, .. } if self.select_row(list, index) => self.activate(list),
            Target::Key(key) => self.on_key(key),
            _ => Vec::new(),
        };
        self.mouse.last = Some(press);
        actions
    }

    /// The transcript and the composer take focus back from the sidebar, as Esc does
    /// there; a click never sends Esc to the composer, where it interrupts a Turn. An
    /// explorer pane takes focus as the bound `explorer_focus` key moves it.
    fn focus_pane(&mut self, pane: Pane) -> Vec<Action> {
        match pane {
            Pane::Transcript | Pane::Composer if self.sidebar.focus.is_some() => {
                self.leave_sidebar()
            }
            Pane::ExplorerTree | Pane::ExplorerPreview => {
                let preview = pane == Pane::ExplorerPreview;
                let open = self.explorer.preview.is_some();
                if open && self.explorer.preview_focused != preview {
                    return self.press_bound("explorer", "explorer_focus");
                }
            }
            Pane::Transcript | Pane::Composer | Pane::Sidebar => {}
        }
        Vec::new()
    }

    /// Press the key `action` is bound to; nothing when it is unbound.
    fn press_bound(&mut self, context: &str, action: &str) -> Vec<Action> {
        self.keymap
            .event(context, action)
            .map(|key| self.on_key(key))
            .unwrap_or_default()
    }

    /// Press the key that runs the selected row of `list`: Enter, or the bound
    /// `palette_run`, `explorer_open`, `workflows_open` or `sidebar_select` key.
    fn activate(&mut self, list: List) -> Vec<Action> {
        match list {
            List::Palette => self.press_bound("palette", "palette_run"),
            List::Explorer => self.press_bound("explorer", "explorer_open"),
            List::Runs | List::Agents => self.press_bound("workflows", "workflows_open"),
            List::Sidebar => self.press_bound("sidebar", "sidebar_select"),
            List::Picker
            | List::Permission
            | List::Question
            | List::Themes
            | List::Messages
            | List::Team
            | List::Tree
            | List::Config => self.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        }
    }

    /// Select row `index` of `list` in the open panel, as moving there with the keys
    /// does. `false`, and nothing changes, when `list` is not open, takes no clicks in
    /// its current step, or has no such row: a stale frame fails closed.
    pub(crate) fn select_row(&mut self, list: List, index: usize) -> bool {
        match (list, &self.overlay) {
            (List::Explorer, Overlay::Files) => self.select_tree_row(index),
            (List::Themes, Overlay::Themes(_)) => self.select_theme(index),
            (List::Runs | List::Agents, Overlay::Workflows) => self.select_run_row(list, index),
            (List::Messages, Overlay::Messages) => self.select_message(index),
            (List::Sidebar, Overlay::None) => self.select_sidebar_row(index),
            _ => self.select_list_row(list, index),
        }
    }

    /// What the selected row of `list` shows, by a key that stays with the item when
    /// the list reloads or reorders: the second click of a row compares it.
    fn selected_item(&self, list: List) -> Option<String> {
        match (list, &self.overlay) {
            (List::Picker, Overlay::Picker(p)) => {
                p.current().map(|c| format!("{:?} {}", p.kind, c.key))
            }
            (List::Palette, Overlay::Palette(p)) => p.current().map(|e| e.name.clone()),
            // The request ID is in the press's context.
            (List::Permission, Overlay::Permission(PermStep::Choose(i))) => Some(i.to_string()),
            (List::Question, Overlay::Question(form)) => {
                Some(format!("{} {}", form.tab, form.cursor))
            }
            (List::Explorer, Overlay::Files) => self.explorer.current().map(|r| r.node.path),
            (List::Themes, Overlay::Themes(s)) => {
                s.current(self.theme_filter).map(|r| r.name.clone())
            }
            (List::Runs, Overlay::Workflows) => self.workflows.current().map(|r| r.id.clone()),
            (List::Agents, Overlay::Workflows) => self.selected_agent(),
            (List::Messages, Overlay::Messages) => self.messaging.selected.clone(),
            (List::Team, Overlay::Team) => self.sidebar.team.ready().and_then(|team| {
                let member = team.members.get(self.team.selected)?;
                Some(member.session_id.clone())
            }),
            (List::Tree, Overlay::Tree) => self.branches.selected_node().map(|n| n.id.clone()),
            (List::Config, Overlay::Settings) => crate::prefs::EDITABLE
                .get(self.settings.config_row)
                .map(|(name, _)| (*name).to_owned()),
            (List::Sidebar, Overlay::None) => self.selected_sidebar_item(),
            _ => None,
        }
    }

    /// The run and the agent selected in the Workflow monitor.
    fn selected_agent(&self) -> Option<String> {
        let view = self.workflows.run.as_ref()?;
        Some(format!("{} {}", view.run_id, view.agent()?.id))
    }

    fn selected_sidebar_item(&self) -> Option<String> {
        use crate::sidebar::Row;
        let s = &self.sidebar;
        Some(match *s.rows().get(s.focus?)? {
            Row::Header(section) => format!("section {section:?}"),
            Row::Workflow(i) => format!("run {}", s.workflows.get(i)?.id),
            Row::Mcp(i) => format!("mcp {}", s.servers().get(i)?.name),
            Row::File(i) => format!("file {}", s.changes().get(i)?.file),
        })
    }

    /// The lists whose selection is one index of the open panel.
    fn select_list_row(&mut self, list: List, index: usize) -> bool {
        let rows = self.prompt_rows();
        let members = self.sidebar.team.ready().map_or(0, |t| t.members.len());
        let branches = self.branches.rows().len();
        let config = crate::prefs::EDITABLE.len();
        let config_tab = self.settings.tab == crate::settings::Tab::Config;
        let (selected, len) = match (list, &mut self.overlay) {
            (List::Picker, Overlay::Picker(p)) if p.pending.is_none() => {
                (&mut p.selected, p.filtered.len())
            }
            (List::Palette, Overlay::Palette(p)) => (&mut p.selected, p.filtered.len()),
            (List::Permission, Overlay::Permission(PermStep::Choose(i))) => (i, rows),
            (List::Question, Overlay::Question(form)) if !form.typing => (&mut form.cursor, rows),
            (List::Team, Overlay::Team) => (&mut self.team.selected, members),
            // A label being typed takes the keys.
            (List::Tree, Overlay::Tree) if self.branches.label.is_none() => {
                (&mut self.branches.selected, branches)
            }
            (List::Config, Overlay::Settings) if config_tab => {
                (&mut self.settings.config_row, config)
            }
            _ => return false,
        };
        choose(selected, index, len)
    }

    /// A theme is shown across the TUI as soon as it is selected, as with the arrows.
    fn select_theme(&mut self, index: usize) -> bool {
        let filter = self.theme_filter;
        let Overlay::Themes(s) = &mut self.overlay else {
            return false;
        };
        let len = s.visible(filter).len();
        if !choose(&mut s.selected, index, len) {
            return false;
        }
        self.preview_selected();
        true
    }

    /// A run of the list, or an agent of the open run; none while a confirmation, the
    /// log or the library takes the keys.
    fn select_run_row(&mut self, list: List, index: usize) -> bool {
        let w = &mut self.workflows;
        if w.confirm.is_some() || w.log.is_some() || w.library_tab {
            return false;
        }
        let runs = w.summaries().len();
        match (list, w.run.as_mut()) {
            (List::Runs, None) => choose(&mut w.selected, index, runs),
            (List::Agents, Some(view)) => {
                let agents = view.detail.data.as_ref().map_or(0, |d| d.agents.len());
                choose(&mut view.selected, index, agents)
            }
            _ => false,
        }
    }

    /// `/messages` selects by message, and a new row folds the expansion, as the
    /// arrows do.
    fn select_message(&mut self, index: usize) -> bool {
        let rows = self.messaging.rows(&self.items, &self.queued);
        let Some(id) = rows.get(index).map(|row| row.id().to_owned()) else {
            return false;
        };
        let m = &mut self.messaging;
        if m.selected.as_deref() != Some(&id) {
            m.selected = Some(id);
            m.expanded = false;
        }
        true
    }

    /// A sidebar row takes the focus there, as `<leader>e` and the arrows do.
    fn select_sidebar_row(&mut self, index: usize) -> bool {
        if !self.sidebar_visible() || index >= self.sidebar.rows().len() {
            return false;
        }
        self.sidebar.focus = Some(index);
        true
    }

    /// A row of the explorer's tree, which takes focus back from the preview as Esc
    /// does there; none while the filter takes typed keys.
    fn select_tree_row(&mut self, index: usize) -> bool {
        let e = &mut self.explorer;
        if e.filtering || index >= e.rows().len() {
            return false;
        }
        e.selected = index;
        e.preview_focused = false;
        true
    }

    /// The option rows of the open prompt: Allow once, Allow always and Reject (none
    /// for the blocked-call approval), or the open question's options and its own
    /// answer row (none on the review tab).
    fn prompt_rows(&self) -> usize {
        match (self.active_request(), &self.overlay) {
            (Some(Request::Permission { action, .. }), Overlay::Permission(_))
                if action != "auto_override" =>
            {
                3
            }
            (Some(Request::Question { questions, .. }), Overlay::Question(form)) => questions
                .get(form.tab)
                .map_or(0, |q| q.options.len() + usize::from(q.custom)),
            _ => 0,
        }
    }
}

/// Move `selected` to `index`; `false` past the end.
fn choose(selected: &mut usize, index: usize, len: usize) -> bool {
    if index >= len {
        return false;
    }
    *selected = index;
    true
}

/// Panels where Up/Down only scroll or move a selection.
fn wheel_moves(overlay: &Overlay) -> bool {
    matches!(
        overlay,
        Overlay::Picker(_)
            | Overlay::Palette(_)
            | Overlay::HookHistory
            | Overlay::HookDefinitions
            | Overlay::Findings
            | Overlay::FileDiff { .. }
            | Overlay::Settings
            | Overlay::Files
            | Overlay::Themes(_)
            | Overlay::Workflows
            | Overlay::TaskLog { .. }
            | Overlay::Messages
            | Overlay::Team
            | Overlay::Collaboration
            | Overlay::Tree
    )
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use crossterm::event::{
        KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    };
    use ratatui::layout::Rect;
    use serde_json::json;

    use super::{Act, List, Pane, Target};
    use crate::app::{Action, App, Overlay, PickerKind};
    use crate::model::{Choice, Request, Session};
    use crate::prefs::Origin;

    fn wheel(kind: MouseEventKind) -> MouseEvent {
        MouseEvent {
            kind,
            column: 10,
            row: 5,
            modifiers: KeyModifiers::NONE,
        }
    }

    fn app() -> App {
        App::new(Session::default(), Vec::new(), "cyber")
    }

    /// An unmodified left press at a cell.
    fn click(app: &mut App, column: u16, row: u16) -> Vec<Action> {
        app.on_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column,
            row,
            modifiers: KeyModifiers::NONE,
        })
    }

    fn line(y: u16) -> Rect {
        Rect::new(0, y, 40, 1)
    }

    fn row(list: List, index: usize) -> Target {
        Target::Row {
            list,
            index,
            act: Act::Second,
        }
    }

    fn palette_selected(app: &App) -> usize {
        match &app.overlay {
            Overlay::Palette(p) => p.selected,
            other => panic!("{other:?}"),
        }
    }

    fn models(app: &mut App) {
        let items = ["gpt-5", "sonnet", "haiku"]
            .map(|name| Choice {
                key: format!("test/{name}"),
                label: name.into(),
                detail: String::new(),
            })
            .to_vec();
        app.open_picker(PickerKind::Models, "Models", items);
    }

    fn picker_selected(app: &App) -> usize {
        match &app.overlay {
            Overlay::Picker(p) => p.selected,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_wheel_scrolls_the_transcript() {
        let mut app = app();
        app.on_mouse(wheel(MouseEventKind::ScrollUp));
        app.on_mouse(wheel(MouseEventKind::ScrollUp));
        assert_eq!(app.scroll, 2 * super::WHEEL_LINES);
        app.on_mouse(wheel(MouseEventKind::ScrollDown));
        assert_eq!(app.scroll, super::WHEEL_LINES);
        app.on_mouse(wheel(MouseEventKind::ScrollDown));
        app.on_mouse(wheel(MouseEventKind::ScrollDown));
        assert_eq!(app.scroll, 0, "scrolling down stops at the newest line");
    }

    #[test]
    fn clicks_change_nothing() {
        let mut app = app();
        assert!(
            app.on_mouse(wheel(MouseEventKind::Down(MouseButton::Left)))
                .is_empty()
        );
        assert_eq!(app.scroll, 0);
    }

    #[test]
    fn the_wheel_never_closes_a_confirmation() {
        let mut app = app();
        app.overlay = Overlay::ConfirmStopTasks;
        assert!(app.on_mouse(wheel(MouseEventKind::ScrollDown)).is_empty());
        assert!(matches!(app.overlay, Overlay::ConfirmStopTasks));
        app.overlay = Overlay::Help;
        app.on_mouse(wheel(MouseEventKind::ScrollUp));
        assert!(matches!(app.overlay, Overlay::Help));
        assert_eq!(app.scroll, 0, "the transcript under a panel stays put");
    }

    #[test]
    fn the_wheel_moves_a_panel_selection() {
        let mut app = app();
        app.open_palette();
        let before = palette_selected(&app);
        app.on_mouse(wheel(MouseEventKind::ScrollDown));
        assert_eq!(palette_selected(&app), before + 1);
        assert_eq!(app.scroll, 0);
    }

    #[test]
    fn mouse_off_leaves_the_mouse_to_the_terminal() {
        let mut app = app();
        assert!(app.mouse_capture(), "captured by default");
        app.prefs.set("mouse", "off", Origin::Saved);
        assert!(!app.mouse_capture());
        // A wheel event still delivered after capture went off scrolls nothing.
        app.on_mouse(wheel(MouseEventKind::ScrollUp));
        assert_eq!(app.scroll, 0, "tui.mouse: off");
        app.open_palette();
        app.on_mouse(wheel(MouseEventKind::ScrollDown));
        assert_eq!(palette_selected(&app), 0, "tui.mouse: off");
    }

    #[test]
    fn clicks_need_capture_a_left_press_and_no_modifier() {
        let mut app = app();
        app.open_palette();
        app.mouse.push(line(3), row(List::Palette, 2));
        let press = |kind, modifiers| MouseEvent {
            kind,
            column: 5,
            row: 3,
            modifiers,
        };
        let left = MouseEventKind::Down(MouseButton::Left);
        for ignored in [
            press(MouseEventKind::Down(MouseButton::Right), KeyModifiers::NONE),
            press(
                MouseEventKind::Down(MouseButton::Middle),
                KeyModifiers::NONE,
            ),
            press(MouseEventKind::Up(MouseButton::Left), KeyModifiers::NONE),
            press(MouseEventKind::Drag(MouseButton::Left), KeyModifiers::NONE),
            press(left, KeyModifiers::SHIFT),
            press(left, KeyModifiers::ALT),
            press(left, KeyModifiers::CONTROL),
        ] {
            assert!(app.on_mouse(ignored).is_empty(), "{ignored:?}");
            assert_eq!(palette_selected(&app), 0, "{ignored:?}");
        }
        app.prefs.set("mouse", "off", Origin::Saved);
        app.on_mouse(press(left, KeyModifiers::NONE));
        assert_eq!(palette_selected(&app), 0, "tui.mouse: off");
        app.prefs.set("mouse", "on", Origin::Saved);
        app.screen_reader = true;
        app.on_mouse(press(left, KeyModifiers::NONE));
        assert_eq!(palette_selected(&app), 0, "screen-reader mode");
        app.screen_reader = false;
        assert!(app.on_mouse(press(left, KeyModifiers::NONE)).is_empty());
        assert_eq!(palette_selected(&app), 2);
    }

    #[test]
    fn the_topmost_hit_wins() {
        let mut app = app();
        app.open_palette();
        app.mouse.push(line(3), row(List::Palette, 1));
        app.mouse
            .push(Rect::new(0, 3, 10, 1), row(List::Palette, 3));
        click(&mut app, 5, 3);
        assert_eq!(palette_selected(&app), 3, "drawn last, on top");
        click(&mut app, 20, 3);
        assert_eq!(palette_selected(&app), 1, "only the first covers column 20");
        // A modal panel hides what was drawn before it, also outside its own rect.
        app.mouse.modal(Rect::new(0, 10, 40, 5));
        assert!(click(&mut app, 5, 3).is_empty());
        assert!(click(&mut app, 5, 11).is_empty());
        assert_eq!(palette_selected(&app), 1);
        app.mouse.push(line(12), row(List::Palette, 4));
        click(&mut app, 5, 12);
        assert_eq!(palette_selected(&app), 4);
        app.mouse.clear();
        click(&mut app, 5, 12);
        assert_eq!(palette_selected(&app), 4, "a new frame has no targets yet");
    }

    #[test]
    fn a_second_click_activates_and_a_key_disarms() {
        let mut app = app();
        models(&mut app);
        app.mouse.push(line(3), row(List::Picker, 1));
        assert!(click(&mut app, 5, 3).is_empty(), "the first click selects");
        assert_eq!(picker_selected(&app), 1);
        // Moving away and back with the keys forgets the armed row.
        app.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        app.on_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert!(click(&mut app, 5, 3).is_empty(), "selected again, not run");
        assert!(matches!(app.overlay, Overlay::Picker(_)));
        assert_eq!(
            click(&mut app, 5, 3),
            vec![Action::SwitchModel("test/sonnet".into())]
        );
        assert!(matches!(app.overlay, Overlay::None));

        // A stale row, or another list's, does nothing.
        models(&mut app);
        app.mouse.clear();
        app.mouse.push(line(3), row(List::Picker, 7));
        app.mouse.push(line(4), row(List::Palette, 0));
        for y in [3, 4, 3, 4] {
            assert!(click(&mut app, 5, y).is_empty());
        }
        assert_eq!(picker_selected(&app), 0);

        // A row armed before a request arrived does not act after it.
        app.mouse.push(line(5), row(List::Picker, 2));
        click(&mut app, 5, 5);
        app.requests.push(Request::Permission {
            id: "per_1".into(),
            session_id: app.session.id.clone(),
            origin: None,
            member: None,
            routed_to: Vec::new(),
            action: "bash".into(),
            resources: Vec::new(),
            patterns: Vec::new(),
            metadata: json!({}),
        });
        assert!(click(&mut app, 5, 5).is_empty(), "the request changed");
        assert_eq!(picker_selected(&app), 2);
    }

    fn choices(keys: &[&str]) -> Vec<Choice> {
        keys.iter()
            .map(|&key| Choice {
                key: key.into(),
                label: key.into(),
                detail: String::new(),
            })
            .collect()
    }

    #[test]
    fn a_list_that_reloads_between_the_clicks_only_rearms() {
        let mut app = app();
        app.open_or_refresh_picker(PickerKind::Tasks, "Tasks", choices(&["t_a", "t_b", "t_c"]));
        app.mouse.push(line(3), row(List::Picker, 1));
        assert!(click(&mut app, 5, 3).is_empty(), "selects t_b");
        // A live reload puts another task under the pointer.
        app.open_or_refresh_picker(PickerKind::Tasks, "Tasks", choices(&["t_c", "t_a", "t_b"]));
        assert!(
            click(&mut app, 5, 3).is_empty(),
            "t_a was never shown selected before this click"
        );
        assert!(matches!(app.overlay, Overlay::Picker(_)));
        assert_eq!(picker_selected(&app), 1);
        assert_eq!(
            click(&mut app, 5, 3),
            vec![Action::OpenTask("t_a".into())],
            "a click on the item it selected confirms"
        );
    }

    #[test]
    fn an_armed_row_does_not_carry_over_to_another_picker() {
        let mut app = app();
        models(&mut app);
        app.mouse.push(line(3), row(List::Picker, 1));
        assert!(click(&mut app, 5, 3).is_empty());
        // `LoadChildren` answers while the `/model` row is armed.
        app.open_or_refresh_picker(PickerKind::Children, "Children", choices(&["s_1", "s_2"]));
        assert!(click(&mut app, 5, 3).is_empty(), "the picker changed");
        assert!(matches!(app.overlay, Overlay::Picker(_)));
        assert_eq!(click(&mut app, 5, 3), vec![Action::Open("s_2".into())]);
    }

    #[test]
    fn a_list_window_stays_until_the_selection_leaves_it() {
        let mouse = super::Mouse::default();
        let window = |selected: usize| mouse.window(List::Palette, selected..selected + 1, 5, 30);
        assert_eq!(window(0), 0);
        assert_eq!(window(20), 16, "down past the window: the row shows last");
        assert_eq!(window(17), 16, "a row inside it leaves it where it is");
        assert_eq!(window(10), 10, "up past it: the row shows first");
        assert_eq!(window(29), 25);
        assert_eq!(
            mouse.window(List::Palette, 2..3, 5, 4),
            0,
            "a shorter list shows from its start"
        );
        assert_eq!(
            mouse.window(List::Explorer, 7..8, 5, 30),
            3,
            "each list has its own window"
        );
    }

    #[test]
    fn a_double_click_on_a_control_acts_once() {
        let mut app = app();
        let start = app.started();
        let toggle = KeyEvent::new(KeyCode::Char('t'), KeyModifiers::CONTROL);
        app.mouse.push(line(3), Target::Key(toggle));
        let at = |app: &mut App, ms: u64, y: u16| {
            app.clock = Some(start + Duration::from_millis(ms));
            click(app, 5, y)
        };
        assert!(!app.show_reasoning);
        at(&mut app, 0, 3);
        assert!(app.show_reasoning, "a control acts on the first click");
        at(&mut app, 300, 3);
        assert!(
            app.show_reasoning,
            "the double click's second press is absorbed"
        );
        at(&mut app, 1_000, 3);
        assert!(!app.show_reasoning, "a later click acts again");
        at(&mut app, 1_600, 3);
        assert!(app.show_reasoning, "600 ms apart is two clicks");

        // A row control selects and runs at once.
        models(&mut app);
        app.mouse.clear();
        app.mouse.push(
            line(4),
            Target::Row {
                list: List::Picker,
                index: 2,
                act: Act::First,
            },
        );
        assert_eq!(
            at(&mut app, 2_200, 4),
            vec![Action::SwitchModel("test/haiku".into())]
        );

        // The second press of a double click is absorbed also when the first one put
        // another control under the pointer.
        app.mouse.clear();
        app.mouse.push(line(3), Target::Key(toggle));
        at(&mut app, 3_000, 3);
        assert!(!app.show_reasoning);
        app.mouse.clear();
        let details = KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL);
        app.mouse.push(line(3), Target::Key(details));
        let expanded = app.expand_tools;
        at(&mut app, 3_200, 3);
        assert_eq!(app.expand_tools, expanded, "absorbed");
        at(&mut app, 4_000, 3);
        assert_eq!(app.expand_tools, !expanded, "a later click acts");
    }

    #[test]
    fn explorer_rows_fail_closed_while_filtering_or_past_the_end() {
        let mut app = app();
        app.overlay = Overlay::Files;
        let level = json!({ "entries": [
            { "name": "a.rs", "path": "a.rs", "kind": "file", "read": "allow" },
            { "name": "b.rs", "path": "b.rs", "kind": "file", "read": "allow" },
        ] });
        let (level, _) = crate::explorer::parse_level(&level);
        app.explorer.apply_level(String::new(), level, None);
        app.explorer.preview_focused = true;
        assert!(!app.select_row(List::Explorer, 2), "past the end");
        app.explorer.filtering = true;
        assert!(!app.select_row(List::Explorer, 1), "the filter takes keys");
        assert!(app.explorer.preview_focused);
        app.explorer.filtering = false;
        assert!(app.select_row(List::Explorer, 1));
        assert_eq!(app.explorer.selected, 1);
        assert!(!app.explorer.preview_focused, "the tree takes focus");
        app.overlay = Overlay::None;
        assert!(!app.select_row(List::Explorer, 0), "the explorer is closed");
    }

    #[test]
    fn a_click_on_the_transcript_takes_focus_back_from_the_sidebar() {
        let mut app = app();
        app.mouse
            .push(Rect::new(0, 0, 60, 20), Target::Pane(Pane::Transcript));
        app.mouse
            .push(Rect::new(60, 0, 40, 20), Target::Pane(Pane::Sidebar));
        app.sidebar.focus = Some(0);
        app.sidebar.drawer = true;
        click(&mut app, 70, 5);
        assert_eq!(
            app.sidebar.focus,
            Some(0),
            "a click on the sidebar keeps it"
        );
        click(&mut app, 5, 5);
        assert_eq!(app.sidebar.focus, None);
        assert!(!app.sidebar.drawer);
    }

    #[test]
    fn a_row_drawn_on_the_transcript_takes_focus_back_from_the_sidebar() {
        let mut app = app();
        app.mouse
            .push(Rect::new(0, 0, 60, 20), Target::Pane(Pane::Transcript));
        let details = KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL);
        app.mouse.push(line(3), Target::Key(details));
        app.sidebar.focus = Some(0);
        app.sidebar.drawer = true;
        let expanded = app.expand_tools;
        click(&mut app, 5, 3);
        assert_eq!(app.sidebar.focus, None);
        assert!(!app.sidebar.drawer);
        assert_eq!(
            app.expand_tools, !expanded,
            "the row's key reached the transcript"
        );
    }

    #[test]
    fn the_tail_of_a_multi_click_never_answers_the_prompt_it_raised() {
        let mut app = app();
        let start = app.started();
        let at = |app: &mut App, ms: u64, y: u16| {
            app.clock = Some(start + Duration::from_millis(ms));
            click(app, 5, y)
        };
        models(&mut app);
        app.mouse.push(line(3), row(List::Picker, 1));
        at(&mut app, 0, 3);
        assert_eq!(
            at(&mut app, 100, 3),
            vec![Action::SwitchModel("test/sonnet".into())]
        );
        // The double click's activation raised a prompt under the pointer.
        app.requests.push(Request::Permission {
            id: "per_1".into(),
            session_id: app.session.id.clone(),
            origin: None,
            member: None,
            routed_to: Vec::new(),
            action: "bash".into(),
            resources: Vec::new(),
            patterns: Vec::new(),
            metadata: json!({}),
        });
        app.sync_overlay();
        app.mouse.clear();
        app.mouse.push(line(3), row(List::Permission, 1));
        app.mouse.push(line(4), row(List::Permission, 0));
        for ms in [200, 350, 500] {
            assert!(at(&mut app, ms, 3).is_empty(), "{ms} ms: the same burst");
        }
        assert!(
            matches!(
                app.overlay,
                Overlay::Permission(crate::app::PermStep::Choose(0))
            ),
            "{:?}",
            app.overlay
        );
        // After a pause, a deliberate double click selects, then answers.
        assert!(at(&mut app, 1_100, 3).is_empty());
        assert!(matches!(
            app.overlay,
            Overlay::Permission(crate::app::PermStep::Choose(1))
        ));
        assert!(at(&mut app, 1_200, 4).is_empty());
        let reply = at(&mut app, 1_300, 4);
        assert!(matches!(&reply[..], [Action::Reply { .. }]), "{reply:?}");
    }

    #[test]
    fn the_wheel_over_the_sidebar_does_nothing() {
        let mut app = app();
        app.mouse
            .push(Rect::new(0, 0, 60, 20), Target::Pane(Pane::Transcript));
        app.mouse
            .push(Rect::new(60, 0, 40, 20), Target::Pane(Pane::Sidebar));
        let at = |column| MouseEvent {
            column,
            ..wheel(MouseEventKind::ScrollUp)
        };
        app.on_mouse(at(70));
        assert_eq!(app.scroll, 0, "over the sidebar");
        app.on_mouse(at(10));
        assert_eq!(app.scroll, super::WHEEL_LINES, "over the transcript");
    }
}
