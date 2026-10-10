//! The runtime's tool host: built-in tools plus tools registered by connected clients.

use std::collections::BTreeMap;
use std::sync::Arc;

use cyber_server::http::remote_tools::RemoteTools;
use cyber_server::runtime::{
    CallState, Invocation, Reconciliation, ToolDef, ToolHost, ToolOutcome, TurnContext,
};
use cyber_tools::BuiltinHost;
use futures::future::BoxFuture;
use tokio_util::sync::CancellationToken;

#[cfg(test)]
mod tests;

pub struct AppHost {
    pub builtin: Arc<BuiltinHost>,
    pub remote: Arc<RemoteTools>,
}

pub(crate) fn materialize_tools(
    builtin: &BuiltinHost,
    remote: &RemoteTools,
    turn: &TurnContext,
) -> Vec<ToolDef> {
    let mut defs = builtin.definitions(turn);
    let client = remote.definitions();
    let names: std::collections::HashSet<_> =
        client.iter().map(|def| def.spec.name.clone()).collect();
    // A hidden higher registration does not reveal a lower executable by the same name.
    defs.retain(|def| !names.contains(&def.spec.name));
    defs.extend(builtin.filter_registered_tools(turn, client, false));
    defs
}

impl ToolHost for AppHost {
    fn open_location(&self, info: &cyber_server::runtime::SessionInfo) {
        self.builtin.open_location(info);
    }

    fn refresh_location(&self, info: &cyber_server::runtime::SessionInfo) {
        self.builtin.refresh_location(info);
    }

    fn wait_for_required_mcp(
        &self,
        info: &cyber_server::runtime::SessionInfo,
        cancel: CancellationToken,
    ) -> BoxFuture<'_, Result<(), cyber_server::runtime::RuntimeError>> {
        self.builtin.wait_for_required_mcp(info, cancel)
    }

    fn shutdown(&self) -> BoxFuture<'_, ()> {
        self.builtin.shutdown()
    }
    fn finalize_skill_output(
        &self,
        directory: &str,
        output: String,
        reminder: &str,
    ) -> Result<String, String> {
        self.builtin
            .finalize_skill_output(directory, output, reminder)
    }

    fn session_budget(
        &self,
        directory: &str,
    ) -> Result<Option<cyber_core::budget::Budget>, String> {
        self.builtin.session_budget(directory)
    }

    fn request_overlay(
        &self,
        turn: &TurnContext,
    ) -> Result<cyber_llm::catalog::RequestOverlay, String> {
        self.builtin.request_overlay(turn)
    }

    fn agent_inference(
        &self,
        turn: &TurnContext,
    ) -> Result<cyber_server::runtime::AgentInference, String> {
        self.builtin.agent_inference(turn)
    }

    fn claim_location<'a>(
        &'a self,
        info: &'a cyber_server::runtime::SessionInfo,
        creating: bool,
        cancel: CancellationToken,
    ) -> BoxFuture<'a, Result<cyber_server::runtime::LocationLease, String>> {
        self.builtin.claim_location(info, creating, cancel)
    }

    fn prepare_child_continuation<'a>(
        &'a self,
        parent: &'a cyber_server::runtime::SessionInfo,
        child: &'a cyber_server::runtime::SessionState,
        owner: &'a cyber_server::runtime::ChildExecution,
        cancel: CancellationToken,
    ) -> BoxFuture<'a, Result<Option<Box<dyn cyber_server::runtime::ChildContinuation>>, String>>
    {
        self.builtin
            .prepare_child_continuation(parent, child, owner, cancel)
    }

    fn deferred_tool_settings(
        &self,
        turn: &TurnContext,
    ) -> Result<cyber_core::config::DeferredToolSettings, String> {
        self.builtin.deferred_tool_settings(turn)
    }

    fn definitions(&self, turn: &TurnContext) -> Vec<ToolDef> {
        materialize_tools(&self.builtin, &self.remote, turn)
    }

    fn execute(&self, call: Invocation, cancel: CancellationToken) -> BoxFuture<'_, ToolOutcome> {
        if self.remote.has(&call.name) {
            let Some(definition) = self
                .remote
                .definitions()
                .into_iter()
                .find(|def| def.spec.name == call.name)
            else {
                return Box::pin(async move {
                    ToolOutcome::Failed(format!("Stale tool call: {}", call.name))
                });
            };
            return self.builtin.execute_registered(
                call,
                definition,
                false,
                serde_json::json!({"source":"client"}),
                cancel,
                |call, cancel| Box::pin(self.remote.execute(call, cancel)),
            );
        }
        self.builtin.execute(call, cancel)
    }

    fn reconcile(&self, directory: &str, call: &CallState) -> BoxFuture<'_, Reconciliation> {
        self.builtin.reconcile(directory, call)
    }

    fn subtask(
        &self,
        turn: TurnContext,
        prompt: String,
        cancel: CancellationToken,
    ) -> BoxFuture<'_, Result<cyber_server::runtime::Job, String>> {
        self.builtin.subtask(turn, prompt, cancel)
    }

    fn subtask_with_agent(
        &self,
        turn: TurnContext,
        prompt: String,
        agent: Option<String>,
        cancel: CancellationToken,
    ) -> BoxFuture<'_, Result<cyber_server::runtime::Job, String>> {
        self.builtin.subtask_with_agent(turn, prompt, agent, cancel)
    }

    fn durable_user_delegation(&self) -> bool {
        self.builtin.durable_user_delegation()
    }

    fn subtask_request(
        &self,
        turn: TurnContext,
        request: cyber_server::runtime::UserSubtask,
        cancel: CancellationToken,
    ) -> BoxFuture<'_, Result<cyber_server::runtime::Job, String>> {
        self.builtin.subtask_request(turn, request, cancel)
    }

    fn shell(
        &self,
        directory: &str,
        session_id: &str,
        command: &str,
    ) -> BoxFuture<'_, Result<String, String>> {
        self.builtin.shell(directory, session_id, command)
    }

    fn context_sources(&self, turn: &TurnContext) -> BTreeMap<String, String> {
        self.builtin
            .context_sources_for_tools(turn, &self.definitions(turn))
    }

    fn context_observations(
        &self,
        turn: &TurnContext,
    ) -> BTreeMap<String, cyber_server::runtime::ContextObservation> {
        self.builtin
            .context_observations_for_tools(turn, &self.definitions(turn))
    }

    fn context_observations_owned<'a>(
        &'a self,
        turn: &'a TurnContext,
    ) -> BoxFuture<'a, BTreeMap<String, cyber_server::runtime::ContextObservation>> {
        Box::pin(async move {
            self.builtin
                .context_observations_owned_for_tools(turn, &self.definitions(turn))
                .await
        })
    }

    fn shell_owned(
        &self,
        directory: &str,
        session_id: &str,
        command: &str,
        cancel: CancellationToken,
    ) -> BoxFuture<'_, Result<String, String>> {
        self.builtin
            .shell_owned(directory, session_id, command, cancel)
    }
}
