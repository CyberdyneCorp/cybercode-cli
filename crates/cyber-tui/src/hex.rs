//! Hex workspace state and owner-client operations. Raw bytes never become terminal
//! text or model input. Navigation is local; pages come from the authorized server.
use crate::app::{Action, App, Overlay};
use crate::model::Session;
use base64::Engine;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
#[path = "hex/edit.rs"]
pub(crate) mod edit;

#[derive(Debug, Clone, PartialEq)]
pub struct Document {
    pub id: String,
    pub path: String,
    pub revision: String,
    pub length: u64,
    pub source: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Request {
    Edit {
        document: Document,
        command: edit::Command,
        operation_id: String,
        generation: u64,
    },
    Open {
        path: String,
        expected_root: Option<String>,
        read_only: bool,
        recover: Option<String>,
        offset: u64,
        generation: u64,
    },
    Page {
        document: Document,
        offset: u64,
        generation: u64,
    },
    Search {
        document: Document,
        hex: String,
        offset: u64,
        generation: u64,
    },
    Close {
        id: String,
        session_id: String,
        directory: String,
    },
}

#[derive(Debug, Clone)]
pub struct Loaded {
    pub document: Document,
    pub offset: u64,
    pub bytes: Vec<u8>,
    pub recovered: Option<Value>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Input {
    Goto,
    FindHex,
    FindText,
    Bookmark,
    InsertHex,
    InsertText,
    ReplaceHex,
    ReplaceText,
}
impl Input {
    pub fn label(self) -> &'static str {
        match self {
            Self::Goto => "Goto",
            Self::FindHex => "Find hex (Tab: UTF-8)",
            Self::FindText => "Find UTF-8 (Tab: hex)",
            Self::Bookmark => "Bookmark name",
            Self::InsertHex => "Insert hex (Tab: UTF-8)",
            Self::InsertText => "Insert UTF-8 (Tab: hex)",
            Self::ReplaceHex => "Overwrite hex (Tab: UTF-8)",
            Self::ReplaceText => "Overwrite UTF-8 (Tab: hex)",
        }
    }
}
#[derive(Debug, Default)]
pub struct Workspace {
    pub edit: edit::State,
    pub document: Option<Document>,
    pub generation: u64,
    pub bytes: Vec<u8>,
    pub page_offset: u64,
    pub cursor: u64,
    pub big_endian: bool,
    pub input: Option<(Input, String)>,
    pub error: Option<String>,
    pub loading: bool,
    pub wanted: bool,
    pub bookmarks: Vec<(String, u64)>,
    pub search_hex: Option<String>,
    pub search_next: Option<u64>,
    pub found: Vec<u64>,
    pub pending_closes: Vec<Request>,
}

impl Workspace {
    pub(crate) fn selection(&self) -> Option<(u64, u64)> {
        let anchor = self.edit.anchor?;
        (anchor != self.cursor).then_some((anchor.min(self.cursor), anchor.max(self.cursor)))
    }

    pub fn forget(&mut self, session: &Session) {
        self.edit = edit::State::default();
        if let Some(document) = self.document.take() {
            self.pending_closes.push(Request::Close {
                id: document.id,
                session_id: session.id.clone(),
                directory: session.directory.clone(),
            });
        }
        self.bytes.clear();
        self.bookmarks.clear();
        self.search_hex = None;
        self.search_next = None;
        self.found.clear();
        self.input = None;
        self.error = None;
        self.loading = false;
        self.wanted = false;
        self.generation = self.generation.wrapping_add(1);
    }

    pub fn byte_slice(&self) -> &[u8] {
        let Some(at) = self
            .cursor
            .checked_sub(self.page_offset)
            .and_then(|at| usize::try_from(at).ok())
        else {
            return &[];
        };
        self.bytes.get(at..).unwrap_or_default()
    }

    pub(crate) fn read(&mut self) -> Vec<Action> {
        let Some(document) = self.document.clone() else {
            return vec![];
        };
        self.generation = self.generation.wrapping_add(1);
        self.loading = true;
        vec![Action::Hex(Request::Page {
            document,
            offset: self.cursor / 4096 * 4096,
            generation: self.generation,
        })]
    }

    pub fn apply(&mut self, generation: u64, result: Result<Loaded, String>) {
        if generation != self.generation {
            return;
        }
        self.loading = false;
        match result {
            Ok(loaded) => {
                if let Some(status) = loaded.recovered
                    && let Err(error) = self.edit.recovered(status, &loaded.document)
                {
                    self.document = Some(loaded.document);
                    self.bytes.clear();
                    self.error = Some(error);
                    return;
                }
                self.document = Some(loaded.document);
                self.page_offset = loaded.offset;
                self.bytes = loaded.bytes;
                self.error = None;
            }
            Err(error) => {
                self.bytes.clear();
                self.error = Some(error);
            }
        }
    }
}

/// Goto is integer arithmetic only: decimal, 0x hex or signed relative movement.
pub fn goto(input: &str, cursor: u64, length: u64) -> Result<u64, &'static str> {
    let (relative, negative, digits) = match input.as_bytes().first() {
        Some(b'+') => (true, false, &input[1..]),
        Some(b'-') => (true, true, &input[1..]),
        _ => (false, false, input),
    };
    let value = match digits.strip_prefix("0x") {
        Some(hex) => u64::from_str_radix(hex, 16),
        None => digits.parse(),
    }
    .map_err(|_| "Enter decimal, 0x hex, +distance or -distance")?;
    let at = if relative {
        if negative {
            cursor.checked_sub(value)
        } else {
            cursor.checked_add(value)
        }
    } else {
        Some(value)
    }
    .ok_or("Offset overflow")?;
    if at > length {
        Err("Offset is past EOF")
    } else {
        Ok(at)
    }
}

impl App {
    pub fn recover_hex_at(
        &mut self,
        id: &str,
        path: &str,
        root: Option<String>,
        offset: u64,
        read_only: bool,
    ) -> Vec<Action> {
        let mut actions = self.open_hex_at(path, root, offset, read_only);
        for action in &mut actions {
            if let Action::Hex(Request::Open { recover, .. }) = action {
                *recover = Some(id.into());
            }
        }
        actions
    }
    pub fn open_hex(&mut self, path: &str) -> Vec<Action> {
        self.open_hex_at(path, None, 0, false)
    }

    pub fn open_hex_at(
        &mut self,
        path: &str,
        expected_root: Option<String>,
        offset: u64,
        read_only: bool,
    ) -> Vec<Action> {
        if path.is_empty() {
            self.toast("Usage: /hex <project-relative path>");
            return vec![];
        }
        let mut actions = self.close_hex_document();
        self.hex.edit = edit::State {
            read_only,
            ..Default::default()
        };
        self.hex.generation = self.hex.generation.wrapping_add(1);
        self.hex.bytes.clear();
        self.hex.cursor = offset;
        self.hex.page_offset = 0;
        self.hex.error = None;
        self.hex.loading = true;
        self.hex.wanted = true;
        self.hex.input = None;
        self.hex.bookmarks.clear();
        self.explorer.awaiting = None;
        self.overlay = Overlay::Hex;
        actions.push(Action::Hex(Request::Open {
            path: path.into(),
            expected_root,
            read_only,
            recover: None,
            offset,
            generation: self.hex.generation,
        }));
        actions
    }

    pub(super) fn close_hex_document(&mut self) -> Vec<Action> {
        self.hex.wanted = false;
        self.hex.bytes.clear();
        self.hex.bookmarks.clear();
        self.hex
            .document
            .take()
            .map(|document| {
                Action::Hex(Request::Close {
                    id: document.id,
                    session_id: self.session.id.clone(),
                    directory: self.session.directory.clone(),
                })
            })
            .into_iter()
            .collect()
    }

    pub(crate) fn hex_key(&mut self, key: KeyEvent) -> Vec<Action> {
        if self.hex.input.is_some() {
            return self.hex_input_key(key);
        }
        if let Some(actions) = self.hex_edit_key(key) {
            return actions;
        }
        match self.keymap.action("hex", &key, false) {
            Some("hex_close") => {
                self.hex.generation = self.hex.generation.wrapping_add(1);
                self.overlay = Overlay::None;
                self.hex.loading = false;
                return self.close_hex_document();
            }
            Some("hex_goto") => self.hex.input = Some((Input::Goto, String::new())),
            Some("hex_search") => self.hex.input = Some((Input::FindHex, String::new())),
            Some("hex_search_next") => return self.hex_search(),
            Some("hex_endian") => self.hex.big_endian = !self.hex.big_endian,
            Some("hex_bookmark") if self.hex.bookmarks.len() < 64 => {
                self.hex.input = Some((Input::Bookmark, String::new()))
            }
            Some("hex_next_bookmark") => {
                if let Some(at) = self
                    .hex
                    .bookmarks
                    .iter()
                    .map(|(_, at)| *at)
                    .filter(|at| *at > self.hex.cursor)
                    .min()
                    .or_else(|| self.hex.bookmarks.iter().map(|(_, at)| *at).min())
                {
                    self.hex.cursor = at;
                    return self.hex.read();
                }
            }
            Some("hex_copy") => {
                if let Some(byte) = self.hex.byte_slice().first() {
                    return vec![Action::Copy(format!("{byte:02x}"))];
                }
            }
            Some("hex_refresh") => return self.hex.read(),
            _ => return self.hex_navigate(key.code),
        }
        vec![]
    }

    fn hex_navigate(&mut self, code: KeyCode) -> Vec<Action> {
        let rows = crate::view::hex::bytes_per_row(
            self.frame_width.load(std::sync::atomic::Ordering::Relaxed),
        ) as u64;
        let length = self.hex.document.as_ref().map_or(0, |d| d.length);
        self.hex.cursor = match code {
            KeyCode::Left => self.hex.cursor.saturating_sub(1),
            KeyCode::Right => self.hex.cursor.saturating_add(1),
            KeyCode::Up => self.hex.cursor.saturating_sub(rows),
            KeyCode::Down => self.hex.cursor.saturating_add(rows),
            KeyCode::PageUp => self.hex.cursor.saturating_sub(rows * 16),
            KeyCode::PageDown => self.hex.cursor.saturating_add(rows * 16),
            KeyCode::Home => 0,
            KeyCode::End => length,
            _ => return vec![],
        }
        .min(length);
        if self.hex.cursor < self.hex.page_offset
            || self.hex.cursor >= self.hex.page_offset + self.hex.bytes.len() as u64
        {
            return self.hex.read();
        }
        vec![]
    }

    fn hex_input_key(&mut self, key: KeyEvent) -> Vec<Action> {
        let (kind, input) = self.hex.input.as_mut().unwrap();
        match key.code {
            KeyCode::Esc => self.hex.input = None,
            KeyCode::Backspace => {
                input.pop();
            }
            KeyCode::Tab => {
                *kind = match *kind {
                    Input::FindHex => Input::FindText,
                    Input::FindText => Input::FindHex,
                    Input::InsertHex => Input::InsertText,
                    Input::InsertText => Input::InsertHex,
                    Input::ReplaceHex => Input::ReplaceText,
                    Input::ReplaceText => Input::ReplaceHex,
                    other => other,
                }
            }
            KeyCode::Char(c)
                if !key.modifiers.contains(KeyModifiers::CONTROL) && input.len() < 8192 =>
            {
                input.push(c)
            }
            KeyCode::Enter => {
                let (kind, input) = self.hex.input.take().unwrap();
                return self.hex_submit(kind, input);
            }
            _ => {}
        }
        vec![]
    }

    fn hex_submit(&mut self, kind: Input, input: String) -> Vec<Action> {
        if matches!(
            kind,
            Input::InsertHex | Input::InsertText | Input::ReplaceHex | Input::ReplaceText
        ) {
            return self.hex_edit_submit(kind, input);
        }
        match kind {
            Input::Goto => {
                let length = self.hex.document.as_ref().map_or(0, |d| d.length);
                match goto(&input, self.hex.cursor, length) {
                    Ok(at) => {
                        self.hex.cursor = at;
                        return self.hex.read();
                    }
                    Err(error) => self.hex.error = Some(error.into()),
                }
            }
            Input::Bookmark => {
                if input.is_empty() || input.len() > 64 {
                    self.hex.error = Some("Bookmark names require 1–64 bytes".into());
                } else if let Some((_, at)) = self
                    .hex
                    .bookmarks
                    .iter_mut()
                    .find(|(name, _)| *name == input)
                {
                    *at = self.hex.cursor;
                } else if self.hex.bookmarks.len() < 64 {
                    self.hex.bookmarks.push((input, self.hex.cursor));
                }
            }
            Input::FindHex | Input::FindText => {
                let hex = if kind == Input::FindText {
                    input
                        .as_bytes()
                        .iter()
                        .map(|b| format!("{b:02x}"))
                        .collect::<String>()
                } else {
                    input
                };
                match cyber_core::binary::decode_hex(&hex, cyber_core::binary::MAX_PATTERN) {
                    Ok(bytes) if !bytes.is_empty() => {
                        self.hex.found.clear();
                        self.hex.search_hex = Some(hex);
                        self.hex.search_next = Some(self.hex.cursor);
                        return self.hex_search();
                    }
                    _ => {
                        self.hex.error = Some(
                            "Find requires 1–4096 literal bytes, encoded as hex or UTF-8".into(),
                        )
                    }
                }
            }
            Input::InsertHex | Input::InsertText | Input::ReplaceHex | Input::ReplaceText => {
                unreachable!()
            }
        }
        vec![]
    }

    fn hex_search(&mut self) -> Vec<Action> {
        if let Some(at) = self
            .hex
            .found
            .iter()
            .copied()
            .find(|at| *at > self.hex.cursor)
        {
            self.hex.cursor = at;
            return self.hex.read();
        }
        let (Some(document), Some(hex), Some(offset)) = (
            self.hex.document.clone(),
            self.hex.search_hex.clone(),
            self.hex.search_next,
        ) else {
            return vec![];
        };
        self.hex.generation = self.hex.generation.wrapping_add(1);
        self.hex.loading = true;
        vec![Action::Hex(Request::Search {
            document,
            hex,
            offset,
            generation: self.hex.generation,
        })]
    }
}

pub(crate) fn apply(app: &mut App, msg: crate::perform::Msg) -> Vec<Action> {
    use crate::perform::Msg;
    match msg {
        Msg::HexEdit {
            session_id,
            directory,
            generation,
            result,
        } if active(app, &session_id, &directory, generation) => {
            return app.hex_edit_reply(result);
        }
        Msg::Hex {
            session_id,
            directory,
            generation,
            opened,
            result,
        } => {
            if active(app, &session_id, &directory, generation) {
                app.hex.apply(generation, result);
            } else if opened && let Ok(loaded) = result {
                return vec![Action::Hex(Request::Close {
                    id: loaded.document.id,
                    session_id,
                    directory,
                })];
            }
        }
        Msg::HexSearch {
            session_id,
            directory,
            generation,
            result,
        } if active(app, &session_id, &directory, generation) => {
            return app.hex_search_reply(result);
        }
        _ => {}
    }
    vec![]
}
fn active(app: &App, session: &str, directory: &str, generation: u64) -> bool {
    session == app.session.id
        && directory == app.session.directory
        && generation == app.hex.generation
        && app.hex.wanted
        && visible_overlay(&app.overlay)
}
fn visible_overlay(overlay: &Overlay) -> bool {
    matches!(
        overlay,
        Overlay::Hex | Overlay::Permission(_) | Overlay::Question(_)
    )
}
impl App {
    pub(crate) fn sync_hex_visibility(&mut self) {
        if self.hex.wanted && !visible_overlay(&self.overlay) {
            self.forget_hex();
        }
    }
    pub(crate) fn forget_hex(&mut self) {
        self.hex.forget(&self.session);
        if matches!(self.overlay, Overlay::Hex) {
            self.overlay = Overlay::None;
        }
    }
    fn hex_search_reply(
        &mut self,
        result: Result<cyber_core::binary::SearchResult, String>,
    ) -> Vec<Action> {
        self.hex.loading = false;
        match result {
            Ok(result) => {
                self.hex.found = result
                    .offsets
                    .into_iter()
                    .take(100)
                    .map(|at| at.0)
                    .collect();
                self.hex.search_next = result.next_offset.map(|at| at.0);
                if let Some(at) = self.hex.found.first() {
                    self.hex.cursor = *at;
                    return self.hex.read();
                }
                self.hex.error = Some(
                    if self.hex.search_next.is_some() {
                        "No matches in this slice; continue search to scan the next slice"
                    } else {
                        "No more matches"
                    }
                    .into(),
                );
            }
            Err(error) => {
                self.hex.bytes.clear();
                self.hex.error = Some(error);
            }
        }
        vec![]
    }
}

fn field(value: &Value, name: &str) -> Result<String, String> {
    value[name]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| "Invalid binary response".into())
}

async fn page(
    client: &cyber_client::Client,
    session: &Session,
    document: Document,
    offset: u64,
) -> Result<Loaded, String> {
    let path = format!(
        "/files/binary/documents/{}/bytes?session_id={}&revision={}&offset={offset}&limit=4096",
        document.id, session.id, document.revision
    );
    let result = client
        .at(&session.directory)
        .get(&path)
        .await
        .map_err(|e| e.to_string())?;
    let bytes = decode_page(&result["data"], &document, offset)?;
    Ok(Loaded {
        document,
        offset,
        bytes,
        recovered: None,
    })
}

/// Validate the bounded wire page before storing or rendering any bytes.
fn decode_page(value: &Value, document: &Document, offset: u64) -> Result<Vec<u8>, String> {
    let encoded = field(value, "base64")?;
    if encoded.len() > 4096usize.div_ceil(3) * 4 {
        return Err("Binary page exceeds limit".into());
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&encoded)
        .map_err(|_| "Invalid byte encoding")?;
    let length = bytes.len() as u64;
    if length > 4096
        || field(value, "length")? != length.to_string()
        || base64::engine::general_purpose::STANDARD.encode(&bytes) != encoded
        || offset
            .checked_add(length)
            .is_none_or(|end| end > document.length)
    {
        return Err("Invalid binary page length or encoding".into());
    }
    if field(value, "offset")? != offset.to_string()
        || field(value, "revision")? != document.revision
        || field(value, "source")? != document.source
    {
        return Err("Stale binary page".into());
    }
    if field(value, "page_sha256")? != format!("{:x}", Sha256::digest(&bytes)) {
        return Err("Binary page fingerprint mismatch".into());
    }
    Ok(bytes)
}

pub async fn perform(
    client: &cyber_client::Client,
    session: &Session,
    request: Request,
) -> crate::perform::Msg {
    use crate::perform::Msg;
    let (generation, opened, result) = match request {
        Request::Edit {
            document,
            command,
            operation_id,
            generation,
        } => {
            return crate::perform::Msg::HexEdit {
                session_id: session.id.clone(),
                directory: session.directory.clone(),
                generation,
                result: edit::perform(client, session, document, command, operation_id).await,
            };
        }
        Request::Close {
            id,
            session_id,
            directory,
        } => {
            let _ = client
                .at(&directory)
                .delete(&format!(
                    "/files/binary/documents/{id}?session_id={session_id}"
                ))
                .await;
            return Msg::Done;
        }
        Request::Search {
            document,
            hex,
            offset,
            generation,
        } => {
            let result = client
                .at(&session.directory)
                .post(
                    &format!("/files/binary/documents/{}/search", document.id),
                    json!({"session_id":session.id,"revision":document.revision,"hex":hex,"offset":offset.to_string()}),
                )
                .await
                .map_err(|e| e.to_string())
                .and_then(|value| {
                    serde_json::from_value(value["data"].clone())
                        .map_err(|_| "Invalid search response".into())
                });
            return Msg::HexSearch {
                session_id: session.id.clone(),
                directory: session.directory.clone(),
                generation,
                result,
            };
        }
        Request::Page {
            document,
            offset,
            generation,
        } => (
            generation,
            false,
            page(client, session, document, offset).await,
        ),
        Request::Open {
            path,
            expected_root,
            read_only,
            recover,
            offset,
            generation,
        } => {
            let result = async {
                if recover.as_ref().is_some_and(|id| {
                    !id.starts_with("bin_") || id.len() > 100 || id.len() <= 4
                        || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
                }) {
                    return Err("Invalid retained binary document identity".into());
                }
                let route = recover.as_ref().map_or_else(
                    || "/files/binary/documents".into(),
                    |id| format!("/files/binary/documents/{id}/recover"),
                );
                let body = if recover.is_some() {
                    json!({"session_id":session.id,"expected_root":expected_root,"read_only":read_only})
                } else {
                    json!({"session_id":session.id,"path":path,"expected_root":expected_root,"read_only":read_only})
                };
                let value = client
                    .at(&session.directory)
                    .post(&route, body)
                    .await
                    .map_err(|e| e.to_string())?;
                let data = &value["data"];
                let document = Document {
                    id: match &recover {
                        Some(id) => id.clone(),
                        None => field(data, "document_id")?,
                    },
                    path: field(data, "path")?,
                    revision: field(data, "revision")?,
                    source: field(data,"source")?,
                    length: field(data, "length")?
                        .parse()
                        .map_err(|_| "Invalid byte length")?,
                };
                let id = document.id.clone();
                let result = if recover.is_some() && (document.path != path || document.source != "draft") {
                    Err("Recovered draft does not match the requested file".into())
                } else if offset > document.length {
                    Err("Offset is past EOF".into())
                } else {
                    page(client, session, document, offset / 4096 * 4096).await.map(|mut loaded| {
                        if recover.is_some() {
                            loaded.recovered = Some(data.clone());
                        }
                        loaded
                    })
                };
                if result.is_err() {
                    let _ = client
                        .at(&session.directory)
                        .delete(&format!(
                            "/files/binary/documents/{id}?session_id={}",
                            session.id
                        ))
                        .await;
                }
                result
            }
            .await;
            (generation, true, result)
        }
    };
    Msg::Hex {
        session_id: session.id.clone(),
        directory: session.directory.clone(),
        generation,
        opened,
        result,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn byte_page_checks_encoding_budget_scope_and_content_evidence() {
        let document = Document {
            source: "live".into(),
            id: "bin_one".into(),
            path: "bytes".into(),
            revision: "observed".into(),
            length: 2,
        };
        let good = json!({"base64":"AP8=", "length":"2", "offset":"0", "revision":"observed", "source":"live",
            "page_sha256":format!("{:x}", Sha256::digest([0,255]))});
        assert_eq!(decode_page(&good, &document, 0).unwrap(), [0, 255]);
        for (key, bad) in [
            ("length", json!("3")),
            ("offset", json!("1")),
            ("revision", json!("other")),
            ("source", json!("draft")),
            ("page_sha256", json!("wrong")),
            ("base64", json!("AP8")),
            ("base64", json!("A".repeat(6000))),
        ] {
            let mut value = good.clone();
            value[key] = bad;
            assert!(decode_page(&value, &document, 0).is_err(), "{key}");
        }
        let short = Document {
            length: 1,
            ..document
        };
        assert!(decode_page(&good, &short, 0).is_err());
    }
    #[test]
    fn goto_preserves_large_offsets_and_checks_relative_overflow_and_eof() {
        assert_eq!(goto("9007199254740993", 0, u64::MAX), Ok(9007199254740993));
        assert_eq!(goto("0x20000000000001", 0, u64::MAX), Ok(9007199254740993));
        assert_eq!(goto("-0x10", 32, 100), Ok(16));
        assert_eq!(goto("+10", 32, 100), Ok(42));
        for (input, cursor, length) in [
            ("-1", 0, 10),
            ("+1", u64::MAX, u64::MAX),
            ("11", 0, 10),
            ("1.5", 0, 10),
        ] {
            assert!(goto(input, cursor, length).is_err());
        }
    }
}

#[cfg(test)]
mod ownership_tests {
    use super::*;
    #[test]
    fn read_only_launch_is_carried_into_the_server_document_admission() {
        let mut app = App::new(
            Session {
                id: "ses_one".into(),
                directory: "/project".into(),
                ..Default::default()
            },
            vec![],
            "cyber",
        );
        let actions = app.open_hex_at("bytes", Some("/project".into()), 0, true);
        assert!(actions.iter().any(|action| matches!(
            action,
            Action::Hex(Request::Open {
                read_only: true,
                ..
            })
        )));
        let actions = app.open_hex("bytes");
        assert!(actions.iter().any(|action| matches!(
            action,
            Action::Hex(Request::Open {
                read_only: false,
                ..
            })
        )));
    }

    #[test]
    fn permission_overlay_keeps_the_hex_reply_and_other_views_retire_it() {
        let mut app = App::new(
            Session {
                id: "ses_one".into(),
                directory: "/project".into(),
                ..Default::default()
            },
            vec![],
            "cyber",
        );
        app.open_hex("bytes");
        app.overlay = Overlay::Permission(crate::app::PermStep::Choose(0));
        app.sync_hex_visibility();
        let message = crate::perform::Msg::Hex {
            session_id: app.session.id.clone(),
            directory: app.session.directory.clone(),
            generation: app.hex.generation,
            opened: true,
            result: Ok(Loaded {
                document: Document {
                    source: "live".into(),
                    id: "bin_one".into(),
                    path: "bytes".into(),
                    revision: "observed".into(),
                    length: 2,
                },
                offset: 0,
                bytes: vec![0, 255],
                recovered: None,
            }),
        };
        assert!(apply(&mut app, message).is_empty());
        assert_eq!(app.hex.bytes, [0, 255]);
        assert!(app.hex.wanted);
        assert!(
            matches!(app.overlay, Overlay::Permission(_)),
            "a byte reply cannot hide a permission request"
        );
        app.overlay = Overlay::Settings;
        app.sync_hex_visibility();
        assert!(app.hex.bytes.is_empty());
        assert!(!app.hex.wanted);
        assert!(
            matches!(&app.hex.pending_closes[..],[Request::Close{id,session_id,..}] if id=="bin_one" && session_id=="ses_one")
        );
    }
}
