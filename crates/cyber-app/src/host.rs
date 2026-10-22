//! The runtime's tool host: built-in tools plus tools registered by connected clients.

use std::collections::BTreeMap;
use std::sync::Arc;

use cyber_server::http::remote_tools::RemoteTools;
use cyber_server::runtime::messaging::PeerTarget;
use cyber_server::runtime::{
    CallState, Invocation, Reconciliation, ToolDef, ToolHost, ToolOutcome, TurnContext,
};
use cyber_tools::BuiltinHost;
use futures::future::BoxFuture;
use tokio_util::sync::CancellationToken;

#[cfg(test)]
mod tests;

/// Looks up a `peers.remotes` entry with its token by name.
pub(crate) type PeerLookup = dyn Fn(&str) -> Result<Option<PeerTarget>, String> + Send + Sync;
/// The names of the enabled `peers.remotes` entries.
pub(crate) type PeerNames = dyn Fn() -> Result<Vec<String>, String> + Send + Sync;

pub struct AppHost {
    pub builtin: Arc<BuiltinHost>,
    pub remote: Arc<RemoteTools>,
    /// The remotes of the user configuration, with their stored tokens.
    pub peers: Arc<PeerLookup>,
    /// The enabled remotes of the user configuration, for the reachable listing.
    pub peer_names: Arc<PeerNames>,
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

    fn end_session(&self, session_id: &str) -> BoxFuture<'_, ()> {
        self.builtin.end_session(session_id)
    }
    fn claude_compat(&self, directory: &str) -> Option<bool> {
        self.builtin.claude_compat(directory)
    }

    fn instruction_read_gate(
        &self,
        directory: &str,
    ) -> Option<cyber_server::runtime::instructions::ReadGate> {
        self.builtin.instruction_read_gate(directory)
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

    fn admit_mode(&self, directory: &str, agent: &str, mode: &str) -> Result<(), String> {
        self.builtin.admit_mode(directory, agent, mode)
    }

    fn config_digest(&self, directory: &str) -> Option<String> {
        self.builtin.config_digest(directory)
    }

    fn daily_budget(&self, directory: &str) -> Result<Option<cyber_core::budget::Budget>, String> {
        self.builtin.daily_budget(directory)
    }

    fn goal_settings(
        &self,
        directory: &str,
    ) -> Result<
        (
            cyber_core::goals::GoalSettings,
            Option<cyber_core::budget::Budget>,
        ),
        String,
    > {
        self.builtin.goal_settings(directory)
    }

    fn loop_settings(
        &self,
        directory: &str,
    ) -> Result<
        (
            cyber_core::loops::LoopSettings,
            Option<cyber_core::budget::Budget>,
        ),
        String,
    > {
        self.builtin.loop_settings(directory)
    }

    fn check(
        &self,
        directory: &str,
        session_id: &str,
        command: &str,
        timeout_ms: u64,
        cancel: CancellationToken,
    ) -> BoxFuture<'_, Result<cyber_server::runtime::CheckOutcome, String>> {
        self.builtin
            .check(directory, session_id, command, timeout_ms, cancel)
    }

    fn loop_command_plan<'a>(
        &'a self,
        turn: &'a TurnContext,
        name: &'a str,
        arguments: &'a str,
    ) -> BoxFuture<
        'a,
        Result<Option<cyber_server::runtime::CommandPlan>, cyber_server::runtime::CommandPlanError>,
    > {
        self.builtin.loop_command_plan(turn, name, arguments)
    }

    fn messaging_settings(
        &self,
        directory: Option<&str>,
    ) -> Result<cyber_core::config::MessagingSettings, String> {
        self.builtin.messaging_settings(directory)
    }

    fn team_settings(
        &self,
        directory: &str,
    ) -> Result<Option<cyber_core::teams::TeamSettings>, String> {
        self.builtin.team_settings(directory)
    }

    fn team_editor(&self, directory: &str, agent: &str) -> bool {
        self.builtin.team_editor(directory, agent)
    }

    fn create_team_session<'a>(
        &'a self,
        lead: &'a cyber_server::runtime::SessionInfo,
        request: cyber_server::runtime::CreateSession,
        worktree: &'a cyber_server::runtime::TeamWorktree,
        cancel: CancellationToken,
    ) -> BoxFuture<'a, Result<cyber_server::runtime::SessionInfo, String>> {
        self.builtin
            .create_team_session(lead, request, worktree, cancel)
    }

    fn create_goal_fork_session<'a>(
        &'a self,
        source: &'a cyber_server::runtime::SessionInfo,
        request: cyber_server::runtime::CreateSession,
        worktree: &'a cyber_server::runtime::GoalWorktree,
        cancel: CancellationToken,
    ) -> BoxFuture<'a, Result<cyber_server::runtime::SessionInfo, String>> {
        self.builtin
            .create_goal_fork_session(source, request, worktree, cancel)
    }

    fn peer_target(&self, name: &str) -> Result<Option<PeerTarget>, String> {
        (self.peers)(name)
    }

    fn peer_names(&self) -> Result<Vec<String>, String> {
        (self.peer_names)()
    }

    fn permission_effect(
        &self,
        turn: &TurnContext,
        action: &str,
        resource: &str,
    ) -> Result<cyber_server::runtime::RuleEffect, String> {
        self.builtin.permission_effect(turn, action, resource)
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

    fn lifecycle_hooks(
        &self,
        event: cyber_server::runtime::LifecycleHook,
        cancel: CancellationToken,
    ) -> BoxFuture<'_, Result<cyber_server::runtime::LifecycleVerdict, String>> {
        self.builtin.lifecycle_hooks(event, cancel)
    }

    fn take_hook_effects(
        &self,
        session_id: &str,
        call_id: &str,
    ) -> cyber_server::runtime::HookEffects {
        self.builtin.take_hook_effects(session_id, call_id)
    }

    fn file_decisions<'a>(
        &'a self,
        turn: &'a TurnContext,
        action: &'a str,
        paths: &'a [std::path::PathBuf],
    ) -> BoxFuture<'a, Vec<cyber_server::runtime::FileDecision>> {
        self.builtin.file_decisions(turn, action, paths)
    }

    fn authorize_file<'a>(
        &'a self,
        turn: &'a TurnContext,
        action: &'a str,
        path: &'a std::path::Path,
        asker: cyber_server::runtime::Asker,
        cancel: CancellationToken,
    ) -> BoxFuture<'a, Result<(), String>> {
        self.builtin
            .authorize_file(turn, action, path, asker, cancel)
    }

    fn resolve_prompt_resources<'a>(
        &'a self,
        turn: &'a TurnContext,
        parts: &'a [cyber_llm::Content],
        asker: cyber_server::runtime::Asker,
        cancel: CancellationToken,
    ) -> BoxFuture<'a, Vec<String>> {
        self.builtin
            .resolve_prompt_resources(turn, parts, asker, cancel)
    }

    fn expand_prompt_template<'a>(
        &'a self,
        turn: &'a TurnContext,
        template: &'a cyber_server::runtime::PromptTemplate,
        parts: &'a [cyber_llm::Content],
        asker: cyber_server::runtime::Asker,
        cancel: CancellationToken,
    ) -> BoxFuture<'a, Result<Vec<cyber_llm::Content>, String>> {
        self.builtin
            .expand_prompt_template(turn, template, parts, asker, cancel)
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

    fn spawn_workflow_agent(
        &self,
        request: cyber_server::runtime::WorkflowAgentRequest,
        cancel: CancellationToken,
    ) -> BoxFuture<'_, Result<cyber_server::runtime::WorkflowAgentOutput, cyber_workflow::Diagnostic>>
    {
        self.builtin.spawn_workflow_agent(request, cancel)
    }

    fn workflow_agent_writes(&self, directory: &str, agent: Option<&str>) -> bool {
        self.builtin.workflow_agent_writes(directory, agent)
    }

    fn warms_prompt_cache(&self) -> bool {
        self.builtin.warms_prompt_cache()
    }

    fn workflow_tool(
        &self,
        request: cyber_server::runtime::WorkflowToolRequest,
        cancel: CancellationToken,
    ) -> BoxFuture<'_, Result<serde_json::Value, cyber_workflow::Diagnostic>> {
        self.builtin.workflow_tool(request, cancel)
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

    fn shell_background(
        &self,
        directory: &str,
        session_id: &str,
        command: &str,
    ) -> BoxFuture<'_, Result<String, String>> {
        self.builtin
            .shell_background(directory, session_id, command)
    }
}
