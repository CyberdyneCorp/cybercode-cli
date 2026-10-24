//! The Session export (`storage-events` → Session export): the server builds the document
//! (`GET /sessions/{id}/export`, which also applies `--sanitize`), and this module renders
//! it as JSON, Markdown or one self-contained HTML page and writes it atomically. The CLI
//! (`cyber sessions export`) and the TUI (`/export`) share it, so every format shows the
//! same content.

mod html;
mod markdown;
mod md;
mod output;

use serde_json::Value;

use crate::{Client, ClientError};

pub use md::markdown;
pub use output::write_atomic;

/// The formats a Session export is written in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Json,
    Markdown,
    Html,
}

impl Format {
    /// The file extension, and the name `/export` takes.
    pub fn extension(self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::Markdown => "md",
            Self::Html => "html",
        }
    }
}

/// What the server puts in the document.
#[derive(Debug, Clone, Copy, Default)]
pub struct Options {
    /// Every branch instead of the active path.
    pub all_branches: bool,
    /// Child Sessions, recursively.
    pub include_children: bool,
    /// Redaction markers instead of content.
    pub sanitize: bool,
}

/// The export document of Session `id`.
pub async fn document(client: &Client, id: &str, options: Options) -> Result<Value, ClientError> {
    let branches = if options.all_branches {
        "&branches=all"
    } else {
        ""
    };
    let path = format!(
        "/sessions/{}/export?sanitize={}&include_children={}{branches}",
        segment(id),
        options.sanitize,
        options.include_children,
    );
    let mut reply = client.get(&path).await?;
    Ok(reply["data"].take())
}

/// Import an export `document` as a new Session of the client's Location; returns the
/// new Session's info.
pub async fn import(client: &Client, document: Value) -> Result<Value, ClientError> {
    let mut reply = client.post("/sessions/import", document).await?;
    Ok(reply["data"].take())
}

/// `document` in `format`; `generator` (`cyber <version>`) signs the HTML footer. JSON is
/// pretty-printed with a final newline.
pub fn render(document: &Value, format: Format, generator: &str) -> String {
    match format {
        Format::Html => html::render(document, generator),
        Format::Markdown => markdown(document),
        Format::Json => {
            let json = serde_json::to_string_pretty(document).unwrap_or_default();
            format!("{json}\n")
        }
    }
}

/// `[image: image/png, 1.2 MiB, shot.png]` or `[pdf: …]` for a stored `attachment` or
/// an inline `document` part, without its bytes; `None` for other parts.
fn placeholder(part: &Value) -> Option<String> {
    let media_type = part["media_type"].as_str().unwrap_or_default();
    let size = match part["type"].as_str()? {
        "attachment" => part["size"].as_u64().unwrap_or_default(),
        "document" => part["data"].as_str().map_or(0, |d| d.len() as u64 / 4 * 3),
        _ => return None,
    };
    let kind = if media_type == "application/pdf" {
        "pdf"
    } else {
        "image"
    };
    let mut fields = vec![media_type.to_owned(), size_label(size)];
    fields.extend(part["name"].as_str().map(str::to_owned));
    Some(format!("[{kind}: {}]", fields.join(", ")))
}

/// `512 B`, `3.4 KiB` or `1.2 MiB`.
fn size_label(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    match bytes {
        0..1024 => format!("{bytes} B"),
        1024..1_048_576 => format!("{:.1} KiB", bytes as f64 / KIB),
        _ => format!("{:.1} MiB", bytes as f64 / (KIB * KIB)),
    }
}

/// A Session ID as one URL path segment.
fn segment(value: &str) -> String {
    value
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn media_parts_become_placeholders_without_their_bytes() {
        let shot = json!({ "type": "attachment", "sha256": "ab", "media_type": "image/png",
            "size": 1_258_291, "name": "shot.png" });
        assert_eq!(
            placeholder(&shot).as_deref(),
            Some("[image: image/png, 1.2 MiB, shot.png]")
        );
        let pdf = json!({ "type": "attachment", "sha256": "cd", "media_type": "application/pdf",
            "size": 3_500 });
        assert_eq!(
            placeholder(&pdf).as_deref(),
            Some("[pdf: application/pdf, 3.4 KiB]")
        );
        let inline = json!({ "type": "document", "media_type": "application/pdf",
            "data": "JVBERi0x", "name": "[redacted:path:1]" });
        assert_eq!(
            placeholder(&inline).as_deref(),
            Some("[pdf: application/pdf, 6 B, [redacted:path:1]]")
        );
        assert_eq!(placeholder(&json!({ "type": "text", "text": "hi" })), None);
    }

    #[test]
    fn markdown_and_html_show_the_placeholders() {
        let doc = json!({
            "session": { "id": "ses_1", "title": "t" },
            "messages": [{ "kind": "user", "id": "m1", "source": "session", "parts": [
                { "type": "text", "text": "look" },
                { "type": "attachment", "sha256": "ab", "media_type": "application/pdf",
                    "size": 2048, "name": "spec.pdf" }
            ] }],
            "exported_ms": 0,
        });
        let wanted = "[pdf: application/pdf, 2.0 KiB, spec.pdf]";
        assert!(markdown(&doc).contains(wanted), "{}", markdown(&doc));
        let page = render(&doc, Format::Html, "cyber 9.9.9");
        assert!(
            page.contains(&format!("<p class=\"placeholder\">{wanted}</p>")),
            "{page}"
        );
    }
}
