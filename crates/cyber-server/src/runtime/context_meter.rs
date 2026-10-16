//! Context window meter (`observability-costs` → Context window meter): the last
//! request's prompt tokens against the model's window and the auto-compaction point.

use serde::Serialize;

use super::compaction::{CompactionConfig, compact_at};

#[derive(Debug, Clone, PartialEq, Serialize, schemars::JsonSchema)]
pub struct ContextMeter {
    /// Prompt tokens of the last request: input + cache read + cache write.
    pub used_tokens: u64,
    /// The model's context window; null when the catalog does not know it.
    pub window: Option<u64>,
    /// The prompt size above which auto-compaction runs; null when the window is
    /// unknown or auto-compaction is off.
    pub compact_at: Option<u64>,
    /// `used_tokens / window`; null when the window is unknown.
    pub utilization: Option<f64>,
}

impl ContextMeter {
    /// `window` is 0 when unknown, as the catalog reports it.
    pub(crate) fn new(
        used_tokens: u64,
        window: u64,
        max_output: u64,
        config: &CompactionConfig,
    ) -> Self {
        let known = (window > 0).then_some(window);
        Self {
            used_tokens,
            window: known,
            compact_at: if config.auto {
                compact_at(window, max_output, config.buffer)
            } else {
                None
            },
            utilization: known.map(|window| used_tokens as f64 / window as f64),
        }
    }
}

impl super::Runtime {
    /// The meter for a Session's last finished step; `None` before its first step and
    /// after a compaction until the next one.
    pub fn context_meter(&self, state: &super::SessionState) -> Option<ContextMeter> {
        let last = state.last_step.as_ref()?;
        let resolved = self
            .inner
            .resolve(&format!("{}/{}", last.provider, last.model))
            .ok();
        let (window, max_output) = resolved.map_or((0, 0), |model| {
            let max_output = model.template.max_output_tokens.unwrap_or(0);
            (model.context_limit, u64::from(max_output))
        });
        Some(ContextMeter::new(
            last.usage.context_tokens(),
            window,
            max_output,
            &self.inner.options.compaction,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_windows_stay_unknown_and_disabled_compaction_has_no_point() {
        let config = CompactionConfig::default();
        let meter = ContextMeter::new(29_470, 131_072, 8_192, &config);
        assert_eq!(meter.window, Some(131_072));
        assert_eq!(
            meter.compact_at,
            Some(131_072 - 8_192.max(config.buffer)),
            "the same point `needs` compacts at"
        );
        assert!((meter.utilization.unwrap() - 29_470.0 / 131_072.0).abs() < 1e-12);

        let unknown = ContextMeter::new(500, 0, 0, &config);
        assert_eq!(
            (unknown.window, unknown.compact_at, unknown.utilization),
            (None, None, None)
        );

        let off = CompactionConfig {
            auto: false,
            ..CompactionConfig::default()
        };
        assert_eq!(ContextMeter::new(1, 100_000, 0, &off).compact_at, None);
    }
}
