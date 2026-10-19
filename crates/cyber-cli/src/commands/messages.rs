//! `cyber messages` (`cross-session-messaging` → Messaging CLI): send a message to a
//! Session and read message histories and the reachable listing through the Messaging
//! API of the background server, so the CLI shares the pipeline (limits, addressing,
//! inbound rules) of every other surface.

use std::io::{IsTerminal, Read};

use clap::{Args, Subcommand, ValueEnum};
use cyber_client::Client;
use serde_json::{Value, json};

use crate::cli::GlobalArgs;
use crate::context::Context;
use crate::error::CliError;
use crate::output;

/// Exit code of an address that needs a Cyber Account (`AccountRequiredError`, the Relay).
pub const EXIT_ACCOUNT_REQUIRED: u8 = 6;
/// The client label the CLI sends as (`client:cli`).
const CLIENT: &str = "cli";
/// The longest first line of a message the history table shows.
const PREVIEW: usize = 60;

#[derive(Debug, Subcommand)]
pub enum MessagesCmd {
    /// Send a message to a Session by ID or name. Exits 0 when delivered or held, 1 when
    /// refused (the reason on stderr) or when a peer cannot be reached, 2 for an
    /// ambiguous or unknown address, an unconfigured peer or a text over 16 KiB, and 6
    /// for an address that needs a Cyber Account.
    #[command(after_help = SEND_EXAMPLES)]
    Send(SendArgs),
    /// The message history of a Session (default: the Location's most recent one), or with
    /// `--reachable` the Sessions a message can be sent to.
    #[command(alias = "ls", after_help = LIST_EXAMPLES)]
    List(ListArgs),
}

const SEND_EXAMPLES: &str = "Examples:
  cyber messages send frontend \"the API moved to port 8080\"
  echo \"deploy is done\" | cyber messages send migration -
  cyber messages send ses_01J... \"rebase on main\" --from backend --notify-when-idle
  cyber messages send migration@desk \"is the migration done?\" --from review";

const LIST_EXAMPLES: &str = "Examples:
  cyber messages list --session frontend --direction in
  cyber messages list --reachable --from backend --format json";

#[derive(Debug, Args)]
pub struct SendArgs {
    /// The target: a `ses_` ID, a subagent name, a Session name, or `<session>@<peer>` for
    /// a Session of a paired machine (which needs `--from`).
    pub to: String,
    /// The message text; `-` (or nothing, with a pipe) reads it from stdin.
    pub text: Option<String>,
    /// Send as this Session (ID, exact title or unique ID prefix) instead of as the
    /// `client:cli` client; a message to a peer Session (`<session>@<peer>`) needs it.
    #[arg(long)]
    pub from: Option<String>,
    /// Also notify the `--from` Session once when the target next goes idle.
    #[arg(long, requires = "from")]
    pub notify_when_idle: bool,
}

#[derive(Debug, Args)]
pub struct ListArgs {
    /// The Session whose history to show (ID, exact title or unique ID prefix).
    #[arg(long, conflicts_with = "reachable")]
    pub session: Option<String>,
    /// Only received (`in`) or sent (`out`) messages.
    #[arg(long, value_enum, conflicts_with = "reachable")]
    pub direction: Option<DirectionArg>,
    /// At most this many records, newest first (1 to 200).
    #[arg(long, default_value_t = 50, value_parser = clap::value_parser!(u16).range(1..=200),
        conflicts_with = "reachable")]
    pub limit: u16,
    /// List the Sessions a message can be sent to instead of a history.
    #[arg(long)]
    pub reachable: bool,
    /// With `--reachable`: list as this Session (ID, exact title or unique ID prefix)
    /// instead of as the `client:cli` client.
    #[arg(long, requires = "reachable")]
    pub from: Option<String>,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum DirectionArg {
    In,
    Out,
}

impl DirectionArg {
    fn as_str(self) -> &'static str {
        match self {
            Self::In => "in",
            Self::Out => "out",
        }
    }
}

pub fn run(cmd: MessagesCmd, ctx: &Context, global: &GlobalArgs) -> Result<(), CliError> {
    // Read stdin before starting the server, so a missing text fails fast.
    let text = match &cmd {
        MessagesCmd::Send(args) => Some(text(args.text.as_deref())?),
        MessagesCmd::List(_) => None,
    };
    let rt = super::serve::runtime()?;
    rt.block_on(async {
        let info = super::serve::start(ctx).await?;
        let client = Client::http(&info.registration.url, Some(info.password))
            .at(&ctx.location.display().to_string());
        match cmd {
            MessagesCmd::Send(args) => send(&client, &args, text.unwrap_or_default(), global).await,
            MessagesCmd::List(args) if args.reachable => reachable(&client, &args, global).await,
            MessagesCmd::List(args) => history(&client, &args, global).await,
        }
    })
}

/// The text argument, or stdin for `-` or a piped stdin.
fn text(arg: Option<&str>) -> Result<String, CliError> {
    match arg {
        Some(text) if text != "-" => return Ok(text.to_owned()),
        None if std::io::stdin().is_terminal() => {
            return Err(CliError::usage(
                "no message text: pass it as an argument or `-` for stdin",
            )
            .with_hint("echo \"deploy is done\" | cyber messages send migration -"));
        }
        _ => {}
    }
    let mut text = String::new();
    std::io::stdin().lock().read_to_string(&mut text)?;
    Ok(text.trim_end_matches(['\r', '\n']).to_owned())
}

async fn send(
    client: &Client,
    args: &SendArgs,
    text: String,
    global: &GlobalArgs,
) -> Result<(), CliError> {
    let from = session_id(client, args.from.as_deref()).await?;
    // One ID for this command, so a lost response replays instead of sending twice.
    let id = cyber_core::ids::new_id("msgx");
    let body = json!({ "id": id, "from": from, "to": args.to, "text": text,
        "notify_when_idle": args.notify_when_idle });
    let value = call(client, "POST", "/messaging/send", Some(body)).await?;
    report(&value["data"], global)
}

/// The ID of the Session `wanted` names, resolved like `cyber sessions` does.
async fn session_id(client: &Client, wanted: Option<&str>) -> Result<Option<String>, CliError> {
    let Some(wanted) = wanted else {
        return Ok(None);
    };
    let session = super::sessions::resolve(client, wanted).await?;
    Ok(session["id"].as_str().map(str::to_owned))
}

/// One Messaging API request as the `client:cli` client; a declared failure becomes its
/// exit code.
async fn call(
    client: &Client,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Result<Value, CliError> {
    let headers = [("X-Cyber-Client".to_owned(), CLIENT.to_owned())];
    let (status, value) = client
        .raw(method, path, body, &headers)
        .await
        .map_err(|e| CliError::runtime(e.to_string()))?;
    if !(200..300).contains(&status) {
        return Err(failure(&value));
    }
    Ok(value)
}

/// `cyber messages list`: one page of a Session's history, newest first.
async fn history(client: &Client, args: &ListArgs, global: &GlobalArgs) -> Result<(), CliError> {
    let session = match args.session.as_deref() {
        Some(wanted) => session_id(client, Some(wanted)).await?.unwrap_or_default(),
        None => latest(client).await?,
    };
    let mut path = format!(
        "/sessions/{}/messaging?limit={}",
        query(&session),
        args.limit
    );
    if let Some(direction) = args.direction {
        path.push_str(&format!("&direction={}", direction.as_str()));
    }
    let page = call(client, "GET", &path, None).await?;
    let records = page["data"].as_array().cloned().unwrap_or_default();
    if output::is_json(global.format) {
        return output::json(&records);
    }
    if records.is_empty() {
        println!("no messages in {session}");
    }
    for record in &records {
        println!("{}", history_row(record));
    }
    Ok(())
}

/// The most recently updated Session of the Location.
async fn latest(client: &Client) -> Result<String, CliError> {
    let page = client
        .get("/sessions?limit=1")
        .await
        .map_err(|e| CliError::runtime(e.to_string()))?;
    page["data"]["data"][0]["id"]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| {
            CliError::runtime("no Sessions in this Location").with_hint("name one with --session")
        })
}

/// time, direction, peer, outcome, size and the first line of the text.
fn history_row(record: &Value) -> String {
    let field = |key: &str| clean(record[key].as_str().unwrap_or_default());
    let peer = if record["direction"] == "in" {
        field("from")
    } else {
        field("to")
    };
    let outcome = match record["reason"].as_str() {
        Some(reason) => format!("{} ({})", field("outcome"), clean(reason)),
        None => field("outcome"),
    };
    let time = field("created_at").replacen('T', " ", 1);
    format!(
        "{}  {:<3}  {:<28}  {:<24}  {:>6}  {}",
        time.get(..19).unwrap_or(&time),
        field("direction"),
        peer,
        outcome,
        record["size"].as_u64().unwrap_or_default(),
        first_line(record["text"].as_str().unwrap_or_default()),
    )
}

/// `cyber messages list --reachable`: the Sessions the caller can message.
async fn reachable(client: &Client, args: &ListArgs, global: &GlobalArgs) -> Result<(), CliError> {
    let path = match session_id(client, args.from.as_deref()).await? {
        Some(from) => format!("/messaging/sessions?from={}", query(&from)),
        None => "/messaging/sessions".to_owned(),
    };
    let listing = call(client, "GET", &path, None).await?;
    let sessions = listing["data"].as_array().cloned().unwrap_or_default();
    if output::is_json(global.format) {
        return output::json(&sessions);
    }
    if sessions.is_empty() {
        println!("no reachable Sessions");
    }
    for s in &sessions {
        let field = |key: &str| clean(s[key].as_str().unwrap_or_default());
        println!(
            "{:<28}  {}  {:<5}  {:<13}  {:<7}  {}",
            field("address"),
            field("session_id"),
            field("kind"),
            field("state"),
            if s["canReceive"] == true {
                "open"
            } else {
                "closed"
            },
            clean(s["location"]["directory"].as_str().unwrap_or_default()),
        );
    }
    Ok(())
}

/// The first line of `text`, cut to [`PREVIEW`] characters with `…`.
fn first_line(text: &str) -> String {
    let line = clean(text.lines().next().unwrap_or_default());
    if line.chars().count() <= PREVIEW {
        return line;
    }
    let cut: String = line.chars().take(PREVIEW - 1).collect();
    format!("{cut}…")
}

/// Text from other Sessions and models; keep it on one line without control characters.
fn clean(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_whitespace() { ' ' } else { c })
        .filter(|c| !c.is_control())
        .collect()
}

/// Percent-encode a query or path value.
pub(crate) fn query(value: &str) -> String {
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

/// Print the outcome; a refusal exits 1 with its reason on stderr.
fn report(result: &Value, global: &GlobalArgs) -> Result<(), CliError> {
    let refused = result["outcome"] == "refused";
    if output::is_json(global.format) {
        output::json(result)?;
    } else if !refused {
        let to = &result["to"];
        println!(
            "{} {} to {} ({})",
            result["message_id"].as_str().unwrap_or_default(),
            result["outcome"].as_str().unwrap_or_default(),
            to["address"].as_str().unwrap_or_default(),
            to["session_id"].as_str().unwrap_or_default(),
        );
        if let Some(watch) = result["watch_id"].as_str() {
            println!("{watch} notifies the sender when the target goes idle");
        }
    }
    if !refused {
        return Ok(());
    }
    let reason = result["reason"].as_str().unwrap_or("refused");
    Err(CliError::runtime(format!(
        "{} was refused: {reason}",
        result["message_id"].as_str().unwrap_or("the message")
    )))
}

/// A declared failure as an exit code: usage errors (2) for what the caller can fix
/// (an unconfigured peer included), 6 for an address that needs an account, 1 otherwise.
fn failure(body: &Value) -> CliError {
    let tag = body["_tag"].as_str().unwrap_or_default();
    let message = body["message"]
        .as_str()
        .unwrap_or("the server refused the message");
    match tag {
        "AmbiguousAddressError" => {
            let candidates: Vec<String> = body["candidates"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|c| {
                    format!(
                        "  {}  {}  {}",
                        c["session_id"].as_str().unwrap_or_default(),
                        c["name"].as_str().unwrap_or_default(),
                        c["location"]["directory"].as_str().unwrap_or_default()
                    )
                })
                .collect();
            CliError::usage(format!("{message}:\n{}", candidates.join("\n")))
                .with_hint("send to one of the Session IDs")
        }
        "AccountRequiredError" => CliError {
            code: EXIT_ACCOUNT_REQUIRED,
            message: message.to_owned(),
            hint: None,
        },
        "PeerNotConfiguredError" => CliError::usage(message).with_hint(format!(
            "pair it with `cyber peers add {} '<code>'`",
            body["peer"].as_str().unwrap_or("<name>")
        )),
        "MessageTooLargeError" | "AddressNotFoundError" | "InvalidRequestError" => {
            CliError::usage(message)
        }
        _ => CliError::runtime(super::tagged(tag, message)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failures_map_to_the_messaging_exit_codes() {
        let ambiguous = failure(&json!({"_tag": "AmbiguousAddressError",
            "message": "`migration` matches 2 Sessions",
            "candidates": [{"session_id": "ses_a", "name": "migration", "location": {"directory": "/a"}},
                {"session_id": "ses_b", "name": "migration", "location": {"directory": "/b"}}]}));
        assert_eq!(ambiguous.code, 2);
        assert!(ambiguous.message.contains("ses_a") && ambiguous.message.contains("ses_b"));
        for (tag, code) in [
            ("MessageTooLargeError", 2),
            ("AddressNotFoundError", 2),
            ("InvalidRequestError", 2),
            ("AccountRequiredError", 6),
            ("PeerNotConfiguredError", 2),
            ("PeerUnreachableError", 1),
            ("PeerAuthError", 1),
            ("ServiceUnavailableError", 1),
        ] {
            assert_eq!(
                failure(&json!({"_tag": tag, "message": "x"})).code,
                code,
                "{tag}"
            );
        }
    }

    #[test]
    fn history_rows_show_the_peer_outcome_and_first_line() {
        let received = history_row(&json!({"direction": "in", "from": "client:cli",
            "to": "ses_b", "outcome": "delivered", "size": 22,
            "created_at": "2026-10-19T10:02:03.456Z", "text": "the API moved\nsecond line"}));
        assert!(
            received.starts_with("2026-10-19 10:02:03  in   client:cli"),
            "{received}"
        );
        assert!(received.contains("delivered") && received.ends_with("    22  the API moved"));
        let refused = history_row(
            &json!({"direction": "out", "from": "ses_a", "to": "nightly",
            "outcome": "refused", "reason": "non_interactive", "size": 5,
            "created_at": "2026-10-19T10:02:03.456Z"}),
        );
        assert!(refused.contains("out  nightly") && refused.contains("refused (non_interactive)"));
        assert!(refused.ends_with("     5  "), "{refused:?}");
    }

    #[test]
    fn previews_are_one_clean_line_cut_with_an_ellipsis() {
        assert_eq!(first_line("a\tb\u{7}c\nnext"), "a bc");
        let long = first_line(&"é".repeat(100));
        assert_eq!(long.chars().count(), PREVIEW);
        assert!(long.ends_with('…'));
        assert_eq!(query("a b/é"), "a%20b%2F%C3%A9");
    }
}
