//! A scripted provider for deterministic evaluation fixtures and tests.

use std::sync::{Mutex, PoisonError};

use futures::future::BoxFuture;
use futures::stream::{self, StreamExt};
use serde::{Deserialize, Serialize};

use super::{Adapter, EventStream};
use crate::error::{ErrorKind, LlmError};
use crate::types::{LlmEvent, LlmRequest};

/// One scripted stream item.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ScriptStep {
    Event(LlmEvent),
    Error { error: ErrorKind, message: String },
}

/// Each `stream` call plays the next turn of the script. A turn whose first step is an
/// error fails when the stream is opened, as an HTTP error would.
pub struct ScriptedAdapter {
    turns: Mutex<std::collections::VecDeque<Vec<ScriptStep>>>,
    requests: Mutex<Vec<LlmRequest>>,
}

impl ScriptedAdapter {
    pub fn new(turns: Vec<Vec<ScriptStep>>) -> Self {
        Self {
            turns: Mutex::new(turns.into()),
            requests: Mutex::default(),
        }
    }

    /// Requests received so far, for assertions.
    pub fn requests(&self) -> Vec<LlmRequest> {
        self.requests
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

impl Adapter for ScriptedAdapter {
    fn stream(&self, request: LlmRequest) -> BoxFuture<'_, Result<EventStream, LlmError>> {
        self.requests
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(request);
        let turn = self
            .turns
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pop_front();
        Box::pin(async move {
            let turn =
                turn.ok_or_else(|| LlmError::new(ErrorKind::InvalidRequest, "script exhausted"))?;
            if let Some(ScriptStep::Error { error, message }) = turn.first() {
                return Err(LlmError::new(*error, message.clone()));
            }
            let items: Vec<Result<LlmEvent, LlmError>> = turn
                .into_iter()
                .map(|step| match step {
                    ScriptStep::Event(e) => Ok(e),
                    ScriptStep::Error { error, message } => Err(LlmError::new(error, message)),
                })
                .collect();
            Ok(stream::iter(items).boxed())
        })
    }
}
