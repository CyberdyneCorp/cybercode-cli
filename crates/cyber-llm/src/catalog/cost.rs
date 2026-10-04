//! Per-step cost (`provider-catalog` → Cost accounting).

use super::{Cost, CostTier};
use crate::types::Usage;

/// USD for one step, or `None` when the model is unpriced. Cache reads and writes fall back
/// to the input price, reasoning to the output price. A tier applies when the prompt
/// (input plus cache) exceeds its threshold.
pub fn compute(cost: Option<&Cost>, usage: &Usage) -> Option<f64> {
    let cost = cost?;
    let prompt = usage.context_tokens();
    let tier: Option<&CostTier> = cost.tiers.iter().rfind(|t| prompt > t.above_input_tokens);
    let input = tier.map_or(cost.input, |t| t.input);
    let output = tier.map_or(cost.output, |t| t.output);
    let cache_read = tier
        .and_then(|t| t.cache_read)
        .or(cost.cache_read)
        .unwrap_or(input);
    let cache_write = tier
        .and_then(|t| t.cache_write)
        .or(cost.cache_write)
        .unwrap_or(input);
    let reasoning = cost.reasoning.unwrap_or(output);
    let per_million = |tokens: u64, price: f64| tokens as f64 * price / 1_000_000.0;
    Some(
        per_million(usage.input, input)
            + per_million(usage.output, output)
            + per_million(usage.reasoning, reasoning)
            + per_million(usage.cache_read, cache_read)
            + per_million(usage.cache_write, cache_write),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn cached_tokens_priced_separately() {
        let cost = Cost {
            input: 3.0,
            output: 15.0,
            cache_read: Some(0.3),
            ..Cost::default()
        };
        let usage = Usage {
            input: 2_000,
            cache_read: 8_000,
            output: 100,
            ..Usage::default()
        };
        let expected = 2_000.0 * 3.0 / 1e6 + 8_000.0 * 0.3 / 1e6 + 100.0 * 15.0 / 1e6;
        assert!(close(compute(Some(&cost), &usage).unwrap(), expected));
    }

    #[test]
    fn unpriced_is_none_and_explicit_zero_is_zero() {
        assert_eq!(
            compute(
                None,
                &Usage {
                    input: 10,
                    ..Usage::default()
                }
            ),
            None
        );
        assert_eq!(
            compute(
                Some(&Cost::default()),
                &Usage {
                    input: 10,
                    ..Usage::default()
                }
            ),
            Some(0.0)
        );
    }

    #[test]
    fn context_tier_applies_above_threshold() {
        let cost = Cost {
            input: 2.0,
            output: 10.0,
            tiers: vec![CostTier {
                above_input_tokens: 200_000,
                input: 4.0,
                output: 18.0,
                ..CostTier::default()
            }],
            ..Cost::default()
        };
        let small = compute(
            Some(&cost),
            &Usage {
                input: 100_000,
                ..Usage::default()
            },
        )
        .unwrap();
        let large = compute(
            Some(&cost),
            &Usage {
                input: 300_000,
                ..Usage::default()
            },
        )
        .unwrap();
        assert!(close(small, 0.2));
        assert!(close(large, 1.2));
    }
}
