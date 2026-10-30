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
    let mut fixtures: BTreeSet<_> =
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
    // Platform tools: powershell on Windows, the PTY tools elsewhere.
    for platform in ["powershell", "pty_start", "pty_write", "pty_read"] {
        if !names.contains(platform) {
            fixtures.remove(platform);
        }
    }
    assert_eq!(names, fixtures);
}

/// The model-facing tool names `builtin-tools` defines: the built-in set and the
/// capability-owned catalog, read from the canonical spec.
fn canonical_registry() -> BTreeSet<String> {
    registry_in(&builtin_tools_spec())
}

fn builtin_tools_spec() -> String {
    std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../openspec/specs/builtin-tools/spec.md"
    ))
    .unwrap()
}

/// The registry listed in a `builtin-tools` spec text.
fn registry_in(spec: &str) -> BTreeSet<String> {
    // Windows checkouts may convert the spec to CRLF line endings.
    let spec = spec.replace("\r\n", "\n");
    let section = |title: &str| -> String {
        let start = spec
            .find(&format!("### Requirement: {title}\n"))
            .expect(title);
        let rest = &spec[start..];
        let end = rest.find("#### Scenario").unwrap_or(rest.len());
        rest[..end].to_string()
    };
    let names = |text: &str| -> Vec<String> {
        text.split('`')
            .skip(1)
            .step_by(2)
            .filter(|token| {
                !token.is_empty() && token.chars().all(|c| c.is_ascii_lowercase() || c == '_')
            })
            .map(str::to_owned)
            .collect()
    };
    let builtin = section("Built-in tool set");
    let catalog = section("Capability-owned tool catalog");
    let listed: String = catalog
        .lines()
        .filter(|line| line.starts_with("- `"))
        .collect::<Vec<_>>()
        .join("\n");
    let mut registry: BTreeSet<String> = names(&builtin).into_iter().collect();
    registry.extend(names(&listed));
    registry
}

#[test]
fn the_registry_reads_the_same_from_a_crlf_checkout() {
    let spec = builtin_tools_spec();
    let crlf = spec.replace("\r\n", "\n").replace('\n', "\r\n");
    assert_eq!(registry_in(&crlf), registry_in(&spec));
}

/// `builtin-tools` → Capability-owned tool catalog: no capability introduces a tool the
/// registry does not list. Tools whose capability or phase is not implemented are
/// absent, so the advertised set is a subset rather than the whole catalog.
#[tokio::test]
async fn advertised_tool_names_are_a_subset_of_the_canonical_registry() {
    let registry = canonical_registry();
    for name in [
        "read",
        "bash",
        "invalid",
        "monitor",
        "task_stop",
        "pty_start",
        "advisor",
    ] {
        assert!(registry.contains(name), "{name} missing from {registry:?}");
    }
    let f = Fixture::new();
    f.set_config(
        json!({"tools":{"websearch":{"backend":"searxng","searxng":{"url":"http://127.0.0.1"}}}}),
    );
    let advertised: BTreeSet<String> = ["default", "plan", "bypass"]
        .into_iter()
        .flat_map(|mode| {
            [false, true]
                .into_iter()
                .flat_map(|patch| f.tool_names(mode, patch))
        })
        .collect();
    assert!(advertised.contains("monitor"), "{advertised:?}");
    let unknown: Vec<_> = advertised.difference(&registry).collect();
    assert!(
        unknown.is_empty(),
        "advertised but not in the registry: {unknown:?}"
    );
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
async fn notebook_edit_golden() {
    let fixture = Fixture::new();
    fixture.write("work.ipynb", &json!({"nbformat":4,"nbformat_minor":0,"metadata":{},"cells":[{"cell_type":"code","metadata":{},"source":["old"],"outputs":[],"execution_count":null}]}).to_string());
    support::golden(
        &fixture,
        "notebook_edit",
        &ok(fixture
            .call(
                "accept-edits",
                "notebook_edit",
                json!({"path":"work.ipynb","mode":"replace","cell_index":0,"new_source":"new"}),
            )
            .await),
    );
}

#[tokio::test]
async fn bash_golden() {
    #[cfg(not(windows))]
    let f = Fixture::new();
    #[cfg(windows)]
    let bash =
        std::path::PathBuf::from(std::env::var_os("ProgramFiles").expect("Windows ProgramFiles"))
            .join("Git/bin/bash.exe");
    #[cfg(windows)]
    assert!(
        bash.is_file(),
        "install Git Bash before Windows shell goldens"
    );
    #[cfg(windows)]
    let f = Fixture::with_policy(
        bash.to_str().expect("Git Bash path must be UTF-8"),
        Some(
            cyber_sandbox::find_helper().expect("build cyber-sandbox-exec before Windows goldens"),
        ),
        Some("full-access".into()),
    );
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
    // Both are framed as untrusted content; the server's port is normalized.
    let (fetched, _) = support::external(
        f.call("default", "webfetch", json!({"url":base,"format":"text"}))
            .await,
    );
    support::golden(&f, "webfetch", &fetched.replace(&base, "http://SERVER"));
    support::golden(
        &f,
        "websearch",
        &support::external(
            f.call("default", "websearch", json!({"query":"rust"}))
                .await,
        )
        .0,
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

#[tokio::test]
async fn mcp_wait_golden() {
    let f = Fixture::new();
    f.configure_mcp_status(
        json!({"disabled":{"type":"local","command":"not-installed","enabled":false}}),
    );
    let output = ok(f
        .call("plan", "wait_for_mcp", json!({"servers":["disabled"]}))
        .await);
    support::golden(&f, "wait_for_mcp", &output);
}

#[tokio::test]
async fn tool_search_golden() {
    let f = Fixture::new();
    let output = ok(f.call("plan", "tool_search", json!({})).await);
    support::golden(&f, "tool_search", &output);
}

#[tokio::test]
async fn lsp_unavailable_golden() {
    let fixture = Fixture::new();
    fixture.set_config(json!({"permissions":{"lsp":"allow"}}));
    assert_eq!(
        support::failed(
            fixture
                .call(
                    "default",
                    "lsp",
                    json!({"operation":"workspace_symbols","query":"main"})
                )
                .await
        ),
        "LSP services are unavailable"
    );
}

#[test]
fn golden_normalization_retains_non_path_text() {
    let fixture = Fixture::new();
    fixture.write("nested/file.txt", "content");
    let input = format!(
        "{}\n{}\nuser text: \\literal\\n",
        fixture.repo.join("nested/file.txt").display(),
        fixture.repo.join("nested").display()
    );
    assert_eq!(
        support::normalized_fixture_paths(&fixture, &input),
        "<repo>/nested/file.txt\n<repo>/nested\nuser text: \\literal\\n"
    );
}
