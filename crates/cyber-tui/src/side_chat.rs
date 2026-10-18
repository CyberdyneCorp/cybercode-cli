//! `/btw <question>` (alias `/side`) (`tui` → Subagent threads and side chat,
//! `session-runtime` → Side chat): an overlay that asks the Session's model beside the
//! conversation. The answer is discarded on close unless the user inserts it into the
//! composer. Also the client side of what the agent delivers (`background-tasks` →
//! Notify tool, Send file to user): a desktop notification and a received-file toast.

use crossterm::event::{KeyCode, KeyEvent};
use serde_json::Value;

use crate::app::{Action, App, Overlay};

/// The open side chat.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SideChat {
    pub question: String,
    /// `None` while the model answers; `Err` holds why it could not.
    pub answer: Option<Result<String, String>>,
    pub scroll: u16,
}

impl App {
    /// `/btw <question>`: open the overlay and ask.
    pub(crate) fn side_command(&mut self, args: &str) -> Vec<Action> {
        let question = args.trim();
        if question.is_empty() {
            self.toast("Usage: /btw <question>");
            return Vec::new();
        }
        self.overlay = Overlay::SideChat(SideChat {
            question: question.to_string(),
            answer: None,
            scroll: 0,
        });
        vec![Action::SideChat {
            session_id: self.session.id.clone(),
            question: question.to_string(),
        }]
    }

    /// The answer to a side question, shown when its overlay is still open.
    pub(crate) fn side_answer(
        &mut self,
        session_id: &str,
        question: &str,
        result: Result<String, String>,
    ) {
        if session_id != self.session.id {
            return;
        }
        if let Overlay::SideChat(chat) = &mut self.overlay
            && chat.question == question
            && chat.answer.is_none()
        {
            chat.answer = Some(result);
        }
    }

    /// Esc discards the side chat; `i` (or Enter) inserts the answer into the composer.
    pub(crate) fn side_key(&mut self, key: KeyEvent) {
        let Overlay::SideChat(chat) = &mut self.overlay else {
            return;
        };
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => chat.scroll = chat.scroll.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => chat.scroll = chat.scroll.saturating_add(1),
            KeyCode::Esc | KeyCode::Char('q') => self.overlay = Overlay::None,
            KeyCode::Char('i') | KeyCode::Enter => {
                if let Some(Ok(answer)) = &chat.answer {
                    let answer = answer.clone();
                    self.overlay = Overlay::None;
                    self.composer.insert_str(&answer);
                }
            }
            _ => {}
        }
    }

    /// `session.notification.sent.1` notifies on the desktop; `session.files.sent.1`
    /// says what arrived and where the server keeps it.
    pub(crate) fn delivery_event(&mut self, kind: &str, data: &Value) {
        if kind.starts_with("session.notification.sent.") {
            let title = data["title"].as_str().unwrap_or_default();
            let body = data["body"].as_str().unwrap_or_default();
            let text = if body.is_empty() {
                title.to_string()
            } else {
                format!("{title}: {body}")
            };
            self.attention(&text);
            self.toast(crate::sidebar::clean(&text));
        } else if kind.starts_with("session.files.sent.") {
            self.toast(received(data));
        }
    }
}

/// `Received report.pdf (12 KB) at <path>` for each delivered file, with the caption.
fn received(data: &Value) -> String {
    let files: Vec<String> = data["files"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|file| {
            format!(
                "{} ({}) at {}",
                file["name"].as_str().unwrap_or("file"),
                size(file["size"].as_u64().unwrap_or(0)),
                file["stored_path"].as_str().unwrap_or_default()
            )
        })
        .collect();
    let caption = data["caption"].as_str().unwrap_or_default();
    let text = format!("Received {}", files.join(", "));
    crate::sidebar::clean(&if caption.is_empty() {
        text
    } else {
        format!("{caption} · {text}")
    })
}

fn size(bytes: u64) -> String {
    match bytes {
        b if b >= 1024 * 1024 => format!("{:.1} MB", b as f64 / (1024.0 * 1024.0)),
        b if b >= 1024 => format!("{} KB", b / 1024),
        b => format!("{b} B"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn received_names_sizes_and_caption() {
        let data = json!({"caption": "Weekly", "files": [
            {"name": "report.pdf", "size": 2_621_440, "stored_path": "/data/sent/file_1/report.pdf"},
            {"name": "a.txt", "size": 12, "stored_path": "/data/sent/file_2/a.txt"}]});
        assert_eq!(
            received(&data),
            "Weekly · Received report.pdf (2.5 MB) at /data/sent/file_1/report.pdf, a.txt (12 B) at /data/sent/file_2/a.txt"
        );
    }
}
