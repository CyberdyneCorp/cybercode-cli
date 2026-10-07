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

impl ToolHost for AppHost {
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

    fn definitions(&self, turn: &TurnContext) -> Vec<ToolDef> {
        let mut defs = self.builtin.definitions(turn);
        defs.extend(
            self.builtin
                .filter_agent_tools(turn, self.remote.definitions()),
        );
        defs
    }

    fn execute(&self, call: Invocation, cancel: CancellationToken) -> BoxFuture<'_, ToolOutcome> {
        if self.remote.has(&call.name) {
            if let Err(error) = self.builtin.check_agent_tool(&call) {
                return Box::pin(async move { ToolOutcome::Failed(error) });
            }
            return Box::pin(self.remote.execute(call, cancel));
        }
        self.builtin.execute(call, cancel)
    }

    fn reconcile(&self, directory: &str, call: &CallState) -> BoxFuture<'_, Reconciliation> {
        self.builtin.reconcile(directory, call)
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
        self.builtin.context_sources(turn)
    }

    fn context_observations(
        &self,
        turn: &TurnContext,
    ) -> BTreeMap<String, cyber_server::runtime::ContextObservation> {
        self.builtin.context_observations(turn)
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
