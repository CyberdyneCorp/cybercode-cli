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
use std::sync::Arc;
use std::time::Duration;

use futures::future::BoxFuture;
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
    /// The longest server-advised delay waited as an ordinary retry; per provider
    /// (`providers.<id>.retry.max_wait_ms`).
    pub retry_after_cap: Duration,
    /// What an advised delay longer than `retry_after_cap` does. It is never shortened:
    /// a retry before the window opens only spends an attempt.
    pub over_cap: OverCap,
    /// How retry waits sleep.
    pub sleeper: Sleeper,
}

/// The cap of a server-advised delay when the provider sets none.
pub const DEFAULT_RETRY_AFTER_CAP: Duration = Duration::from_secs(60);

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: 4,
            base_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(30),
            retry_after_cap: DEFAULT_RETRY_AFTER_CAP,
            over_cap: OverCap::Fail,
            sleeper: Sleeper::default(),
        }
    }
}

/// What an advised delay beyond the cap does (`provider-catalog` → Gemini error and quota
/// classification): an interactive Session fails at once so the fallback chain and the
/// user can decide; an unattended run waits the whole delay, up to a bound the runtime
/// derives from `usage_limit.max_wait` and the Session's wall deadlines.
#[derive(Debug, Clone, Default)]
pub enum OverCap {
    #[default]
    Fail,
    Wait {
        /// The longest delay waited.
        up_to: Duration,
        sleeper: Sleeper,
    },
}

/// Sleeps for a retry wait. The default is the Tokio timer; the runtime waits a long
/// unattended delay on its wait clock, and tests skip time.
#[derive(Clone)]
pub struct Sleeper(Arc<dyn Fn(Duration) -> BoxFuture<'static, ()> + Send + Sync>);

impl Sleeper {
    pub fn new(sleep: impl Fn(Duration) -> BoxFuture<'static, ()> + Send + Sync + 'static) -> Self {
        Self(Arc::new(sleep))
    }

    pub fn sleep(&self, duration: Duration) -> BoxFuture<'static, ()> {
        (self.0)(duration)
    }
}

impl Default for Sleeper {
    fn default() -> Self {
        Self::new(|duration| Box::pin(tokio::time::sleep(duration)))
    }
}

impl std::fmt::Debug for Sleeper {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Sleeper(..)")
    }
}

impl RetryPolicy {
    /// Delay before retry number `attempt` (1-based); `None` when the server advised a
    /// delay beyond the cap that this run does not wait.
    pub fn delay(&self, attempt: u32, error: &LlmError) -> Option<Duration> {
        self.plan(attempt, error).map(|(delay, _)| delay)
    }

    /// The delay before retry `attempt` and what sleeps it.
    fn plan(&self, attempt: u32, error: &LlmError) -> Option<(Duration, &Sleeper)> {
        let Some(after) = error.retry_after else {
            let exp = self
                .base_delay
                .saturating_mul(2_u32.saturating_pow(attempt.saturating_sub(1)));
            let capped = exp.min(self.max_delay);
            return Some((capped.mul_f64(0.5 + jitter() * 0.5), &self.sleeper));
        };
        if after <= self.retry_after_cap {
            return Some((after, &self.sleeper));
        }
        match &self.over_cap {
            OverCap::Wait { up_to, sleeper } if after <= *up_to => Some((after, sleeper)),
            _ => None,
        }
    }
}

/// The failure of an advised delay beyond the cap, naming both.
fn over_cap(mut failure: LlmError, cap: Duration) -> LlmError {
    let after = failure.retry_after.unwrap_or_default();
    failure.message = format!(
        "{}; the provider advised waiting {}, beyond the {} retry wait cap",
        failure.message,
        span(after),
        span(cap)
    );
    failure
}

/// `37s`, `10m`, `1h5m`.
fn span(duration: Duration) -> String {
    let seconds = duration.as_secs_f64().ceil() as u64;
    match (seconds / 3600, seconds % 3600 / 60, seconds % 60) {
        (0, 0, s) => format!("{s}s"),
        (0, m, 0) => format!("{m}m"),
        (0, m, s) => format!("{m}m{s}s"),
        (h, 0, 0) => format!("{h}h"),
        (h, m, _) => format!("{h}h{m}m"),
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
        let Some((delay, sleeper)) = self.policy.plan(self.attempt + 1, &failure) else {
            return Err(over_cap(failure, self.policy.retry_after_cap));
        };
        let sleep = sleeper.sleep(delay);
        self.attempt += 1;
        (self.on_retry)(&RetryNotice {
            attempt: self.attempt,
            delay,
            error: &failure,
            discarded,
        });
        sleep.await;
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

    /// Regression: an advised delay beyond the cap was shortened to the cap, so the
    /// retry came before the window opened and spent an attempt. Now it fails at once,
    /// or, when the run waits, waits the whole delay.
    #[test]
    fn an_advised_delay_is_waited_whole_or_not_at_all() {
        let mut p = RetryPolicy::default();
        let mut e = LlmError::new(ErrorKind::RateLimit, "x");
        e.retry_after = Some(Duration::from_secs(7));
        assert_eq!(p.delay(1, &e), Some(Duration::from_secs(7)));
        e.retry_after = Some(Duration::from_secs(600));
        assert_eq!(p.delay(1, &e), None, "never a shortened wait");
        p.over_cap = OverCap::Wait {
            up_to: Duration::from_secs(900),
            sleeper: Sleeper::default(),
        };
        assert_eq!(p.delay(1, &e), Some(Duration::from_secs(600)));
        e.retry_after = Some(Duration::from_secs(901));
        assert_eq!(p.delay(1, &e), None, "beyond the bound fails too");
        p.retry_after_cap = Duration::from_secs(240);
        e.retry_after = Some(Duration::from_secs(240));
        assert_eq!(p.delay(1, &e), Some(Duration::from_secs(240)));
    }

    #[test]
    fn backoff_grows_and_caps() {
        let p = RetryPolicy::default();
        let e = LlmError::new(ErrorKind::ProviderInternal, "x");
        let first = p.delay(1, &e).unwrap();
        assert!(first >= Duration::from_millis(500) && first <= Duration::from_secs(1));
        assert!(p.delay(10, &e).unwrap() <= Duration::from_secs(30));
    }

    #[test]
    fn spans_read_as_people_write_them() {
        assert_eq!(span(Duration::from_secs(37)), "37s");
        assert_eq!(span(Duration::from_secs(600)), "10m");
        assert_eq!(span(Duration::from_secs(250)), "4m10s");
        assert_eq!(span(Duration::from_secs(3900)), "1h5m");
    }
}
