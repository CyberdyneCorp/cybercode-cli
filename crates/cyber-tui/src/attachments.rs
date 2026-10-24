//! Large paste, image and PDF input in the composer (`tui` → Large paste and image
//! input): the chip table beside the composer text, paste routing, the clipboard, and
//! the parts a draft submits.
//!
//! A chip is a label in the text (`[Pasted text #N +L lines]`, `[Image #N]`,
//! `[PDF #N]`) with its payload in the table. A label edited by hand is no longer a
//! chip: only labels still in the text are submitted.

mod clipboard;

use std::path::{Path, PathBuf};

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[cfg(test)]
pub use clipboard::Fake;
pub use clipboard::{Clipboard, ClipboardError};

/// The client-side attachment limit; the server's `attachments.max_bytes` is the one
/// that counts.
pub const MAX_BYTES: u64 = 20 * 1024 * 1024;
pub const TOO_LARGE: &str = "Attachment exceeds 20 MiB";
pub const NO_IMAGE: &str = "No image on the clipboard";
const GONE: &str = "Clipboard image from the saved draft is gone";

/// A paste longer than this many characters or lines becomes a text chip.
const LARGE_CHARS: usize = 1_000;
const LARGE_LINES: usize = 25;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Text,
    Image,
    Pdf,
}

impl Kind {
    fn index(self) -> usize {
        self as usize
    }

    /// An image or PDF by its file extension.
    pub fn of_path(path: &Path) -> Option<Self> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        match ext.as_str() {
            "png" | "jpg" | "jpeg" | "gif" | "webp" => Some(Self::Image),
            "pdf" => Some(Self::Pdf),
            _ => None,
        }
    }

    fn of_media_type(media_type: &str) -> Self {
        if media_type == "application/pdf" {
            Self::Pdf
        } else {
            Self::Image
        }
    }
}

/// What a chip stands for.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum Payload {
    Text {
        text: String,
    },
    /// A file on this machine, by absolute path.
    Path {
        path: String,
    },
    /// A clipboard image saved as a private file in the TUI state directory.
    Clipboard {
        file: String,
    },
    /// A blob the Session already stores (an input taken back from the queue).
    Stored {
        sha256: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Chip {
    pub kind: Kind,
    pub number: u32,
    pub payload: Payload,
}

impl Chip {
    pub fn label(&self) -> String {
        let n = self.number;
        match (&self.kind, &self.payload) {
            (Kind::Text, Payload::Text { text }) => {
                format!("[Pasted text #{n} +{} lines]", text.lines().count())
            }
            (Kind::Pdf, _) => format!("[PDF #{n}]"),
            _ => format!("[Image #{n}]"),
        }
    }
}

/// Remove the private clipboard files of `chips`.
pub fn discard(chips: &[Chip]) {
    for chip in chips {
        if let Payload::Clipboard { file } = &chip.payload {
            let _ = std::fs::remove_file(file);
        }
    }
}

/// The chips of the draft and where clipboard images are kept.
pub struct Attachments {
    pub chips: Vec<Chip>,
    /// The next number per [`Kind`]; numbers are never reused within a draft.
    next: [u32; 3],
    /// The TUI state directory; clipboard images go below it.
    pub dir: Option<PathBuf>,
    pub clipboard: Box<dyn Clipboard>,
    /// `tui.paste_summary`.
    pub paste_summary: bool,
}

impl Default for Attachments {
    fn default() -> Self {
        Self {
            chips: Vec::new(),
            next: [1; 3],
            dir: None,
            clipboard: Box::new(clipboard::System),
            paste_summary: true,
        }
    }
}

/// What a bracketed paste becomes.
#[derive(Debug, PartialEq)]
pub enum Pasted {
    /// Labels to insert, and a toast for paths that could not be attached.
    Chips(String, Option<&'static str>),
    Text(String),
}

impl Attachments {
    /// Add a chip and return its label.
    pub fn add(&mut self, kind: Kind, payload: Payload) -> String {
        let number = self.next[kind.index()];
        self.next[kind.index()] += 1;
        let chip = Chip {
            kind,
            number,
            payload,
        };
        let label = chip.label();
        self.chips.push(chip);
        label
    }

    /// Route a bracketed paste: dropped image or PDF paths, then a large text, then
    /// plain text.
    pub fn paste(&mut self, text: &str) -> Pasted {
        if let Some(paths) = dropped_paths(text, !cfg!(windows)) {
            return self.attach_paths(paths);
        }
        let text = text.replace("\r\n", "\n").replace('\r', "\n");
        let large = text.chars().count() > LARGE_CHARS || text.lines().count() > LARGE_LINES;
        if self.paste_summary && large {
            let label = self.add(Kind::Text, Payload::Text { text });
            return Pasted::Chips(label, None);
        }
        Pasted::Text(text)
    }

    fn attach_paths(&mut self, paths: Vec<(PathBuf, Kind)>) -> Pasted {
        let mut labels = Vec::new();
        let mut toast = None;
        for (path, kind) in paths {
            match self.attach_path(&path, kind) {
                Ok(label) => labels.push(label),
                Err(message) => toast = Some(message),
            }
        }
        let mut labels = labels.join(" ");
        if !labels.is_empty() {
            labels.push(' ');
        }
        Pasted::Chips(labels, toast)
    }

    fn attach_path(&mut self, path: &Path, kind: Kind) -> Result<String, &'static str> {
        let size = std::fs::metadata(path)
            .map_err(|_| "Attachment not found")?
            .len();
        if size > MAX_BYTES {
            return Err(TOO_LARGE);
        }
        let path = path.display().to_string();
        Ok(self.add(kind, Payload::Path { path }))
    }

    /// An `@` completion of `key` (relative to `directory`): a chip label for an
    /// existing image or PDF, `None` for anything else.
    pub fn mention(&mut self, key: &str, directory: &str) -> Option<Result<String, &'static str>> {
        let path = Path::new(directory).join(key);
        let kind = Kind::of_path(&path).filter(|_| path.is_file())?;
        Some(self.attach_path(&path, kind))
    }

    /// `composer.paste_image`: the clipboard image as a chip, saved as a private file.
    pub fn paste_image(&mut self) -> Result<String, String> {
        let bytes = match self.clipboard.image(MAX_BYTES) {
            Ok(Some(bytes)) => bytes,
            Ok(None) => return Err(NO_IMAGE.into()),
            Err(ClipboardError::MissingTool(message)) => return Err(message.into()),
            Err(ClipboardError::TooLarge) => return Err(TOO_LARGE.into()),
            Err(ClipboardError::Failed(error)) => return Err(format!("Clipboard: {error}")),
        };
        let dir = self
            .dir
            .as_ref()
            .ok_or("Clipboard: no state directory")?
            .join("clipboard");
        let file = save_private(&dir, &bytes).map_err(|e| format!("Clipboard: {e}"))?;
        let file = file.display().to_string();
        Ok(self.add(Kind::Image, Payload::Clipboard { file }))
    }

    /// Backspace right after a label removes the whole chip; false when there is none.
    pub fn backspace(&mut self, composer: &mut crate::composer::Composer) -> bool {
        let (before, _) = composer.around_cursor();
        let Some(at) = self.chips.iter().position(|c| before.ends_with(&c.label())) else {
            return false;
        };
        let chip = self.chips.remove(at);
        for _ in 0..chip.label().chars().count() {
            composer.backspace();
        }
        discard(&[chip]);
        true
    }

    /// Delete right before a label removes the whole chip; false when there is none.
    pub fn delete(&mut self, composer: &mut crate::composer::Composer) -> bool {
        let (_, after) = composer.around_cursor();
        let Some(at) = self
            .chips
            .iter()
            .position(|c| after.starts_with(&c.label()))
        else {
            return false;
        };
        let chip = self.chips.remove(at);
        for _ in 0..chip.label().chars().count() {
            composer.delete();
        }
        discard(&[chip]);
        true
    }

    /// Submit: the chips whose labels are still in `text`, in label order. The draft's
    /// table starts over; clipboard files of chips edited away are removed.
    pub fn take(&mut self, text: &str) -> Vec<Chip> {
        let (mut used, gone): (Vec<Chip>, Vec<Chip>) = std::mem::take(&mut self.chips)
            .into_iter()
            .partition(|chip| text.contains(&chip.label()));
        discard(&gone);
        used.sort_by_key(|chip| text.find(&chip.label()));
        self.next = [1; 3];
        used
    }

    /// Clear the draft's chips.
    pub fn clear(&mut self) {
        discard(&std::mem::take(&mut self.chips));
        self.next = [1; 3];
    }

    /// Put chips back (a restored draft, a refused prompt). A clipboard image whose
    /// file is gone is dropped with a toast; a chip whose label the table already holds
    /// is dropped, so one label never sends two attachments.
    pub fn restore(&mut self, chips: Vec<Chip>) -> Option<&'static str> {
        let mut toast = None;
        for chip in chips {
            if self.chips.iter().any(|c| c.label() == chip.label()) {
                discard(&[chip]);
                continue;
            }
            if let Payload::Clipboard { file } = &chip.payload
                && !Path::new(file).is_file()
            {
                toast = Some(GONE);
                continue;
            }
            let next = &mut self.next[chip.kind.index()];
            *next = (*next).max(chip.number + 1);
            self.chips.push(chip);
        }
        toast
    }

    /// An input taken back from the queue: its stored attachments become chips. The
    /// k-th `[Image #N]` (or `[PDF #N]`) label in `text` takes the k-th stored image
    /// (or PDF); one without a label gets a new label after the text.
    pub fn take_back(&mut self, text: &str, stored: &[Stored]) -> String {
        let mut text = text.to_owned();
        let mut seen = [0usize; 3];
        for item in stored {
            let numbers = label_numbers(&text, item.kind);
            let k = &mut seen[item.kind.index()];
            let payload = Payload::Stored {
                sha256: item.sha256.clone(),
            };
            match numbers.get(*k) {
                Some(&number) => {
                    self.restore(vec![Chip {
                        kind: item.kind,
                        number,
                        payload,
                    }]);
                    *k += 1;
                }
                None => {
                    let label = self.add(item.kind, payload);
                    text = format!("{} {label}", text.trim_end());
                }
            }
        }
        text
    }
}

/// The numbers of the `[Image #N]` or `[PDF #N]` labels in `text`, in order.
fn label_numbers(text: &str, kind: Kind) -> Vec<u32> {
    let prefix = if kind == Kind::Pdf {
        "[PDF #"
    } else {
        "[Image #"
    };
    text.match_indices(prefix)
        .filter_map(|(at, _)| {
            let rest = &text[at + prefix.len()..];
            let (digits, _) = rest.split_once(']')?;
            digits.parse().ok()
        })
        .collect()
}

/// A stored attachment of a queued input.
#[derive(Debug, Clone, PartialEq)]
pub struct Stored {
    pub sha256: String,
    pub kind: Kind,
}

/// The stored attachments among `parts`.
pub fn stored(parts: &Value) -> Vec<Stored> {
    parts
        .as_array()
        .into_iter()
        .flatten()
        .filter(|part| part["type"] == "attachment")
        .filter_map(|part| {
            Some(Stored {
                sha256: part["sha256"].as_str()?.to_owned(),
                kind: Kind::of_media_type(part["media_type"].as_str().unwrap_or_default()),
            })
        })
        .collect()
}

/// `[Image: shot.png, 12 KB]` or `[PDF: spec.pdf, 1.2 MB]` for a media part of the
/// transcript; `[Image]` without a name.
pub fn transcript_label(part: &Value) -> String {
    let kind = match part["type"].as_str() {
        Some("document") => Kind::Pdf,
        Some("attachment") => Kind::of_media_type(part["media_type"].as_str().unwrap_or_default()),
        _ => Kind::Image,
    };
    let word = if kind == Kind::Pdf { "PDF" } else { "Image" };
    let Some(name) = part["name"].as_str() else {
        return format!("[{word}]");
    };
    match part["size"].as_u64() {
        Some(bytes) => format!("[{word}: {name}, {}]", size(bytes)),
        None => format!("[{word}: {name}]"),
    }
}

fn size(bytes: u64) -> String {
    match bytes {
        b if b >= 1024 * 1024 => format!("{:.1} MB", b as f64 / (1024.0 * 1024.0)),
        b if b >= 1024 => format!("{} KB", b / 1024),
        b => format!("{b} B"),
    }
}

/// The image or PDF type of `bytes` by their magic, as the server sniffs them.
pub fn sniff(bytes: &[u8]) -> Option<&'static str> {
    const MAGIC: &[(&[u8], &str)] = &[
        (b"\x89PNG\r\n\x1a\n", "image/png"),
        (b"\xff\xd8\xff", "image/jpeg"),
        (b"GIF87a", "image/gif"),
        (b"GIF89a", "image/gif"),
        (b"%PDF-", "application/pdf"),
    ];
    if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    MAGIC
        .iter()
        .find(|(magic, _)| bytes.starts_with(magic))
        .map(|(_, media_type)| *media_type)
}

/// Write `bytes` to a new private (`0600`) file in `dir`.
fn save_private(dir: &Path, bytes: &[u8]) -> std::io::Result<PathBuf> {
    use std::io::Write as _;
    std::fs::create_dir_all(dir)?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let path = dir.join(format!("{}-{stamp}.png", std::process::id()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options.open(&path)?.write_all(bytes)?;
    Ok(path)
}

/// The paths of a drag-and-drop paste: every whitespace-separated token is a quoted,
/// backslash-escaped (when `escapes`) or `file://` absolute path of an existing image
/// or PDF. `None` when any token is not.
pub fn dropped_paths(text: &str, escapes: bool) -> Option<Vec<(PathBuf, Kind)>> {
    let tokens = tokenize(text.trim(), escapes)?;
    if tokens.is_empty() {
        return None;
    }
    tokens
        .into_iter()
        .map(|token| {
            let path = PathBuf::from(file_url(&token).unwrap_or(token));
            let kind = Kind::of_path(&path)?;
            (path.is_absolute() && path.is_file()).then_some((path, kind))
        })
        .collect()
}

/// Split `text` at unquoted whitespace, removing quotes and (when `escapes`) the
/// backslash before an escaped character. `None` on an unterminated quote.
pub fn tokenize(text: &str, escapes: bool) -> Option<Vec<String>> {
    let mut tokens = Vec::new();
    let mut current: Option<String> = None;
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match c {
            c if c.is_whitespace() => tokens.extend(current.take()),
            '\'' | '"' => {
                let token = current.get_or_insert_with(String::new);
                quoted(&mut chars, c, escapes, token)?;
            }
            '\\' if escapes => current.get_or_insert_with(String::new).push(chars.next()?),
            c => current.get_or_insert_with(String::new).push(c),
        }
    }
    tokens.extend(current);
    Some(tokens)
}

/// Read up to the closing `quote` into `token`. Inside double quotes a backslash
/// escapes `"` and `\` (when `escapes`).
fn quoted(
    chars: &mut std::str::Chars<'_>,
    quote: char,
    escapes: bool,
    token: &mut String,
) -> Option<()> {
    loop {
        match chars.next()? {
            c if c == quote => return Some(()),
            '\\' if escapes && quote == '"' => token.push(chars.next()?),
            c => token.push(c),
        }
    }
}

/// The path of a `file://` URL, percent-decoded; `file:///C:/x` is `C:/x`.
fn file_url(token: &str) -> Option<String> {
    let rest = token.strip_prefix("file://")?;
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    let mut bytes = Vec::with_capacity(rest.len());
    let mut iter = rest.bytes();
    while let Some(b) = iter.next() {
        if b == b'%' {
            let hex = [iter.next()?, iter.next()?];
            bytes.push(u8::from_str_radix(std::str::from_utf8(&hex).ok()?, 16).ok()?);
        } else {
            bytes.push(b);
        }
    }
    let path = String::from_utf8(bytes).ok()?;
    let drive = path.len() > 2 && path.as_bytes()[2] == b':' && path.starts_with('/');
    Some(if drive { path[1..].to_owned() } else { path })
}

/// The `@path` tokens of `text` that name an existing image or PDF under `directory`.
pub fn mentioned(text: &str, directory: &str) -> Vec<(PathBuf, Kind)> {
    text.split_whitespace()
        .filter_map(|word| word.strip_prefix('@'))
        .filter_map(|token| {
            let path = Path::new(directory).join(token);
            let kind = Kind::of_path(&path)?;
            path.is_file().then_some((path, kind))
        })
        .collect()
}

/// `text` with each text chip's label replaced by its pasted content.
pub fn expand_text(text: &str, chips: &[Chip]) -> String {
    let mut expanded = text.to_owned();
    for chip in chips {
        if let Payload::Text { text } = &chip.payload {
            expanded = expanded.replacen(&chip.label(), text, 1);
        }
    }
    expanded
}

/// Whether `text` names an image or PDF chip of the draft.
pub fn names_media(text: &str, chips: &[Chip]) -> bool {
    chips
        .iter()
        .any(|chip| chip.kind != Kind::Text && text.contains(&chip.label()))
}

/// The text with text chips expanded (media labels stay so the model can tell the
/// media apart), and one part per media chip and `@` media mention: a `file` part the
/// server reads when `local`, else inline bytes; `attachment` for a stored blob.
pub fn submit_parts(
    text: &str,
    chips: &[Chip],
    directory: &str,
    local: bool,
) -> Result<(String, Vec<Value>), String> {
    let expanded = expand_text(text, chips);
    let mut media = Vec::new();
    for chip in chips {
        match &chip.payload {
            Payload::Text { .. } => {}
            Payload::Path { path } => media.push(path_part(Path::new(path), chip.kind, local)?),
            Payload::Clipboard { file } => media.push(inline_part(
                Path::new(file),
                chip.kind,
                Some("clipboard.png"),
            )?),
            Payload::Stored { sha256 } => {
                media.push(json!({ "type": "attachment", "sha256": sha256 }))
            }
        }
    }
    for (path, kind) in mentioned(text, directory) {
        media.push(path_part(&path, kind, local)?);
    }
    Ok((expanded, media))
}

fn path_part(path: &Path, kind: Kind, local: bool) -> Result<Value, String> {
    let name = path.file_name().and_then(|n| n.to_str());
    if !local {
        return inline_part(path, kind, name);
    }
    if std::fs::metadata(path)
        .map_err(|e| format!("{}: {e}", path.display()))?
        .len()
        > MAX_BYTES
    {
        return Err(TOO_LARGE.into());
    }
    Ok(json!({ "type": "file", "path": path.display().to_string(), "name": name }))
}

/// The file's bytes as an inline `image` or `document` part.
fn inline_part(path: &Path, kind: Kind, name: Option<&str>) -> Result<Value, String> {
    use std::io::Read as _;
    let file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(TOO_LARGE.into());
    }
    let media_type =
        sniff(&bytes).ok_or_else(|| format!("{}: not an image or PDF", path.display()))?;
    let data = base64::engine::general_purpose::STANDARD.encode(&bytes);
    let part = if kind == Kind::Pdf {
        "document"
    } else {
        "image"
    };
    Ok(json!({ "type": part, "media_type": media_type, "data": data, "name": name }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenizer_reads_quotes_escapes_and_file_urls() {
        let tokens = |text: &str| tokenize(text, true);
        assert_eq!(
            tokens("'/Users/me/Desktop/Screen Shot.png'"),
            Some(vec!["/Users/me/Desktop/Screen Shot.png".into()])
        );
        assert_eq!(
            tokens(r"/tmp/My\ Shot.png  /tmp/b.pdf"),
            Some(vec!["/tmp/My Shot.png".into(), "/tmp/b.pdf".into()])
        );
        assert_eq!(
            tokens(r#""/tmp/say \"hi\".png""#),
            Some(vec![r#"/tmp/say "hi".png"#.into()])
        );
        assert_eq!(tokens("'/tmp/open"), None, "unterminated quote");
        assert_eq!(tokens("   "), Some(vec![]));
        // Windows paths keep their backslashes.
        assert_eq!(
            tokenize(r#""C:\Users\me\Shot 1.png" C:\x.pdf"#, false),
            Some(vec![r"C:\Users\me\Shot 1.png".into(), r"C:\x.pdf".into()])
        );
        assert_eq!(
            file_url("file:///Users/me/Screen%20Shot.png").as_deref(),
            Some("/Users/me/Screen Shot.png")
        );
        assert_eq!(
            file_url("file://localhost/tmp/a.png").as_deref(),
            Some("/tmp/a.png")
        );
        assert_eq!(
            file_url("file:///C:/x/a.png").as_deref(),
            Some("C:/x/a.png")
        );
        assert_eq!(file_url("file:///bad%zz"), None);
        assert_eq!(file_url("/tmp/a.png"), None);
    }

    #[test]
    fn only_existing_absolute_media_paths_are_drops() {
        let dir = tempfile::tempdir().unwrap();
        let shot = dir.path().join("Screen Shot.png");
        std::fs::write(&shot, b"\x89PNG\r\n\x1a\n").unwrap();
        std::fs::write(dir.path().join("notes.txt"), "x").unwrap();
        let quoted = format!("'{}'", shot.display());
        assert_eq!(
            dropped_paths(&quoted, true),
            Some(vec![(shot.clone(), Kind::Image)])
        );
        let text = format!(
            "'{}' '{}'",
            shot.display(),
            dir.path().join("notes.txt").display()
        );
        assert_eq!(
            dropped_paths(&text, true),
            None,
            "a text file is not a drop"
        );
        let missing = format!("'{}'", dir.path().join("gone.png").display());
        assert_eq!(dropped_paths(&missing, true), None);
        assert_eq!(dropped_paths("Screen Shot.png", true), None);
        assert_eq!(dropped_paths("look at this", true), None);
    }

    #[test]
    fn labels_number_per_kind_and_take_keeps_label_order() {
        let mut a = Attachments::default();
        let text = a.add(
            Kind::Text,
            Payload::Text {
                text: "a\nb\nc".into(),
            },
        );
        let image = a.add(Kind::Image, Payload::Stored { sha256: "s".into() });
        let pdf = a.add(Kind::Pdf, Payload::Stored { sha256: "p".into() });
        assert_eq!(
            (text.as_str(), image.as_str(), pdf.as_str()),
            ("[Pasted text #1 +3 lines]", "[Image #1]", "[PDF #1]")
        );
        assert_eq!(
            a.add(Kind::Image, Payload::Stored { sha256: "t".into() }),
            "[Image #2]"
        );
        let used = a.take("[PDF #1] then [Image #1]");
        let labels: Vec<String> = used.iter().map(Chip::label).collect();
        assert_eq!(labels, ["[PDF #1]", "[Image #1]"]);
        assert!(a.chips.is_empty());
    }

    #[test]
    fn taken_back_inputs_pair_labels_with_stored_blobs() {
        let mut a = Attachments::default();
        let stored = [
            Stored {
                sha256: "aa".into(),
                kind: Kind::Image,
            },
            Stored {
                sha256: "bb".into(),
                kind: Kind::Image,
            },
        ];
        let text = a.take_back("compare [Image #3]", &stored);
        assert_eq!(text, "compare [Image #3] [Image #4]");
        let used = a.take(&text);
        assert_eq!(
            used.iter()
                .map(|c| (c.number, c.payload.clone()))
                .collect::<Vec<_>>(),
            [
                (
                    3,
                    Payload::Stored {
                        sha256: "aa".into()
                    }
                ),
                (
                    4,
                    Payload::Stored {
                        sha256: "bb".into()
                    }
                )
            ]
        );
    }

    #[test]
    fn restore_never_holds_two_chips_with_one_label() {
        let mut a = Attachments::default();
        let draft = a.add(
            Kind::Image,
            Payload::Path {
                path: "/tmp/shot.png".into(),
            },
        );
        let stored = Chip {
            kind: Kind::Image,
            number: 1,
            payload: Payload::Stored {
                sha256: "aa".into(),
            },
        };
        a.restore(vec![stored]);
        let used = a.take(&draft);
        assert_eq!(used.len(), 1);
        assert!(matches!(used[0].payload, Payload::Path { .. }));
    }

    #[test]
    fn a_peer_gets_inline_bytes_and_a_local_server_a_file_part() {
        let dir = tempfile::tempdir().unwrap();
        let pdf = dir.path().join("spec.pdf");
        std::fs::write(&pdf, b"%PDF-1.4\n").unwrap();
        let mut a = Attachments::default();
        let label = a.add(
            Kind::Pdf,
            Payload::Path {
                path: pdf.display().to_string(),
            },
        );
        let pasted = a.add(
            Kind::Text,
            Payload::Text {
                text: "one\ntwo".into(),
            },
        );
        let text = format!("read {label} and {pasted}");
        let chips = a.take(&text);
        let directory = dir.path().display().to_string();
        let (expanded, local) = submit_parts(&text, &chips, &directory, true).unwrap();
        assert_eq!(expanded, "read [PDF #1] and one\ntwo");
        assert_eq!(
            local,
            [json!({"type": "file", "path": pdf.display().to_string(), "name": "spec.pdf"})]
        );
        let (_, peer) = submit_parts(&text, &chips, &directory, false).unwrap();
        assert_eq!(peer[0]["type"], "document");
        assert_eq!(peer[0]["media_type"], "application/pdf");
        assert_eq!(peer[0]["data"], "JVBERi0xLjQK");
        // A typed `@` mention of a PDF is attached too.
        let (_, mentioned) = submit_parts("see @spec.pdf", &[], &directory, true).unwrap();
        assert_eq!(mentioned[0]["type"], "file");
    }

    #[test]
    fn sniffing_matches_the_server_types() {
        assert_eq!(sniff(b"\x89PNG\r\n\x1a\nrest"), Some("image/png"));
        assert_eq!(sniff(b"%PDF-1.7"), Some("application/pdf"));
        assert_eq!(sniff(b"RIFF\0\0\0\0WEBPVP8 "), Some("image/webp"));
        assert_eq!(sniff(b"hello"), None);
    }
}
