use super::MemoryError;
use regex::Regex;
use std::collections::BTreeMap;
use std::sync::LazyLock;

const CREDENTIAL_PATTERNS: &[&str] = &[
    r#"(?i)\b(?:[a-z0-9_-]*(?:api[_-]?key|password|passwd|token|secret)|authorization)["']?\s*[:=]\s*\S+"#,
    r"(?i)\bsk-(?:proj-|ant-)?[a-z0-9_-]{8,}",
    r"(?i)\b(?:gh[pousr]_[a-z0-9]{8,}|github_pat_[a-z0-9_]{8,}|AKIA[0-9A-Z]{16})",
    r"-----BEGIN (?:[A-Z ]+ )?PRIVATE KEY-----",
];
static CREDENTIALS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    CREDENTIAL_PATTERNS
        .iter()
        .map(|pattern| Regex::new(pattern).expect("static memory credential pattern"))
        .collect()
});

pub(super) fn check(text: &str) -> Result<(), MemoryError> {
    if CREDENTIALS.iter().any(|pattern| pattern.is_match(text))
        || text
            .split(|c: char| !c.is_alphanumeric() && !matches!(c, '_' | '-' | '+' | '/' | '='))
            .any(high_entropy)
    {
        return Err(MemoryError::Secret);
    }
    Ok(())
}

fn high_entropy(token: &str) -> bool {
    let total = token.chars().count();
    if total <= 32 {
        return false;
    }
    let mut counts = BTreeMap::<char, usize>::new();
    for character in token.chars() {
        *counts.entry(character).or_default() += 1;
    }
    let total = total as f64;
    let entropy: f64 = counts
        .into_values()
        .map(|count| {
            let probability = count as f64 / total;
            -probability * probability.log2()
        })
        .sum();
    entropy >= 4.0
}
