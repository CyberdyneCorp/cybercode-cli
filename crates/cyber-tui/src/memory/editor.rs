//! A retained local draft; review fingerprints remain bound to its original Location.
use super::{Scope, safe};
use crate::{composer::Composer, model::Session};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use cyber_core::memory::{MemoryDocument, MemoryEditReview};

const LIMIT: usize = 1024 * 1024;

#[derive(Debug, Clone, PartialEq)]
pub struct Save {
    pub name: String,
    pub content: String,
    pub fingerprint: String,
    pub key: String,
}
#[derive(Debug)]
pub(super) struct Draft {
    pub scope: Scope,
    pub directory: String,
    session: String,
    pub name: String,
    pub review: Option<MemoryEditReview>,
    pub composer: Composer,
    pub retained_key: Option<String>,
}
impl Draft {
    pub fn new(scope: Scope, session: &Session, review: MemoryEditReview) -> Self {
        let mut composer = Composer::new(Vec::new());
        composer.set_text(review.original.as_deref().unwrap_or_default());
        Self {
            scope,
            directory: session.directory.clone(),
            session: session.id.clone(),
            name: review.name.clone(),
            review: Some(review),
            composer,
            retained_key: None,
        }
    }
    pub fn belongs(&self, scope: Scope, session: &Session) -> bool {
        self.scope == scope && self.directory == session.directory && self.session == session.id
    }
    pub fn snapshot(&self) -> Result<Save, String> {
        let review = self
            .review
            .as_ref()
            .ok_or("Obtain a fresh review with Ctrl-R before saving")?;
        let content = self.composer.text();
        if content == review.original.as_deref().unwrap_or_default() {
            return Err("No changes to save".into());
        }
        let document = MemoryDocument::for_write(&content).map_err(|e| e.to_string())?;
        if document.metadata.name != self.name {
            return Err("The draft must keep the reviewed note name".into());
        }
        Ok(Save {
            name: self.name.clone(),
            content,
            fingerprint: review.fingerprint.clone(),
            key: cyber_core::ids::new_id("mwe"),
        })
    }
    pub fn paste(&mut self, text: &str) -> Result<(), String> {
        let text = text.replace("\r\n", "\n").replace('\r', "\n");
        if self.composer.text().len().saturating_add(text.len()) > LIMIT {
            return Err("Draft exceeds 1 MiB; input was refused".into());
        }
        self.composer.insert_str(&text);
        Ok(())
    }
    pub fn key(&mut self, key: KeyEvent) -> Result<(), String> {
        if key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            return Ok(());
        }
        match key.code {
            KeyCode::Char(c) => self.paste(&c.to_string())?,
            KeyCode::Enter => self.paste("\n")?,
            KeyCode::Tab => self.paste("\t")?,
            KeyCode::Backspace => self.composer.backspace(),
            KeyCode::Delete => self.composer.delete(),
            KeyCode::Left => self.composer.left(),
            KeyCode::Right => self.composer.right(),
            KeyCode::Home => self.composer.home(),
            KeyCode::End => self.composer.end(),
            KeyCode::Up => self.composer.up(),
            KeyCode::Down => self.composer.down(),
            _ => {}
        }
        Ok(())
    }
    pub fn lines(&self) -> Vec<String> {
        let (row, col) = self.composer.cursor();
        let mut lines = vec![format!(
            "Editing {} · Ctrl-S review save · Ctrl-R fresh review · Esc retain draft",
            safe(&self.name)
        )];
        if let Some(review) = &self.review {
            lines.push("Current reviewed original:".into());
            lines.extend(
                review
                    .original
                    .as_deref()
                    .unwrap_or_default()
                    .lines()
                    .map(safe),
            );
        }
        lines.push(format!("Draft · cursor {}:{}", row + 1, col + 1));
        lines.extend(
            self.composer
                .lines()
                .into_iter()
                .enumerate()
                .map(|(i, line)| {
                    if i != row {
                        return safe(&line);
                    }
                    let mut chars: Vec<_> = line.chars().collect();
                    chars.insert(col, '▏');
                    safe(&chars.into_iter().collect::<String>())
                }),
        );
        lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn draft() -> Draft {
        let session = Session {
            id: "session".into(),
            directory: "/repo".into(),
            ..Default::default()
        };
        Draft::new(
            Scope::Global,
            &session,
            MemoryEditReview {
                name: "policy".into(),
                original: Some(
                    "---\nname: policy\ndescription: Policy\ntype: reference\n---\n\nFact\n".into(),
                ),
                fingerprint: "a".repeat(64),
            },
        )
    }
    #[test]
    fn editor_normalizes_paste_bounds_input_and_refuses_secret_or_identity_changes() {
        let mut draft = draft();
        assert!(draft.snapshot().is_err());
        draft.paste("Unicode 🦀\r\nnext\rlast").unwrap();
        assert!(draft.composer.text().ends_with("Unicode 🦀\nnext\nlast"));
        let before = draft.composer.text();
        assert!(draft.paste(&"x".repeat(LIMIT)).is_err());
        assert_eq!(draft.composer.text(), before);
        assert_eq!(draft.snapshot().unwrap().fingerprint, "a".repeat(64));
        draft
            .composer
            .set_text(&before.replace("name: policy", "name: other"));
        assert!(draft.snapshot().is_err());
        draft
            .composer
            .set_text(&format!("{before}\npassword=private"));
        assert!(draft.snapshot().is_err());
    }
    #[test]
    fn editing_unicode_cursor_never_emits_terminal_controls() {
        let mut draft = draft();
        draft.paste("🦀\u{1b}[31m").unwrap();
        draft
            .key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE))
            .unwrap();
        draft
            .key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE))
            .unwrap();
        assert!(!draft.lines().join("\n").contains('\u{1b}'));
        assert!(draft.lines().join("\n").contains('▏'));
    }
}
