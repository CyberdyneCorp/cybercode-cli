//! Credential masking (`sandbox` → Credential masking).
#![cfg_attr(windows, allow(unsafe_code))]

/// Remove credential-looking variables unless explicitly allowed. `extra` adds exact names,
/// such as provider credential variables from the catalog. Exact names follow
/// the platform's case rules; no glob or Unicode normalization is applied here.
pub fn mask_env(
    vars: impl Iterator<Item = (String, String)>,
    allow: &[String],
    extra: &[String],
) -> Vec<(String, String)> {
    vars.filter(|(name, _)| {
        contains_name(allow, name, false) || !(is_secret(name) || contains_name(extra, name, true))
    })
    .collect()
}

fn contains_name(names: &[String], name: &str, on_error: bool) -> bool {
    names
        .iter()
        .any(|candidate| equal_name(name, candidate).unwrap_or(on_error))
}

#[cfg(not(windows))]
fn equal_name(name: &str, candidate: &str) -> Option<bool> {
    Some(name == candidate)
}

#[cfg(windows)]
fn equal_name(name: &str, candidate: &str) -> Option<bool> {
    use windows_sys::Win32::Globalization::{CSTR_EQUAL, CompareStringOrdinal};
    if name.is_ascii() && candidate.is_ascii() {
        return Some(name.eq_ignore_ascii_case(candidate));
    }
    let name: Vec<u16> = name.encode_utf16().collect();
    let candidate: Vec<u16> = candidate.encode_utf16().collect();
    let length = i32::try_from(name.len()).ok()?;
    let candidate_length = i32::try_from(candidate.len()).ok()?;
    // Explicit UTF-16 lengths keep both live buffers valid through the call.
    match unsafe {
        CompareStringOrdinal(
            name.as_ptr(),
            length,
            candidate.as_ptr(),
            candidate_length,
            1,
        )
    } {
        0 => None,
        result => Some(result == CSTR_EQUAL),
    }
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

    #[cfg(windows)]
    #[test]
    fn windows_catalog_credentials_are_case_insensitive() {
        let vars = [
            "ExampleCredential",
            "examplecredential",
            "ExampleåCredential",
            "PATH",
            "ExampleCredentialExtra",
        ]
        .map(|name| (name.to_owned(), "fixture".into()));
        let kept = mask_env(
            vars.into_iter(),
            &[],
            &["EXAMPLECREDENTIAL".into(), "EXAMPLEÅCREDENTIAL".into()],
        );
        assert_eq!(
            kept.iter()
                .map(|(name, _)| name.as_str())
                .collect::<Vec<_>>(),
            ["PATH", "ExampleCredentialExtra"]
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_exceptions_follow_case_but_not_unicode_normalization() {
        let vars = ["Npm_Token", "Credå_SECRET", "Creda\u{30a}_SECRET"]
            .map(|name| (name.to_owned(), "fixture".into()));
        let kept = mask_env(
            vars.into_iter(),
            &["NPM_TOKEN".into(), "CREDÅ_SECRET".into()],
            &[],
        );
        assert_eq!(
            kept.iter()
                .map(|(name, _)| name.as_str())
                .collect::<Vec<_>>(),
            ["Npm_Token", "Credå_SECRET"]
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn unix_catalog_and_exception_names_remain_case_sensitive() {
        let vars = [
            "ExampleCredential",
            "EXAMPLECREDENTIAL",
            "Npm_Token",
            "NPM_TOKEN",
        ]
        .map(|name| (name.to_owned(), "fixture".into()));
        let kept = mask_env(
            vars.into_iter(),
            &["NPM_TOKEN".into()],
            &["EXAMPLECREDENTIAL".into()],
        );
        assert_eq!(
            kept.iter()
                .map(|(name, _)| name.as_str())
                .collect::<Vec<_>>(),
            ["ExampleCredential", "NPM_TOKEN"]
        );
    }
}
