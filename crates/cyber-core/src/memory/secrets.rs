use super::MemoryError;
use std::collections::BTreeMap;

/// Refuse credential-shaped text (the shared detectors of [`crate::redact`]) and text
/// that only looks random (high entropy), which memory should not keep either.
pub(super) fn check(text: &str) -> Result<(), MemoryError> {
    if crate::redact::contains_credential(text)
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
    redact_credentials_with(text, "[REDACTED]")
}

/// [`redact_credentials`] with a caller-chosen `marker`, such as `***` for telemetry.
pub fn redact_credentials_with(text: &str, marker: &str) -> String {
    crate::redact::redact_credentials_with(text, marker)
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
    fn the_marker_is_the_callers_and_never_expanded() {
        let text = "key sk-abcdefghijkl and ghp_abcdefghijkl";
        assert_eq!(
            super::redact_credentials_with(text, "***"),
            "key *** and ***"
        );
        assert_eq!(
            super::redact_credentials_with("sk-abcdefghijkl", "$0"),
            "$0"
        );
    }

    #[test]
    fn github_and_copilot_tokens_are_redacted() {
        let copilot = "tid=0123456789abcdef;exp=4000000000;sku=copilot_pro;8kp=1:ffee";
        for token in ["gho_abcdefgh12345678", "ghu_abcdefgh12345678", copilot] {
            let redacted = super::redact_credentials(&format!("using {token} now"));
            assert_eq!(redacted, "using [REDACTED] now", "{token}");
            assert!(super::check(token).is_err(), "{token}");
        }
        assert_eq!(super::redact_credentials("tid=12;"), "tid=12;");
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
