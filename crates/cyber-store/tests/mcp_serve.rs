//! MCP serve records (`mcp-server`): Session and input attribution and key fingerprints.

use cyber_core::paths::DatabaseLocation;
use cyber_store::mcp_serve::{self, McpServeInput, McpServeSession};
use cyber_store::{EventRegistry, Store, StoreOptions};

fn store(dir: &std::path::Path) -> Store {
    Store::open(StoreOptions::new(
        DatabaseLocation::File(dir.join("cyber.db")),
        EventRegistry::default(),
    ))
    .unwrap()
}

#[test]
fn sessions_keep_their_first_mcp_client_and_survive_a_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let db = store(dir.path());
    let first = McpServeSession {
        session_id: "ses_a".into(),
        mcp_client: "claude-code".into(),
        created_ms: 10,
    };
    mcp_serve::record_session(&db, &first).unwrap();
    mcp_serve::record_session(
        &db,
        &McpServeSession {
            mcp_client: "codex".into(),
            ..first.clone()
        },
    )
    .unwrap();
    drop(db);
    let db = store(dir.path());
    let clients =
        mcp_serve::session_clients(&db, vec!["ses_a".into(), "ses_other".into()]).unwrap();
    assert_eq!(clients.len(), 1);
    assert_eq!(clients["ses_a"], "claude-code");
    assert!(
        mcp_serve::session_clients(&db, Vec::new())
            .unwrap()
            .is_empty()
    );
}

#[test]
fn inputs_are_listed_newest_first() {
    let dir = tempfile::tempdir().unwrap();
    let db = store(dir.path());
    for (n, depth) in [(1, 0), (2, 1), (3, 0)] {
        mcp_serve::record_input(
            &db,
            &McpServeInput {
                message_id: format!("msg_{n}"),
                session_id: "ses_a".into(),
                mcp_client: "claude-code".into(),
                depth,
                admitted_ms: n,
                idempotency_key: (n == 2).then(|| "k".into()),
            },
        )
        .unwrap();
    }
    let recent = mcp_serve::recent_inputs(&db, "ses_a", 2).unwrap();
    let ids: Vec<&str> = recent.iter().map(|i| i.message_id.as_str()).collect();
    assert_eq!(ids, ["msg_3", "msg_2"]);
    assert_eq!(recent[1].depth, 1);
    assert_eq!(recent[1].idempotency_key.as_deref(), Some("k"));
    assert!(
        mcp_serve::recent_inputs(&db, "ses_b", 5)
            .unwrap()
            .is_empty()
    );
    let one = mcp_serve::input(&db, "msg_2").unwrap().unwrap();
    assert_eq!((one.session_id.as_str(), one.admitted_ms), ("ses_a", 2));
    assert_eq!(mcp_serve::input(&db, "msg_9").unwrap(), None);

    // `record_input` keeps the first record; `replace_input` (a retried send that was
    // never admitted) replaces it.
    let retried = McpServeInput {
        session_id: "ses_b".into(),
        depth: 2,
        ..one
    };
    mcp_serve::record_input(&db, &retried).unwrap();
    assert_eq!(mcp_serve::input(&db, "msg_2").unwrap().unwrap().depth, 1);
    mcp_serve::replace_input(&db, &retried).unwrap();
    assert_eq!(mcp_serve::input(&db, "msg_2").unwrap(), Some(retried));
}

#[test]
fn key_fingerprints_expire_and_are_never_replaced_while_live() {
    let dir = tempfile::tempdir().unwrap();
    let db = store(dir.path());
    assert!(mcp_serve::claim_key(&db, "mcp:t:k", "fp1", 100, 0).unwrap());
    assert_eq!(
        mcp_serve::key_fingerprint(&db, "mcp:t:k", 0)
            .unwrap()
            .as_deref(),
        Some("fp1")
    );
    assert_eq!(
        mcp_serve::key_fingerprint(&db, "mcp:t:k", 100).unwrap(),
        None
    );
    // Regression: a concurrent call with other arguments under the same key is refused
    // and leaves the first call's fingerprint, so the first call still replays.
    assert!(!mcp_serve::claim_key(&db, "mcp:t:k", "fp2", 200, 0).unwrap());
    assert!(mcp_serve::claim_key(&db, "mcp:t:k", "fp1", 200, 0).unwrap());
    assert_eq!(
        mcp_serve::key_fingerprint(&db, "mcp:t:k", 0)
            .unwrap()
            .as_deref(),
        Some("fp1")
    );
    // Once expired, the key is free for other arguments.
    assert!(mcp_serve::claim_key(&db, "mcp:t:k", "fp2", 300, 100).unwrap());
    // A later claim drops expired fingerprints of other keys.
    mcp_serve::claim_key(&db, "mcp:t:old", "fp", 50, 0).unwrap();
    mcp_serve::claim_key(&db, "mcp:t:new", "fp", 300, 60).unwrap();
    assert_eq!(
        mcp_serve::key_fingerprint(&db, "mcp:t:old", 0).unwrap(),
        None
    );
}
