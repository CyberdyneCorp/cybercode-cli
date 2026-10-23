//! `/export [html|md|json] [path]` (`tui` → Export command): the current Session through
//! the renderers of `cyber sessions export`, to a file. The default name is
//! `<title-slug>-<short-id>.<ext>` in the Location; an existing file is replaced only
//! after the user confirms, and the written path is shown with a note that the file
//! holds the full conversation.

use std::path::{Path, PathBuf};

use crossterm::event::{KeyCode, KeyEvent};
use cyber_client::Client;
use cyber_client::export::{self, Format, Options};

use crate::app::{Action, App, Overlay};
use crate::perform::Msg;

/// The longest slug of a default file name.
const SLUG_MAX: usize = 48;

/// One export to write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub session_id: String,
    pub format: Format,
    pub path: PathBuf,
    /// Replace an existing file: set once the user confirmed it.
    pub overwrite: bool,
}

impl App {
    /// `/export`, `/export md`, `/export notes.md`, `/export json out/run.json`.
    pub(crate) fn export_command(&mut self, args: &str) -> Vec<Action> {
        let (format, path) = parse(args);
        let name =
            path.unwrap_or_else(|| default_name(&self.session.title, &self.session.id, format));
        vec![Action::Export(Request {
            session_id: self.session.id.clone(),
            format,
            path: Path::new(&self.session.directory).join(name),
            overwrite: false,
        })]
    }

    /// The target exists: ask before replacing it.
    pub(crate) fn export_exists(&mut self, request: Request) {
        self.overlay = Overlay::ConfirmExport(request);
    }

    /// y or Enter replaces the file; anything else writes nothing.
    pub(crate) fn confirm_export_key(&mut self, key: KeyEvent) -> Vec<Action> {
        let Overlay::ConfirmExport(request) = std::mem::replace(&mut self.overlay, Overlay::None)
        else {
            return Vec::new();
        };
        if matches!(key.code, KeyCode::Char('y') | KeyCode::Enter) {
            return vec![Action::Export(Request {
                overwrite: true,
                ..request
            })];
        }
        self.toast(format!(
            "Not exported: {} is unchanged",
            request.path.display()
        ));
        Vec::new()
    }
}

/// The format and the path of `/export` arguments. A leading `html`, `md` or `json` names
/// the format; otherwise a path's extension does, and HTML is the default.
fn parse(args: &str) -> (Format, Option<String>) {
    let args = args.trim();
    let (first, rest) = args.split_once(' ').unwrap_or((args, ""));
    if let Some(format) = named(first) {
        let rest = rest.trim();
        return (format, (!rest.is_empty()).then(|| rest.to_owned()));
    }
    if args.is_empty() {
        return (Format::Html, None);
    }
    let format = Path::new(args)
        .extension()
        .and_then(|e| e.to_str())
        .and_then(named)
        .unwrap_or(Format::Html);
    (format, Some(args.to_owned()))
}

fn named(name: &str) -> Option<Format> {
    [Format::Html, Format::Markdown, Format::Json]
        .into_iter()
        .find(|f| f.extension() == name)
}

/// `<title-slug>-<short-id>.<ext>`: the slug is lowercase ASCII with every run of other
/// characters turned into `-`, at most [`SLUG_MAX`] characters; the short ID is the first
/// eight characters after the `ses_` prefix, lowercased.
pub fn default_name(title: &str, id: &str, format: Format) -> String {
    let short: String = id
        .strip_prefix("ses_")
        .unwrap_or(id)
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(8)
        .collect::<String>()
        .to_ascii_lowercase();
    let slug = slug(title);
    let stem = match (slug.is_empty(), short.is_empty()) {
        (true, true) => "session".to_owned(),
        (true, false) => format!("session-{short}"),
        (false, true) => slug,
        (false, false) => format!("{slug}-{short}"),
    };
    format!("{stem}.{}", format.extension())
}

fn slug(title: &str) -> String {
    let mut slug = String::new();
    for c in title.chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c.to_ascii_lowercase());
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    slug.truncate(SLUG_MAX);
    slug.trim_end_matches('-').to_owned()
}

/// Write the export, or report that the target exists and has not been confirmed.
pub async fn perform(client: &Client, request: Request) -> Result<Msg, String> {
    if !request.overwrite && std::fs::symlink_metadata(&request.path).is_ok() {
        return Ok(Msg::ExportExists(request));
    }
    let document = export::document(client, &request.session_id, Options::default())
        .await
        .map_err(|e| format!("export: {e}"))?;
    let generator = format!("cyber {}", cyber_core::version::build_info().version);
    let rendered = export::render(&document, request.format, &generator);
    export::write_atomic(&request.path, rendered.as_bytes())
        .map_err(|e| format!("export: cannot write {}: {e}", request.path.display()))?;
    Ok(Msg::Toast(format!(
        "Exported to {} · it contains the full conversation",
        request.path.display()
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_names_follow_the_slug_rules() {
        assert_eq!(
            default_name("Auth refactor", "ses_01M58P862KXYZ", Format::Html),
            "auth-refactor-01m58p86.html"
        );
        assert_eq!(
            default_name("  Fix: «login» flow!! ", "ses_01ABCDEFGH", Format::Markdown),
            "fix-login-flow-01abcdef.md"
        );
        assert_eq!(
            default_name("Ünïcode", "ses_1", Format::Json),
            "n-code-1.json"
        );
        assert_eq!(
            default_name("", "ses_01ABCDEFGH", Format::Html),
            "session-01abcdef.html"
        );
        let long = default_name(&"word ".repeat(20), "ses_01ABCDEFGH", Format::Html);
        assert_eq!(long, format!("{}wor-01abcdef.html", "word-".repeat(9)));
        assert!(slug(&"a".repeat(60)).len() == SLUG_MAX);
    }

    #[test]
    fn arguments_name_the_format_and_the_path() {
        assert_eq!(parse(""), (Format::Html, None));
        assert_eq!(parse("md"), (Format::Markdown, None));
        assert_eq!(
            parse("json out/run file.json"),
            (Format::Json, Some("out/run file.json".into()))
        );
        assert_eq!(
            parse("notes.md"),
            (Format::Markdown, Some("notes.md".into()))
        );
        assert_eq!(parse("report"), (Format::Html, Some("report".into())));
    }
}
