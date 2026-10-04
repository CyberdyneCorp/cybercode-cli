//! Credential masking (`sandbox` → Credential masking).

/// Remove credential-looking variables unless explicitly allowed. `extra` adds exact names,
/// such as provider credential variables from the catalog.
pub fn mask_env(
    vars: impl Iterator<Item = (String, String)>,
    allow: &[String],
    extra: &[String],
) -> Vec<(String, String)> {
    vars.filter(|(name, _)| allow.contains(name) || !(is_secret(name) || extra.contains(name)))
        .collect()
}

fn is_secret(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    upper.ends_with("_TOKEN")
        || upper.ends_with("_KEY")
        || upper.ends_with("_SECRET")
        || upper.contains("PASSWORD")
        || upper.starts_with("AWS_")
        || upper == "GITHUB_TOKEN"
        || upper == "CYBER_AUTH_CONTENT"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_are_removed_unless_allowed() {
        let vars = [
            "PATH",
            "OPENAI_API_KEY",
            "GH_TOKEN",
            "DB_PASSWORD",
            "AWS_REGION",
            "NPM_TOKEN",
            "HOME",
            "CUSTOM_CRED",
        ]
        .map(|n| (n.to_string(), "x".to_string()));
        let kept: Vec<String> = mask_env(
            vars.into_iter(),
            &["NPM_TOKEN".into()],
            &["CUSTOM_CRED".into()],
        )
        .into_iter()
        .map(|(n, _)| n)
        .collect();
        assert_eq!(kept, vec!["PATH", "NPM_TOKEN", "HOME"]);
    }
}
