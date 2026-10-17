//! `cyber sessions` (`cli-commands` → Resource command groups): list, show, rename and
//! delete the Location's Sessions through the background server. A Session is named by
//! its ID, an exact title or a unique ID prefix; [`resolve`] is shared with
//! `cyber --resume` and `cyber exec --session`.

use clap::Subcommand;
use cyber_client::Client;
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
    #[command(external_subcommand)]
    Other(Vec<String>),
}

const LATER: &[&str] = &[
    "export",
    "import",
    "fork",
    "rewind",
    "repair-context",
    "share",
    "unshare",
];

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
        SessionsCmd::Other(_) => unreachable!("handled before connecting"),
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

/// Titles come from models and users; keep them on one line without control characters.
fn clean(text: &str) -> String {
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

fn encode(value: &str) -> String {
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

fn api(e: cyber_client::ClientError) -> CliError {
    match e.tag() {
        Some("InvalidRequestError") => CliError::usage(e.to_string()),
        _ => CliError::runtime(e.to_string()),
    }
}
