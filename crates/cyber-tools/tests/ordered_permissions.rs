//! Ordered rule arrays retain order and participate in actual tool admission.
mod support;

use cyber_server::runtime::ToolHost;
use cyber_tools::permissions::{Effect, evaluate, parse_rules};
use serde_json::json;
use std::collections::BTreeMap;
use support::{Fixture, failed, ok};
use tokio_util::sync::CancellationToken;

#[test]
fn ordered_rules_preserve_order_and_source_pointers() {
    let value = json!([
        {"action":"read","resource":"*","effect":"deny"},
        {"action":"read","resource":"public.txt","effect":"allow"},
        {"action":"edit","resource":"*","effect":"ask"}
    ]);
    let sources = BTreeMap::from([(
        "/permissions/1/effect".into(),
        "project:/repo/cyber.json".into(),
    )]);
    let rules = parse_rules(&value, &sources);
    assert_eq!(rules.len(), 3);
    assert_eq!(evaluate(&rules, "read", "secret.txt").0, Effect::Deny);
    assert_eq!(evaluate(&rules, "read", "public.txt").0, Effect::Allow);
    assert_eq!(rules[1].source, "project:/repo/cyber.json");
    assert_eq!(rules[2].effect, Effect::Ask);
}

#[tokio::test]
async fn ordered_session_denial_refuses_bypass_without_writing() {
    let f = Fixture::new();
    f.write("a.txt", "original");
    ok(f.call("bypass", "read", json!({"path":"a.txt"})).await);
    let mut inv = f.invocation(
        "bypass",
        "write",
        json!({"path":"a.txt","content":"changed"}),
    );
    inv.rules = json!([{ "action":"edit", "resource":"*", "effect":"deny" }]);
    let error = failed(f.host.execute(inv, CancellationToken::new()).await);
    assert!(error.contains("denied"), "{error}");
    assert_eq!(f.read("a.txt"), "original");
}

#[tokio::test]
async fn later_session_allow_wins_within_its_layer() {
    let f = Fixture::new();
    f.write("a.txt", "original");
    ok(f.call("bypass", "read", json!({"path":"a.txt"})).await);
    let mut inv = f.invocation(
        "dont-ask",
        "write",
        json!({"path":"a.txt","content":"changed"}),
    );
    inv.rules = json!([
        {"action":"edit","resource":"*","effect":"deny"},
        {"action":"edit","resource":"*","effect":"allow"}
    ]);
    ok(f.host.execute(inv, CancellationToken::new()).await);
    assert_eq!(f.read("a.txt"), "changed");
}

#[tokio::test]
async fn session_allow_cannot_widen_a_config_deny_ceiling() {
    let f = Fixture::new();
    f.write("a.txt", "original");
    ok(f.call("bypass", "read", json!({"path":"a.txt"})).await);
    f.set_config(json!({"permissions":[{"action":"edit","resource":"*","effect":"deny"}]}));
    let mut inv = f.invocation(
        "bypass",
        "write",
        json!({"path":"a.txt","content":"changed"}),
    );
    inv.rules = json!([{ "action":"edit", "resource":"*", "effect":"allow" }]);
    let error = failed(f.host.execute(inv, CancellationToken::new()).await);
    assert!(error.contains("denied"), "{error}");
    assert_eq!(f.read("a.txt"), "original");
}
