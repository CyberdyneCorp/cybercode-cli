use cyber_core::commands::{StaticCommand, configured};
use serde_json::json;

#[test]
fn markdown_frontmatter_preserves_body_and_exact_hint_spelling() {
    let parsed = cyber_core::commands::markdown("\u{feff}---\r\ndescription: Review code\r\nargument-hint: '<paths>'\r\n---\r\n\r\nReview $ARGUMENTS\r\n").unwrap();
    let command = StaticCommand::parse(&parsed.definition).unwrap();
    assert_eq!(command.expand("src"), "Review src\r\n");
    assert_eq!(command.argument_hint.as_deref(), Some("<paths>"));
    assert_eq!(parsed.sources["template"], "body");
    assert_eq!(parsed.sources["argument_hint"], "frontmatter:argument-hint");
    let plain = cyber_core::commands::markdown("Review code").unwrap();
    assert_eq!(plain.sources.len(), 1);
    assert_eq!(
        StaticCommand::parse(&plain.definition)
            .unwrap()
            .expand("carefully"),
        "Review code\n\ncarefully"
    );
    assert!(cyber_core::commands::markdown("---\n---\nReview").is_ok());
}

#[test]
fn markdown_unsupported_definitions_are_never_partially_accepted() {
    for document in [
        "---\nagent: reviewer\n---\nprivate-body",
        "---\nmodel: private-model\n---\nprivate-body",
        "---\nsubagent: true\n---\nprivate-body",
        "---\ndescription: [private-value]\n---\nprivate-body",
        "---\nargument_hint: one\nargument-hint: two\n---\nprivate-body",
        "---\ndescription: private-value\n---suffix\nprivate-body",
        "---\ndescription: private-value",
        "!`echo private-value`",
        "Review @private-file",
    ] {
        let error = cyber_core::commands::markdown(document).err().unwrap();
        assert!(!error.contains("private-"));
    }
    for document in ["!`echo value`", "Review @src/main.rs"] {
        let parsed = cyber_core::commands::markdown_template(document).unwrap();
        assert!(StaticCommand::parse_template(&parsed.definition).is_ok());
    }
    assert!(
        cyber_core::commands::markdown(&format!(
            "---\ndescription: {}\n---\nBody",
            "x".repeat(16385)
        ))
        .is_err()
    );
}

#[test]
fn nested_names_reserved_namespace_and_argument_expansion() {
    let commands = configured(&json!({"commands": {
        "db/migrate":{"template":"Migrate $1 in $2","description":"Migration","argument_hint":"<step> <targets>"},
        "goal":{"template":"Project goal $ARGUMENTS"},
        "plain":{"template":"Review code"}
    }}));
    assert!(commands.unavailable.is_empty());
    assert_eq!(
        commands.entries["db/migrate"].expand("42 \"auth module\" extra"),
        "Migrate 42 in auth module extra"
    );
    assert_eq!(
        commands.entries["project:goal"].expand("fix"),
        "Project goal fix"
    );
    assert_eq!(
        commands.entries["plain"].expand("carefully"),
        "Review code\n\ncarefully"
    );
    assert!(!commands.entries.contains_key("goal"));
}

#[test]
fn unsupported_execution_and_overrides_never_become_static_commands() {
    for value in [
        json!({"template":"!`touch sentinel`"}),
        json!({"template":" \n\t "}),
        json!({"template":"Review @src/main.rs"}),
        json!({"template":"{file:secret}"}),
        json!({"template":"{env:SECRET}"}),
        json!({"template":"review", "agent":"reviewer"}),
        json!({"template":"review", "model":"corp/model"}),
        json!({"template":"review", "subtask":true}),
        json!({"template":"x".repeat(65537)}),
        json!({"template":"private\0value"}),
    ] {
        let error = StaticCommand::parse(&value).unwrap_err();
        assert!(!error.contains("secret") && !error.contains("sentinel"));
    }
    for value in [
        json!({"template":"!`touch sentinel`"}),
        json!({"template":"Review @src/main.rs"}),
    ] {
        assert!(StaticCommand::parse_template(&value).is_ok());
    }
    assert!(StaticCommand::parse_template(&json!({"template":"{env:SECRET}"})).is_err());
    let commands =
        configured(&json!({"commands":{"../bad":{"template":"bad"},"safe":{"template":"OK"}}}));
    assert_eq!(commands.entries.len(), 1);
    assert_eq!(commands.unavailable, ["invalid command name"]);
}

#[test]
fn configured_templates_keep_injections_and_bind_trust_digests() {
    let commands =
        configured(&json!({"commands":{"status":{"template":"Status !`git status $1`"}}}));
    let expanded = commands.entries["status"].expand_template("-s");
    assert_eq!(expanded.text, "Status !`git status -s`");
    assert_eq!(expanded.injections[0].command, "git status -s");
    let digest = cyber_core::commands::template_digest("command", "status", "!`a`");
    assert!(digest.starts_with("sha256:") && digest.len() == 71);
    assert_ne!(
        digest,
        cyber_core::commands::template_digest("command", "other", "!`a`")
    );
    assert_ne!(
        digest,
        cyber_core::commands::template_digest("command", "status", "!`b`")
    );
}

/// `skills-commands` → Custom command definitions and Agent, model and subtask overrides:
/// the override fields parse from frontmatter and configuration, and invalid values are
/// refused instead of silently accepted.
#[test]
fn command_overrides_parse_and_invalid_values_are_refused() {
    let parsed = cyber_core::commands::markdown_template(
        "---\ndescription: Review\nagent: reviewer\nmodel: corp/review\nvariant: high\nsubtask: true\n---\nReview $ARGUMENTS\n",
    )
    .unwrap();
    assert_eq!(parsed.sources["agent"], "frontmatter:agent");
    assert_eq!(parsed.sources["subtask"], "frontmatter:subtask");
    let command = StaticCommand::parse_template(&parsed.definition).unwrap();
    assert_eq!(command.agent.as_deref(), Some("reviewer"));
    assert_eq!(command.model.as_deref(), Some("corp/review"));
    assert_eq!(command.variant.as_deref(), Some("high"));
    assert!(command.subtask);
    assert!(command.has_overrides());
    let configured = StaticCommand::parse_template(
        &json!({"template":"Go","agent":"general","model":"corp/m#low","subtask":false}),
    )
    .unwrap();
    assert!(!configured.subtask && configured.has_overrides());
    assert!(
        !StaticCommand::parse_template(&json!({"template":"Go"}))
            .unwrap()
            .has_overrides()
    );
    for (value, reason) in [
        (json!({"template":"Go","model":"no-provider"}), "model"),
        (json!({"template":"Go","model":"corp/"}), "model"),
        (json!({"template":"Go","model":"corp/m#"}), "model"),
        (json!({"template":"Go","agent":""}), "agent"),
        (json!({"template":"Go","agent":"a\nb"}), "agent"),
        (json!({"template":"Go","variant":""}), "variant"),
        (json!({"template":"Go","variant":"a#b"}), "variant"),
        (
            json!({"template":"Go","model":"corp/m#low","variant":"high"}),
            "variant",
        ),
        (json!({"template":"Go","subtask":"yes"}), "definition"),
        (json!({"template":"Go","agent":7}), "definition"),
        (json!({"template":"Go","unknown":"x"}), "definition"),
    ] {
        let error = StaticCommand::parse_template(&value).unwrap_err();
        assert!(error.contains(reason), "{value}: {error}");
    }
    for document in [
        "---\nmodel: private-model\n---\nprivate-body",
        "---\nsubtask: maybe\n---\nprivate-body",
        "---\nagent: [private]\n---\nprivate-body",
    ] {
        let error = cyber_core::commands::markdown_template(document)
            .err()
            .unwrap();
        assert!(!error.contains("private"), "{error}");
    }
    // Import review stays literal: overrides are not imported.
    assert!(StaticCommand::parse(&parsed.definition).is_err());
}
