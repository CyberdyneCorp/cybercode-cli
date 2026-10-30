//! `tool-registry` → Tool output redaction through the real host and runtime: what the
//! model receives and what the Session stores are the same redacted text, overflow files
//! keep the raw text, and write tools refuse redaction markers.

mod support;

#[cfg(unix)]
use cyber_llm::Content;
#[cfg(unix)]
use cyber_server::runtime::ToolHost;
#[cfg(unix)]
use serde_json::Value;
use serde_json::json;
#[cfg(unix)]
use std::collections::BTreeMap;
use support::Fixture;

#[cfg(unix)]
const GITHUB: &str = "ghp_aBcDeFgHiJkLmNoPqRsTuVwXyZ0123456789";
#[cfg(unix)]
const NPM: &str = "npm_abcdefghijklmnopqrstuvwxyz0123456789";
#[cfg(unix)]
/// Shapeless values only an exact match can find: an environment secret and a
/// configured provider key.
const DEPLOY: &str = "plain-deploy-value-0042";
#[cfg(unix)]
const PROVIDER_KEY: &str = "corp-provider-key-0123456789";
#[cfg(unix)]
const PEM: &str = "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQ\nAAAAMwAAAAtzc2gtZWQyNTUxOQAAACD3r9cZ\n-----END OPENSSH PRIVATE KEY-----";

#[cfg(unix)]
/// The tool results of a request, by call.
fn tool_results(request: &cyber_llm::LlmRequest) -> BTreeMap<String, String> {
    request
        .messages
        .iter()
        .flat_map(|message| &message.content)
        .filter_map(|part| match part {
            Content::ToolResult {
                call_id, output, ..
            } => Some((call_id.clone(), output.clone())),
            _ => None,
        })
        .collect()
}

#[cfg(unix)]
/// The stored `session.tool.settled` payloads, by call.
fn settled(f: &Fixture, session: &str) -> BTreeMap<String, Value> {
    f.store
        .read_events(session, -1, 500)
        .unwrap()
        .events
        .into_iter()
        .filter(|event| event.kind == "session.tool.settled.1")
        .map(|event| {
            (
                event.data["call_id"].as_str().unwrap().to_owned(),
                event.data,
            )
        })
        .collect()
}

#[cfg(unix)]
fn assert_hidden(text: &str) {
    for secret in [
        GITHUB,
        NPM,
        DEPLOY,
        PROVIDER_KEY,
        "b3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQ",
    ] {
        assert!(
            !text.contains(secret),
            "{secret} reached the model:\n{text}"
        );
    }
}

#[cfg(unix)]
fn overflow_path(output: &str) -> String {
    output
        .rsplit("full output at ")
        .next()
        .unwrap()
        .trim_end_matches(']')
        .to_owned()
}

/// `tool-registry` → Tool output redaction, scenarios "Shell output", "Credentials file",
/// "Known secret values" and "Overflowed result": bash `echo $SECRET`, a read of a
/// `.npmrc`, exact environment and provider-key values, and a result past the budget are
/// redacted in the model's tool results and in the stored settlements alike.
#[cfg(unix)]
#[tokio::test]
async fn model_and_stored_history_get_the_same_redacted_results() {
    use support::flow::{Flow, call, text};
    let flow = Flow::new(
        vec![
            call(
                "c_echo",
                "bash",
                json!({"command": "export GITHUB_TOKEN=$(cat token.txt); echo \"token is $GITHUB_TOKEN\""}),
            ),
            call("c_npmrc", "read", json!({"path": ".npmrc"})),
            call("c_known", "bash", json!({"command": "cat deploy.txt"})),
            call(
                "c_big",
                "bash",
                json!({"command": "seq 1 3000; cat key.pem"}),
            ),
            text("done"),
        ],
        false,
    );
    let f = &flow.f;
    f.env.set("DEPLOY_TOKEN", DEPLOY);
    f.set_config(json!({"providers": {"corp": {"api_key": PROVIDER_KEY}}}));
    f.write("token.txt", GITHUB);
    f.write(
        ".npmrc",
        &format!("//registry.npmjs.org/:_authToken={NPM}\nregistry=https://registry.npmjs.org/\n"),
    );
    f.write(
        "deploy.txt",
        &format!("deploy={DEPLOY}\nprovider={PROVIDER_KEY}\n"),
    );
    f.write("key.pem", PEM);
    let session = flow.session("bypass").await;
    flow.prompt(&session, "look around").await;
    flow.settle(&session).await;

    let requests = flow.main.requests();
    let seen = tool_results(requests.last().unwrap());
    let stored = settled(f, &session);
    assert_eq!(seen.len(), 4, "{seen:#?}");
    for (call, output) in &seen {
        assert_hidden(output);
        // The model and the stored history never differ.
        assert_eq!(stored[call]["output"], output.as_str(), "{call}");
    }
    assert!(
        seen["c_echo"].contains("token is [REDACTED:github_token]"),
        "{}",
        seen["c_echo"]
    );
    assert!(
        seen["c_npmrc"].contains(":_authToken=[REDACTED:npm_token]"),
        "{}",
        seen["c_npmrc"]
    );
    assert!(seen["c_npmrc"].contains("registry=https://registry.npmjs.org/"));
    assert!(
        seen["c_known"].contains("deploy=[REDACTED:known_secret]")
            && seen["c_known"].contains("provider=[REDACTED:known_secret]"),
        "{}",
        seen["c_known"]
    );
    // The kept tail of the overflowed result hides the key whole.
    assert!(
        seen["c_big"].contains("[REDACTED:private_key]"),
        "{}",
        seen["c_big"]
    );
    assert!(seen["c_big"].contains("[output truncated:"));
    assert_eq!(
        stored["c_echo"]["redactions"],
        json!({"count": 1, "kinds": ["github_token"]})
    );
    assert_eq!(
        stored["c_known"]["redactions"],
        json!({"count": 2, "kinds": ["known_secret"]})
    );
    assert_eq!(
        stored["c_big"]["redactions"]["kinds"],
        json!(["private_key"])
    );
    // The Session's calls carry the same `redactions` for clients.
    let state = flow.runtime.state(&session).await.unwrap();
    assert_eq!(state.calls["c_npmrc"].redactions.as_ref().unwrap().count, 1);

    // The overflow file keeps the raw text on this machine; reading it back redacts it.
    let path = overflow_path(&seen["c_big"]);
    assert!(std::fs::read_to_string(&path).unwrap().contains(PEM));
    // A page that starts inside the key shows none of it, and one that ends inside it
    // shows only the marker: the file is redacted whole before it is paged.
    let (tail, tail_provenance) = support::external(
        f.call("bypass", "read", json!({"path": path, "offset": 3002}))
            .await,
    );
    assert_eq!(
        tail_provenance.source,
        cyber_server::runtime::ContentSource::Overflow
    );
    assert_eq!(tail_provenance.origin, path);
    support::framed(&tail, "overflow", &path);
    assert_hidden(&tail);
    assert!(!tail.contains("REDACTED"), "{tail}");
    let (reread, reread_provenance) = support::external(
        f.call("bypass", "read", json!({"path": path, "offset": 2990}))
            .await,
    );
    assert_eq!(reread_provenance, tail_provenance);
    support::framed(&reread, "overflow", &path);
    assert!(reread.contains("[REDACTED:private_key]"), "{reread}");
    assert_hidden(&reread);
    assert_eq!(
        f.host.take_call_redactions("ses_test", "call_read").kinds,
        ["private_key"]
    );
}

/// The user's own config can turn redaction off; extra patterns and the allow list
/// apply to built-in results.
#[cfg(unix)]
#[tokio::test]
async fn user_settings_switch_patterns_and_allow_entries() {
    let f = Fixture::new();
    f.write(
        "ids.txt",
        "corp_abcdef012345 AKIAIOSFODNN7EXAMPLE AKIAIOSFODNN7EXAMPL3\n",
    );
    f.set_config(json!({"tool_output": {"redact": {
        "patterns": ["corp_[a-z0-9]{12}"], "allow": ["AKIAIOSFODNN7EXAMPLE"]}}}));
    let shown = support::ok(
        f.call("bypass", "bash", json!({"command": "cat ids.txt"}))
            .await,
    );
    assert_eq!(
        shown.trim_end(),
        "[REDACTED:custom] AKIAIOSFODNN7EXAMPLE [REDACTED:aws_access_key]"
    );
    f.set_config(json!({"tool_output": {"redact": {"enabled": false}}}));
    let shown = support::ok(
        f.call("bypass", "bash", json!({"command": "cat ids.txt"}))
            .await,
    );
    assert!(shown.contains("AKIAIOSFODNN7EXAMPL3"), "{shown}");
}

/// `builtin-tools` → Redaction markers are never written: `write`, `edit`,
/// `apply_patch` and shell commands refuse a marker, so the placeholder cannot replace
/// the real secret; a marker the file already holds may be kept.
#[tokio::test]
async fn write_tools_refuse_redaction_markers() {
    let f = Fixture::new();
    f.write(".env", "API_TOKEN=abc123realvalue\nDEBUG=1\n");
    let read = support::ok(f.call("bypass", "read", json!({"path": ".env"})).await);
    assert!(read.contains("API_TOKEN=[REDACTED:assignment]"), "{read}");
    let marker = "[REDACTED:assignment]";
    let refusals = [
        f.call("bypass", "write", json!({"path": ".env", "content": format!("API_TOKEN={marker}\nDEBUG=0\n")}))
            .await,
        f.call("bypass", "edit", json!({"path": ".env", "old_string": "DEBUG=1", "new_string": format!("DEBUG=1\nOTHER_TOKEN={marker}")}))
            .await,
        f.call("bypass", "apply_patch", json!({"patch": format!(
            "*** Begin Patch\n*** Update File: .env\n@@\n-DEBUG=1\n+DEBUG={marker}\n*** End Patch"
        )}))
        .await,
        f.call("bypass", "bash", json!({"command": format!("printf 'API_TOKEN={marker}\\n' > .env")}))
            .await,
    ];
    for refusal in refusals {
        let message = support::failed(refusal);
        assert!(message.contains("redaction marker"), "{message}");
    }
    assert_eq!(f.read(".env"), "API_TOKEN=abc123realvalue\nDEBUG=1\n");
    // A marker in old_string can never match: the file holds the real value.
    let missing = support::failed(
        f.call("bypass", "edit", json!({"path": ".env", "old_string": format!("API_TOKEN={marker}"), "new_string": "API_TOKEN=x"}))
            .await,
    );
    assert!(
        missing.contains("stands in for a hidden value"),
        "{missing}"
    );
    // Documentation that already shows a marker can still be edited.
    f.write("docs.md", "Secrets look like [REDACTED:github_token].\n");
    support::ok(f.call("bypass", "read", json!({"path": "docs.md"})).await);
    support::ok(
        f.call(
            "bypass",
            "edit",
            json!({"path": "docs.md", "old_string": "look like", "new_string": "appear as"}),
        )
        .await,
    );
    assert_eq!(
        f.read("docs.md"),
        "Secrets appear as [REDACTED:github_token].\n"
    );
}
