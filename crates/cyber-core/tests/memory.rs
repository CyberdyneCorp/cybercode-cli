//! Shared memory format and admission boundaries, without filesystem authority.
use cyber_core::memory::{
    MemoryDocument, MemoryError, MemoryMetadata, MemorySettings, MemoryType, directory,
    index_snapshot, render_index, validate_name,
};
use serde_json::json;
use std::collections::HashMap;

fn document(name: &str, kind: MemoryType, body: &str) -> MemoryDocument {
    MemoryDocument::new(
        MemoryMetadata {
            name: name.into(),
            description: "Useful background context".into(),
            kind,
        },
        body.into(),
    )
    .unwrap()
}

#[test]
fn all_memory_types_round_trip_with_required_feedback_and_project_explanations() {
    for kind in [
        MemoryType::User,
        MemoryType::Feedback,
        MemoryType::Project,
        MemoryType::Reference,
    ] {
        let original = document(
            "test-db-policy",
            kind,
            "Use real database tests.\n\n**Why:** Queries need integration coverage.\n**How to apply:** When changing storage.",
        );
        let rendered = original.render_for_write().unwrap();
        assert_eq!(MemoryDocument::for_write(&rendered).unwrap(), original);
        assert_eq!(
            MemoryDocument::parse(&format!("\u{feff}{}", rendered.replace('\n', "\r\n"))).unwrap(),
            original
        );
    }
}

#[test]
fn malformed_missing_duplicate_and_multiline_frontmatter_is_refused_without_input_disclosure() {
    for yaml in [
        "name: valid\ndescription: private supplied text",
        "name: valid\ndescription: private supplied text\ntype: unknown",
        "name: valid\nname: duplicate\ndescription: private supplied text\ntype: user",
        "name: valid\ndescription: |\n  private supplied text\n  second line\ntype: user",
        "name: [private supplied text]\ndescription: useful\ntype: user",
        "name: ../private\ndescription: private supplied text\ntype: user",
    ] {
        let text = format!("---\n{yaml}\n---\nA fact");
        let error = MemoryDocument::parse(&text).unwrap_err().to_string();
        assert!(!error.contains("private supplied text"));
    }
    for text in [
        "no frontmatter",
        "---\nname: valid\n---invalid\nA fact",
        "---\nname: valid\ndescription: useful\ntype: user\n---\n",
    ] {
        assert!(MemoryDocument::parse(text).is_err());
    }
}

#[test]
fn feedback_and_project_notes_require_nonempty_explanation_lines() {
    for kind in [MemoryType::Feedback, MemoryType::Project] {
        for body in [
            "A fact",
            "**Why:** because",
            "**Why:**\n**How to apply:** next time",
            "**Why:** because\n**How to apply:**",
        ] {
            assert!(
                MemoryDocument::new(
                    MemoryMetadata {
                        name: "rule".into(),
                        description: "Useful fact".into(),
                        kind
                    },
                    body.into()
                )
                .is_err()
            );
        }
    }
    assert!(
        MemoryDocument::new(
            MemoryMetadata {
                name: "rule".into(),
                description: "Useful fact".into(),
                kind: MemoryType::User
            },
            "Ordinary preference".into()
        )
        .is_ok()
    );
}

#[test]
fn slug_and_project_identity_validation_prevent_escaping_memory_components() {
    for name in [
        "",
        "UPPER",
        "../secret",
        "a/b",
        "a\\b",
        "a.md",
        "a--b",
        "-a",
        "a-",
        "a b",
        "é",
    ] {
        assert!(validate_name(name).is_err());
    }
    for name in ["a", "test-db-policy", "123-rule"] {
        validate_name(name).unwrap();
    }
    let base = std::path::Path::new("/data");
    assert_eq!(
        directory(base, "prj_7f3a").unwrap(),
        base.join("memory/prj_7f3a")
    );
    assert_eq!(
        directory(base, "global").unwrap(),
        base.join("memory/global")
    );
    for id in [
        "prj_",
        "prj_../escape",
        "prj_a/b",
        "prj_a\\b",
        "other",
        "..",
        "global/escape",
    ] {
        assert!(directory(base, id).is_err());
    }
}

#[test]
fn write_admission_rejects_credentials_keys_and_high_entropy_in_body_or_frontmatter() {
    for secret in [
        "OPENAI_API_KEY=sk-...",
        "password: short",
        r#"{"password":"short"}"#,
        "'api_key': 'short'",
        "Authorization: Bearer abc",
        "access_token=abc",
        "sk-proj-aaaaaaaaaaaa",
        "ghp_aaaaaaaaaaaa",
        "-----BEGIN RSA PRIVATE KEY-----",
        "-----BEGIN PRIVATE KEY-----",
        "0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ",
        "αβγδεζηθικλμνξοπρστυφχψω0123456789",
    ] {
        let text = format!("---\nname: note\ndescription: useful\ntype: user\n---\n{secret}");
        assert_eq!(MemoryDocument::for_write(&text), Err(MemoryError::Secret));
        assert_eq!(
            MemoryDocument::parse(&text).unwrap().render_for_write(),
            Err(MemoryError::Secret)
        );
        let text = format!("---\nname: note\ndescription: '{secret}'\ntype: user\n---\nA fact");
        assert_eq!(MemoryDocument::for_write(&text), Err(MemoryError::Secret));
    }
    let low_entropy = document(
        "rule",
        MemoryType::User,
        "A repeated marker aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa is ordinary text.",
    );
    assert!(low_entropy.render_for_write().is_ok());
    let short = document("rule", MemoryType::User, "0123456789abcdefghijklmnopqrstuv");
    assert!(short.render_for_write().is_ok());
}

#[test]
fn index_is_deterministic_escapes_description_and_refuses_duplicate_names() {
    let first = document("a-rule", MemoryType::User, "A fact");
    let mut last = document("z-rule", MemoryType::Reference, "A fact");
    last.metadata.description = "See [guide] <server> & details".into();
    let index = render_index([&last, &first]).unwrap();
    assert_eq!(
        index,
        "- [a-rule](a-rule.md) — Useful background context\n- [z-rule](z-rule.md) — See \\[guide\\] &lt;server&gt; &amp; details\n"
    );
    assert_eq!(render_index([&first, &last]).unwrap(), index);
    assert!(render_index([&first, &first]).is_err());
    assert_eq!(render_index([]).unwrap(), "");
    last.metadata.description = "password=private".into();
    assert_eq!(render_index([&last]), Err(MemoryError::Secret));
}

#[test]
fn index_line_limit_is_exact_and_empty_or_unterminated_inputs_remain_valid() {
    let lines = "line\n".repeat(200);
    assert_eq!(index_snapshot(&lines).text, lines);
    assert!(!index_snapshot(&lines).truncated);
    let snapshot = index_snapshot(&format!("{lines}later"));
    assert_eq!(
        snapshot.text,
        format!("{lines}[memory index truncated; read MEMORY.md for more]")
    );
    assert!(snapshot.truncated);
    assert_eq!(
        index_snapshot("fact without newline").text,
        "fact without newline"
    );
    assert_eq!(index_snapshot("").text, "");
}

#[test]
fn index_byte_limit_preserves_utf8_and_uses_whichever_bound_is_smaller() {
    let text = format!("{}éremaining", "a".repeat(24_999));
    let snapshot = index_snapshot(&text);
    assert!(snapshot.truncated);
    assert_eq!(
        snapshot.text,
        format!(
            "{}\n[memory index truncated; read MEMORY.md for more]",
            "a".repeat(24_999)
        )
    );
    let text = "é".repeat(12_500);
    assert_eq!(index_snapshot(&text).text, text);
    assert!(!index_snapshot(&text).truncated);
    let text = "é".repeat(12_501);
    assert!(index_snapshot(&text).text.starts_with(&"é".repeat(12_500)));
    assert!(index_snapshot(&text).truncated);
}

#[test]
fn settings_defaults_and_disable_override_enforce_read_only_intent() {
    let env = HashMap::<String, String>::new();
    let settings = MemorySettings::from_config(&json!({}), &env).unwrap();
    assert!(settings.enabled && settings.generate && settings.writable());
    let settings =
        MemorySettings::from_config(&json!({"memory":{"generate":false}}), &env).unwrap();
    assert!(settings.enabled && !settings.writable());
    for value in ["1", "TRUE", "yes", "on"] {
        let env = HashMap::from([("CYBER_DISABLE_MEMORY".into(), value.into())]);
        let settings =
            MemorySettings::from_config(&json!({"memory":{"enabled":true}}), &env).unwrap();
        assert!(!settings.enabled && !settings.writable());
    }
    for value in [
        json!(null),
        json!(false),
        json!({"enabled":"true"}),
        json!({"generate":null}),
    ] {
        assert!(MemorySettings::from_config(&json!({"memory":value}), &env).is_err());
    }
}

#[test]
fn actual_configuration_loading_validates_memory_without_creating_storage() {
    use cyber_core::config::{self, LoadRequest};
    use cyber_core::paths::Paths;
    let dir = tempfile::tempdir().unwrap();
    let env = HashMap::from([(
        "CYBER_HOME".into(),
        dir.path().join("cyber").display().to_string(),
    )]);
    let paths = Paths::resolve(&env, dir.path());
    paths.ensure().unwrap();
    let load = |value| {
        std::fs::write(paths.config.join("cyber.jsonc"), value).unwrap();
        config::load(&LoadRequest {
            location: dir.path(),
            paths: &paths,
            env: &env,
            home: dir.path(),
            profile: None,
            overrides: &[],
            flags: json!({}),
        })
    };
    for value in [
        json!(false),
        json!(null),
        json!({"enabled":"private supplied text"}),
        json!({"generate":0}),
    ] {
        let error = load(json!({"memory":value}).to_string())
            .unwrap_err()
            .to_string();
        assert!(error.contains("memory"));
        assert!(!error.contains("private supplied text"));
    }
    let resolved = load(json!({"memory":{"enabled":true,"generate":false}}).to_string()).unwrap();
    let settings = MemorySettings::from_config(&resolved.value, &env).unwrap();
    assert!(settings.enabled && !settings.generate && !settings.writable());
    assert_eq!(
        std::fs::read_dir(paths.data.join("memory"))
            .unwrap()
            .count(),
        0
    );
}
