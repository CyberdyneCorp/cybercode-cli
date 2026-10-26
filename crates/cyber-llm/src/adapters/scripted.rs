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
    fn prepare_reserved(
        &self,
        request: LlmRequest,
        _context_limit: u64,
    ) -> Result<crate::reservations::PreparedReservation, crate::reservations::BoundUnavailable>
    {
        let turns = self.turns.lock().unwrap_or_else(PoisonError::into_inner);
        let mut maximum = crate::Usage::default();
        for turn in turns.iter() {
            let mut usage = crate::Usage::default();
            for step in turn {
                if let ScriptStep::Event(LlmEvent::Usage(value)) = step {
                    usage = checked_usage(usage, *value)?;
                }
            }
            maximum.input = maximum.input.max(usage.input);
            maximum.output = maximum.output.max(usage.output);
            maximum.reasoning = maximum.reasoning.max(usage.reasoning);
            maximum.cache_read = maximum.cache_read.max(usage.cache_read);
            maximum.cache_write = maximum.cache_write.max(usage.cache_write);
        }
        let bound = crate::reservations::RequestBound::from_usage(
            maximum,
            "scripted_finite_fixture_v1",
            &serde_json::json!({"model":request.model,"classes":maximum}),
        )?;
        Ok(crate::reservations::PreparedReservation { request, bound })
    }
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

fn checked_usage(
    a: crate::Usage,
    b: crate::Usage,
) -> Result<crate::Usage, crate::reservations::BoundUnavailable> {
    let add = |a: u64, b: u64| {
        a.checked_add(b)
            .ok_or_else(|| crate::reservations::unavailable("scripted token bound overflow"))
    };
    Ok(crate::Usage {
        input: add(a.input, b.input)?,
        output: add(a.output, b.output)?,
        reasoning: add(a.reasoning, b.reasoning)?,
        cache_read: add(a.cache_read, b.cache_read)?,
        cache_write: add(a.cache_write, b.cache_write)?,
    })
}
