//! webfetch, websearch and skill through the host, against a local HTTP server.

mod support;

use serde_json::json;
use support::{Fixture, failed, ok, serve};

fn html(body: &str) -> (u16, Vec<(String, String)>, Vec<u8>) {
    (
        200,
        vec![("content-type".into(), "text/html; charset=utf-8".into())],
        body.as_bytes().to_vec(),
    )
}

#[tokio::test]
async fn webfetch_converts_html_to_markdown_or_text() {
    let f = Fixture::new();
    f.set_config(json!({"permissions": {"webfetch": "allow"}}));
    let base =
        serve(|_, _| html("<html><body><h1>Title</h1><p>Hello <b>world</b></p></body></html>"))
            .await;
    let md = ok(f
        .call(
            "default",
            "webfetch",
            json!({"url": format!("{base}/page")}),
        )
        .await);
    assert!(md.contains("# Title") && md.contains("**world**"), "{md}");
    let text = ok(f
        .call(
            "default",
            "webfetch",
            json!({"url": format!("{base}/page"), "format": "text"}),
        )
        .await);
    assert!(text.contains("Hello") && !text.contains("<p>"), "{text}");
    let raw = ok(f
        .call(
            "default",
            "webfetch",
            json!({"url": format!("{base}/page"), "format": "html"}),
        )
        .await);
    assert!(raw.contains("<h1>Title</h1>"), "{raw}");
}

#[tokio::test]
async fn webfetch_rejects_other_schemes_and_oversized_bodies() {
    let f = Fixture::new();
    f.set_config(json!({"permissions": {"webfetch": "allow"}}));
    let out = failed(
        f.call("default", "webfetch", json!({"url": "file:///etc/passwd"}))
            .await,
    );
    assert_eq!(out, "Only http and https URLs are supported");
    let base = serve(|_, _| {
        (
            200,
            vec![("content-type".into(), "text/plain".into())],
            vec![b'a'; 5 * 1024 * 1024 + 1],
        )
    })
    .await;
    let big = failed(f.call("default", "webfetch", json!({"url": base})).await);
    assert!(big.contains("exceeds 5 MiB"), "{big}");
}

#[tokio::test]
async fn webfetch_retries_a_cloudflare_challenge_with_the_cyber_agent() {
    let f = Fixture::new();
    f.set_config(json!({"permissions": {"webfetch": "allow"}}));
    let base = serve(|_, head| {
        if head.contains("user-agent: cyber\r\n") {
            (
                200,
                vec![("content-type".into(), "text/plain".into())],
                b"passed".to_vec(),
            )
        } else {
            (
                403,
                vec![("cf-mitigated".into(), "challenge".into())],
                b"challenge".to_vec(),
            )
        }
    })
    .await;
    assert_eq!(
        ok(f.call("default", "webfetch", json!({"url": base})).await),
        "passed"
    );
}

#[tokio::test]
async fn webfetch_asks_by_default() {
    let f = Fixture::new();
    let out = failed(
        f.call("default", "webfetch", json!({"url": "https://example.com"}))
            .await,
    );
    assert!(out.contains("no interactive approver"), "{out}");
}

#[tokio::test]
async fn websearch_is_hidden_without_a_backend_and_uses_searxng_when_configured() {
    let f = Fixture::new();
    assert!(
        !f.tool_names("default", false)
            .contains(&"websearch".to_string())
    );
    let base = serve(|path, _| {
        assert!(path.starts_with("/search?q=rust+async") && path.contains("format=json"), "{path}");
        let body = json!({"results": [
            {"title": "Async Book", "url": "https://rust-lang.github.io/async-book/", "content": "Asynchronous programming in Rust"},
            {"title": "Blocked", "url": "https://spam.example/x", "content": "nope"}]});
        (200, vec![("content-type".into(), "application/json".into())], body.to_string().into_bytes())
    })
    .await;
    f.set_config(json!({"permissions": {"websearch": "allow"}, "tools": {"websearch": {"backend": "searxng", "searxng": {"url": base}}}}));
    assert!(
        f.tool_names("default", false)
            .contains(&"websearch".to_string())
    );
    let out = ok(f
        .call(
            "default",
            "websearch",
            json!({"query": "rust async", "blocked_domains": ["spam.example"]}),
        )
        .await);
    assert_eq!(
        out,
        "1. Async Book\n   https://rust-lang.github.io/async-book/\n   Asynchronous programming in Rust"
    );
}

#[tokio::test]
async fn websearch_rotates_past_a_rate_limited_backend() {
    let f = Fixture::new();
    let limited = serve(|_, _| (429, vec![], b"slow down".to_vec())).await;
    f.env.set("SEARXNG_URL", &limited);
    f.set_config(json!({"permissions": {"websearch": "allow"}}));
    let out = failed(f.call("default", "websearch", json!({"query": "x"})).await);
    assert!(out.contains("rate limited"), "{out}");
}

fn skill(f: &Fixture, rel: &str, name: &str, description: &str, body: &str) {
    f.write(
        &format!("{rel}/SKILL.md"),
        &format!("---\nname: {name}\ndescription: {description}\n---\n{body}"),
    );
}

#[tokio::test]
async fn skill_loads_body_with_arguments_and_sibling_files() {
    let f = Fixture::new();
    skill(
        &f,
        ".cyber/skills/release-notes",
        "release-notes",
        "Write release notes",
        "Summarize changes since $1.\n",
    );
    f.write(".cyber/skills/release-notes/template.md", "# Notes");
    let out = ok(f
        .call(
            "default",
            "skill",
            json!({"name": "release-notes", "arguments": "v1.2"}),
        )
        .await);
    let base = f.repo.join(".cyber/skills/release-notes");
    assert_eq!(
        out,
        format!(
            "<skill name=\"release-notes\" base=\"{}\">\nSummarize changes since v1.2.\n\nFiles in this skill:\ntemplate.md\n</skill>",
            base.display()
        )
    );
}

#[tokio::test]
async fn unknown_skills_list_the_available_names() {
    let f = Fixture::new();
    skill(&f, ".cyber/skills/a", "alpha", "first", "A");
    skill(&f, ".cyber/skills/hidden", "hidden", "user only", "H");
    f.write(
        ".cyber/skills/hidden/SKILL.md",
        "---\nname: hidden\ndescription: user only\ndisable-model-invocation: true\n---\nH",
    );
    let out = failed(f.call("default", "skill", json!({"name": "nope"})).await);
    assert_eq!(out, "Skill \"nope\" not found. Available: alpha");
}

#[tokio::test]
async fn skill_directories_are_readable_without_prompting() {
    let f = Fixture::new();
    let user_skill = f
        .dir
        .path()
        .canonicalize()
        .unwrap()
        .join("home/.config/cyber/skills/notes");
    std::fs::create_dir_all(&user_skill).unwrap();
    std::fs::write(
        user_skill.join("SKILL.md"),
        "---\nname: notes\ndescription: d\n---\nB",
    )
    .unwrap();
    std::fs::write(user_skill.join("ref.md"), "reference text\n").unwrap();
    let out = ok(f
        .call(
            "default",
            "read",
            json!({"path": user_skill.join("ref.md").display().to_string()}),
        )
        .await);
    assert!(out.contains("reference text"), "{out}");
}

#[tokio::test]
async fn skills_listing_excludes_denied_and_user_only_skills() {
    let f = Fixture::new();
    skill(&f, ".cyber/skills/a", "alpha", "first skill", "A");
    skill(&f, ".cyber/skills/b", "beta", "second skill", "B");
    f.write(
        ".cyber/skills/c/SKILL.md",
        "---\nname: gamma\ndescription: user only\ndisable-model-invocation: true\n---\nC",
    );
    f.set_config(json!({"permissions": {"skill": {"beta": "deny"}}}));
    let turn = cyber_server::runtime::TurnContext {
        session_id: "s".into(),
        directory: f.repo.display().to_string(),
        agent: "build".into(),
        mode: "default".into(),
        prefers_apply_patch: false,
        rules: serde_json::Value::Null,
    };
    let sources = cyber_server::runtime::ToolHost::context_sources(&*f.host, &turn);
    assert_eq!(
        sources["core/skills"],
        "Skills you can load with the skill tool:\n<available_skills>\n- alpha: first skill\n</available_skills>"
    );
}
