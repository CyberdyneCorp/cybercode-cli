use cyber_core::commands::{StaticCommand, configured};
use serde_json::json;

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
