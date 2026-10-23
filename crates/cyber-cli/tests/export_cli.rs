//! `cyber sessions export --format html` (`storage-events` → Session export) through the
//! real binary and the background server it starts, with a scripted model: the
//! self-contained page, inert injected markup, children nested under the call that
//! started them, `--sanitize` parity between JSON and HTML, and the atomic `--output`
//! that never writes through a symlink.

use std::path::PathBuf;
use std::process::{Command, Output};

use serde_json::{Value, json};

#[path = "../../cyber-app/tests/support/scripted_model.rs"]
mod scripted_model;

/// The final answer: Markdown with injected markup, a `javascript:` link and an external
/// link, which the HTML page must show as inert text.
const ANSWER: &str = "Here is **bold** and `code`.\n\n<img src=x onerror=alert(1)>\n\n\
[x](javascript:alert(1)) and [site](https://example.com/SECRET-URL)\n\n\
```rust\nfn main() {}\n```\n";

/// `EXPORT-ME` writes `a.txt` and delegates to `explore` in one step, then answers with
/// [`ANSWER`] and reasoning; the child answers `CHILD-ANSWER`; anything else is `Done.`.
fn reply(body: &Value) -> Value {
    let last = body["messages"]
        .as_array()
        .and_then(|m| m.last())
        .cloned()
        .unwrap_or_default();
    if body.to_string().contains("Write a short title") {
        return json!({ "content": "Export demo" });
    }
    if last["role"] == "tool" {
        return json!({ "reasoning_content": "REASONING-SECRET weighing it", "content": ANSWER });
    }
    let text = last.to_string();
    if text.contains("CHILD-TASK") {
        return json!({ "content": "CHILD-ANSWER from the child" });
    }
    if !text.contains("EXPORT-ME") {
        return json!({ "content": "Done." });
    }
    let write = json!({ "path": "a.txt", "content": "alpha <script>x</script>\n" });
    let agent = json!({ "prompt": "CHILD-TASK inspect", "agent": "explore",
        "description": "Inspect the repository" });
    let call = |index: u32, id: &str, name: &str, arguments: &Value| {
        json!({ "index": index, "id": id, "type": "function",
            "function": { "name": name, "arguments": arguments.to_string() } })
    };
    json!({ "tool_calls": [call(0, "call_write", "write", &write),
        call(1, "call_agent", "agent", &agent)] })
}

struct Env {
    _dir: tempfile::TempDir,
    root: PathBuf,
    catalog: PathBuf,
}

impl Env {
    /// A git Location (snapshots track git worktrees) whose default is the scripted model.
    fn new(model: &scripted_model::ScriptedModel) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let catalog = root.join("models.json");
        std::fs::write(&catalog, "{}").unwrap();
        let config = root.join("cyber-home/config");
        std::fs::create_dir_all(&config).unwrap();
        let mut settings = model.config();
        settings["permissions"] = json!({ "agent": "allow" });
        std::fs::write(config.join("cyber.jsonc"), settings.to_string()).unwrap();
        let repo = root.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let git = Command::new("git")
            .args(["init", "-q", "-b", "main"])
            .current_dir(&repo)
            .status()
            .unwrap();
        assert!(git.success());
        Self {
            _dir: dir,
            root,
            catalog,
        }
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_cyber"));
        command
            .args(args)
            .env_clear()
            .env("HOME", self.root.join("home"))
            .env("CYBER_HOME", self.root.join("cyber-home"))
            .env("CYBER_MODELS_PATH", &self.catalog)
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .current_dir(self.repo());
        #[cfg(windows)]
        if let Some(system) = std::env::var_os("SystemRoot") {
            command.env("SystemRoot", system);
        }
        command
    }

    fn repo(&self) -> PathBuf {
        self.root.join("repo")
    }

    fn cyber(&self, args: &[&str]) -> Output {
        self.command(args).output().unwrap()
    }

    /// Run `EXPORT-ME` with a secret in the prompt; returns the Session ID.
    fn session(&self) -> String {
        let args = [
            "exec",
            "--mode",
            "accept-edits",
            "--format",
            "json",
            "EXPORT-ME PROMPT-SECRET",
        ];
        let output = self.cyber(&args);
        let text = exits(&output, 0);
        let done: Value =
            serde_json::from_slice(&output.stdout).unwrap_or_else(|_| panic!("{text}"));
        done["session_id"].as_str().unwrap().to_owned()
    }

    fn export(&self, args: &[&str]) -> String {
        let mut all = vec!["sessions", "export"];
        all.extend(args);
        let output = self.cyber(&all);
        exits(&output, 0);
        String::from_utf8(output.stdout).unwrap()
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = self.cyber(&["service", "stop"]);
    }
}

fn exits(output: &Output, code: i32) -> String {
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.status.code(), Some(code), "{text}");
    text
}

/// The tags the page may contain; anything else means Session content became markup.
const TAGS: &[&str] = &[
    "!doctype",
    "html",
    "head",
    "meta",
    "title",
    "style",
    "body",
    "main",
    "header",
    "footer",
    "section",
    "article",
    "div",
    "span",
    "time",
    "p",
    "pre",
    "code",
    "details",
    "summary",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "dl",
    "dt",
    "dd",
    "ul",
    "ol",
    "li",
    "strong",
    "em",
    "del",
    "sup",
    "sub",
    "blockquote",
    "table",
    "thead",
    "tbody",
    "tr",
    "th",
    "td",
    "br",
    "hr",
];

const CSP: &str = "<meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; style-src 'unsafe-inline'\">";

/// Every `<...>` tag of `html`; Session content is escaped, so each is the renderer's.
fn tags(html: &str) -> Vec<&str> {
    let mut found = Vec::new();
    let mut rest = html;
    while let Some(start) = rest.find('<') {
        let tail = &rest[start..];
        let end = tail.find('>').expect("an unclosed tag");
        found.push(&tail[..=end]);
        rest = &tail[end + 1..];
    }
    found
}

/// The page has no script, no external resource and no link, and every tag is known.
fn assert_self_contained(html: &str) {
    assert!(html.starts_with("<!doctype html>\n"), "{html}");
    assert!(html.contains(CSP), "{html}");
    assert!(!html.contains("url("), "no CSS url()");
    for tag in tags(html) {
        let name = tag
            .trim_start_matches(['<', '/'])
            .split(|c: char| c.is_whitespace() || c == '>')
            .next()
            .unwrap()
            .to_ascii_lowercase();
        assert!(TAGS.contains(&name.as_str()), "unexpected tag {tag}");
        let lower = tag.to_ascii_lowercase();
        for attribute in [
            " src=",
            " href=",
            "onerror",
            "onload",
            "http:",
            "https:",
            "javascript:",
        ] {
            assert!(!lower.contains(attribute), "{attribute} in {tag}");
        }
    }
}

/// Every `[redacted:<kind>:<n>]` marker in `text`.
fn markers(text: &str) -> std::collections::BTreeSet<String> {
    let mut found = std::collections::BTreeSet::new();
    let mut rest = text;
    while let Some(start) = rest.find("[redacted:") {
        let tail = &rest[start..];
        let end = tail.find(']').unwrap();
        found.insert(tail[..=end].to_owned());
        rest = &tail[end + 1..];
    }
    found
}

/// One Session, exported as HTML to a file and to stdout, with children, sanitized, and
/// through `--output` edge cases.
#[test]
fn sessions_export_to_json_and_self_contained_html() {
    let model = scripted_model::ScriptedModel::start(reply);
    let env = Env::new(&model);
    let id = env.session();
    html_is_self_contained_and_shows_every_message(&env, &id);
    children_nest_under_the_call_that_started_them(&env, &id);
    sanitize_redacts_the_same_in_json_and_html(&env, &id);
    output_is_atomic_and_never_follows_a_symlink(&env, &id);
}

/// The scenarios "Self-contained HTML" and "Injected markup is inert".
fn html_is_self_contained_and_shows_every_message(env: &Env, id: &str) {
    let path = env.repo().join("run.html");
    let printed = env.export(&[id, "--format", "html", "--output", path.to_str().unwrap()]);
    assert_eq!(printed, "", "--output writes nothing to stdout");
    let html = std::fs::read_to_string(&path).unwrap();
    assert_self_contained(&html);
    assert_self_contained(&env.export(&[id, "--format", "html"]));
    for shown in [
        "<h1>Export demo</h1>",
        "<dt>Turns</dt><dd>1</dd>",
        "300 in · 30 out · 0 reasoning",
        "test/main",
        "<p>EXPORT-ME PROMPT-SECRET</p>",
        // Injected markup and links are text.
        "&lt;img src=x onerror=alert(1)&gt;",
        "<span class=\"link\">x</span> <span class=\"url\">(javascript:alert(1))</span>",
        "<span class=\"url\">(https://example.com/SECRET-URL)</span>",
        "<strong>bold</strong> and <code>code</code>",
        "<pre><code>fn main() {}\n</code></pre>",
        // Reasoning is collapsed; tool calls are collapsible with input and output.
        "<details class=\"reasoning\"><summary>Reasoning</summary>",
        "REASONING-SECRET weighing it",
        "<span class=\"tool-name\">write</span> <span class=\"status\">ok</span>",
        "&quot;content&quot;: &quot;alpha &lt;script&gt;x&lt;/script&gt;\\n&quot;",
        "<pre>Created a.txt</pre>",
        "<span class=\"tool-name\">agent</span>",
        "Exported by cyber ",
    ] {
        assert!(html.contains(shown), "{shown}\n{html}");
    }
    assert!(!html.contains("<details class=\"reasoning\" open"));
    assert!(
        !html.contains("class=\"child\""),
        "children only when asked"
    );
}

/// `--include-children` nests the subagent's Session after the `agent` call, before
/// the answer that followed it; JSON nests it as a document of its own.
fn children_nest_under_the_call_that_started_them(env: &Env, id: &str) {
    let doc: Value = serde_json::from_str(&env.export(&[id, "--include-children"])).unwrap();
    let children = doc["children"].as_array().unwrap();
    assert_eq!(children.len(), 1, "{doc}");
    assert_eq!(children[0]["session"]["parent_id"], id);
    assert_eq!(
        children[0]["messages"][1]["text"],
        "CHILD-ANSWER from the child"
    );
    let html = env.export(&[id, "--include-children", "--format", "html"]);
    assert_self_contained(&html);
    let at = |needle: &str| {
        html.find(needle)
            .unwrap_or_else(|| panic!("{needle}\n{html}"))
    };
    let call = at("<span class=\"tool-name\">agent</span>");
    let child = at("<section class=\"child\">");
    assert!(
        call < child && child < at("<strong>bold</strong>"),
        "{html}"
    );
    assert!(at("<p>CHILD-ANSWER from the child</p>") > child);
    assert!(at("<p>CHILD-TASK inspect</p>") > child);
}

/// The scenario "Sanitized export", for JSON and HTML alike: no prompt, response, path
/// or URL is left, token totals stay, and the page shows the JSON's markers only.
fn sanitize_redacts_the_same_in_json_and_html(env: &Env, id: &str) {
    let plain: Value = serde_json::from_str(&env.export(&[id, "--include-children"])).unwrap();
    let text = env.export(&[id, "--include-children", "--sanitize"]);
    let doc: Value = serde_json::from_str(&text).unwrap();
    let html = env.export(&[id, "--include-children", "--sanitize", "--format", "html"]);
    assert_self_contained(&html);
    let location = env.root.file_name().unwrap().to_str().unwrap().to_owned();
    for secret in [
        "PROMPT-SECRET",
        "REASONING-SECRET",
        "SECRET-URL",
        "a.txt",
        "alpha",
        "CHILD-TASK",
        "CHILD-ANSWER",
        "Export demo",
        "onerror",
        location.as_str(),
    ] {
        assert!(!text.contains(secret), "{secret} in the JSON:\n{text}");
        assert!(!html.contains(secret), "{secret} in the page:\n{html}");
    }
    assert_eq!(doc["sanitized"], true);
    assert_eq!(doc["totals"], plain["totals"]);
    assert_eq!(doc["session"]["id"], id);
    let ids = |d: &Value| -> Vec<Value> {
        d["messages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["id"].clone())
            .collect()
    };
    assert_eq!(ids(&doc), ids(&plain));
    let (json_markers, page_markers) = (markers(&text), markers(&html));
    assert!(!page_markers.is_empty());
    assert!(
        page_markers.is_subset(&json_markers),
        "the page shows markers the JSON lacks: {:?}",
        page_markers.difference(&json_markers).collect::<Vec<_>>()
    );
    let prompt = doc["messages"][0]["parts"][0]["text"].as_str().unwrap();
    let title = doc["session"]["title"].as_str().unwrap();
    assert!(html.contains(&format!("<p>{prompt}</p>")), "{html}");
    assert!(html.contains(&format!("<h1>{title}</h1>")), "{html}");
    assert!(html.contains("<footer>Exported by cyber ") && html.contains("Sanitized:"));
}

/// `--output` replaces a file atomically and keeps its permissions, refuses a symlink
/// without touching its target, and fails when the directory is missing.
fn output_is_atomic_and_never_follows_a_symlink(env: &Env, id: &str) {
    let existing = env.repo().join("run.json");
    std::fs::write(&existing, "old").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&existing, std::fs::Permissions::from_mode(0o640)).unwrap();
    }
    env.export(&[id, "--output", existing.to_str().unwrap()]);
    let written: Value =
        serde_json::from_str(&std::fs::read_to_string(&existing).unwrap()).unwrap();
    assert_eq!(written["session"]["id"], id);
    let missing = env.repo().join("nope").join("run.html");
    let out = env.cyber(&[
        "sessions",
        "export",
        id,
        "--output",
        missing.to_str().unwrap(),
    ]);
    let text = exits(&out, 1);
    assert!(text.contains("cannot write the export"), "{text}");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode =
            |path: &std::path::Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            mode(&existing),
            0o640,
            "an existing file keeps its permissions"
        );
        let fresh = env.repo().join("fresh.html");
        env.export(&[id, "--format", "html", "--output", fresh.to_str().unwrap()]);
        assert_eq!(mode(&fresh), 0o600, "a new export is private to its owner");

        let victim = env.root.join("victim.txt");
        std::fs::write(&victim, "keep").unwrap();
        let link = env.repo().join("link.html");
        std::os::unix::fs::symlink(&victim, &link).unwrap();
        let out = env.cyber(&[
            "sessions",
            "export",
            id,
            "--format",
            "html",
            "--output",
            link.to_str().unwrap(),
        ]);
        let text = exits(&out, 1);
        assert!(text.contains("symlink"), "{text}");
        assert_eq!(std::fs::read_to_string(&victim).unwrap(), "keep");
        assert!(
            std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }
    let leftovers: Vec<String> = std::fs::read_dir(env.repo())
        .unwrap()
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|name| name.starts_with(".cyber-export-"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}

/// `--format html` is refused outside `cyber sessions export`, and export refuses the
/// formats it does not write, before contacting a server.
#[test]
fn html_format_is_only_for_export() {
    let model = scripted_model::ScriptedModel::start(reply);
    let env = Env::new(&model);
    let text = exits(&env.cyber(&["sessions", "list", "--format", "html"]), 2);
    assert!(
        text.contains("only for \"cyber sessions export\""),
        "{text}"
    );
    let text = exits(&env.cyber(&["--format", "html", "stats"]), 2);
    assert!(text.contains("only for"), "{text}");
    let text = exits(
        &env.cyber(&["sessions", "export", "x", "--format", "text"]),
        2,
    );
    assert!(text.contains("writes --format json, md or html"), "{text}");
}
