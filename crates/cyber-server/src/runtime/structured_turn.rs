//! A root prompt's structured result across its Turns (`session-runtime` → Structured
//! output): a valid `structured_output` call ends the answer, a Turn that ends without
//! one is asked again, and three failed submissions end it with `StructuredOutputError`.

use std::{borrow::Cow, collections::BTreeMap, sync::Arc};

use cyber_store::NewEvent;

use super::bus::LiveEvent;
use super::drain::TurnEnd;
use super::events::{SYSTEM_ADDED, SystemAdded, event};
use super::structured::{MISSING_REASON, ROOT_ATTEMPTS, ROOT_TOOL, ResultState, SETTLED};
use super::{Handle, Inner, RuntimeError, SessionState, StructuredOutcome, StructuredSchema};

const KIND: &str = "structured_output";

impl Inner {
    /// A terminal Drain exit must not strand its root input's result. Keep this
    /// before releasing the Drain entry, so another pass cannot replace ownership.
    /// Shutdown preserves an open attempt for recovery in the next process.
    pub(crate) async fn root_result_at_exit(
        &self,
        session_id: &str,
        failure: Option<&(String, String)>,
        interrupted: bool,
    ) -> Result<(), RuntimeError> {
        if self.closed.is_cancelled() {
            return Ok(());
        }
        let handle = self.handle(session_id).await?;
        let mut state = handle.state.lock().await;
        if self.closed.is_cancelled() || !state.result.root_open() {
            return Ok(());
        }
        let why = match failure {
            Some((kind, message)) => format!("{kind}: {message}"),
            None if interrupted => "interrupted".into(),
            None => "execution ended before structured_output".into(),
        };
        if let Some(outcome) = state.result.terminal_outcome(&why) {
            self.commit_locked(&mut state, vec![event(SETTLED, &outcome)])?;
        }
        Ok(())
    }

    /// After a Turn's calls settle: the input's valid result, or its last failure, ends
    /// the answer; otherwise the model goes on.
    pub(crate) async fn root_result_end(
        &self,
        handle: &Arc<Handle>,
    ) -> Result<Option<TurnEnd>, RuntimeError> {
        let mut state = handle.state.lock().await;
        let result = &state.result;
        let Some(input) = result.input.clone().filter(|_| result.root_open()) else {
            return Ok(None);
        };
        let attempts = result.attempts;
        if let Some((_, value)) = result.returned.clone() {
            let outcome = StructuredOutcome {
                message_id: input,
                value: Some(value),
                error: None,
                attempts,
            };
            self.commit_locked(&mut state, vec![event(SETTLED, &outcome)])?;
            return Ok(Some(TurnEnd::Done));
        }
        if attempts < ROOT_ATTEMPTS {
            return Ok(None);
        }
        let why = result
            .error
            .as_ref()
            .map_or_else(|| "no valid result".to_owned(), |(_, e)| e.clone());
        drop(state);
        self.fail_root_result(handle, input, attempts, &why)
            .await
            .map(Some)
    }

    /// A Turn that ended without calling the finalizer counts as a failed submission:
    /// the model is asked again, until the third.
    pub(crate) async fn root_result_missing(
        &self,
        handle: &Arc<Handle>,
    ) -> Result<TurnEnd, RuntimeError> {
        let mut state = handle.state.lock().await;
        let result = &state.result;
        let Some(input) = result.input.clone().filter(|_| result.root_open()) else {
            return Ok(TurnEnd::Done);
        };
        let attempts = result.attempts + 1;
        if attempts >= ROOT_ATTEMPTS {
            drop(state);
            let why = format!("the answer ended without calling {ROOT_TOOL}");
            return self.fail_root_result(handle, input, attempts, &why).await;
        }
        let reminder = SystemAdded {
            message_id: cyber_core::ids::new_id("msg"),
            text: format!(
                "Your answer must end with a call to {ROOT_TOOL} whose input matches its schema. Call it now with your final answer."
            ),
            reason: MISSING_REASON.into(),
        };
        self.commit_locked(&mut state, vec![event(SYSTEM_ADDED, &reminder)])?;
        Ok(TurnEnd::Tools)
    }

    /// Record `StructuredOutputError` on the input and fail the Turn with it.
    async fn fail_root_result(
        &self,
        handle: &Arc<Handle>,
        input: String,
        attempts: u32,
        why: &str,
    ) -> Result<TurnEnd, RuntimeError> {
        let message = format!("StructuredOutputError: {why}");
        let outcome = StructuredOutcome {
            message_id: input,
            value: None,
            error: Some(message.clone()),
            attempts,
        };
        let session_id = {
            let mut state = handle.state.lock().await;
            self.commit_locked(&mut state, vec![event(SETTLED, &outcome)])?;
            state.info.id.clone()
        };
        self.bus.publish(LiveEvent::Error {
            session_id,
            kind: KIND.into(),
            message: message.clone(),
        });
        Ok(TurnEnd::Failed {
            kind: KIND.into(),
            message,
        })
    }
}

/// Freeze each replaced root input's outcome without changing live state. The
/// caller inserts it immediately before its replacement's promotion in one append.
pub(crate) fn promotion_outcomes(
    state: &SessionState,
    inputs: &[String],
) -> BTreeMap<String, NewEvent> {
    let mut outcomes = BTreeMap::new();
    if state.info.parent_id.is_some() {
        return outcomes;
    }
    let mut previous = Cow::Borrowed(&state.result);
    for input in inputs {
        if let Some(outcome) = previous.terminal_outcome(&format!("superseded by input {input}")) {
            outcomes.insert(input.clone(), event(SETTLED, &outcome));
        }
        // Match on_promoted's schema selection for an intermediate input in this
        // batch. Uncompilable historical schemas grant no new result ownership.
        previous = Cow::Owned(
            state
                .inbox
                .iter()
                .find(|row| row.message_id == *input)
                .and_then(|row| row.output_schema.clone())
                .and_then(|schema| StructuredSchema::new(schema).ok())
                .map(|schema| ResultState::root(schema, input.clone()))
                .unwrap_or_default(),
        );
    }
    outcomes
}
