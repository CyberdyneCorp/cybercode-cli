//! Provider-reported usage windows (`observability-costs` → Provider-reported limits).
//!
//! The provider adapters record what each provider reported in its response headers;
//! this only reads those readings back. Nothing is derived from local usage, so a
//! provider that reports nothing is absent.

use serde::Serialize;

use super::Runtime;

pub use cyber_llm::limits::{LimitReading, UsageWindow, WindowPeriod};

/// The `GET /usage/limits` reply.
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct UsageLimits {
    /// The latest reading per provider and window, by provider then window.
    pub limits: Vec<LimitReading>,
}

impl Runtime {
    pub fn usage_limits(&self) -> UsageLimits {
        UsageLimits {
            limits: self.inner.resolver.limits(),
        }
    }
}
