//! Retry policy (`provider-catalog` → Retry policy).
//!
//! A retryable failure before any assistant output reopens the request silently. One after
//! partial output discards that attempt: the stream yields [`LlmEvent::Discarded`] and then
//! the events of a fresh attempt of the same request. Retries before and after output share
//! one budget. Nothing is replayed after protocol completion, since a completed stream ends.
//!
//! The stream never knows about tool dispatch: a consumer SHALL NOT start a tool call of the
//! answer while the stream can still be replayed, which the Drain guarantees by dispatching
//! only after the stream ended.

use std::pin::Pin;
use std::time::Duration;

use futures::stream::{self, Stream, StreamExt};

use crate::adapters::{Adapter, EventStream};
use crate::error::LlmError;
use crate::types::{LlmEvent, LlmRequest};

/// A stream that may replay its request; it borrows the adapter it reopens with.
pub type RetryingStream<'a> = Pin<Box<dyn Stream<Item = Result<LlmEvent, LlmError>> + Send + 'a>>;

#[derive(Debug, Clone)]
pub struct RetryPolicy {
    pub max_retries: u32,
    pub base_delay: Duration,
    pub max_delay: Duration,
    pub retry_after_cap: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: 4,
            base_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(30),
            retry_after_cap: Duration::from_secs(60),
        }
    }
}

impl RetryPolicy {
    /// Delay before retry number `attempt` (1-based).
    pub fn delay(&self, attempt: u32, error: &LlmError) -> Duration {
        if let Some(after) = error.retry_after {
            return after.min(self.retry_after_cap);
        }
        let exp = self
            .base_delay
            .saturating_mul(2_u32.saturating_pow(attempt.saturating_sub(1)));
        let capped = exp.min(self.max_delay);
        capped.mul_f64(0.5 + jitter() * 0.5)
    }
}

/// One retry, announced before its wait.
#[derive(Debug)]
pub struct RetryNotice<'e> {
    /// 1-based retry number.
    pub attempt: u32,
    pub delay: Duration,
    pub error: &'e LlmError,
    /// The failed attempt had streamed output, which the consumer must drop.
    pub discarded: bool,
}

/// Open a stream, retrying retryable failures before any output, and replaying the request
/// when a retryable failure interrupts partial output. `on_retry` is called before each wait.
pub async fn open_with_retry<'a>(
    adapter: &'a dyn Adapter,
    request: &LlmRequest,
    policy: &RetryPolicy,
    on_retry: impl FnMut(&RetryNotice<'_>) + Send + 'a,
) -> Result<RetryingStream<'a>, LlmError> {
    let mut replay = Replay {
        adapter,
        request: request.clone(),
        policy: policy.clone(),
        on_retry,
        attempt: 0,
    };
    let first = replay.open().await?;
    Ok(stream::unfold((replay, Phase::Live(first)), step).boxed())
}

struct Replay<'a, F> {
    adapter: &'a dyn Adapter,
    request: LlmRequest,
    policy: RetryPolicy,
    on_retry: F,
    /// Retries used so far.
    attempt: u32,
}

enum Phase {
    Live(EventStream),
    /// The attempt failed after output and was discarded; the next one opens after the wait.
    Reopen(LlmError),
    Done,
}

impl<F: FnMut(&RetryNotice<'_>)> Replay<'_, F> {
    /// Open an attempt that reaches its first output, retrying failures before it.
    async fn open(&mut self) -> Result<EventStream, LlmError> {
        loop {
            let failure = match self.adapter.stream(self.request.clone()).await {
                Ok(stream) => match first_output(stream).await {
                    Ok(stream) => return Ok(stream),
                    Err(e) => e,
                },
                Err(e) => e,
            };
            self.wait(failure, false).await?;
        }
    }

    fn may_retry(&self, failure: &LlmError) -> bool {
        failure.kind.is_retryable() && self.attempt < self.policy.max_retries
    }

    /// Announce the next retry and wait for it, or give the failure back.
    async fn wait(&mut self, failure: LlmError, discarded: bool) -> Result<(), LlmError> {
        if !self.may_retry(&failure) {
            return Err(failure);
        }
        self.attempt += 1;
        let delay = self.policy.delay(self.attempt, &failure);
        (self.on_retry)(&RetryNotice {
            attempt: self.attempt,
            delay,
            error: &failure,
            discarded,
        });
        tokio::time::sleep(delay).await;
        Ok(())
    }
}

type Item = Result<LlmEvent, LlmError>;

/// The next item of a replaying stream.
async fn step<F: FnMut(&RetryNotice<'_>)>(
    (mut replay, phase): (Replay<'_, F>, Phase),
) -> Option<(Item, (Replay<'_, F>, Phase))> {
    match phase {
        Phase::Done => None,
        Phase::Live(mut stream) => match stream.next().await {
            None => None,
            Some(Ok(event)) => Some((Ok(event), (replay, Phase::Live(stream)))),
            // The attempt already streamed output (`first_output`), so a retry discards it.
            Some(Err(e)) if replay.may_retry(&e) => {
                Some((Ok(LlmEvent::Discarded), (replay, Phase::Reopen(e))))
            }
            Some(Err(e)) => Some((Err(e), (replay, Phase::Done))),
        },
        Phase::Reopen(failure) => {
            let reopened = match replay.wait(failure, true).await {
                Ok(()) => replay.open().await,
                Err(e) => Err(e),
            };
            match reopened {
                Ok(mut stream) => {
                    // `first_output` returns a stream with at least one item unless it ended.
                    let first = stream.next().await?;
                    Some((first, (replay, Phase::Live(stream))))
                }
                Err(e) => Some((Err(e), (replay, Phase::Done))),
            }
        }
    }
}

/// Buffer events until the first output event; an error before that fails the attempt.
async fn first_output(mut stream: EventStream) -> Result<EventStream, LlmError> {
    let mut buffered: Vec<Result<LlmEvent, LlmError>> = Vec::new();
    while let Some(item) = stream.next().await {
        let item = item?;
        let output = item.is_output();
        buffered.push(Ok(item));
        if output {
            break;
        }
    }
    Ok(stream::iter(buffered).chain(stream).boxed())
}

fn jitter() -> f64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let mut x = u64::from(nanos) ^ 0x9E37_79B9_7F4A_7C15;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    (x % 1_000_000) as f64 / 1_000_000.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ErrorKind;

    #[test]
    fn retry_after_is_honored_and_capped() {
        let p = RetryPolicy::default();
        let mut e = LlmError::new(ErrorKind::RateLimit, "x");
        e.retry_after = Some(Duration::from_secs(7));
        assert_eq!(p.delay(1, &e), Duration::from_secs(7));
        e.retry_after = Some(Duration::from_secs(600));
        assert_eq!(p.delay(1, &e), Duration::from_secs(60));
    }

    #[test]
    fn backoff_grows_and_caps() {
        let p = RetryPolicy::default();
        let e = LlmError::new(ErrorKind::ProviderInternal, "x");
        let first = p.delay(1, &e);
        assert!(first >= Duration::from_millis(500) && first <= Duration::from_secs(1));
        assert!(p.delay(10, &e) <= Duration::from_secs(30));
    }
}
