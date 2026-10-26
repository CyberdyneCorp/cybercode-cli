use super::accounting::Attempt;
use cyber_llm::{ErrorKind, EventStream, LlmError};
use futures::StreamExt;
use std::sync::Arc;

/// If an opening future or a stream is dropped, preserve the allocation and flush
/// already observed usage. A lost process cannot run this task; its durable uncertain
/// allocation remains for owner reconciliation instead.
pub(super) struct Lease {
    attempt: Arc<Attempt>,
    armed: bool,
}
impl Lease {
    pub fn new(attempt: Arc<Attempt>) -> Self {
        Self {
            attempt,
            armed: true,
        }
    }
    pub fn disarm(&mut self) {
        self.armed = false;
    }
}
impl Drop for Lease {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        self.attempt.ended(false);
        let attempt = Arc::clone(&self.attempt);
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                if let Err(error) = attempt.flush().await {
                    attempt.failed(error);
                }
            });
        }
    }
}

pub(super) fn wrap(stream: EventStream, attempt: Arc<Attempt>, lease: Lease) -> EventStream {
    futures::stream::unfold(
        (Some(stream), attempt, lease),
        |(mut stream, attempt, mut lease)| async move {
            let inner = stream.as_mut()?;
            let next = if let Some(deadline) = attempt.deadline {
                match tokio::time::timeout_at(deadline, inner.next()).await {
                    Ok(next) => next,
                    Err(_) => {
                        attempt.ended(false);
                        let error = attempt.wall_error();
                        let message = error.to_string();
                        attempt.failed(error);
                        if let Err(error) = attempt.flush().await {
                            attempt.failed(error);
                        }
                        lease.disarm();
                        return Some((
                            Err(LlmError::new(ErrorKind::InvalidRequest, message)),
                            (None, attempt, lease),
                        ));
                    }
                }
            } else {
                inner.next().await
            };
            match next {
                Some(Ok(event)) => {
                    if let Err(message) = attempt.observe(&event) {
                        attempt.ended(false);
                        if let Err(error) = attempt.flush().await {
                            attempt.failed(error);
                        }
                        lease.disarm();
                        return Some((
                            Err(LlmError::new(ErrorKind::InvalidRequest, message)),
                            (None, attempt, lease),
                        ));
                    }
                    Some((Ok(event), (stream, attempt, lease)))
                }
                Some(Err(error)) => {
                    attempt.ended(false);
                    let result = match attempt.flush().await {
                        Ok(()) => Err(error),
                        Err(error) => {
                            let message = error.to_string();
                            attempt.failed(error);
                            Err(LlmError::new(ErrorKind::InvalidRequest, message))
                        }
                    };
                    lease.disarm();
                    Some((result, (None, attempt, lease)))
                }
                None => {
                    attempt.ended(true);
                    match attempt.flush().await {
                        Ok(()) => {
                            lease.disarm();
                            None
                        }
                        Err(error) => {
                            let message = error.to_string();
                            attempt.failed(error);
                            lease.disarm();
                            Some((
                                Err(LlmError::new(ErrorKind::InvalidRequest, message)),
                                (None, attempt, lease),
                            ))
                        }
                    }
                }
            }
        },
    )
    .boxed()
}
