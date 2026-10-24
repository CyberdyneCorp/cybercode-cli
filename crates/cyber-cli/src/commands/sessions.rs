//! `cyber sessions` (`cli-commands` → Resource command groups): list, show, rename,
//! delete and export the Location's Sessions through the background server, and their
//! branches (`tree`, `checkout`, `label`). A Session is named by
//! its ID, an exact title or a unique ID prefix; [`resolve`] is shared with
//! `cyber --resume` and `cyber exec --session`.

use clap::Subcommand;
use cyber_client::{Client, export};
use serde_json::{Value, json};

use crate::cli::GlobalArgs;
use crate::context::Context;
use crate::error::CliError;
use crate::output;

#[derive(Debug, Subcommand)]
pub enum SessionsCmd {
    /// The Location's Sessions, most recently updated first.
    #[command(alias = "ls")]
    List {
        /// At most this many (1 to 200).
        #[arg(long, default_value_t = 50, value_parser = clap::value_parser!(u16).range(1..=200))]
        limit: u16,
    },
    /// One Session: title, Location, model, status and usage.
    Show {
        /// ID, exact title or unique ID prefix.
        session: String,
    },
    /// Give a Session a title to find and resume it by later.
    Rename {
        /// ID, exact title or unique ID prefix.
        session: String,
        /// The new title; words are joined with spaces.
        #[arg(required = true, num_args = 1..)]
        title: Vec<String>,
    },
    /// Delete a Session with its child Sessions.
    #[command(alias = "rm")]
    Delete {
        /// ID, exact title or unique ID prefix.
        session: String,
        /// Confirm the deletion; it cannot be undone.
        #[arg(long)]
        yes: bool,
    },
    /// The Session's user-message tree: `*` marks the active path.
    Tree {
        /// ID, exact title or unique ID prefix.
        session: String,
    },
    /// Make another branch active; the Session must be idle.
    Checkout {
        /// ID, exact title or unique ID prefix.
        session: String,
        /// A message ID, a branch label or a unique message ID prefix; the newest tip
        /// under it becomes active.
        message: String,
        /// Also restore files to the branch's last snapshot, keeping your own edits.
        #[arg(long)]
        code: bool,
        /// Make the message itself the tip; the next prompt branches there.
        #[arg(long)]
        exact: bool,
    },
    /// Set or remove a branch label.
    Label {
        /// ID, exact title or unique ID prefix.
        session: String,
        /// The label: letters, digits, `.`, `_` and `-`, at most 48.
        name: String,
        /// The message to label (ID, label or unique prefix); the active tip by default.
        #[arg(long, conflicts_with = "delete")]
        at: Option<String>,
        /// Remove the label instead.
        #[arg(long)]
        delete: bool,
    },
    /// Write a Session as JSON (default), Markdown (--format md) or one self-contained
    /// HTML page (--format html): its info, messages, tool calls, goals and workflow runs.
    Export {
        /// ID, exact title or unique ID prefix.
        session: String,
        /// Replace prompts, answers, paths, URLs, tool inputs and outputs and metadata
        /// with [redacted:<kind>:<n>] markers, keeping structure, token counts and timings.
        #[arg(long)]
        sanitize: bool,
        /// Include child Sessions (subagents, workflow agents), recursively.
        #[arg(long)]
        include_children: bool,
        /// Include every branch; each message's `parent_message_id` links the tree.
        #[arg(long)]
        all_branches: bool,
        /// Write the export to this file, atomically, instead of stdout. A symlink is
        /// refused; an existing file keeps its permissions, a new one is private.
        #[arg(short, long, value_name = "FILE")]
        output: Option<std::path::PathBuf>,
    },
    /// Import a JSON export (`cyber sessions export`) as a new Session in this Location,
    /// with fresh IDs and its attachments, which are verified before anything is written.
    Import {
        /// The export file.
        file: std::path::PathBuf,
    },
    #[command(external_subcommand)]
    Other(Vec<String>),
}

const LATER: &[&str] = &["fork", "rewind", "repair-context", "share", "unshare"];

pub fn run(cmd: SessionsCmd, ctx: &Context, global: &GlobalArgs) -> Result<(), CliError> {
    if let SessionsCmd::Other(args) = &cmd {
        let name = args.first().map(String::as_str).unwrap_or_default();
        return Err(if LATER.contains(&name) {
            CliError::unavailable(&format!("`cyber sessions {name}`"), "P1 or later")
        } else {
            CliError::usage(format!("unknown sessions command \"{name}\""))
        });
    }
    let rt = super::serve::runtime()?;
    rt.block_on(async {
        let info = super::serve::start(ctx).await?;
        let client = Client::http(&info.registration.url, Some(info.password))
            .at(&ctx.location.display().to_string());
        dispatch(&client, cmd, global).await
    })
}

async fn dispatch(client: &Client, cmd: SessionsCmd, global: &GlobalArgs) -> Result<(), CliError> {
    match cmd {
        SessionsCmd::List { limit } => list(client, limit, global).await,
        SessionsCmd::Show { session } => {
            let found = resolve(client, &session).await?;
            show(&found, global)
        }
        SessionsCmd::Rename { session, title } => {
            let found = resolve(client, &session).await?;
            let id = found["id"].as_str().unwrap_or_default();
            let renamed = client
                .patch(
                    &format!("/sessions/{id}"),
                    json!({ "title": title.join(" ") }),
                )
                .await
                .map_err(api)?;
            show(&renamed["data"], global)
        }
        SessionsCmd::Delete { session, yes } => {
            let found = resolve(client, &session).await?;
            let id = found["id"].as_str().unwrap_or_default();
            if !yes {
                return Err(CliError::usage(format!(
                    "deleting {id} ({}) cannot be undone",
                    found["title"].as_str().unwrap_or_default()
                ))
                .with_hint("add --yes to delete it"));
            }
            client
                .delete(&format!("/sessions/{id}"))
                .await
                .map_err(api)?;
            println!("deleted {id}");
            Ok(())
        }
        SessionsCmd::Export {
            session,
            sanitize,
            include_children,
            all_branches,
            output,
        } => {
            let format = export_format(global)?;
            let id = session_id(client, &session).await?;
            let options = export::Options {
                all_branches,
                include_children,
                sanitize,
            };
            let document = export::document(client, &id, options).await.map_err(api)?;
            write_export(&document, format, output.as_deref())
        }
        SessionsCmd::Import { file } => import(client, &file, global).await,
        SessionsCmd::Other(_) => unreachable!("handled before connecting"),
        branch => dispatch_branch(client, branch, global).await,
    }
}

/// The branch commands: `tree`, `checkout` and `label`.
async fn dispatch_branch(
    client: &Client,
    cmd: SessionsCmd,
    global: &GlobalArgs,
) -> Result<(), CliError> {
    match cmd {
        SessionsCmd::Tree { session } => {
            let id = session_id(client, &session).await?;
            super::branches::tree(client, &id, global).await
        }
        SessionsCmd::Checkout {
            session,
            message,
            code,
            exact,
        } => {
            let id = session_id(client, &session).await?;
            super::branches::checkout(client, &id, &message, code, exact, global).await
        }
        SessionsCmd::Label {
            session,
            name,
            at,
            delete,
        } => {
            let id = session_id(client, &session).await?;
            super::branches::label(client, &id, &name, at.as_deref(), delete).await
        }
        _ => unreachable!("not a branch command"),
    }
}

async fn list(client: &Client, limit: u16, global: &GlobalArgs) -> Result<(), CliError> {
    let page = client
        .get(&format!("/sessions?limit={limit}"))
        .await
        .map_err(api)?;
    let mut rows = Vec::new();
    for s in page["data"]["data"].as_array().into_iter().flatten() {
        rows.push(json!({
            "id": s["id"], "title": s["title"], "directory": s["directory"],
            "updated_at": s["updated_at"], "status": status(client, s).await,
        }));
    }
    if output::is_json(global.format) {
        return output::json(&rows);
    }
    if rows.is_empty() {
        println!("no Sessions in this Location");
    }
    for row in &rows {
        println!(
            "{}  {:<8}  {}  {}",
            row["id"].as_str().unwrap_or_default(),
            row["status"].as_str().unwrap_or_default(),
            updated(&row["updated_at"]),
            clean(row["title"].as_str().unwrap_or_default()),
        );
    }
    Ok(())
}

/// `archived`, else the live `idle` or `running` the Session route reports (list rows
/// carry no run state).
async fn status(client: &Client, row: &Value) -> String {
    if row["archived"] == true {
        return "archived".into();
    }
    let id = row["id"].as_str().unwrap_or_default();
    match client.get(&format!("/sessions/{}", encode(id))).await {
        Ok(found) => found["data"]["status"]
            .as_str()
            .unwrap_or("idle")
            .to_owned(),
        Err(_) => "unknown".into(),
    }
}

fn show(session: &Value, global: &GlobalArgs) -> Result<(), CliError> {
    if output::is_json(global.format) {
        return output::json(session);
    }
    let text = |key: &str| clean(session[key].as_str().unwrap_or_default());
    output::rows(&[
        ("id", text("id")),
        ("title", text("title")),
        ("location", text("directory")),
        ("model", text("model")),
        ("agent", text("agent")),
        ("status", text("status")),
        ("updated", updated(&session["updated_at"])),
        ("resume", format!("cyber -r {}", text("id"))),
    ]);
    Ok(())
}

/// The Session named by `wanted`: its ID, then an exact title, then a unique ID prefix,
/// among the Location's Sessions. Several matching titles or prefixes are a usage error
/// that lists them, so a later command can use the ID.
pub async fn resolve(client: &Client, wanted: &str) -> Result<Value, CliError> {
    if !wanted.is_empty()
        && let Ok(found) = client.get(&format!("/sessions/{}", encode(wanted))).await
    {
        return Ok(found["data"].clone());
    }
    let page = client.get("/sessions?limit=200").await.map_err(api)?;
    let all = page["data"]["data"].as_array().cloned().unwrap_or_default();
    let titled: Vec<&Value> = all.iter().filter(|s| s["title"] == wanted).collect();
    let matches = if titled.is_empty() {
        all.iter()
            .filter(|s| {
                !wanted.is_empty() && s["id"].as_str().is_some_and(|id| id.starts_with(wanted))
            })
            .collect()
    } else {
        titled
    };
    match matches.as_slice() {
        [one] => Ok((*one).clone()),
        [] => Err(CliError::runtime(format!("Session not found: {wanted}"))
            .with_hint("run \"cyber sessions list\"")),
        several => {
            let ids: Vec<String> = several
                .iter()
                .map(|s| {
                    format!(
                        "{} ({})",
                        s["id"].as_str().unwrap_or_default(),
                        clean(s["title"].as_str().unwrap_or_default())
                    )
                })
                .collect();
            Err(CliError::usage(format!(
                "\"{wanted}\" matches {} Sessions: {}",
                several.len(),
                ids.join(", ")
            ))
            .with_hint("use the Session ID"))
        }
    }
}

/// The export format `--format` names: `json` (the default), `md` or `html`.
fn export_format(global: &GlobalArgs) -> Result<export::Format, CliError> {
    match global.format {
        None | Some(crate::cli::Format::Json) => Ok(export::Format::Json),
        Some(crate::cli::Format::Md) => Ok(export::Format::Markdown),
        Some(crate::cli::Format::Html) => Ok(export::Format::Html),
        Some(_) => Err(CliError::usage(
            "cyber sessions export writes --format json, md or html",
        )),
    }
}

/// The rendered export on stdout, or atomically in `output`.
fn write_export(
    document: &Value,
    format: export::Format,
    output: Option<&std::path::Path>,
) -> Result<(), CliError> {
    let generator = format!("cyber {}", cyber_core::version::build_info().version);
    let rendered = export::render(document, format, &generator);
    match output {
        Some(path) => export::write_atomic(path, rendered.as_bytes()).map_err(|e| {
            CliError::runtime(format!(
                "cannot write the export to {}: {e}",
                path.display()
            ))
        }),
        None => {
            print!("{rendered}");
            Ok(())
        }
    }
}

/// `cyber sessions import`: prints `Imported session: <id>`, or the Session as JSON.
async fn import(
    client: &Client,
    file: &std::path::Path,
    global: &GlobalArgs,
) -> Result<(), CliError> {
    let text = std::fs::read_to_string(file)
        .map_err(|e| CliError::runtime(format!("cannot read {}: {e}", file.display())))?;
    let document: Value = serde_json::from_str(&text).map_err(|e| {
        CliError::usage(format!(
            "{} is not a JSON Session export: {e}",
            file.display()
        ))
    })?;
    let imported = export::import(client, document).await.map_err(api)?;
    if output::is_json(global.format) {
        return output::json(&imported);
    }
    println!(
        "Imported session: {}",
        imported["id"].as_str().unwrap_or_default()
    );
    Ok(())
}

async fn session_id(client: &Client, wanted: &str) -> Result<String, CliError> {
    let found = resolve(client, wanted).await?;
    Ok(found["id"].as_str().unwrap_or_default().to_owned())
}

/// Titles come from models and users; keep them on one line without control characters.
pub(super) fn clean(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_whitespace() { ' ' } else { c })
        .filter(|c| !c.is_control())
        .collect()
}

/// How long ago `value` (milliseconds since the epoch) was: `42s`, `5m`, `3h`, `2d`.
fn updated(value: &Value) -> String {
    let Some(at) = value.as_i64() else {
        return String::new();
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64);
    let secs = (now - at).max(0) / 1000;
    let age = match secs {
        0..60 => format!("{secs}s"),
        60..3600 => format!("{}m", secs / 60),
        3600..86_400 => format!("{}h", secs / 3600),
        _ => format!("{}d", secs / 86_400),
    };
    format!("{age:>4} ago")
}

pub(super) fn encode(value: &str) -> String {
    value
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

pub(super) fn api(e: cyber_client::ClientError) -> CliError {
    match e.tag() {
        Some("InvalidRequestError") => CliError::usage(e.to_string()),
        _ => CliError::runtime(e.to_string()),
    }
}
