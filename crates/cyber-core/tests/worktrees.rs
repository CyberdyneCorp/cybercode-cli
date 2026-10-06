use cyber_core::worktrees::{Cleanup, Name, Settings};
use serde_json::json;

#[test]
fn settings_resolve_defaults_and_explicit_overrides() {
    let defaults = Settings::from_config(&json!({})).unwrap();
    assert_eq!(defaults.base, "HEAD");
    assert_eq!(defaults.branch_prefix, "cyber/");
    assert_eq!(defaults.cleanup, Cleanup::Auto);
    assert_eq!(defaults.root, None);
    assert!(defaults.setup.is_empty());
    let settings = Settings::from_config(&json!({"worktrees": {
        "root": "../isolated", "base": "main", "branch_prefix": "task/",
        "setup": ["npm ci"], "cleanup": "keep"
    }}))
    .unwrap();
    assert_eq!(settings.base, "main");
    assert_eq!(settings.branch_prefix, "task/");
    assert_eq!(settings.root.unwrap().to_str(), Some("../isolated"));
    assert_eq!(settings.setup, ["npm ci"]);
    assert_eq!(settings.cleanup, Cleanup::Keep);
}

#[test]
fn malformed_settings_fail_before_lifecycle_operations() {
    for value in [
        json!(false),
        json!(null),
        json!({"cleanup": "delete"}),
        json!({"setup": [42]}),
        json!({"setup": [" "]}),
        json!({"root": ""}),
        json!({"base": " "}),
        json!({"branch_prefix": 42}),
    ] {
        assert!(
            Settings::from_config(&json!({"worktrees": value}))
                .unwrap_err()
                .starts_with("worktrees")
        );
    }
}

#[test]
fn names_accept_syntax_boundaries_and_reject_paths() {
    for value in ["a", "0", "test.branch-1", &"a".repeat(63)] {
        assert_eq!(Name::parse(value).unwrap().as_str(), value);
    }
    for value in [
        "",
        ".",
        "..",
        "../outside",
        "/absolute",
        "a/b",
        "a\\b",
        "-option",
        ".hidden",
        "UPPER",
        "name space",
        "café",
        "a\0b",
        &"a".repeat(64),
    ] {
        assert!(Name::parse(value).is_err(), "accepted {value:?}");
    }
}

#[test]
fn generated_names_have_documented_shape() {
    let name = Name::generate();
    let parts: Vec<_> = name.as_str().split('-').collect();
    assert_eq!(parts.len(), 3);
    assert!(parts[0].bytes().all(|byte| byte.is_ascii_lowercase()));
    assert!(parts[1].bytes().all(|byte| byte.is_ascii_lowercase()));
    assert_eq!(parts[2].len(), 4);
    assert!(parts[2].bytes().all(|byte| byte.is_ascii_hexdigit()));
    assert_eq!(Name::parse(name.as_str()).unwrap(), name);
}
