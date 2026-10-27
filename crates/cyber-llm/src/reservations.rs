//! Credential-free request bounds for reservation admission. These are conservative
//! scheduling envelopes against declared protocol/model limits, not billing guarantees.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{LlmRequest, Usage, catalog::Cost};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestBound {
    /// Per-class maxima; input/cache and output/reasoning maxima are correlated.
    pub classes: Usage,
    pub total_tokens: u64,
    pub input_group: u64,
    pub output_group: u64,
    pub source: String,
    pub normalized_sha256: String,
}

pub struct PreparedReservation {
    /// Only approved reservation-mode shaping is applied; adapters send this request.
    pub request: LlmRequest,
    pub bound: RequestBound,
}

#[derive(Debug, Clone, thiserror::Error)]
#[error("BudgetBoundUnavailableError: {0}")]
pub struct BoundUnavailable(pub String);

pub fn unavailable(reason: &str) -> BoundUnavailable {
    BoundUnavailable(reason.into())
}

impl RequestBound {
    pub fn from_usage(
        classes: Usage,
        source: &str,
        normalized: &serde_json::Value,
    ) -> Result<Self, BoundUnavailable> {
        let input_group = classes
            .input
            .checked_add(classes.cache_read)
            .and_then(|v| v.checked_add(classes.cache_write))
            .ok_or_else(|| unavailable("input bound overflow"))?;
        let output_group = classes
            .output
            .checked_add(classes.reasoning)
            .ok_or_else(|| unavailable("output bound overflow"))?;
        let total_tokens = input_group
            .checked_add(output_group)
            .ok_or_else(|| unavailable("total bound overflow"))?;
        Ok(Self {
            classes,
            total_tokens,
            input_group,
            output_group,
            source: source.into(),
            normalized_sha256: digest(normalized)?,
        })
    }

    /// Include every price tier and premium that may apply. Input/cache and
    /// output/reasoning classes partition their respective group allowances.
    pub fn cost_microusd(&self, cost: &Cost) -> Result<u64, BoundUnavailable> {
        let mut input = vec![cost.input];
        let mut output = vec![cost.output, cost.reasoning.unwrap_or(cost.output)];
        if self.classes.cache_read > 0 {
            input.push(cost.cache_read.unwrap_or(cost.input));
        }
        if self.classes.cache_write > 0 {
            input.push(cost.cache_write.unwrap_or(cost.input));
        }
        for tier in &cost.tiers {
            input.push(tier.input);
            output.push(tier.output);
            if self.classes.cache_read > 0 {
                input.push(tier.cache_read.or(cost.cache_read).unwrap_or(tier.input));
            }
            if self.classes.cache_write > 0 {
                input.push(tier.cache_write.or(cost.cache_write).unwrap_or(tier.input));
            }
        }
        let input = maximum(input)?;
        let output = maximum(output)?;
        // USD/million tokens is numerically equal to microUSD/token.
        let amount = self.input_group as f64 * input + self.output_group as f64 * output;
        if !amount.is_finite() || amount < 0.0 || amount >= u64::MAX as f64 {
            return Err(unavailable("priced request bound overflow"));
        }
        Ok(amount.ceil() as u64)
    }
}

pub(crate) fn digest(value: &serde_json::Value) -> Result<String, BoundUnavailable> {
    let bytes = serde_json::to_vec(value)
        .map_err(|_| unavailable("normalized request cannot be fingerprinted"))?;
    Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
}

fn maximum(prices: Vec<f64>) -> Result<f64, BoundUnavailable> {
    if prices.iter().any(|v| !v.is_finite() || *v < 0.0) {
        return Err(unavailable("catalog price is not finite and nonnegative"));
    }
    Ok(prices.into_iter().fold(0.0, f64::max))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::CostTier;

    #[test]
    fn price_bounds_include_tiers_reasoning_and_cache_write_premiums() {
        let bound = RequestBound::from_usage(
            Usage {
                input: 10,
                cache_write: 20,
                output: 5,
                reasoning: 5,
                ..Usage::default()
            },
            "scripted",
            &serde_json::json!({}),
        )
        .unwrap();
        let cost = Cost {
            input: 1.0,
            output: 2.0,
            reasoning: Some(7.0),
            cache_write: Some(3.0),
            tiers: vec![CostTier {
                above_input_tokens: 100,
                input: 4.0,
                output: 6.0,
                cache_write: Some(9.0),
                ..CostTier::default()
            }],
            ..Cost::default()
        };
        assert_eq!(bound.cost_microusd(&cost).unwrap(), 340);
        assert!(
            bound
                .cost_microusd(&Cost {
                    input: f64::NAN,
                    ..Cost::default()
                })
                .is_err()
        );
    }
}
