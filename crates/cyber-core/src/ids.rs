//! Prefixed, time-ordered identifiers such as `ses_01J…`.

/// Create a new identifier with the given prefix, for example `new_id("ses")`.
pub fn new_id(prefix: &str) -> String {
    format!("{prefix}_{}", ulid::Ulid::new())
}

/// Whether `id` is `<prefix>_<non-empty>`.
pub fn has_prefix(id: &str, prefix: &str) -> bool {
    id.strip_prefix(prefix)
        .and_then(|rest| rest.strip_prefix('_'))
        .is_some_and(|rest| !rest.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_prefixed_and_unique() {
        let a = new_id("ses");
        let b = new_id("ses");
        assert!(has_prefix(&a, "ses"));
        assert_ne!(a, b);
        assert!(!has_prefix("ses_", "ses"));
        assert!(!has_prefix("sesx_1", "ses"));
    }
}
