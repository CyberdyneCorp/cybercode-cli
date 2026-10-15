//! Planned command template expansion at admission (`skills-commands` → Shell output
//! injection, File and agent references).
//!
//! Each `` !`cmd` `` recorded by the planner is gated as a `bash` call: the agent's tool
//! list, skill tool restrictions and `PreToolUse` hooks apply, then permission under the
//! Session's agent, mode and rules (an `ask` prompts the attached client; plan and
//! dont-ask make it a denial), then every allowed command runs concurrently in the sandbox with a
//! timeout, the runtime's cancellation, a stdout cap and credential redaction. A
//! project-controlled template runs nothing until its exact digest is approved. `@`
//! references attach files and directories through the `read` tool's own checks, or
//! name a subagent; anything else stays text.
use std::path::Path;
use std::sync::atomic::Ordering;
use std::time::Duration;

use cyber_core::hooks::HookAction;
use cyber_llm::Content;
use cyber_server::runtime::{
    Asker, Invocation, PromptTemplate, ToolHost, ToolOutcome, TurnContext,
};
use serde_json::json;
use tokio_util::sync::CancellationToken;

use super::{BuiltinHost, Ctx};
use crate::tools::{ToolError, bash};

/// Per-command limit (`skills-commands` → Shell output injection).
pub(crate) const INJECTION_TIMEOUT: Duration = Duration::from_secs(30);
/// Per-command stdout cap.
const OUTPUT_CAP: usize = 20 * 1024;
/// Stdout read past the cap, so a credential straddling the cut is still recognised and
/// redacted before truncation.
const REDACTION_MARGIN: usize = 4 * 1024;
/// References resolved per template; later tokens stay text.
const REFERENCE_LIMIT: usize = 16;
/// Attached reference text per template.
const ATTACHED_LIMIT: usize = 256 * 1024;
const CANCELLED: &str = "command template expansion was cancelled";

impl BuiltinHost {
    /// Override the per-command injection timeout (tests use a short one).
    pub fn set_injection_timeout(&self, timeout: Duration) {
        let millis = u64::try_from(timeout.as_millis())
            .unwrap_or(u64::MAX)
            .max(1);
        self.injection_timeout_ms.store(millis, Ordering::SeqCst);
    }

    /// Whether the checkout approved this exact project template digest.
    pub(crate) fn template_trusted(&self, location: &Path, digest: &str) -> bool {
        let Some(config) = self.hook_config.get() else {
            return false;
        };
        let root = cyber_core::config::project_root(location);
        config
            .trust
            .is_template_approved(&root, digest)
            .unwrap_or(false)
    }

    pub(crate) async fn expand_template(
        &self,
        turn: &TurnContext,
        template: &PromptTemplate,
        parts: &[Content],
        asker: Asker,
        cancel: CancellationToken,
    ) -> Result<Vec<Content>, String> {
        let tokens = match parts.first() {
            Some(Content::Text { text }) => reference_tokens(text, template),
            _ => Vec::new(),
        };
        let references = self.reference_parts(turn, &tokens, &asker, &cancel).await?;
        let outputs = self
            .injection_outputs(turn, template, &asker, &cancel)
            .await?;
        let mut expanded = template.apply(parts, &outputs)?;
        expanded.extend(references);
        Ok(expanded)
    }

    /// One substitution per recorded span, in order.
    async fn injection_outputs(
        &self,
        turn: &TurnContext,
        template: &PromptTemplate,
        asker: &Asker,
        cancel: &CancellationToken,
    ) -> Result<Vec<String>, String> {
        let commands: Vec<&str> = template
            .injections
            .iter()
            .map(|injection| injection.command.as_str())
            .collect();
        if commands.is_empty() {
            return Ok(Vec::new());
        }
        if template.project && !self.template_trusted(Path::new(&turn.directory), &template.digest)
        {
            return Ok(commands.iter().map(|c| notice("untrusted", c)).collect());
        }
        // Gates and permission first, one prompt at a time; allowed commands then run
        // together.
        let mut admitted = Vec::with_capacity(commands.len());
        for command in &commands {
            let invocation = injection_invocation(turn, command, asker);
            admitted.push(self.admit_injection(invocation, cancel).await?);
        }
        let timeout = self.injection_timeout_ms.load(Ordering::SeqCst);
        let runs = commands.iter().zip(admitted).map(|(command, admitted)| {
            let cancel = cancel.clone();
            async move {
                match admitted {
                    Some(invocation) => self.run_injection(&invocation, timeout, cancel).await,
                    None => Ok(notice("denied", command)),
                }
            }
        });
        futures::future::join_all(runs).await.into_iter().collect()
    }

    /// The synthetic `bash` call passes every gate a model's `bash` call passes in
    /// `execute`: the one-shot override, skill tool restrictions, the agent's tool list
    /// and `PreToolUse` hooks (a deny blocks it, a rewrite replaces its input), then
    /// permission. `Ok(None)` is a denial; a failing gate fails closed.
    async fn admit_injection(
        &self,
        mut invocation: Invocation,
        cancel: &CancellationToken,
    ) -> Result<Option<Invocation>, String> {
        let Some(bash) = self.tools.iter().find(|t| t.def().spec.name == "bash") else {
            return Ok(None);
        };
        let gated = invocation
            .asker
            .validate_auto_override(&invocation.name, &invocation.input)
            .is_ok()
            && self.check_skill_tool(&invocation).await.is_ok()
            && self.check_agent_tool(&invocation).is_ok();
        if !gated {
            return Ok(None);
        }
        let schema = &bash.def().spec.input_schema;
        let decision = match self
            .pre_tool_hooks(&mut invocation, schema, cancel.clone())
            .await
        {
            Ok(decision) => decision,
            Err(ToolError::Failed(_)) => return Ok(None),
            Err(ToolError::Aborted) => return Err(CANCELLED.into()),
        };
        let ctx = self
            .injection_ctx(&invocation, decision, cancel.clone())
            .await?;
        match bash::authorize_injection(&ctx, injection_command(&invocation)).await {
            Ok(()) => Ok(Some(invocation)),
            Err(ToolError::Failed(_)) => Ok(None),
            Err(ToolError::Aborted) => Err(CANCELLED.into()),
        }
    }

    async fn run_injection(
        &self,
        invocation: &Invocation,
        timeout_ms: u64,
        cancel: CancellationToken,
    ) -> Result<String, String> {
        let command = injection_command(invocation);
        let ctx = self.injection_ctx(invocation, None, cancel).await?;
        match bash::run_injected(&ctx, command, timeout_ms, OUTPUT_CAP + REDACTION_MARGIN).await {
            Ok(output) => Ok(bounded_output(&output)),
            Err(ToolError::Failed(message)) if message.contains("SandboxUnavailableError") => {
                Ok("[unavailable: sandbox]".into())
            }
            Err(ToolError::Failed(_)) => Ok(notice("failed", command)),
            Err(ToolError::Aborted) => Err(CANCELLED.into()),
        }
    }

    async fn injection_ctx<'a>(
        &'a self,
        invocation: &'a Invocation,
        hook_decision: Option<HookAction>,
        cancel: CancellationToken,
    ) -> Result<Ctx<'a>, String> {
        Ok(Ctx {
            skill_paths: Default::default(),
            loaded_skill: Default::default(),
            compiler_feedback: Default::default(),
            hook_decision,
            denial: Default::default(),
            host: self,
            policy: self.policy(invocation).await?,
            location: super::canonical(Path::new(&invocation.directory)),
            inv: invocation,
            cancel,
        })
    }

    /// Attached parts for the template's references, in token order.
    async fn reference_parts(
        &self,
        turn: &TurnContext,
        tokens: &[String],
        asker: &Asker,
        cancel: &CancellationToken,
    ) -> Result<Vec<Content>, String> {
        if tokens.is_empty() {
            return Ok(Vec::new());
        }
        let agents = self.subagent_names(Path::new(&turn.directory));
        let mut parts = Vec::new();
        let mut attached = 0;
        for token in tokens {
            if agents.iter().any(|agent| agent == token) {
                parts.push(agent_part(token));
                continue;
            }
            let Some(text) = self.read_reference(turn, token, asker, cancel).await? else {
                continue;
            };
            if attached + text.len() > ATTACHED_LIMIT {
                break;
            }
            attached += text.len();
            parts.push(Content::Text { text });
        }
        Ok(parts)
    }

    fn subagent_names(&self, location: &Path) -> Vec<String> {
        let Ok((config, _)) = (self.opts.config)(location) else {
            return Vec::new();
        };
        cyber_core::config::resolve_agents(&config)
            .map(|profiles| {
                profiles
                    .into_values()
                    .filter(|profile| profile.subagent_capable() && !profile.hidden)
                    .map(|profile| profile.name)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// An existing path, read as the `read` tool, so `external_directory` and `read`
    /// rules, asks and hooks apply exactly as for a model call. A missing or refused
    /// path attaches nothing.
    async fn read_reference(
        &self,
        turn: &TurnContext,
        token: &str,
        asker: &Asker,
        cancel: &CancellationToken,
    ) -> Result<Option<String>, String> {
        let root = cyber_core::config::project_root(Path::new(&turn.directory));
        let path = super::resolve_path(&root, &self.opts.home, token);
        if !path.exists() {
            return Ok(None);
        }
        let invocation = Invocation {
            registration: None,
            session_id: turn.session_id.clone(),
            directory: turn.directory.clone(),
            agent: turn.agent.clone(),
            mode: turn.mode.clone(),
            message_id: asker.message_id.clone(),
            call_id: asker.call_id.clone(),
            name: "read".into(),
            input: json!({"path": path.display().to_string()}),
            attempt: 1,
            operation_key: cyber_core::ids::new_id("op"),
            asker: asker.clone(),
            rules: turn.rules.clone(),
        };
        let output = match self.execute(invocation, cancel.clone()).await {
            ToolOutcome::Ok(output) => output,
            ToolOutcome::SkillSuggestions {
                failed: false,
                output,
                ..
            } => output,
            ToolOutcome::Aborted => {
                return Err(CANCELLED.into());
            }
            _ => return Ok(None),
        };
        let kind = if path.is_dir() { "directory" } else { "file" };
        Ok(Some(format!(
            "<{kind} path=\"{}\">\n{output}\n</{kind}>",
            escape_attribute(token)
        )))
    }
}

fn injection_invocation(turn: &TurnContext, command: &str, asker: &Asker) -> Invocation {
    Invocation {
        registration: None,
        session_id: turn.session_id.clone(),
        directory: turn.directory.clone(),
        agent: turn.agent.clone(),
        mode: turn.mode.clone(),
        message_id: asker.message_id.clone(),
        call_id: asker.call_id.clone(),
        name: "bash".into(),
        input: json!({"command": command}),
        attempt: 1,
        operation_key: cyber_core::ids::new_id("op"),
        asker: asker.clone(),
        rules: turn.rules.clone(),
    }
}

fn injection_command(invocation: &Invocation) -> &str {
    invocation.input["command"].as_str().unwrap_or_default()
}

/// A visible substitution for a command that did not run.
fn notice(kind: &str, command: &str) -> String {
    format!("[{kind}: {command}]")
}

/// Credential-shaped text redacted, then at most [`OUTPUT_CAP`] bytes.
fn bounded_output(output: &str) -> String {
    let mut text = cyber_core::memory::redact_credentials(output);
    if text.len() > OUTPUT_CAP {
        let mut end = OUTPUT_CAP;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
        text.push_str("\n[output truncated at 20 KiB]");
    }
    text
}

fn agent_part(name: &str) -> Content {
    Content::Text {
        text: format!(
            "<agent_invocation name=\"{name}\">The user invoked the {name} subagent: use the agent tool with agent \"{name}\" for this request.</agent_invocation>"
        ),
    }
}

fn escape_attribute(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
}

/// `@` tokens of the template text outside its shell spans, without trailing
/// punctuation, deduplicated and bounded. `@server:uri` stays with the MCP mention path.
pub(crate) fn reference_tokens(text: &str, template: &PromptTemplate) -> Vec<String> {
    let mut masked = text.to_string();
    for injection in &template.injections {
        if let Some(span) = text.get(injection.start..injection.end) {
            masked.replace_range(injection.start..injection.end, &" ".repeat(span.len()));
        }
    }
    let mut tokens: Vec<String> = Vec::new();
    for word in masked.split_whitespace() {
        let Some(token) = word.strip_prefix('@') else {
            continue;
        };
        let token = token.trim_end_matches(['.', ',', ';', ':', '!', '?', ')', ']', '"', '\'']);
        if token.is_empty() || mcp_mention(token) || tokens.iter().any(|seen| seen == token) {
            continue;
        }
        tokens.push(token.to_string());
        if tokens.len() == REFERENCE_LIMIT {
            break;
        }
    }
    tokens
}

/// A server-name-like prefix and `:`, unless the token is a drive-letter path
/// (`C:\x` or `C:/x`).
fn mcp_mention(token: &str) -> bool {
    let bytes = token.as_bytes();
    let drive = bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'\\' | b'/');
    !drive
        && token.split_once(':').is_some_and(|(server, _)| {
            (1..=48).contains(&server.len())
                && server
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use cyber_server::runtime::TemplateInjection;

    fn template(text: &str, commands: &[&str]) -> PromptTemplate {
        let injections = commands
            .iter()
            .map(|command| {
                let span = format!("!`{command}`");
                let start = text.find(&span).unwrap();
                TemplateInjection {
                    start,
                    end: start + span.len(),
                    command: (*command).into(),
                }
            })
            .collect();
        PromptTemplate {
            kind: "command".into(),
            name: "t".into(),
            project: false,
            digest: String::new(),
            injections,
        }
    }

    #[test]
    fn references_skip_shell_spans_mcp_mentions_and_punctuation() {
        let text =
            "See @src/lib.rs, @explore and @docs:file:///a !`cat @secret` @C:\\x @src/lib.rs @";
        assert_eq!(
            reference_tokens(text, &template(text, &["cat @secret"])),
            ["src/lib.rs", "explore", "C:\\x"]
        );
    }

    #[test]
    fn output_is_redacted_and_capped() {
        let redacted = bounded_output("ok\naws_secret_access_key = abc123\n");
        assert!(redacted.starts_with("ok\n") && !redacted.contains("abc123"));
        let long = bounded_output(&"é".repeat(OUTPUT_CAP));
        assert!(long.len() <= OUTPUT_CAP + 40);
        assert!(long.ends_with("[output truncated at 20 KiB]"));
    }
}
