//! `/rename [title]` (`session-runtime` → Fork, rename, archive, delete): rename the
//! current Session at once, or edit its title in a small dialog first.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde_json::Value;

use crate::app::{Action, App, Overlay};

impl App {
    /// `/rename New name` renames now; a bare `/rename` opens the dialog prefilled with
    /// the current title.
    pub(crate) fn rename_command(&mut self, args: &str) -> Vec<Action> {
        if args.trim().is_empty() {
            self.overlay = Overlay::Rename(crate::sidebar::clean(&self.session.title));
            return Vec::new();
        }
        self.rename_to(args)
    }

    fn rename_to(&mut self, title: &str) -> Vec<Action> {
        let title = title.trim();
        if title.is_empty() {
            self.toast("A Session title cannot be empty");
            return Vec::new();
        }
        vec![Action::RenameSession {
            id: self.session.id.clone(),
            title: title.to_string(),
        }]
    }

    /// Keys of the rename dialog: type, Backspace, Enter to save, Esc to cancel.
    pub(crate) fn rename_key(&mut self, key: KeyEvent) -> Vec<Action> {
        let Overlay::Rename(title) = &mut self.overlay else {
            return Vec::new();
        };
        match key.code {
            KeyCode::Esc => self.overlay = Overlay::None,
            KeyCode::Enter => {
                let title = std::mem::take(title);
                self.overlay = Overlay::None;
                return self.rename_to(&title);
            }
            KeyCode::Backspace => {
                title.pop();
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => title.clear(),
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => title.push(c),
            _ => {}
        }
        Vec::new()
    }

    /// A pasted title goes into an open rename dialog on one line.
    pub(crate) fn rename_paste(&mut self, text: &str) -> bool {
        let Overlay::Rename(title) = &mut self.overlay else {
            return false;
        };
        title.extend(
            text.chars()
                .map(|c| if c.is_whitespace() { ' ' } else { c }),
        );
        true
    }

    /// `session.renamed.1` and `session.title.generated.1` carry the new title, so the
    /// header, sidebar and window title change before the next refresh; a rename from
    /// another client shows up too.
    pub(crate) fn title_event(&mut self, kind: &str, data: &Value) {
        let titled =
            kind.starts_with("session.renamed.") || kind.starts_with("session.title.generated.");
        if let Some(title) = data["title"].as_str().filter(|_| titled) {
            self.session.title = title.to_string();
        }
    }
}
