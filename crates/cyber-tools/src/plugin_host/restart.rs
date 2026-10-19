//! Restart and backoff (`plugins-marketplace` → Restart and backoff): a crashed host
//! restarts after 1 s, doubling with each further crash to at most 60 s; a host that ran
//! for longer than the cap starts again from 1 s. The fifth crash within 10 minutes
//! disables the plugin for the rest of the process.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

pub(crate) const BASE: Duration = Duration::from_secs(1);
/// The longest delay, as a multiple of the base delay (60 s for the 1 s base).
const CAP_FACTOR: u32 = 60;
pub(crate) const WINDOW: Duration = Duration::from_secs(10 * 60);
pub(crate) const LIMIT: usize = 5;

/// What follows a crash.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AfterCrash {
    RestartIn(Duration),
    Disable,
}

/// The crashes of one plugin: those within the window, and the current streak.
#[derive(Debug, Default)]
pub(crate) struct Crashes {
    times: VecDeque<Instant>,
    streak: u32,
}

impl Crashes {
    /// Record a crash at `now` of a host that ran for `uptime`. `base` is [`BASE`] except
    /// where a test shortens it; the cap scales with it.
    pub(crate) fn record(&mut self, now: Instant, uptime: Duration, base: Duration) -> AfterCrash {
        while self
            .times
            .front()
            .is_some_and(|first| now.duration_since(*first) >= WINDOW)
        {
            self.times.pop_front();
        }
        self.times.push_back(now);
        if self.times.len() >= LIMIT {
            return AfterCrash::Disable;
        }
        let cap = base.saturating_mul(CAP_FACTOR);
        if uptime > cap {
            self.streak = 0;
        }
        self.streak += 1;
        let delay = base.saturating_mul(1 << (self.streak - 1).min(16)).min(cap);
        AfterCrash::RestartIn(delay)
    }

    pub(crate) fn count(&self) -> usize {
        self.times.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHORT: Duration = Duration::from_millis(10);

    #[test]
    fn backoff_doubles_from_one_second_and_the_fifth_crash_disables() {
        let start = Instant::now();
        let mut crashes = Crashes::default();
        let delays: Vec<_> = (0..4)
            .map(|i| crashes.record(start + Duration::from_secs(i), SHORT, BASE))
            .collect();
        assert_eq!(
            delays,
            [1, 2, 4, 8].map(|s| AfterCrash::RestartIn(Duration::from_secs(s)))
        );
        assert_eq!(
            crashes.record(start + Duration::from_secs(5), SHORT, BASE),
            AfterCrash::Disable
        );
    }

    #[test]
    fn spaced_crashes_keep_doubling_to_the_sixty_second_cap() {
        let start = Instant::now();
        let mut crashes = Crashes::default();
        // One crash every four minutes never puts five in a ten-minute window.
        let delays: Vec<_> = (0..9)
            .map(|i| crashes.record(start + Duration::from_secs(240 * i), SHORT, BASE))
            .collect();
        let expected = [1, 2, 4, 8, 16, 32, 60, 60, 60];
        assert_eq!(
            delays,
            expected.map(|s| AfterCrash::RestartIn(Duration::from_secs(s)))
        );
        assert!(crashes.count() < LIMIT);
        // A host that stayed up past the cap starts the streak again.
        let later = start + Duration::from_secs(240 * 9);
        assert_eq!(
            crashes.record(later, Duration::from_secs(61), BASE),
            AfterCrash::RestartIn(BASE)
        );
    }
}
