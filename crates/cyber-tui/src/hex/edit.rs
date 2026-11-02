//! Explicit local commands. A request is issued once; a lost response requires
//! status/reconciliation before another command can be submitted.
use super::*;
#[path = "edit_client.rs"]
mod client;
pub use client::perform;

#[derive(Debug, Default)]
pub struct State {
    pub read_only: bool,
    pub prepared: bool,
    pub enabled: bool,
    pub dirty: bool,
    pub uncertain: bool,
    pub closing: bool,
    pub nibble: Option<u8>,
    pub anchor: Option<u64>,
    pub notice: Option<String>,
    pub review_scroll: usize,
    pub review_start: usize,
    pub review: Option<(cyber_core::binary::receipts::SaveReview, bool)>,
    pub last_operation: Option<String>,
}
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Prepare,
    Copy {
        offset: u64,
        length: u64,
    },
    Patch {
        offset: u64,
        delete: u64,
        hex: String,
    },
    History(&'static str),
    Review {
        restore: bool,
        start: usize,
    },
    Save {
        fingerprint: String,
        restore: bool,
    },
    Reconcile,
}
#[derive(Debug, Clone)]
pub struct Reply {
    pub status: Value,
    pub clipboard: Option<String>,
    pub review: Option<(cyber_core::binary::receipts::SaveReview, bool)>,
    pub operation: Option<Value>,
}

#[derive(serde::Deserialize)]
struct Status {
    #[serde(flatten)]
    metadata: cyber_core::binary::Metadata,
    prepared: bool,
    dirty: bool,
    pending_operation: Option<cyber_core::binary::receipts::Operation>,
}
fn status(
    value: Value,
    document: &Document,
    review: Option<&(cyber_core::binary::receipts::SaveReview, bool)>,
) -> Result<Status, String> {
    if value.get("pending_operation").is_none() {
        return Err("Missing binary operation state".into());
    }
    let status: Status = serde_json::from_value(value).map_err(|_| "Invalid binary status")?;
    if status.metadata.path != document.path
        || review.is_some_and(|(review, _)| review.draft.revision != status.metadata.revision)
    {
        return Err("Binary response scope/revision changed".into());
    }
    Ok(status)
}

impl App {
    fn hex_edit_request(&mut self, command: Command) -> Vec<Action> {
        let Some(document) = self.hex.document.clone() else {
            return vec![];
        };
        if self.hex.loading {
            return vec![];
        }
        if let Command::Review { start, .. } = &command {
            self.hex.edit.review_start = *start;
            self.hex.edit.review_scroll = 0;
        }
        self.hex.edit.nibble = None;
        self.hex.edit.anchor = None;
        self.hex.edit.notice = None;
        self.hex.edit.review = None;
        self.hex.error = None;
        self.hex.generation = self.hex.generation.wrapping_add(1);
        self.hex.loading = true;
        let operation_id = cyber_core::ids::new_id("op");
        if matches!(
            command,
            Command::Patch { .. } | Command::History(_) | Command::Save { .. }
        ) {
            self.hex.edit.last_operation = Some(operation_id.clone());
        }
        vec![Action::Hex(Request::Edit {
            operation_id,
            document,
            command,
            generation: self.hex.generation,
        })]
    }
    pub(super) fn hex_edit_key(&mut self, key: KeyEvent) -> Option<Vec<Action>> {
        if self.hex.edit.review.is_some() && matches!(key.code, KeyCode::PageUp | KeyCode::PageDown)
        {
            return Some(self.hex_review_move(key.code == KeyCode::PageDown));
        }
        if matches!(
            key.code,
            KeyCode::Left
                | KeyCode::Right
                | KeyCode::Up
                | KeyCode::Down
                | KeyCode::PageUp
                | KeyCode::PageDown
                | KeyCode::Home
                | KeyCode::End
        ) {
            if key.modifiers.contains(KeyModifiers::SHIFT) {
                self.hex.edit.anchor.get_or_insert(self.hex.cursor);
            } else {
                self.hex.edit.anchor = None;
            }
        }
        if self.keymap.action("hex", &key, false) == Some("hex_copy_selection")
            && let Some((offset, end)) = self.hex.selection()
        {
            return Some(self.hex_edit_request(Command::Copy {
                offset,
                length: end - offset,
            }));
        }
        if let Some(actions) = self.hex_edit_interrupt(key) {
            return Some(actions);
        }
        let command = self.hex_edit_command(key);
        if let Some(command) = command {
            if self.hex.edit.uncertain && !matches!(command, Command::Reconcile) {
                self.hex.error =
                    Some("Outcome requires Ctrl+R reconciliation before editing".into());
                return Some(vec![]);
            }
            return Some(self.hex_edit_request(command));
        }
        if !self.hex.edit.enabled || self.hex.edit.uncertain {
            return None;
        }
        Some(self.hex_edit_content_key(key))
    }
    fn hex_review_move(&mut self, down: bool) -> Vec<Action> {
        let Some((review, restore)) = &self.hex.edit.review else {
            return vec![];
        };
        if down && self.hex.edit.review_scroll + 1 < review.draft.ranges.len() {
            self.hex.edit.review_scroll += 1;
        } else if down && let Some(start) = review.draft.next_range {
            return self.hex_edit_request(Command::Review {
                restore: *restore,
                start,
            });
        } else if !down && self.hex.edit.review_scroll > 0 {
            self.hex.edit.review_scroll -= 1;
        } else if !down && self.hex.edit.review_start > 0 {
            return self.hex_edit_request(Command::Review {
                restore: *restore,
                start: self.hex.edit.review_start.saturating_sub(128),
            });
        }
        vec![]
    }
    fn hex_edit_command(&mut self, key: KeyEvent) -> Option<Command> {
        match self.keymap.action("hex", &key, false) {
            Some("hex_edit") if !self.hex.edit.read_only => {
                if self.hex.edit.prepared {
                    self.hex.edit.enabled = !self.hex.edit.enabled;
                    return None;
                }
                Some(Command::Prepare)
            }
            Some("hex_reconcile") => Some(Command::Reconcile),
            Some("hex_save") if self.hex.edit.prepared => match self.hex.edit.review.as_ref() {
                Some((review, restore)) => Some(Command::Save {
                    fingerprint: review.fingerprint.clone(),
                    restore: *restore,
                }),
                None => Some(Command::Review {
                    restore: false,
                    start: 0,
                }),
            },
            Some("hex_restore") if self.hex.edit.prepared => Some(Command::Review {
                restore: true,
                start: 0,
            }),
            Some("hex_undo") if self.hex.edit.enabled => Some(Command::History("undo")),
            Some("hex_redo") if self.hex.edit.enabled => Some(Command::History("redo")),
            _ => None,
        }
    }
    fn hex_edit_interrupt(&mut self, key: KeyEvent) -> Option<Vec<Action>> {
        if self.hex.edit.closing {
            return Some(self.hex_close_choice(key));
        }
        if key.code == KeyCode::Esc {
            if self.hex.edit.nibble.take().is_some() || self.hex.edit.review.take().is_some() {
                return Some(vec![]);
            }
            if self.hex.edit.dirty
                || self.hex.edit.uncertain
                || (self.hex.edit.prepared && self.hex.loading)
            {
                self.hex.edit.closing = true;
                return Some(vec![]);
            }
        }
        if self.hex.loading {
            return if key.code == KeyCode::Esc {
                None
            } else {
                Some(vec![])
            };
        }
        None
    }
    fn hex_edit_content_key(&mut self, key: KeyEvent) -> Vec<Action> {
        match key.code {
            _ if self.keymap.action("hex", &key, false) == Some("hex_insert") => {
                self.hex.input = Some((Input::InsertHex, String::new()))
            }
            _ if self.keymap.action("hex", &key, false) == Some("hex_replace") => {
                self.hex.input = Some((Input::ReplaceHex, String::new()))
            }
            _ if self.keymap.action("hex", &key, false) == Some("hex_delete") => {
                if let Some((offset, end)) = self.hex.selection() {
                    return self.hex_edit_request(Command::Patch {
                        offset,
                        delete: end - offset,
                        hex: String::new(),
                    });
                }
                let length = self.hex.document.as_ref().map_or(0, |d| d.length);
                if self.hex.cursor < length {
                    return self.hex_edit_request(Command::Patch {
                        offset: self.hex.cursor,
                        delete: 1,
                        hex: String::new(),
                    });
                }
            }
            KeyCode::Char(c)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                    && c.is_ascii_hexdigit() =>
            {
                self.hex.edit.anchor = None;
                let value = c.to_digit(16).unwrap() as u8;
                if let Some(high) = self.hex.edit.nibble.take() {
                    let delete = u64::from(
                        self.hex
                            .document
                            .as_ref()
                            .is_some_and(|d| self.hex.cursor < d.length),
                    );
                    return self.hex_edit_request(Command::Patch {
                        offset: self.hex.cursor,
                        delete,
                        hex: format!("{:02x}", high * 16 + value),
                    });
                }
                self.hex.edit.nibble = Some(value);
            }
            _ => {
                self.hex.edit.nibble = None;
                return self.hex_navigate(key.code);
            }
        }
        vec![]
    }
    fn hex_close_choice(&mut self, key: KeyEvent) -> Vec<Action> {
        match key.code {
            KeyCode::Esc => self.hex.edit.closing = false,
            KeyCode::Char('k') => {
                self.hex.generation = self.hex.generation.wrapping_add(1);
                self.overlay = Overlay::None;
                return self.close_hex_document();
            }
            KeyCode::Char('d') if !self.hex.edit.uncertain && !self.hex.loading => {
                self.hex.edit.closing = false;
                return self.hex_edit_request(Command::History("discard"));
            }
            _ => {}
        }
        vec![]
    }
    pub(super) fn hex_edit_submit(&mut self, kind: Input, input: String) -> Vec<Action> {
        if !self.hex.edit.enabled || self.hex.edit.uncertain {
            return vec![];
        }
        let bytes = if matches!(kind, Input::InsertText | Input::ReplaceText) {
            Ok(input.into_bytes())
        } else {
            cyber_core::binary::decode_hex(&input, 4096)
                .map_err(|_| "Enter complete hex byte pairs")
        };
        match bytes {
            Ok(bytes) if !bytes.is_empty() && bytes.len() <= 4096 => {
                let length = self.hex.document.as_ref().map_or(0, |d| d.length);
                let selection = self.hex.selection();
                let offset = selection.map_or(self.hex.cursor, |(start, _)| start);
                let delete = if matches!(kind, Input::ReplaceHex | Input::ReplaceText) {
                    selection.map_or(
                        (bytes.len() as u64).min(length.saturating_sub(offset)),
                        |(start, end)| end - start,
                    )
                } else {
                    0
                };
                let hex = bytes.iter().map(|b| format!("{b:02x}")).collect();
                self.hex_edit_request(Command::Patch {
                    offset,
                    delete,
                    hex,
                })
            }
            _ => {
                self.hex.error =
                    Some("Enter 1–4096 bytes, with complete hex pairs or literal UTF-8".into());
                vec![]
            }
        }
    }
    pub(super) fn hex_edit_reply(&mut self, result: Result<Reply, String>) -> Vec<Action> {
        self.hex.loading = false;
        match result {
            Err(error) => {
                self.hex.bytes.clear();
                self.hex.edit.uncertain = true;
                self.hex.error = Some(format!(
                    "{error}; Ctrl+R checks retained state without retrying a save"
                ));
                vec![]
            }
            Ok(reply) => {
                let clipboard = reply.clipboard;
                let Some(document) = self.hex.document.as_ref() else {
                    return vec![];
                };
                let status = match status(reply.status, document, reply.review.as_ref()) {
                    Ok(status) => status,
                    Err(error) => return self.hex_edit_reply(Err(error)),
                };
                let document = self.hex.document.as_mut().unwrap();
                document.revision = status.metadata.revision;
                document.source = match status.metadata.source {
                    cyber_core::binary::Source::Live => "live",
                    cyber_core::binary::Source::Draft => "draft",
                }
                .into();
                document.length = status.metadata.length.0;
                self.hex.cursor = self.hex.cursor.min(document.length);
                self.hex.edit.prepared = status.prepared;
                self.hex.edit.enabled = status.prepared && !self.hex.edit.read_only;
                self.hex.edit.dirty = status.dirty;
                self.hex.edit.uncertain = status.pending_operation.is_some();
                self.hex.edit.review = reply.review;
                if let Some(operation) = reply.operation.as_ref() {
                    self.hex.edit.last_operation = operation["id"].as_str().map(str::to_owned);
                    self.hex.edit.notice = Some(format!(
                        "{}: {} · file effect {}{}",
                        operation["kind"].as_str().unwrap_or("operation"),
                        operation["outcome"].as_str().unwrap_or("unknown"),
                        operation["file_effect"].as_str().unwrap_or("unknown"),
                        if operation["notification_pending"] == true {
                            " · notification pending"
                        } else {
                            ""
                        }
                    ));
                }
                self.hex.found.clear();
                self.hex.search_hex = None;
                self.hex.search_next = None;
                let mut actions = self.hex.read();
                if let Some(hex) = clipboard {
                    actions.push(Action::Copy(hex));
                }
                actions
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn app() -> App {
        let mut app = App::new(
            Session {
                id: "ses_test".into(),
                directory: "/project".into(),
                ..Default::default()
            },
            vec![],
            "cyber",
        );
        app.open_hex("bytes");
        app.hex.loading = false;
        app.hex.document = Some(Document {
            id: "bin_test".into(),
            path: "bytes".into(),
            revision: "draft_one".into(),
            source: "draft".into(),
            length: 2,
        });
        app.hex.bytes = vec![0, 255];
        app.hex.edit.prepared = true;
        app.hex.edit.enabled = true;
        app
    }
    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }
    #[test]
    fn review_navigation_reaches_ranges_beyond_the_first_server_page() {
        let mut app = app();
        let range = serde_json::json!({"original_offset":"0","original_length":"1","draft_offset":"0","draft_length":"1","before_sha256":"a".repeat(64),"after_sha256":"b".repeat(64),"before_hex":"00","after_hex":"ff","preview_truncated":false});
        let review=serde_json::from_value(serde_json::json!({"draft":{"revision":"draft_one","base_sha256":"a".repeat(64),"proposed_sha256":"b".repeat(64),"base_length":"2","proposed_length":"2","original_bytes":"1","proposed_bytes":"1","range_count":129,"ranges":vec![range;128],"next_range":128,"fingerprint":"c".repeat(64)},"permissions":420,"owner_uid":0,"owner_gid":0,"metadata_sha256":"d".repeat(64),"effects":"review only","fingerprint":"e".repeat(64)})).unwrap();
        app.hex.edit.review = Some((review, false));
        app.hex.edit.review_scroll = 126;
        assert!(app.hex_key(key(KeyCode::PageDown)).is_empty());
        assert_eq!(app.hex.edit.review_scroll, 127);
        let actions = app.hex_key(key(KeyCode::PageDown));
        assert!(matches!(
            &actions[..],
            [Action::Hex(Request::Edit {
                command: Command::Review {
                    start: 128,
                    restore: false
                },
                ..
            })]
        ));
        assert_eq!(app.hex.document.as_ref().unwrap().revision, "draft_one");
        assert_eq!(app.hex.bytes, [0, 255]);
    }

    #[test]
    fn nibble_is_local_until_complete_and_escape_cancels_without_a_patch() {
        let mut app = app();
        assert!(app.hex_key(key(KeyCode::Char('a'))).is_empty());
        assert_eq!(app.hex.edit.nibble, Some(10));
        assert!(app.hex_key(key(KeyCode::Esc)).is_empty());
        assert!(app.hex.wanted);
        assert_eq!(app.hex.edit.nibble, None);
        app.hex_key(key(KeyCode::Char('f')));
        let actions = app.hex_key(key(KeyCode::Char('1')));
        assert!(
            matches!(&actions[..],[Action::Hex(Request::Edit {command:Command::Patch {offset:0,delete:1,hex},..})] if hex=="f1")
        );
        assert_eq!(
            app.hex.bytes,
            [0, 255],
            "source and displayed page are unchanged before acknowledgement"
        );
    }
    #[test]
    fn dirty_close_requires_a_choice_and_unknown_never_discards() {
        let mut app = app();
        app.hex.edit.dirty = true;
        assert!(app.hex_key(key(KeyCode::Esc)).is_empty());
        assert!(app.hex.edit.closing);
        app.hex.edit.uncertain = true;
        assert!(app.hex_key(key(KeyCode::Char('d'))).is_empty());
        assert!(app.hex.wanted);
        let actions = app.hex_key(key(KeyCode::Char('k')));
        assert!(matches!(&actions[..], [Action::Hex(Request::Close { .. })]));
        assert!(!app.hex.wanted);
    }
    #[test]
    fn uncertain_reply_requires_reconciliation_and_stale_reply_cannot_change_the_draft() {
        let mut app = app();
        app.hex_edit_reply(Err("connection lost".into()));
        assert!(app.hex.edit.uncertain);
        assert!(app.hex.bytes.is_empty());
        assert!(
            app.hex_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL))
                .is_empty()
        );
        let actions = app.hex_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL));
        assert!(matches!(
            &actions[..],
            [Action::Hex(Request::Edit {
                command: Command::Reconcile,
                ..
            })]
        ));
        let generation = app.hex.generation;
        let session_id = app.session.id.clone();
        let directory = app.session.directory.clone();
        super::super::apply(
            &mut app,
            crate::perform::Msg::HexEdit {
                session_id,
                directory,
                generation: generation.wrapping_sub(1),
                result: Ok(Reply {
                    clipboard: None,
                    status: json!({"path":"bytes","revision":"stale","source":"draft","length":"1"}),
                    review: None,
                    operation: None,
                }),
            },
        );
        assert_eq!(app.hex.document.as_ref().unwrap().revision, "draft_one");
        assert!(app.hex.edit.uncertain);
    }
    #[test]
    fn read_only_view_never_prepares_and_text_insertion_is_encoding_explicit() {
        let mut app = app();
        app.hex.edit = State {
            read_only: true,
            ..Default::default()
        };
        assert!(app.hex_key(key(KeyCode::F(2))).is_empty());
        app.hex.edit.read_only = false;
        app.hex.edit.enabled = true;
        let actions = app.hex_edit_submit(Input::InsertText, "é".into());
        assert!(
            matches!(&actions[..],[Action::Hex(Request::Edit {command:Command::Patch {delete:0,hex,..},..})] if hex=="c3a9")
        );
    }
}
