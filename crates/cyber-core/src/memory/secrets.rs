use super::MemoryError;
use regex::Regex;
use std::collections::BTreeMap;
use std::sync::LazyLock;

const CREDENTIAL_PATTERNS: &[&str] = &[
    r#"(?i)\b(?:[a-z0-9_-]*(?:api[_-]?key|password|passwd|token|secret)|authorization)["']?\s*[:=]\s*\S+"#,
    r"(?i)\bsk-(?:proj-|ant-)?[a-z0-9_-]{8,}",
    r"(?i)\b(?:gh[pousr]_[a-z0-9]{8,}|github_pat_[a-z0-9_]{8,}|AKIA[0-9A-Z]{16})",
    // Peer grant tokens (`cyp_pgr…_<secret>`) of `cross-session-messaging` → Peer grants.
    r"\bcyp_[A-Za-z0-9_-]{8,}",
    r"-----BEGIN (?:[A-Z ]+ )?PRIVATE KEY-----",
];
static CREDENTIALS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    CREDENTIAL_PATTERNS
        .iter()
        .map(|pattern| Regex::new(pattern).expect("static memory credential pattern"))
        .collect()
});

/// Assignments whose key only contains a credential word, as in `~/.aws/credentials`.
static REDACTED_ASSIGNMENTS: LazyLock<[Regex; 1]> = LazyLock::new(|| {
    [Regex::new(
        r#"(?i)\b[a-z0-9_-]*(?:secret|access[_-]?key|session[_-]?token)[a-z0-9_-]*["']?\s*[:=]\s*\S+"#,
    )
    .expect("static redaction pattern")]
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

/// Replace each credential-shaped match with `[REDACTED]`; text that only looks random
/// (high entropy) is kept, since command output such as hashes is legitimately so.
pub fn redact_credentials(text: &str) -> String {
    CREDENTIALS
        .iter()
        .chain(REDACTED_ASSIGNMENTS.iter())
        .fold(text.to_string(), |text, pattern| {
            pattern.replace_all(&text, "[REDACTED]").into_owned()
        })
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

#[cfg(test)]
mod tests {
    #[test]
    fn credential_shaped_text_is_redacted_and_the_rest_kept() {
        let text = "ok\naws_secret_access_key = abc123\nkey AKIAABCDEFGHIJKLMNOP done\n";
        let redacted = super::redact_credentials(text);
        assert!(!redacted.contains("abc123") && !redacted.contains("AKIAABCDEFGHIJKLMNOP"));
        assert!(redacted.starts_with("ok\n") && redacted.contains("done"));
        assert_eq!(super::redact_credentials("plain hi"), "plain hi");
    }

    #[test]
    fn peer_grant_tokens_are_redacted_and_refused() {
        let token = "cyp_pgr01HZX0ABCDEF_Qm9vbGVhbi1zZWNyZXQtdmFsdWUtNDMtY2hhcnMtbG9uZw";
        let redacted = super::redact_credentials(&format!("Authorization: Bearer {token}"));
        assert!(!redacted.contains(token), "{redacted}");
        let redacted = super::redact_credentials(&format!("code t={token} ok"));
        assert_eq!(redacted, "code t=[REDACTED] ok");
        assert!(super::check(&format!("paired with {token}")).is_err());
        assert_eq!(super::redact_credentials("cyp_ alone"), "cyp_ alone");
    }
}
