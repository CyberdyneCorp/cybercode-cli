//! Environment access behind a trait so path and config resolution are testable.

use std::collections::HashMap;

/// A source of environment variables. Empty values are treated as unset.
pub trait EnvSource {
    fn get(&self, key: &str) -> Option<String>;

    /// `1`, `true`, `yes` and `on` (case-insensitive) are true.
    fn flag(&self, key: &str) -> bool {
        self.get(key)
            .is_some_and(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
    }
}

/// The real process environment.
pub struct ProcessEnv;

impl EnvSource for ProcessEnv {
    fn get(&self, key: &str) -> Option<String> {
        std::env::var(key).ok().filter(|v| !v.is_empty())
    }
}

impl EnvSource for HashMap<String, String> {
    fn get(&self, key: &str) -> Option<String> {
        HashMap::get(self, key).filter(|v| !v.is_empty()).cloned()
    }
}
