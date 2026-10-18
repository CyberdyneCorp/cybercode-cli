//! The Status tab (`tui` → Settings Status tab): versions, the Session, its model and
//! context, git and the MCP and language servers, one `label: value` row each. A
//! source that is not reported is left out; one that fails says so.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use serde_json::Value;

use crate::app::App;
use crate::model::{ContextMeter, Session};
use crate::sidebar::{Git, LspServer, Machine, McpServer, clean};
use crate::view::footer::compact;

/// The width of the context meter.
const METER: usize = 30;
/// Labels are padded to this width.
const LABEL: usize = 15;

/// A source's answer.
#[derive(Debug, Clone, PartialEq)]
pub enum Section<T> {
    Ready(T),
    /// 404 or 503: not reported, so not shown.
    Missing,
    Failed(String),
}

/// What `/health` reports.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Health {
    pub version: Option<String>,
    /// The server binary's build identity; `None` from servers that predate it.
    pub build: Option<String>,
    pub sandbox: Option<String>,
    pub account: Option<String>,
}

impl Health {
    pub fn parse(v: &Value) -> Self {
        let text = |v: &Value| {
            v.as_str()
                .or_else(|| v["status"].as_str())
                .map(clean)
                .filter(|s| !s.is_empty())
        };
        Self {
            version: text(&v["version"]),
            build: text(&v["build"]),
            sandbox: text(&v["sandbox"]).or_else(|| sandbox_summary(&v["sandbox"])),
            account: text(&v["account"]),
        }
    }
}

/// `/health`'s `sandbox` object as one row: `workspace-write · available · isolation:
/// sandbox` (`sandbox` → Container detection: `isolation: container`).
fn sandbox_summary(v: &Value) -> Option<String> {
    let policy = v["policy"].as_str()?;
    let mut parts = vec![clean(policy)];
    if let Some(enforcement) = v["enforcement"].as_str() {
        parts.push(clean(enforcement));
    }
    if let Some(isolation) = v["isolation"].as_str() {
        parts.push(format!("isolation: {}", clean(isolation)));
    }
    Some(parts.join(" · "))
}

#[derive(Debug, Clone, PartialEq)]
pub struct Status {
    pub health: Section<Health>,
    pub session: Section<Session>,
    /// `None` inside: outside git.
    pub vcs: Section<Option<Git>>,
    pub mcp: Section<Vec<McpServer>>,
    pub lsp: Section<Vec<LspServer>>,
    /// `GET /system`: the server machine, boxed to keep the message small.
    pub machine: Section<Box<Machine>>,
}

/// The tab's rows. `width` is the text width.
pub fn lines(app: &App, width: usize) -> Vec<Line<'static>> {
    let load = &app.settings.status;
    let mut out = Vec::new();
    let Some(status) = &load.data else {
        out.push(muted(app, loading_text(load.error.as_deref())));
        return out;
    };
    let rows = Rows { app, width };
    rows.versions(status, &mut out);
    out.push(Line::default());
    rows.session(status, &mut out);
    out.push(Line::default());
    rows.model(status, &mut out);
    out.push(Line::default());
    rows.servers(status, &mut out);
    if let Some(error) = &load.error {
        out.push(Line::default());
        out.push(failed(app, "status", error));
    }
    out
}

fn loading_text(error: Option<&str>) -> String {
    match error {
        Some(error) => format!("Could not load status: {error} · r to retry"),
        None => "Loading…".into(),
    }
}

fn muted(app: &App, text: String) -> Line<'static> {
    Line::from(Span::styled(
        text,
        Style::default().fg(app.theme.text_muted),
    ))
}

/// `Could not load <section>: <error> · r to retry`.
pub fn failed(app: &App, section: &str, error: &str) -> Line<'static> {
    Line::from(Span::styled(
        format!("Could not load {section}: {} · r to retry", clean(error)),
        Style::default().fg(app.theme.error),
    ))
}

struct Rows<'a> {
    app: &'a App,
    width: usize,
}

impl Rows<'_> {
    /// `label:` padded, then the value spans, cut to the width.
    fn row(&self, label: &str, value: Vec<Span<'static>>) -> Line<'static> {
        let t = &self.app.theme;
        let mut spans = vec![Span::styled(
            format!("{:<LABEL$}", format!("{label}:")),
            Style::default().fg(t.text_muted),
        )];
        let mut room = self.width.saturating_sub(LABEL);
        for span in value {
            let text = crate::view::layout::truncate_end(&span.content, room);
            room = room.saturating_sub(unicode_width::UnicodeWidthStr::width(text.as_str()));
            spans.push(Span::styled(text, span.style));
        }
        Line::from(spans)
    }

    fn text(&self, label: &str, value: String) -> Line<'static> {
        let style = Style::default().fg(self.app.theme.text);
        self.row(label, vec![Span::styled(value, style)])
    }

    /// `Client` and `Server` versions; both in `warning` when the versions or the builds
    /// differ.
    fn versions(&self, status: &Status, out: &mut Vec<Line<'static>>) {
        let t = &self.app.theme;
        let client = self.app.client_version.clone();
        let server = match &status.health {
            Section::Ready(health) => health.version.clone().map(|version| {
                let same_build = health.build.as_deref() == Some(self.app.client_build.as_str());
                if version == client && !same_build {
                    format!("{version} (older build)")
                } else {
                    version
                }
            }),
            Section::Missing => None,
            Section::Failed(error) => {
                out.push(self.text("Client", client));
                out.push(failed(self.app, "server version", error));
                return;
            }
        };
        match server {
            Some(server) if server != client => {
                let warning = Style::default().fg(t.warning);
                out.push(self.row("Client", vec![Span::styled(client, warning)]));
                out.push(self.row(
                    "Server",
                    vec![Span::styled(
                        format!("{server} · restart the service"),
                        warning,
                    )],
                ));
            }
            Some(server) => {
                out.push(self.text("Client", client));
                out.push(self.text("Server", server));
            }
            None => out.push(self.text("Client", client)),
        }
    }

    fn session(&self, status: &Status, out: &mut Vec<Line<'static>>) {
        let session = match &status.session {
            Section::Ready(session) => session,
            Section::Missing => &self.app.session,
            Section::Failed(error) => {
                out.push(failed(self.app, "session", error));
                &self.app.session
            }
        };
        let title = match clean(&session.title) {
            title if title.is_empty() => "New session".to_owned(),
            title => title,
        };
        out.push(self.text("Session", title));
        out.push(self.text("ID", clean(&session.id)));
        let home = self.app.home.as_deref();
        out.push(self.text(
            "Location",
            crate::sidebar::home_path(&session.directory, home),
        ));
        self.branch(status, out);
    }

    fn branch(&self, status: &Status, out: &mut Vec<Line<'static>>) {
        let t = &self.app.theme;
        match &status.vcs {
            Section::Ready(Some(git)) => {
                let name = git.branch.clone().unwrap_or_else(|| match &git.head {
                    Some(head) => {
                        format!("detached at {}", head.chars().take(8).collect::<String>())
                    }
                    None => "detached".into(),
                });
                let mut spans = vec![Span::styled(name, Style::default().fg(t.text))];
                if git.dirty {
                    spans.push(Span::styled(
                        " · uncommitted changes",
                        Style::default().fg(t.warning),
                    ));
                }
                out.push(self.row("Branch", spans));
            }
            Section::Ready(None) => out.push(self.text("Branch", "not a git repository".into())),
            Section::Missing => {}
            Section::Failed(error) => out.push(failed(self.app, "git state", error)),
        }
    }

    fn model(&self, status: &Status, out: &mut Vec<Line<'static>>) {
        let session = match &status.session {
            Section::Ready(session) => session,
            _ => &self.app.session,
        };
        out.push(self.text("Agent", crate::turns::title_case(&session.agent)));
        out.push(self.model_row(session));
        out.push(self.text("Mode", session.mode_label()));
        if let Some(meter) = session.measured_context() {
            out.push(self.context_row(&meter));
            if let Some(line) = self.compaction_row(&meter) {
                out.push(line);
            }
        }
    }

    /// `amini-coder (test) · high`: the display name, provider and variant.
    fn model_row(&self, session: &Session) -> Line<'static> {
        let reference = clean(&session.model);
        let (base, variant) = reference
            .split_once('#')
            .map_or((reference.as_str(), None), |(b, v)| (b, Some(v)));
        let mut text = self.app.catalog.model_name(base);
        if let Some((provider, _)) = base.split_once('/') {
            text.push_str(&format!(" ({provider})"));
        }
        if let Some(variant) = variant {
            text.push_str(&format!(" · {variant}"));
        }
        self.text("Model", text)
    }

    /// A 30-column meter, then `29.5K / 128.0K tokens (22%)`.
    fn context_row(&self, meter: &ContextMeter) -> Line<'static> {
        let t = &self.app.theme;
        let tokens = compact(meter.used_tokens);
        let (Some(window), Some(percent)) = (meter.window, meter.percent()) else {
            return self.text("Context", format!("{tokens} tokens (window unknown)"));
        };
        let text = format!("{tokens} / {} tokens ({percent:.0}%)", compact(window));
        let warn = percent > 85.0;
        let style = Style::default().fg(if warn { t.warning } else { t.text });
        if self.app.screen_reader {
            let sentence = crate::sidebar::context_sentence(meter);
            return self.row("Context", vec![Span::styled(sentence, style)]);
        }
        let filled = ((percent / 100.0) * METER as f64)
            .round()
            .clamp(0.0, METER as f64) as usize;
        self.row(
            "Context",
            vec![
                Span::styled("█".repeat(filled), Style::default().fg(t.meter_fill)),
                Span::styled(
                    "░".repeat(METER - filled),
                    Style::default().fg(t.meter_empty),
                ),
                Span::styled(format!(" {text}"), style),
            ],
        )
    }

    /// The auto-compaction threshold, when the window is known.
    fn compaction_row(&self, meter: &ContextMeter) -> Option<Line<'static>> {
        meter.window?;
        let value = match meter.compact_at {
            Some(at) => format!("at {} tokens", compact(at)),
            None => "off".into(),
        };
        Some(self.text("Auto-compact", value))
    }

    fn servers(&self, status: &Status, out: &mut Vec<Line<'static>>) {
        match &status.mcp {
            Section::Ready(servers) => out.push(self.mcp_row(servers)),
            Section::Missing => {}
            Section::Failed(error) => out.push(failed(self.app, "MCP servers", error)),
        }
        match &status.lsp {
            Section::Ready(servers) => out.push(self.text("LSP", lsp_summary(servers))),
            Section::Missing => {}
            Section::Failed(error) => out.push(failed(self.app, "language servers", error)),
        }
        match &status.machine {
            Section::Ready(machine) => {
                out.push(self.text("Machine", crate::sidebar::machine_sentence(machine)));
            }
            Section::Missing => {}
            Section::Failed(error) => out.push(failed(self.app, "machine stats", error)),
        }
        if let Section::Ready(health) = &status.health {
            if let Some(sandbox) = &health.sandbox {
                out.push(self.text("Sandbox", sandbox.clone()));
            }
            if let Some(account) = &health.account {
                out.push(self.text("Account", account.clone()));
            }
        }
    }

    /// `2 of 3 connected · failed: docs`.
    fn mcp_row(&self, servers: &[McpServer]) -> Line<'static> {
        let t = &self.app.theme;
        if servers.is_empty() {
            return self.text("MCP", "none configured".into());
        }
        let connected = servers.iter().filter(|s| s.status == "connected").count();
        let mut spans = vec![Span::styled(
            format!("{connected} of {} connected", servers.len()),
            Style::default().fg(t.text),
        )];
        let failed: Vec<&str> = servers
            .iter()
            .filter(|s| s.status == "failed")
            .map(|s| s.name.as_str())
            .collect();
        if !failed.is_empty() {
            spans.push(Span::styled(
                format!(" · failed: {}", failed.join(", ")),
                Style::default().fg(t.error),
            ));
        }
        self.row("MCP", spans)
    }
}

/// `rust-analyzer connected, pyright starting`, or `none running`.
fn lsp_summary(servers: &[LspServer]) -> String {
    if servers.is_empty() {
        return "none running".into();
    }
    servers
        .iter()
        .map(|s| format!("{} {}", s.id, s.status))
        .collect::<Vec<_>>()
        .join(", ")
}

/// A heading row in bold `text`.
pub fn heading(app: &App, text: &str) -> Line<'static> {
    Line::from(Span::styled(
        text.to_owned(),
        Style::default()
            .fg(app.theme.text)
            .add_modifier(Modifier::BOLD),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn health_sandbox_object_shows_policy_enforcement_and_isolation() {
        let health = Health::parse(&serde_json::json!({
            "sandbox": {"policy": "full-access", "profile": "full", "enforcement": "off", "isolation": "container"}
        }));
        assert_eq!(
            health.sandbox.as_deref(),
            Some("full-access · off · isolation: container")
        );
        let legacy = Health::parse(&serde_json::json!({"sandbox": "unavailable"}));
        assert_eq!(legacy.sandbox.as_deref(), Some("unavailable"));
    }
}
