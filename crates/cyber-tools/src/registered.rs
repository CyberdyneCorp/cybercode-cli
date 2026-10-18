//! Schema, permissions, hooks and output settlement for externally registered tools.
use crate::host::{BuiltinHost, Ctx, DenialHooks, fully_denied};
use crate::permissions::{Effect, Mode, Request, Rule};
use crate::tools::ToolError;
use cyber_core::hooks::HookAction;
use cyber_server::runtime::{Invocation, ToolDef, ToolOutcome, TurnContext};
use futures::future::BoxFuture;
use serde_json::Value;
use std::path::PathBuf;
use tokio_util::sync::CancellationToken;

pub(crate) struct RegisteredOptions {
    pub read_only: bool,
    pub metadata: Value,
    pub output_token_limit: Option<usize>,
    /// The tool's MCP annotations; `Null` for a tool that declares none (an
    /// application-registered tool).
    pub annotations: Value,
}

/// How a registered tool's annotations drive permission modes (`tool-registry` →
/// Annotations drive modes and hooks). MCP hints take their protocol defaults when
/// absent: a tool that is not read-only is destructive, and every tool is open-world.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct ToolHints {
    pub destructive: bool,
    pub open_world: bool,
}

impl ToolHints {
    pub(crate) fn of(annotations: &Value, read_only: bool) -> Self {
        if !annotations.is_object() {
            return Self::default();
        }
        Self {
            destructive: !read_only && annotations["destructiveHint"] != false,
            open_world: annotations["openWorldHint"] != false,
        }
    }
}

impl BuiltinHost {
    /// Visibility does not grant execution authority; current policy is checked again on calls.
    pub fn filter_registered_tools(
        &self,
        turn: &TurnContext,
        tools: Vec<ToolDef>,
        read_only: bool,
    ) -> Vec<ToolDef> {
        if Mode::parse(&turn.mode) == Mode::Plan && !read_only {
            return Vec::new();
        }
        let Ok(rules) = self.session_rules(
            std::path::Path::new(&turn.directory),
            Some(&turn.agent),
            &turn.rules,
        ) else {
            return Vec::new();
        };
        self.filter_agent_tools(
            turn,
            tools
                .into_iter()
                .filter(|tool| !fully_denied(&rules, &tool.spec.name))
                .collect(),
        )
    }

    /// The owner still verifies its native registration after all awaited admission checks.
    pub fn execute_registered<'a>(
        &'a self,
        inv: Invocation,
        definition: ToolDef,
        read_only: bool,
        metadata: Value,
        cancel: CancellationToken,
        execute: impl FnOnce(Invocation, CancellationToken) -> BoxFuture<'a, ToolOutcome> + Send + 'a,
    ) -> BoxFuture<'a, ToolOutcome> {
        self.execute_registered_with_output_limit(
            inv,
            definition,
            RegisteredOptions {
                read_only,
                metadata,
                output_token_limit: None,
                annotations: Value::Null,
            },
            cancel,
            execute,
        )
    }

    pub(crate) fn execute_registered_with_output_limit<'a>(
        &'a self,
        mut inv: Invocation,
        definition: ToolDef,
        options: RegisteredOptions,
        cancel: CancellationToken,
        execute: impl FnOnce(Invocation, CancellationToken) -> BoxFuture<'a, ToolOutcome> + Send + 'a,
    ) -> BoxFuture<'a, ToolOutcome> {
        let RegisteredOptions {
            read_only,
            metadata,
            output_token_limit,
            annotations,
        } = options;
        let hints = ToolHints::of(&annotations, read_only);
        Box::pin(async move {
            if inv.name != definition.spec.name || inv.registration != definition.registration {
                return ToolOutcome::Failed(format!("Stale tool call: {}", inv.name));
            }
            // Hook envelopes of this call carry its annotations.
            let _annotations = self.call_annotations(&inv, annotations);
            if let Err(error) = inv.asker.validate_auto_override(&inv.name, &inv.input) {
                return ToolOutcome::Failed(error.to_string());
            }
            if let Err(error) = self.check_skill_tool(&inv).await {
                return ToolOutcome::Failed(error);
            }
            if let Err(error) = self.check_agent_tool(&inv) {
                return ToolOutcome::Failed(error);
            }
            if let Err(error) = crate::schema::validate(&definition.spec.input_schema, &inv.input) {
                return ToolOutcome::Failed(error);
            }
            let decision = match self
                .pre_tool_hooks(&mut inv, &definition.spec.input_schema, cancel.clone())
                .await
            {
                Ok(decision) => decision,
                Err(error) => return failure(error),
            };
            let started = std::time::Instant::now();
            let ctx = match self
                .registered_ctx(&inv, read_only, decision, &cancel, DenialHooks::armed())
                .await
            {
                Ok(ctx) => ctx,
                Err(error) => return ToolOutcome::Failed(error),
            };
            let server = metadata["server"].as_str().unwrap_or(&inv.name).to_owned();
            let authorized = match ctx
                .authorize(
                    request(&inv, read_only, hints),
                    vec!["*".into()],
                    metadata.clone(),
                )
                .await
            {
                Ok(()) if hints.open_world => open_world_gate(&ctx, &server).await,
                other => other,
            };
            let retry = match &authorized {
                Err(ToolError::Failed(error)) => {
                    ctx.denial.take_retry(&ToolOutcome::Failed(error.clone()))
                }
                _ => None,
            };
            let (inv, authorized) = match retry {
                None => (inv.clone(), authorized),
                Some(input) => {
                    let retry = Retry {
                        schema: &definition.spec.input_schema,
                        read_only,
                        hints,
                        decision,
                        metadata,
                    };
                    self.retry_registered(&inv, retry, input, &cancel).await
                }
            };
            let outcome = match authorized {
                Err(error) => failure(error),
                Ok(()) if cancel.is_cancelled() => ToolOutcome::Aborted,
                Ok(()) => self.bound_registered_output(
                    &ctx,
                    execute(inv.clone(), cancel.clone()).await,
                    output_token_limit,
                ),
            };
            match self.post_tool_hooks(&inv, &outcome, started, cancel).await {
                Ok(()) => outcome,
                Err(ToolError::Aborted) => ToolOutcome::Aborted,
                Err(ToolError::Failed(error)) => {
                    ToolOutcome::Failed(format!("Tool settled; post-hook failed: {error}"))
                }
            }
        })
    }

    /// Current policy, with the registration's default rule, for one attempt.
    async fn registered_ctx<'c>(
        &'c self,
        inv: &'c Invocation,
        read_only: bool,
        decision: Option<HookAction>,
        cancel: &CancellationToken,
        denial: DenialHooks,
    ) -> Result<Ctx<'c>, String> {
        let mut policy = self.policy(inv).await?;
        if let Some(reason) = policy.skill_tool_denial(&inv.name) {
            return Err(reason);
        }
        let index = policy
            .rules
            .iter()
            .rposition(|rule| rule.source == "default")
            .map_or(0, |index| index + 1);
        let effect = if read_only {
            Effect::Allow
        } else {
            Effect::Ask
        };
        policy
            .rules
            .insert(index, Rule::new(&inv.name, "*", effect, "default"));
        Ok(Ctx {
            skill_paths: Default::default(),
            loaded_skill: Default::default(),
            compiler_feedback: Default::default(),
            host: self,
            inv,
            policy,
            location: PathBuf::from(&inv.directory),
            cancel: cancel.clone(),
            hook_decision: decision,
            denial,
        })
    }

    /// Re-validate and re-authorize a PermissionDenied retry exactly once; its
    /// denial is final, a hook allow does not carry, and changed input passes
    /// PreToolUse again.
    async fn retry_registered(
        &self,
        inv: &Invocation,
        retry: Retry<'_>,
        input: Value,
        cancel: &CancellationToken,
    ) -> (Invocation, Result<(), ToolError>) {
        let retried = self
            .retried_invocation(inv, retry.schema, input, retry.decision, cancel.clone())
            .await;
        let (retried, decision) = match retried {
            Ok(retried) => retried,
            Err(error) => return (inv.clone(), Err(error)),
        };
        let authorized = match self
            .registered_ctx(
                &retried,
                retry.read_only,
                decision,
                cancel,
                DenialHooks::default(),
            )
            .await
        {
            Ok(ctx) => {
                let server = retry.metadata["server"]
                    .as_str()
                    .unwrap_or(&retried.name)
                    .to_owned();
                match ctx
                    .authorize(
                        request(&retried, retry.read_only, retry.hints),
                        vec!["*".into()],
                        retry.metadata,
                    )
                    .await
                {
                    Ok(()) if retry.hints.open_world => open_world_gate(&ctx, &server).await,
                    other => other,
                }
            }
            Err(error) => Err(ToolError::Failed(error)),
        };
        (retried, authorized)
    }

    fn bound_registered_output(
        &self,
        ctx: &Ctx<'_>,
        outcome: ToolOutcome,
        token_limit: Option<usize>,
    ) -> ToolOutcome {
        match outcome {
            ToolOutcome::Ok(output) => self
                .budget(&ctx.location)
                .apply_with_token_limit(output, false, token_limit)
                .map_or_else(ToolOutcome::Crashed, ToolOutcome::Ok),
            ToolOutcome::Structured { output, value } => self
                .budget(&ctx.location)
                .apply_with_token_limit(output, false, token_limit)
                .map_or_else(ToolOutcome::Crashed, |output| ToolOutcome::Structured {
                    output,
                    value,
                }),
            ToolOutcome::SkillSuggestions {
                failed,
                output,
                value,
                skills,
            } => self
                .budget(&ctx.location)
                .apply_with_token_limit(output, false, token_limit)
                .map_or_else(ToolOutcome::Crashed, |output| {
                    ToolOutcome::SkillSuggestions {
                        failed,
                        output,
                        value,
                        skills,
                    }
                }),
            ToolOutcome::Failed(output) => self
                .budget(&ctx.location)
                .apply_with_token_limit(output, false, token_limit)
                .map_or_else(ToolOutcome::Crashed, ToolOutcome::Failed),
            other => other,
        }
    }
}

struct Retry<'a> {
    schema: &'a Value,
    read_only: bool,
    hints: ToolHints,
    decision: Option<HookAction>,
    metadata: Value,
}

fn request(inv: &Invocation, read_only: bool, hints: ToolHints) -> Request {
    Request {
        action: inv.name.clone(),
        resources: vec!["*".into()],
        read_only,
        destructive: hints.destructive,
        ..Default::default()
    }
}

/// An open-world tool reaches beyond the workspace, so with the sandbox network off it
/// needs a `network` permission (resource `mcp:<server>`) before it runs.
async fn open_world_gate(ctx: &Ctx<'_>, server: &str) -> Result<(), crate::tools::ToolError> {
    if !crate::sandboxing::network_off(ctx) {
        return Ok(());
    }
    let resource = format!("mcp:{server}");
    let req = Request {
        action: "network".into(),
        resources: vec![resource.clone()],
        read_only: true,
        ..Request::default()
    };
    let metadata = serde_json::json!({"server": server, "tool": ctx.inv.name, "open_world": true});
    ctx.authorize(req, vec![resource], metadata).await
}

/// Annotations of a registered call in flight, removed when the call settles.
pub(crate) struct CallAnnotations<'a> {
    host: &'a BuiltinHost,
    key: Option<(String, String)>,
}

impl Drop for CallAnnotations<'_> {
    fn drop(&mut self) {
        if let Some(key) = self.key.take() {
            self.host
                .tool_annotations
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(&key);
        }
    }
}

impl BuiltinHost {
    fn call_annotations(&self, inv: &Invocation, annotations: Value) -> CallAnnotations<'_> {
        let key = annotations.is_object().then(|| {
            let key = (inv.session_id.clone(), inv.call_id.clone());
            self.tool_annotations
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(key.clone(), annotations);
            key
        });
        CallAnnotations { host: self, key }
    }

    /// The annotations a hook envelope carries as `tool_annotations` for a call.
    pub(crate) fn annotations_of(&self, inv: &Invocation) -> Option<Value> {
        self.tool_annotations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&(inv.session_id.clone(), inv.call_id.clone()))
            .cloned()
    }
}

fn failure(error: ToolError) -> ToolOutcome {
    match error {
        ToolError::Aborted => ToolOutcome::Aborted,
        ToolError::Failed(error) => ToolOutcome::Failed(error),
    }
}
