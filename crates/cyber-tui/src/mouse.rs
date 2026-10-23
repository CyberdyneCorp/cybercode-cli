//! Mouse input in fullscreen mode (`tui` → Rendering modes and terminal integration):
//! the wheel scrolls the transcript, or the open panel, instead of the terminal's own
//! scrollback. `tui.mouse: off` leaves the mouse to the terminal, for native selection.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};

use crate::app::{Action, App, Overlay};

/// Transcript lines one wheel notch scrolls.
pub const WHEEL_LINES: u16 = 3;

impl App {
    /// Whether the TUI captures the mouse: on unless `tui.mouse` is `off`.
    pub fn mouse_capture(&self) -> bool {
        self.prefs.get("mouse") != "off"
    }

    /// A wheel notch scrolls the transcript, or moves Up/Down in a panel that scrolls
    /// or lists. Prompts and confirmations ignore the wheel, so a stray notch never
    /// answers or closes them. Clicks and drags are not used.
    pub fn on_mouse(&mut self, event: MouseEvent) -> Vec<Action> {
        let up = match event.kind {
            MouseEventKind::ScrollUp => true,
            MouseEventKind::ScrollDown => false,
            _ => return Vec::new(),
        };
        if let Overlay::None = self.overlay {
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
        let code = if up { KeyCode::Up } else { KeyCode::Down };
        self.on_key(KeyEvent::new(code, KeyModifiers::NONE))
    }
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
            | Overlay::Tree
    )
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

    use crate::app::{App, Overlay};
    use crate::model::Session;
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
        let selected = |app: &App| match &app.overlay {
            Overlay::Palette(p) => p.selected,
            _ => unreachable!(),
        };
        let before = selected(&app);
        app.on_mouse(wheel(MouseEventKind::ScrollDown));
        assert_eq!(selected(&app), before + 1);
        assert_eq!(app.scroll, 0);
    }

    #[test]
    fn mouse_off_leaves_the_mouse_to_the_terminal() {
        let mut app = app();
        assert!(app.mouse_capture(), "captured by default");
        app.prefs.set("mouse", "off", Origin::Saved);
        assert!(!app.mouse_capture());
    }
}
