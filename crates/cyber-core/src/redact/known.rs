//! Exact values of secrets the server already knows: environment variables and config
//! entries whose names look secret (provider keys, MCP server environment and headers).
//! The values are only matched, never logged.

use serde_json::Value;

/// Name fragments, without `_` and `-`, that make a variable or key secret.
const SECRET_NAMES: &[&str] = &[
    "TOKEN",
    "SECRET",
    "PASSWORD",
    "PASSWD",
    "APIKEY",
    "ACCESSKEY",
    "PRIVATEKEY",
    "CREDENTIAL",
    "AUTHKEY",
    "SESSIONKEY",
    "AUTHORIZATION",
    "COOKIE",
];

/// Suffixes of names that hold where a secret is, or a property of it, not the secret.
const NOT_SECRET_SUFFIXES: &[&str] = &[
    "PATH", "FILE", "DIR", "URL", "URI", "SOCK", "ENDPOINT", "NAME", "HOST", "PORT", "TYPE", "TTL",
    "EXPIRY", "EXPIRES", "COUNT", "LIMIT", "MODE", "HEADER",
];

/// The shortest exact value matched: shorter ones would hide ordinary words.
pub const MIN_KNOWN_LEN: usize = 8;

/// Whether an environment variable, config key or header name names a secret, such as
/// `OPENAI_API_KEY`, `apiKey`, `DB_PASSWORD` or `Authorization`.
pub fn secret_name(name: &str) -> bool {
    let folded: String = name
        .chars()
        .filter(|c| !matches!(c, '_' | '-' | '.'))
        .map(|c| c.to_ascii_uppercase())
        .collect();
    SECRET_NAMES.iter().any(|word| folded.contains(word))
        && !NOT_SECRET_SUFFIXES
            .iter()
            .any(|suffix| folded.ends_with(suffix))
}

/// Whether a value of a secret-named variable is worth matching exactly: long enough,
/// and not a flag, number or path.
fn plausible(value: &str) -> bool {
    let value = value.trim();
    value.chars().count() >= MIN_KNOWN_LEN
        && !value.bytes().all(|b| b.is_ascii_digit())
        && !matches!(
            value.to_ascii_lowercase().as_str(),
            "true" | "false" | "disabled" | "enabled"
        )
        && !value.starts_with(['/', '~', '.', '$', '{'])
        && !(value.len() > 2 && value.as_bytes()[1] == b':' && value.as_bytes()[2] == b'\\')
}

/// The values of secret-named environment variables.
pub fn env_secrets(vars: impl IntoIterator<Item = (String, String)>) -> Vec<String> {
    let mut values: Vec<String> = vars
        .into_iter()
        .filter(|(name, value)| secret_name(name) && plausible(value))
        .flat_map(|(_, value)| with_unscheme(value))
        .collect();
    values.sort();
    values.dedup();
    values
}

/// The string values under secret-named keys of a resolved config document, such as
/// `providers.<p>.api_key`, `mcp.<s>.environment.GITHUB_TOKEN` or an `Authorization`
/// header. `tool_output` (the redaction settings themselves) is skipped.
pub fn config_secrets(config: &Value) -> Vec<String> {
    let mut values = Vec::new();
    if let Some(map) = config.as_object() {
        for (key, child) in map.iter().filter(|(key, _)| *key != "tool_output") {
            walk(key, child, false, &mut values);
        }
    }
    values.sort();
    values.dedup();
    values
}

fn walk(key: &str, value: &Value, inherited: bool, out: &mut Vec<String>) {
    let secret = inherited || secret_name(key);
    match value {
        Value::String(text) if secret && plausible(text) => {
            out.extend(with_unscheme(text.clone()));
        }
        Value::Object(map) => {
            for (child_key, child) in map {
                walk(child_key, child, secret, out);
            }
        }
        Value::Array(items) => {
            for item in items {
                walk(key, item, secret, out);
            }
        }
        _ => {}
    }
}

/// `value`, and for an `Authorization`-style `Bearer <token>` also the bare token, which
/// is what usually appears in output.
fn with_unscheme(value: String) -> Vec<String> {
    let bare = value
        .split_once(' ')
        .filter(|(scheme, _)| {
            matches!(
                scheme.to_ascii_lowercase().as_str(),
                "bearer" | "basic" | "token" | "digest"
            )
        })
        .map(|(_, rest)| rest.trim().to_string())
        .filter(|rest| plausible(rest));
    std::iter::once(value).chain(bare).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn secret_names_and_their_exceptions() {
        for name in [
            "OPENAI_API_KEY",
            "apiKey",
            "DB_PASSWORD",
            "Authorization",
            "x-api-key",
            "AWS_SECRET_ACCESS_KEY",
            "_authToken",
            "GITHUB_PAT_TOKEN",
        ] {
            assert!(secret_name(name), "{name}");
        }
        for name in [
            "PWD",
            "PATH",
            "SSH_AUTH_SOCK",
            "GIT_ASKPASS",
            "TOKEN_URL",
            "GOOGLE_APPLICATION_CREDENTIALS_FILE",
            "KEYBOARD",
            "TERM_SESSION_ID",
        ] {
            assert!(!secret_name(name), "{name}");
        }
    }

    #[test]
    fn env_values_need_a_secret_name_and_a_plausible_value() {
        let vars = [
            ("DEPLOY_TOKEN", "s3cr3t-deploy-value"),
            ("HOME", "/Users/someone"),
            ("TOKEN_TTL", "86400000"),
            ("API_TOKEN", "short"),
            ("CACHE_TOKEN", "/var/cache/token"),
            ("RETRY_TOKENS", "12345678"),
        ]
        .map(|(name, value)| (name.to_string(), value.to_string()));
        assert_eq!(env_secrets(vars), ["s3cr3t-deploy-value"]);
    }

    #[test]
    fn config_values_under_secret_keys_and_bearer_tokens() {
        let config = json!({
            "providers": {"corp": {"api_key": "corp-key-0123456789", "url": "https://x"}},
            "mcp": {"gh": {
                "environment": {"GITHUB_TOKEN": "ghp_fromconfig0123", "LOG_LEVEL": "debugging"},
                "headers": {"Authorization": "Bearer abc.def-ghi-12345"},
            }},
            "tool_output": {"redact": {"allow": ["literal-allowed-token-value"]}},
            "compaction": {"keep": {"tokens": 20000}},
        });
        let mut found = config_secrets(&config);
        found.sort();
        assert_eq!(
            found,
            [
                "Bearer abc.def-ghi-12345",
                "abc.def-ghi-12345",
                "corp-key-0123456789",
                "ghp_fromconfig0123"
            ]
        );
    }
}
