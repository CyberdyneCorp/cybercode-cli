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
