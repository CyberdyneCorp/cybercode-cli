//! Exact retained controls, polled without blocking stdin on physical cleanup.
use super::*;
use cyber_client::ClientError;
use cyber_server::runtime::{InputInterruptReceipt, InputInterruptStatus};

const WAIT: Duration = Duration::from_secs(30);
const REQUEST_WAIT: Duration = Duration::from_secs(5);
enum Failure {
    Retry,
    Refused,
}
async fn bounded(
    request: impl std::future::Future<Output = Result<Value, ClientError>>,
) -> Result<Value, Failure> {
    match tokio::time::timeout(REQUEST_WAIT, request).await {
        Ok(Ok(response)) => Ok(response),
        Ok(Err(ClientError::Transport(_))) | Err(_) => Err(Failure::Retry),
        Ok(Err(_)) => Err(Failure::Refused),
    }
}

pub(super) struct Pending {
    request: String,
    inputs: Vec<String>,
    started: Instant,
}
impl Pending {
    fn expired(&self) -> bool {
        self.started.elapsed() >= WAIT
    }
    fn body(&self) -> Value {
        json!({"request_id":self.request,"input_message_ids":self.inputs})
    }
    fn receipt(&self, response: &Value) -> Result<InputInterruptReceipt, ()> {
        let receipt: InputInterruptReceipt =
            serde_json::from_value(response["data"].clone()).map_err(|_| ())?;
        if receipt.request_id != self.request
            || receipt.input_message_ids != self.inputs
            || receipt.generation.is_empty()
            || receipt.generation.len() > 256
            || receipt.session_seq.is_none_or(|seq| seq < 0)
            || (receipt.status == InputInterruptStatus::Acknowledged
                && !receipt.problems.is_empty())
        {
            return Err(());
        }
        Ok(receipt)
    }
}
impl Driver<'_> {
    pub(super) fn stopping_input(&self, id: &str) -> bool {
        self.interruptions
            .iter()
            .any(|pending| pending.inputs.iter().any(|input| input == id))
    }
    pub(super) async fn interrupt(&mut self) {
        let mut inputs = self.inputs.identities();
        if inputs.iter().all(|id| self.stopping_input(id)) {
            return;
        }
        inputs.sort();
        let pending = Pending {
            request: cyber_core::ids::new_id("op"),
            inputs,
            started: Instant::now(),
        };
        let response = bounded(self.client.post(
            &format!("/sessions/{}/interrupt-inputs", self.id),
            pending.body(),
        ))
        .await;
        match response {
            Ok(response) if pending.receipt(&response).is_ok() => {
                self.interruptions.push_back(pending)
            }
            Err(Failure::Retry) => self.interruptions.push_back(pending),
            _ => self.interruption_failed(&pending).await,
        }
    }
    pub(super) async fn poll_interruption(&mut self) {
        let next = self
            .interruptions
            .iter()
            .position(Pending::expired)
            .unwrap_or(0);
        let Some(pending) = self.interruptions.remove(next) else {
            return;
        };
        if pending.expired() {
            return self.interruption_failed(&pending).await;
        }
        let response = self.lookup_interruption(&pending).await;
        let receipt = match response {
            Ok(response) => match pending.receipt(&response) {
                Ok(receipt) => receipt,
                Err(()) => return self.interruption_failed(&pending).await,
            },
            Err(Failure::Retry) => {
                self.interruptions.push_back(pending);
                return;
            }
            Err(_) => return self.interruption_failed(&pending).await,
        };
        match receipt.status {
            InputInterruptStatus::Unknown => self.interruption_failed(&pending).await,
            InputInterruptStatus::Requested => self.interruptions.push_back(pending),
            InputInterruptStatus::Acknowledged
                if self
                    .seen_seq
                    .is_none_or(|seq| Some(seq) < receipt.session_seq) =>
            {
                self.interruptions.push_back(pending)
            }
            InputInterruptStatus::Acknowledged => self.interruption_finished(&pending),
        }
    }
    async fn lookup_interruption(&self, pending: &Pending) -> Result<Value, Failure> {
        let path = format!("/sessions/{}/interrupt-inputs/{}", self.id, pending.request);
        bounded(async {
            match self.client.get(&path).await {
                // A transport failure may have occurred before the original intent.
                // Re-submit that same identity/cohort; never manufacture a new target.
                Err(ClientError::Api {
                    status: 404,
                    ref tag,
                    ..
                }) if tag == "InputInterruptionNotFoundError" => {
                    self.client
                        .post(
                            &format!("/sessions/{}/interrupt-inputs", self.id),
                            pending.body(),
                        )
                        .await
                }
                response => response,
            }
        })
        .await
    }
    fn interruption_finished(&mut self, pending: &Pending) {
        for id in &pending.inputs {
            if let Some(input) = self.inputs.get_mut(id)
                && input.structured.is_none()
            {
                input
                    .error
                    .get_or_insert_with(|| "Input interrupted".into());
            }
            if !self.stopping_input(id)
                && let Some(input) = self.inputs.complete(id)
            {
                self.emit_result(&input);
            }
        }
        self.answering.retain(|id| self.inputs.contains(id));
        if self.answering.is_empty() {
            self.step = None;
            self.run.retire_driver_answer();
            self.answer_done = false;
        }
        // Receipts already withdraw exact owned requests. Stale buffered asks
        // are no longer reply authority after their input slots have retired.
        self.asks
            .retain(|_, ask| ask.inputs.iter().all(|id| self.inputs.contains(id)));
    }
    async fn interruption_failed(&mut self, pending: &Pending) {
        let message = format!(
            "Input interruption could not be reconciled; inspect Session {} and interruption {}",
            self.id, pending.request
        );
        for id in &pending.inputs {
            if let Some(input) = self.inputs.get_mut(id) {
                input.error.get_or_insert_with(|| message.clone());
            }
        }
        self.run.error = Some(message);
        self.background_failed = true;
        self.closed = true;
        self.release().await;
    }
}
