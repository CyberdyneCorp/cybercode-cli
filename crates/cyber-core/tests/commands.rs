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
        "Review code\n\nARGUMENTS: carefully"
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
        "Review code\n\nARGUMENTS: carefully"
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
        json!({"template":"review", "subtask":false}),
        json!({"template":"x".repeat(65537)}),
        json!({"template":"private\0value"}),
    ] {
        let error = StaticCommand::parse(&value).unwrap_err();
        assert!(!error.contains("secret") && !error.contains("sentinel"));
    }
    let commands =
        configured(&json!({"commands":{"../bad":{"template":"bad"},"safe":{"template":"OK"}}}));
    assert_eq!(commands.entries.len(), 1);
    assert_eq!(commands.unavailable, ["invalid command name"]);
}
