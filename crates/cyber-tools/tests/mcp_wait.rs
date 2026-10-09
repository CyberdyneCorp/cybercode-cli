//! Portable read-only wait tool validation and terminal observations.
mod support;
use serde_json::{Value, json};
use support::{Fixture, failed, ok};

#[tokio::test]
async fn terminal_states_return_without_starting_servers_or_creating_owners() {
    let f = Fixture::new();
    f.configure_mcp_status(json!({
        "disabled":{"type":"local","command":"not-installed","enabled":false},
        "unstarted":{"type":"local","command":"not-installed"},
        "remote":{"type":"remote","url":"https://example.invalid/mcp","headers":{"Authorization":"private-secret"}}
    }));
    let output = ok(f
        .call(
            "plan",
            "wait_for_mcp",
            json!({"servers":["disabled","unstarted","remote"]}),
        )
        .await);
    assert!(!output.contains("private-secret"));
    let output: Value = serde_json::from_str(&output).unwrap();
    assert_eq!(output["timed_out"], false);
    assert_eq!(output["servers"][0]["status"], "disabled");
    assert_eq!(output["servers"][1]["status"], "failed");
    assert_eq!(output["servers"][2]["status"], "failed");
    assert!(
        cyber_server::runtime::mcp_connections(&f.store, &f.repo)
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn invalid_names_timeout_and_unknown_servers_fail_without_startup() {
    let f = Fixture::new();
    f.configure_mcp_status(
        json!({"disabled":{"type":"local","command":"not-installed","enabled":false}}),
    );
    for input in [
        json!({"servers":[]}),
        json!({"servers":["disabled","disabled"]}),
        json!({"servers":["disabled"],"timeout":61}),
        json!({"servers":["disabled"],"timeout":-1}),
        json!({"servers":["disabled"],"timeout":0.5}),
        json!({"servers":["bad/name"]}),
    ] {
        assert!(matches!(
            f.call("default", "wait_for_mcp", input).await,
            cyber_server::runtime::ToolOutcome::Failed(_)
        ));
    }
    assert_eq!(
        failed(
            f.call("default", "wait_for_mcp", json!({"servers":["missing"]}))
                .await
        ),
        "Unknown MCP server: missing"
    );
    assert!(
        cyber_server::runtime::mcp_connections(&f.store, &f.repo)
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn permission_denial_hides_and_refuses_wait_tool() {
    let f = Fixture::new();
    f.configure_mcp_status(
        json!({"disabled":{"type":"local","command":"not-installed","enabled":false}}),
    );
    f.set_config(json!({"permissions":{"wait_for_mcp":"deny"}}));
    assert!(
        !f.tool_names("default", false)
            .contains(&"wait_for_mcp".to_string())
    );
    assert!(matches!(
        f.call("default", "wait_for_mcp", json!({"servers":["disabled"]}))
            .await,
        cyber_server::runtime::ToolOutcome::Failed(_)
    ));
}
