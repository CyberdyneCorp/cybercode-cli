//! Calls to a Session's browser tools: the registered-tool path plus the network policy
//! for each navigation, then artifacts (screenshots, redacted console and network
//! captures) and an evidence record for every call that captured one.

use std::sync::{Arc, PoisonError};

use base64::Engine as _;
use cyber_server::runtime::{
    BrowserArtifact, ContentSource, EvidenceStatus, Invocation, Provenance, RetrySafety, ToolDef,
    ToolOutcome, ToolScope, Viewport,
};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::SessionBrowser;
use super::evidence::{self, Check, Visit};
use crate::host::BuiltinHost;
use crate::mcp::DiscoveredTool;

/// The text and media of an MCP tool result.
struct Parts {
    text: String,
    media: Vec<(String, Vec<u8>)>,
}

impl Parts {
    fn of(result: &Value) -> Result<Self, String> {
        let content = result["content"]
            .as_array()
            .ok_or("MCP result has no content list")?;
        let mut text = Vec::new();
        let mut media = Vec::new();
        for item in content {
            match item["type"].as_str() {
                Some("text") => text.push(item["text"].as_str().unwrap_or_default().to_string()),
                Some("image") => media.push(image(item)?),
                _ => return Err("browser result has an unsupported content type".into()),
            }
        }
        Ok(Self {
            text: text.join("\n"),
            media,
        })
    }
}

fn image(item: &Value) -> Result<(String, Vec<u8>), String> {
    let media_type = item["mimeType"].as_str().unwrap_or("image/png");
    if !matches!(media_type, "image/png" | "image/jpeg" | "image/webp") {
        return Err(format!("browser image type {media_type} is not supported"));
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(item["data"].as_str().unwrap_or_default())
        .map_err(|_| "browser image data is not base64")?;
    Ok((media_type.to_string(), bytes))
}

/// The host a navigation goes to; only `browser_navigate` navigates by URL.
fn navigation_host(remote: &str, input: &Value) -> Result<Option<String>, String> {
    if remote != "browser_navigate" {
        return Ok(None);
    }
    let raw = input["url"].as_str().unwrap_or_default();
    let url = url::Url::parse(raw).map_err(|e| format!("Invalid url {raw:?}: {e}"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err("The browser navigates to http(s) URLs only".into());
    }
    let host = url.host_str().ok_or("The URL has no host")?;
    Ok(Some(
        host.trim_start_matches('[')
            .trim_end_matches(']')
            .to_string(),
    ))
}

/// One line for the action summary.
fn summarize(remote: &str, input: &Value) -> String {
    let field = |key: &str| input[key].as_str().unwrap_or_default().to_string();
    let action = remote.strip_prefix("browser_").unwrap_or(remote);
    match action {
        "navigate" => format!("navigate {}", field("url")),
        "click" | "hover" => format!("{action} {}", field("element")),
        "type" => format!("type into {}", field("element")),
        "resize" => format!("resize {}x{}", input["width"], input["height"]),
        other => other.replace('_', " "),
    }
}

/// The artifact kind a text result is kept as, and the check it completes.
fn text_capture(remote: &str) -> Option<(&'static str, Option<Check>)> {
    match remote {
        "browser_console_messages" => Some(("console", Some(Check::Console))),
        "browser_network_requests" => Some(("network", Some(Check::Network))),
        "browser_snapshot" => Some(("snapshot", None)),
        _ => None,
    }
}

fn console_errors(text: &str) -> usize {
    text.lines()
        .filter(|line| {
            let line = line.to_ascii_lowercase();
            line.contains("[error]") || line.trim_start().starts_with("error")
        })
        .count()
}

fn failed_requests(text: &str) -> usize {
    text.lines()
        .filter(|line| {
            let lower = line.to_ascii_lowercase();
            lower.contains("failed") || lower.contains("net::err") || has_error_status(line)
        })
        .count()
}

fn has_error_status(line: &str) -> bool {
    line.split('[').skip(1).any(|part| {
        part.split(']')
            .next()
            .and_then(|code| code.trim().parse::<u16>().ok())
            .is_some_and(|code| (400..600).contains(&code))
    })
}

/// What the browser observed for a successful call, into `visit`.
fn observe(visit: &mut Visit, remote: &str, input: &Value, text: &str) {
    match remote {
        "browser_navigate" => {
            let url = input["url"].as_str().unwrap_or_default();
            visit.navigated(url);
            visit.observed.push(format!("loaded {url}"));
        }
        "browser_resize" => {
            let size = |key: &str| input[key].as_u64().and_then(|n| u32::try_from(n).ok());
            if let (Some(width), Some(height)) = (size("width"), size("height")) {
                visit.viewport = Some(Viewport { width, height });
            }
        }
        "browser_console_messages" => visit.observed.push(format!(
            "console: {} error message(s)",
            console_errors(text)
        )),
        "browser_network_requests" => visit.observed.push(format!(
            "network: {} failed request(s)",
            failed_requests(text)
        )),
        _ => {}
    }
}

impl BuiltinHost {
    /// The Session's live browser and the tool an `mcp__` call names on it.
    pub(crate) fn browser_binding(
        &self,
        inv: &Invocation,
    ) -> Option<(Arc<SessionBrowser>, String, DiscoveredTool)> {
        let entry = self.live_browser(&inv.session_id, &inv.directory)?;
        let (binding, tool) = entry.tool(&inv.name)?;
        Some((entry, binding, tool))
    }

    pub(crate) async fn execute_browser(
        &self,
        inv: Invocation,
        (entry, binding, tool): (Arc<SessionBrowser>, String, DiscoveredTool),
        cancel: CancellationToken,
    ) -> ToolOutcome {
        // A malformed URL fails before any prompt; the host the gates check is read again
        // from the input that runs, after hooks may have rewritten it.
        if let Err(error) = navigation_host(&tool.remote_name, &inv.input) {
            return ToolOutcome::Failed(error);
        }
        let remote = tool.remote_name.clone();
        let network_target: crate::registered::NetworkTarget =
            Arc::new(move |input| navigation_host(&remote, input));
        let definition = ToolDef {
            scope: ToolScope::Mcp,
            deferred: false,
            registration: Some(binding),
            spec: tool.spec(),
            retry_safety: RetrySafety::Never,
            concurrency_safe: false,
        };
        let annotations = match &tool.definition["annotations"] {
            Value::Null => json!({}),
            annotations => annotations.clone(),
        };
        let options = crate::registered::RegisteredOptions {
            read_only: tool.read_only_hint(),
            metadata: json!({"server": entry.name, "annotations": annotations}),
            output_token_limit: None,
            annotations,
            network_target: Some(network_target),
        };
        self.execute_registered_with_output_limit(
            inv,
            definition,
            options,
            cancel,
            move |inv, cancel| {
                Box::pin(async move { self.call_browser(&entry, &tool, &inv, &cancel).await })
            },
        )
        .await
    }

    async fn call_browser(
        &self,
        entry: &Arc<SessionBrowser>,
        tool: &DiscoveredTool,
        inv: &Invocation,
        cancel: &CancellationToken,
    ) -> ToolOutcome {
        let timeout = match self.browser_timeout(&inv.directory, &entry.name) {
            Ok(timeout) => timeout,
            Err(error) => return ToolOutcome::Failed(error),
        };
        let result = {
            let mut server = tokio::select! {
                server = entry.server.lock() => server,
                _ = cancel.cancelled() => return ToolOutcome::Aborted,
            };
            let Some(owner) = server.as_mut() else {
                return ToolOutcome::Failed("The browser is not running".into());
            };
            tokio::select! {
                result = owner.call_exposed_tool(&inv.name, inv.input.clone(), timeout) => result,
                _ = cancel.cancelled() => {
                    // The request may still be in flight: never reuse that connection.
                    drop(server);
                    self.drop_browser(entry);
                    return ToolOutcome::Aborted;
                }
            }
        };
        match result {
            Ok(value) => self.settle_browser(entry, tool, inv, &value).await,
            Err(error) => {
                self.drop_browser(entry);
                ToolOutcome::Failed(format!("The browser failed: {error}"))
            }
        }
    }

    /// Keep the captures, update the visit, record evidence and describe the result.
    async fn settle_browser(
        &self,
        entry: &SessionBrowser,
        tool: &DiscoveredTool,
        inv: &Invocation,
        result: &Value,
    ) -> ToolOutcome {
        let parts = match Parts::of(result) {
            Ok(parts) => parts,
            Err(error) => return ToolOutcome::Failed(error),
        };
        let failed = result["isError"] == true;
        let remote = tool.remote_name.as_str();
        let (artifacts, notes) = if failed {
            (Vec::new(), Vec::new())
        } else {
            match self.capture(&entry.session_id, remote, &parts) {
                Ok(captured) => captured,
                Err(error) => return ToolOutcome::Failed(error),
            }
        };
        let taken = {
            let mut visit = entry.visit.lock().unwrap_or_else(PoisonError::into_inner);
            let action = summarize(remote, &inv.input);
            if failed {
                visit.actions.push(format!("{action} (failed)"));
            } else {
                visit.actions.push(action);
                observe(&mut visit, remote, &inv.input, &parts.text);
                completed(&mut visit, remote, !parts.media.is_empty());
            }
            let record = !artifacts.is_empty();
            visit.artifacts.extend(artifacts);
            record.then(|| visit.take())
        };
        let mut lines: Vec<String> = Some(parts.text)
            .into_iter()
            .filter(|t| !t.is_empty())
            .collect();
        lines.extend(notes);
        if let (Some(taken), Some(runtime)) = (taken, self.runtime()) {
            let directory = std::path::Path::new(&inv.directory);
            match evidence::record(
                &runtime,
                &inv.session_id,
                &inv.call_id,
                directory,
                EvidenceStatus::Observed,
                taken,
            )
            .await
            {
                Ok(evidence) => lines.push(format!("[browser evidence {} recorded]", evidence.id)),
                Err(error) => {
                    return ToolOutcome::Failed(format!(
                        "Browser evidence was not recorded: {error}"
                    ));
                }
            }
        }
        // The page's text is framed as `browser` data by the registry.
        ToolOutcome::External {
            output: lines.join("\n"),
            value: result.get("structuredContent").filter(|_| !failed).cloned(),
            failed,
            provenance: Provenance::new(
                ContentSource::Browser,
                format!("mcp:{}/{remote}", entry.name),
            ),
        }
    }

    /// Store the result's images and its console, network or snapshot text (redacted).
    fn capture(
        &self,
        session_id: &str,
        remote: &str,
        parts: &Parts,
    ) -> Result<(Vec<BrowserArtifact>, Vec<String>), String> {
        let root = self.browser_root();
        let mut artifacts = Vec::new();
        let mut notes = Vec::new();
        for (media_type, bytes) in &parts.media {
            let artifact =
                evidence::store_artifact(&root, session_id, "screenshot", media_type, bytes)?;
            notes.push(format!(
                "[screenshot saved to {} (sha256:{}); read that file to view it]",
                artifact.path, artifact.sha256
            ));
            artifacts.push(artifact);
        }
        if let Some((kind, _)) = text_capture(remote) {
            let redacted = cyber_core::memory::redact_credentials(&parts.text);
            artifacts.push(evidence::store_artifact(
                &root,
                session_id,
                kind,
                "text/plain",
                redacted.as_bytes(),
            )?);
        }
        Ok((artifacts, notes))
    }
}

/// Mark the checks a successful call completed.
fn completed(visit: &mut Visit, remote: &str, captured_media: bool) {
    if captured_media {
        visit.checked(Check::Screenshot);
    }
    if let Some((_, Some(check))) = text_capture(remote) {
        visit.checked(check);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_http_navigation_is_admitted_and_names_its_host() {
        let host = |url: &str| navigation_host("browser_navigate", &json!({"url": url}));
        assert_eq!(
            host("http://127.0.0.1:3000/").unwrap().as_deref(),
            Some("127.0.0.1")
        );
        assert_eq!(host("http://[::1]:8080/").unwrap().as_deref(), Some("::1"));
        assert!(host("file:///etc/passwd").is_err());
        assert!(host("javascript:alert(1)").is_err());
        assert_eq!(navigation_host("browser_click", &json!({})).unwrap(), None);
    }

    #[test]
    fn console_errors_and_failed_requests_are_counted() {
        assert_eq!(
            console_errors("[LOG] ready\n[ERROR] boom at app.js:3\nError: second"),
            2
        );
        assert_eq!(
            failed_requests(
                "[GET] http://a/ => [200] OK\n[GET] http://a/x.js => [404] Not Found\n[GET] http://b/ => net::ERR_FAILED"
            ),
            2
        );
    }
}
