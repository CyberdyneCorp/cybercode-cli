//! `cyber telemetry show` (`cli-commands` → Telemetry status command): the background
//! server's OpenTelemetry export status from `GET /api/v1/telemetry`. The values are the
//! server's own, because the server's environment is the one that exports. Header values
//! and Cyber Account identifiers are never printed.

use clap::Subcommand;
use cyber_server::http::telemetry::{SignalStatus, TelemetryStatus, mask};

use crate::cli::GlobalArgs;
use crate::context::Context;
use crate::error::CliError;
use crate::output;

#[derive(Debug, Subcommand)]
pub enum TelemetryCmd {
    /// Export settings per signal with their sources, counters, ignored project values
    /// and whether a restart is required. Exits 0 whether or not export is on.
    Show,
}

pub fn run(cmd: TelemetryCmd, ctx: &Context, global: &GlobalArgs) -> Result<(), CliError> {
    match cmd {
        TelemetryCmd::Show => show(ctx, global),
    }
}

fn show(ctx: &Context, global: &GlobalArgs) -> Result<(), CliError> {
    let rt = super::serve::runtime()?;
    let value = rt.block_on(async {
        let info = super::serve::start(ctx).await?;
        cyber_client::Client::http(&info.registration.url, Some(info.password))
            .at(&ctx.location.display().to_string())
            .get("/telemetry")
            .await
            .map_err(|e| CliError::runtime(e.to_string()))
    })?;
    let mut status: TelemetryStatus = serde_json::from_value(value["data"].clone())
        .map_err(|e| CliError::runtime(format!("the server sent no telemetry status: {e}")))?;
    mask(&mut status);
    if output::is_json(global.format) {
        return output::json(&status);
    }
    print_text(&status);
    Ok(())
}

fn print_text(status: &TelemetryStatus) {
    match &status.reason {
        Some(reason) if !status.enabled => println!("OpenTelemetry export: off ({reason})"),
        _ if status.enabled => println!("OpenTelemetry export: on"),
        _ => println!("OpenTelemetry export: off"),
    }
    println!(
        "log_prompts: {} ({})",
        status.log_prompts.value, status.log_prompts.source
    );
    if status.restart_required {
        println!("restart required: the configuration changed since the server started");
    }
    println!(
        "product telemetry: {}",
        if status.product.enabled { "on" } else { "off" }
    );
    for (name, signal) in [
        ("traces", &status.signals.traces),
        ("metrics", &status.signals.metrics),
        ("logs", &status.signals.logs),
    ] {
        print_signal(name, signal);
    }
    println!("dropped events: {}", status.dropped_events);
    let resource = &status.resource;
    println!(
        "resource: service.name={} service.version={} os.type={} account signed in: {}",
        resource.service_name,
        resource.service_version,
        resource.os_type,
        if resource.account_signed_in {
            "yes"
        } else {
            "no"
        }
    );
    if !status.ignored.is_empty() {
        println!("ignored (project telemetry settings are never read):");
        for ignored in &status.ignored {
            println!("  {}  {}", ignored.pointer, ignored.source);
        }
    }
}

fn print_signal(name: &str, signal: &SignalStatus) {
    let state = match &signal.reason {
        Some(reason) => format!("off ({reason})"),
        None if signal.enabled => "on".into(),
        None => "off".into(),
    };
    println!("{name}: {state}");
    let headers = if signal.headers.is_empty() {
        "(none)".to_string()
    } else {
        let pairs: Vec<String> = signal
            .headers
            .iter()
            .map(|h| format!("{}={}", h.name, h.value))
            .collect();
        pairs.join(", ")
    };
    let sources = &signal.sources;
    let timeout = format!("{} ms", signal.timeout_ms);
    for (key, value, source) in [
        ("endpoint", signal.endpoint.as_str(), &sources.endpoint),
        ("protocol", signal.protocol.as_str(), &sources.protocol),
        ("headers", headers.as_str(), &sources.headers),
        ("timeout", timeout.as_str(), &sources.timeout),
    ] {
        println!("  {key:<9}{value}  [{source}]");
    }
    println!(
        "  exported {}, failed {}, dropped {}",
        signal.exported, signal.failed, signal.dropped
    );
    if let Some(error) = &signal.last_error {
        println!("  last error: {} (at {} ms)", error.message, error.at_ms);
    }
}
