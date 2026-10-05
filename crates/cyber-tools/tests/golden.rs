//! Stable model-facing output contracts for every registered built-in tool.
mod support;

use serde_json::json;
use std::collections::BTreeSet;
use support::{Fixture, ok, serve};

#[tokio::test]
async fn every_registered_tool_has_a_golden() {
    let f = Fixture::new();
    f.set_config(
        json!({"tools":{"websearch":{"backend":"searxng","searxng":{"url":"http://127.0.0.1"}}}}),
    );
    let names: BTreeSet<_> = ["default", "plan"]
        .into_iter()
        .flat_map(|mode| {
            [false, true]
                .into_iter()
                .flat_map(|patch| f.tool_names(mode, patch))
        })
        .collect();
    let fixtures: BTreeSet<_> =
        std::fs::read_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/goldens"))
            .unwrap()
            .map(|e| {
                e.unwrap()
                    .path()
                    .file_stem()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .to_string()
            })
            .collect();
    assert_eq!(names, fixtures);
}

#[tokio::test]
async fn filesystem_goldens() {
    let f = Fixture::new();
    f.write("a.txt", "alpha\nbeta\n");
    support::golden(
        &f,
        "read",
        &ok(f.call("default", "read", json!({"path":"a.txt"})).await),
    );
    support::golden(
        &f,
        "list",
        &ok(f.call("default", "list", json!({"path":"."})).await),
    );
    support::golden(
        &f,
        "glob",
        &ok(f.call("default", "glob", json!({"pattern":"*.txt"})).await),
    );
    support::golden(
        &f,
        "grep",
        &ok(f.call("default", "grep", json!({"pattern":"beta"})).await),
    );
    support::golden(
        &f,
        "write",
        &ok(f
            .call(
                "accept-edits",
                "write",
                json!({"path":"b.txt","content":"new\n"}),
            )
            .await),
    );
    assert_eq!(f.read("b.txt"), "new\n");
    support::golden(
        &f,
        "edit",
        &ok(f
            .call(
                "accept-edits",
                "edit",
                json!({"path":"a.txt","old_string":"beta","new_string":"BETA"}),
            )
            .await),
    );
    assert_eq!(f.read("a.txt"), "alpha\nBETA\n");
    support::golden(
        &f,
        "apply_patch",
        &ok(f
            .call(
                "accept-edits",
                "apply_patch",
                json!({"patch":"*** Begin Patch\n*** Add File: c.txt\n+patched\n*** End Patch"}),
            )
            .await),
    );
    assert_eq!(f.read("c.txt"), "patched\n");
}

#[tokio::test]
async fn bash_golden() {
    let f = Fixture::new();
    f.set_config(json!({"permissions":{"bash":"allow"}}));
    support::golden(
        &f,
        "bash",
        &ok(f
            .call(
                "default",
                "bash",
                json!({"command":"printf 'hello\\n'; exit 3"}),
            )
            .await),
    );
}

#[tokio::test]
async fn todo_golden() {
    let f = Fixture::new();
    support::golden(
        &f,
        "todo",
        &ok(f
            .call(
                "default",
                "todo",
                json!({"op":"create","subject":"Ship P0"}),
            )
            .await),
    );
}

#[tokio::test]
async fn web_goldens() {
    let base = serve(|_, _| (200, vec![("content-type".into(),"application/json".into())],
        br#"{"results":[{"title":"Rust","url":"https://rust-lang.org/","content":"Systems language"}]}"#.to_vec())).await;
    let f = Fixture::new();
    f.set_config(json!({"permissions":{"webfetch":"allow","websearch":"allow"},"tools":{"websearch":{"backend":"searxng","searxng":{"url":base}}}}));
    support::golden(
        &f,
        "webfetch",
        &ok(f
            .call("default", "webfetch", json!({"url":base,"format":"text"}))
            .await),
    );
    support::golden(
        &f,
        "websearch",
        &ok(f
            .call("default", "websearch", json!({"query":"rust"}))
            .await),
    );
}

#[tokio::test]
async fn skill_golden() {
    let f = Fixture::new();
    f.write(
        ".cyber/skills/notes/SKILL.md",
        "---\nname: notes\ndescription: Write notes\n---\nSummarize $1.\n",
    );
    f.write(".cyber/skills/notes/template.md", "# Notes\n");
    support::golden(
        &f,
        "skill",
        &ok(f
            .call("default", "skill", json!({"name":"notes","arguments":"P0"}))
            .await),
    );
}
