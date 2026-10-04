//! Retry policy (`provider-catalog` → Retry policy).
//!
//! Retries happen only before any assistant output: a stream that already produced output
//! and then fails is returned to the caller, which settles the Turn as interrupted.

use std::time::Duration;

use futures::stream::{self, StreamExt};

use crate::adapters::{Adapter, EventStream};
use crate::error::LlmError;
use crate::types::{LlmEvent, LlmRequest};

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

/// Open a stream, retrying retryable failures that happen before any output.
/// `on_retry(attempt, delay, error)` is called before each wait.
pub async fn open_with_retry(
    adapter: &dyn Adapter,
    request: &LlmRequest,
    policy: &RetryPolicy,
    mut on_retry: impl FnMut(u32, Duration, &LlmError),
) -> Result<EventStream, LlmError> {
    let mut attempt = 0;
    loop {
        let failure = match adapter.stream(request.clone()).await {
            Ok(stream) => match first_output(stream).await {
                Ok(stream) => return Ok(stream),
                Err(e) => e,
            },
            Err(e) => e,
        };
        if !failure.kind.is_retryable() || attempt >= policy.max_retries {
            return Err(failure);
        }
        attempt += 1;
        let delay = policy.delay(attempt, &failure);
        on_retry(attempt, delay, &failure);
        tokio::time::sleep(delay).await;
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
