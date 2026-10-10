//! Schema, permissions, hooks and output settlement for externally registered tools.
use crate::host::{BuiltinHost, Ctx, fully_denied};
use crate::permissions::{Effect, Mode, Request, Rule};
use crate::tools::ToolError;
use cyber_server::runtime::{Invocation, ToolDef, ToolOutcome, TurnContext};
use futures::future::BoxFuture;
use serde_json::Value;
use std::path::PathBuf;
use tokio_util::sync::CancellationToken;

pub(crate) struct RegisteredOptions {
    pub read_only: bool,
    pub metadata: Value,
    pub output_token_limit: Option<usize>,
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
        } = options;
        Box::pin(async move {
            if inv.name != definition.spec.name || inv.registration != definition.registration {
                return ToolOutcome::Failed(format!("Stale tool call: {}", inv.name));
            }
            if let Err(error) = inv.asker.validate_auto_override(&inv.name, &inv.input) {
                return ToolOutcome::Failed(error.to_string());
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
            let mut policy = match self.policy(&inv).await {
                Ok(policy) => policy,
                Err(error) => return ToolOutcome::Failed(error),
            };
            let index = policy
                .rules
                .iter()
                .rposition(|rule| rule.source == "default")
                .map_or(0, |index| index + 1);
            policy.rules.insert(
                index,
                Rule::new(
                    &inv.name,
                    "*",
                    if read_only {
                        Effect::Allow
                    } else {
                        Effect::Ask
                    },
                    "default",
                ),
            );
            let ctx = Ctx {
                compiler_feedback: Default::default(),
                host: self,
                inv: &inv,
                policy,
                location: PathBuf::from(&inv.directory),
                cancel: cancel.clone(),
                hook_decision: decision,
            };
            let started = std::time::Instant::now();
            let request = Request {
                action: inv.name.clone(),
                resources: vec!["*".into()],
                read_only,
                ..Default::default()
            };
            let outcome = match ctx.authorize(request, vec!["*".into()], metadata).await {
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
            ToolOutcome::Failed(output) => self
                .budget(&ctx.location)
                .apply_with_token_limit(output, false, token_limit)
                .map_or_else(ToolOutcome::Crashed, ToolOutcome::Failed),
            other => other,
        }
    }
}

fn failure(error: ToolError) -> ToolOutcome {
    match error {
        ToolError::Aborted => ToolOutcome::Aborted,
        ToolError::Failed(error) => ToolOutcome::Failed(error),
    }
}
