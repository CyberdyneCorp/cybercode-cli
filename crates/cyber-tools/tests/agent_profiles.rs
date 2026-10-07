//! Profile restrictions apply to materialization and direct dispatch.

mod support;

use cyber_server::runtime::ToolHost;
use serde_json::json;
use support::{Fixture, failed, ok};
use tokio_util::sync::CancellationToken;

#[test]
fn per_turn_tool_globs_and_allow_lists_follow_live_profile_configuration() {
    let f = Fixture::new();
    assert!(f.tool_names("default", false).contains(&"bash".into()));
    f.set_config(json!({"agents":{"build":{"tools":{"deny":["bash","web*"]}}}}));
    let names = f.tool_names("default", false);
    assert!(!names.contains(&"bash".into()));
    assert!(!names.contains(&"webfetch".into()));
    assert!(names.contains(&"read".into()));
    f.set_config(json!({"agents":{"build":{"tools":{"allow":["read"]}}}}));
    assert_eq!(f.tool_names("default", false), ["read"]);
}

#[tokio::test]
async fn direct_dispatch_cannot_override_profile_denials_with_bypass_mode() {
    let f = Fixture::new();
    f.write("a.txt", "original");
    ok(f.call("bypass", "read", json!({"path":"a.txt"})).await);
    f.set_config(json!({"agents":{"build":{"tools":{"deny":["write"]}}}}));
    let error = failed(
        f.call(
            "bypass",
            "write",
            json!({"path":"a.txt","content":"changed"}),
        )
        .await,
    );
    assert!(
        error.contains("write") && error.contains("build"),
        "{error}"
    );
    assert_eq!(f.read("a.txt"), "original");
}

#[tokio::test]
async fn hidden_disabled_and_unknown_profiles_cannot_dispatch_tools() {
    let f = Fixture::new();
    f.write("a.txt", "original");
    f.set_config(json!({"agents":{"disabled":{"disabled":true}}}));
    for agent in ["title", "disabled", "unknown"] {
        let mut invocation = f.invocation("bypass", "read", json!({"path":"a.txt"}));
        invocation.agent = agent.into();
        let error = failed(f.host.execute(invocation, CancellationToken::new()).await);
        assert!(error.contains(agent), "{error}");
    }
}
