//! `cyber mcp serve` (`mcp-server` → MCP serve transports): Cyber Code as an MCP server
//! over the shared local server. `stdio` is a bridge that relays JSON-RPC frames between
//! stdin/stdout and `/api/v1/mcp/serve` and never opens a database; `http` prints the
//! endpoint of the running server.

mod bridge;
mod link;

use std::io::Write as _;
use std::path::Path;
use std::sync::Mutex;

use clap::{Args, ValueEnum};
use cyber_core::config::{
    McpServeSettings, derive_client_label as derive_label, valid_client_label as valid_label,
};
use serde_json::{Map, Value, json};

use crate::context::Context;
use crate::error::CliError;

/// The endpoint inside `/api/v1`.
const ENDPOINT: &str = "/mcp/serve";
/// JSON-RPC error for a request the bridge could not deliver or that the server refused
/// before dispatch.
const TRANSPORT_ERROR: i64 = -32010;

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum McpServeTransport {
    /// Relay MCP frames between stdin/stdout and the shared server.
    Stdio,
    /// Print the endpoint URL of the shared server for HTTP clients.
    Http,
}

#[derive(Debug, Args)]
pub struct McpServeArgs {
    #[arg(long, value_enum, default_value = "stdio")]
    pub transport: McpServeTransport,
    /// Attribution label sent as X-Cyber-MCP-Client (^[a-z0-9][a-z0-9_-]{0,47}$);
    /// defaults to the client's `clientInfo.name`.
    #[arg(long, value_name = "LABEL")]
    pub client: Option<String>,
}

pub fn run(args: McpServeArgs, ctx: &Context) -> Result<(), CliError> {
    if let Some(label) = &args.client
        && !valid_label(label)
    {
        return Err(CliError::usage(format!(
            "--client {label:?} must match ^[a-z0-9][a-z0-9_-]{{0,47}}$"
        )));
    }
    let settings = McpServeSettings::from_config(&ctx.global_config()?).map_err(CliError::usage)?;
    if !settings.enabled {
        return Err(CliError::runtime("MCP serve is disabled")
            .with_hint("set mcp_serve.enabled to true in the global config"));
    }
    warn_ignored(ctx);
    let rt = super::serve::runtime()?;
    let result = match args.transport {
        McpServeTransport::Http => rt.block_on(http(ctx)),
        McpServeTransport::Stdio => {
            let log = Log::open(&ctx.paths.data.join("log"));
            rt.block_on(bridge::run(ctx, args.client, log))
        }
    };
    // The stdin reader may still block on a read; it must not hold the process open.
    rt.shutdown_timeout(std::time::Duration::from_millis(200));
    result
}

/// `mcp_serve` outside the global config is ignored; say so on stderr.
fn warn_ignored(ctx: &Context) {
    let Ok(resolved) = ctx.config() else {
        return;
    };
    for warning in resolved.warnings.iter().filter(|w| w.contains("mcp_serve")) {
        eprintln!("warning: {warning}");
    }
}

/// `--transport http`: ensure the server runs and print where to reach it, never the
/// password.
async fn http(ctx: &Context) -> Result<(), CliError> {
    let info = super::serve::start(ctx).await?;
    let url = info.registration.url.trim_end_matches('/');
    if url.is_empty() {
        return Err(CliError::runtime(
            "the registered server has no TCP listener",
        ));
    }
    println!("{url}/api/v1{ENDPOINT}");
    println!(
        "Authenticate with HTTP Basic as user \"cyber\"; print the password with `cyber service password`."
    );
    Ok(())
}

/// Why a request got no answer from the server: JSON-RPC error -32010 with the
/// `server-api` error's `_tag` and `reason` (and receipt fields when present).
#[derive(Debug, Clone, PartialEq)]
struct Failure {
    message: String,
    data: Value,
}

impl Failure {
    /// The server could not be reached.
    fn unreachable() -> Self {
        Self {
            message: "the Cyber server is unreachable".into(),
            data: json!({ "_tag": "ServiceUnavailableError", "reason": "unreachable" }),
        }
    }

    /// An HTTP failure with a `server-api` error body.
    fn from_body(status: u16, body: &Value) -> Self {
        let mut data = Map::new();
        let tag = body["_tag"].as_str().unwrap_or("UnknownError");
        data.insert("_tag".into(), json!(tag));
        for key in ["reason", "session_id", "message_id"] {
            if let Some(value) = body.get(key).filter(|v| !v.is_null()) {
                data.insert(key.into(), value.clone());
            }
        }
        data.insert("httpStatus".into(), json!(status));
        let message = body["message"]
            .as_str()
            .filter(|m| !m.is_empty())
            .map_or_else(
                || format!("the Cyber server answered {status}"),
                str::to_owned,
            );
        Self {
            message,
            data: Value::Object(data),
        }
    }

    /// Add the receipt of a prompt admitted before the failure, so the client can
    /// re-attach with `cyber_prompt_wait` instead of admitting it again.
    fn with_receipt(mut self, receipt: Option<&Value>) -> Self {
        for key in ["session_id", "message_id"] {
            if let Some(value) = receipt.and_then(|r| r.get(key)).filter(|v| v.is_string()) {
                self.data[key] = value.clone();
            }
        }
        self
    }

    /// The JSON-RPC error response to request `id`.
    fn frame(&self, id: &Value) -> Value {
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": TRANSPORT_ERROR, "message": self.message, "data": self.data },
        })
    }
}

/// Bridge logs: JSON lines in `<data>/log/mcp-serve.log`, mirrored to stderr. Stdout
/// carries MCP frames only.
struct Log {
    file: Mutex<Option<std::fs::File>>,
}

impl Log {
    fn open(dir: &Path) -> Self {
        let _ = std::fs::create_dir_all(dir);
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("mcp-serve.log"))
            .ok();
        Self {
            file: Mutex::new(file),
        }
    }

    fn write(&self, level: &str, msg: &str, fields: Value) {
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis());
        let mut line = json!({ "ts": ts, "level": level, "msg": msg });
        if let (Some(line), Value::Object(fields)) = (line.as_object_mut(), fields) {
            line.extend(fields);
        }
        eprintln!("cyber mcp serve: {line}");
        let mut file = self
            .file
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(file) = file.as_mut() {
            let _ = writeln!(file, "{line}");
        }
    }

    fn info(&self, msg: &str, fields: Value) {
        self.write("info", msg, fields);
    }

    fn warn(&self, msg: &str, fields: Value) {
        self.write("warn", msg, fields);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_api_errors_map_to_transport_errors() {
        let body = json!({
            "_tag": "ForbiddenError",
            "message": "MCP serve accepts loopback clients only",
            "reason": "loopback_only",
            "ref": "err_1",
        });
        let failure = Failure::from_body(403, &body);
        let frame = failure.frame(&json!(7));
        assert_eq!(frame["id"], 7);
        assert_eq!(frame["error"]["code"], -32010);
        assert_eq!(
            frame["error"]["message"],
            "MCP serve accepts loopback clients only"
        );
        assert_eq!(
            frame["error"]["data"],
            json!({ "_tag": "ForbiddenError", "reason": "loopback_only", "httpStatus": 403 })
        );

        let bare = Failure::from_body(502, &json!("<html>"));
        assert_eq!(
            bare.data,
            json!({ "_tag": "UnknownError", "httpStatus": 502 })
        );
        assert_eq!(bare.message, "the Cyber server answered 502");
        let down = Failure::unreachable().frame(&json!("a"));
        assert_eq!(down["error"]["data"]["_tag"], "ServiceUnavailableError");
        assert_eq!(down["error"]["data"]["reason"], "unreachable");
        assert!(down["error"]["data"].get("httpStatus").is_none());
    }
}
