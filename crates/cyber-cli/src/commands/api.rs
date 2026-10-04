//! `cyber api` (`server-api` → Raw API command).

use clap::Args;
use serde_json::Value;

use crate::context::Context;
use crate::error::CliError;

#[derive(Debug, Args)]
pub struct ApiArgs {
    /// An operation ID (`v1.session.list`) or `METHOD /path`.
    #[arg(required = true, num_args = 1..=2)]
    pub target: Vec<String>,
    /// Request body (JSON by default).
    #[arg(short = 'd', long)]
    pub data: Option<String>,
    /// Extra header as `Name: value` (repeatable, up to 100).
    #[arg(short = 'H', long = "header")]
    pub headers: Vec<String>,
    /// Path or query parameter as `key=value` (repeatable).
    #[arg(long = "param")]
    pub params: Vec<String>,
}

pub fn run(args: ApiArgs, ctx: &Context) -> Result<(), CliError> {
    if args.headers.len() > 100 {
        return Err(CliError::usage("at most 100 headers"));
    }
    let (method, path) = target(&args.target)?;
    let path = fill(&path, &args.params)?;
    let body = args
        .data
        .as_ref()
        .map(|d| serde_json::from_str::<Value>(d).unwrap_or_else(|_| Value::String(d.clone())));
    let headers = args
        .headers
        .iter()
        .map(|h| {
            h.split_once(':')
                .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
                .ok_or_else(|| CliError::usage(format!("bad header {h:?}; use 'Name: value'")))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let rt = super::serve::runtime()?;
    let (status, value) = rt.block_on(async {
        let info = super::serve::start(ctx).await?;
        let client = cyber_client::Client::http(&info.registration.url, Some(info.password))
            .at(&ctx.location.display().to_string());
        client
            .raw(&method, &path, body, &headers)
            .await
            .map_err(|e| CliError::runtime(e.to_string()))
    })?;
    match &value {
        Value::String(s) => println!("{s}"),
        Value::Null => {}
        other => println!(
            "{}",
            serde_json::to_string_pretty(other).unwrap_or_default()
        ),
    }
    if (200..300).contains(&status) {
        Ok(())
    } else {
        Err(CliError::silent(1))
    }
}

fn target(parts: &[String]) -> Result<(String, String), CliError> {
    match parts {
        [method, path] => Ok((
            method.to_uppercase(),
            path.trim_start_matches("/api/v1").to_string(),
        )),
        [id] if id.starts_with("v1.") => {
            let (m, p) = cyber_server::http::openapi::lookup(id)
                .ok_or_else(|| CliError::usage(format!("unknown operation {id}")))?;
            Ok((m.to_uppercase(), p.to_string()))
        }
        [single] => Err(CliError::usage(format!(
            "expected an operation ID or METHOD /path, got {single:?}"
        ))),
        _ => Err(CliError::usage("expected an operation ID or METHOD /path")),
    }
}

/// Substitute `{name}` path parameters; the rest become query parameters.
fn fill(path: &str, params: &[String]) -> Result<String, CliError> {
    let mut path = path.to_string();
    let mut query = Vec::new();
    for p in params {
        let (k, v) = p
            .split_once('=')
            .ok_or_else(|| CliError::usage(format!("bad --param {p:?}; use key=value")))?;
        let placeholder = format!("{{{k}}}");
        if path.contains(&placeholder) {
            path = path.replace(&placeholder, v);
        } else {
            query.push(format!("{k}={v}"));
        }
    }
    if path.contains('{') {
        return Err(CliError::usage(format!(
            "missing path parameter in {path}; pass --param name=value"
        )));
    }
    if !query.is_empty() {
        path = format!(
            "{path}{}{}",
            if path.contains('?') { "&" } else { "?" },
            query.join("&")
        );
    }
    Ok(path)
}
