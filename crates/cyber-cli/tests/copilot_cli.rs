//! `cyber providers login|logout copilot` and Turns on `copilot/<model>` through the real
//! binary (`provider-credentials` → GitHub Copilot subscription login, `provider-catalog` →
//! GitHub Copilot subscription provider). An in-process fake GitHub and Copilot API on
//! 127.0.0.1 answer the device flow, the token mint, the model list and the model calls;
//! nothing reaches the internet.
// Login refuses on Windows, so only the refusal test runs there and the fixtures go unused.
#![cfg_attr(windows, allow(dead_code, unused_imports))]
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::{Arc, Mutex};

use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

const GITHUB_TOKEN: &str = "gho_fake0123456789abcdefGHIJ";
const COPILOT_TOKEN: &str = "tid=0123456789abcdef;exp=4000000000;sku=copilot_pro;8kp=1:ffee";

#[derive(Default)]
struct Seen {
    /// The form of each device-code request.
    codes: Vec<String>,
    /// `(path, x-initiator, authorization)` of each model call.
    calls: Vec<(String, String, String)>,
    /// The JSON body of each model call but title generation.
    bodies: Vec<Value>,
    /// The `x-initiator` of each title-generation call.
    titles: Vec<String>,
}

struct GitHub {
    origin: String,
    seen: Arc<Mutex<Seen>>,
    _runtime: tokio::runtime::Runtime,
}

fn header(headers: &HeaderMap, name: &str) -> String {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned()
}

fn sse(events: &[(&str, Value)]) -> String {
    events
        .iter()
        .map(|(name, data)| match name {
            &"" => format!("data: {data}\n\n"),
            name => format!("event: {name}\ndata: {data}\n\n"),
        })
        .collect()
}

fn stream(body: String) -> Response {
    ([("content-type", "text/event-stream")], body).into_response()
}

const HOOK_SYSTEM: &str = "Judge the supplied hook event";
const SUMMARY_SYSTEM: &str = "provided web page content";
const CHILD_TASK: &str = "CHILD TASK";

/// What a model call is for: a prompt hook check, a `webfetch` summary, a child
/// Session's call or a call of the Turn itself.
fn kind(body: &Value) -> &'static str {
    let text = body.to_string();
    if text.contains(HOOK_SYSTEM) {
        "hook"
    } else if text.contains(SUMMARY_SYSTEM) {
        "summary"
    } else if text.contains(CHILD_TASK) && !text.contains("delegate") {
        "child"
    } else {
        "turn"
    }
}

/// The tool call the prompt asks for: `webfetch` with a prompt, an `agent` delegation or,
/// by default, a `read`.
fn tool_call(body: &Value) -> Value {
    let text = body.to_string();
    let (name, arguments) = if text.contains("fetch the page") {
        let url: String = text
            .split("PAGE=")
            .nth(1)
            .unwrap_or_default()
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || ":/.".contains(*c))
            .collect();
        (
            "webfetch",
            json!({"url": url, "prompt": "what does it say"}),
        )
    } else if text.contains("delegate") {
        (
            "agent",
            json!({"prompt": CHILD_TASK, "description": "answer the child task"}),
        )
    } else {
        ("read", json!({"path": "notes.txt"}))
    };
    json!({"index": 0, "id": format!("call_{name}"), "type": "function",
        "function": {"name": name, "arguments": arguments.to_string()}})
}

/// A Chat Completions Turn: a tool call first, then text once the tool result is back.
/// Calls without tools (the title), hook checks, summaries and child Sessions get text.
fn chat(body: &Value) -> Response {
    let answered = body["tools"].is_null()
        || kind(body) != "turn"
        || body["messages"]
            .as_array()
            .is_some_and(|messages| messages.iter().any(|m| m["role"] == "tool"));
    let (delta, finish) = if kind(body) == "hook" {
        let decision = json!({"decision": "allow", "reason": "reading is fine"});
        (json!({"content": decision.to_string()}), "stop")
    } else if answered {
        (json!({"content": "read it"}), "stop")
    } else {
        (json!({"tool_calls": [tool_call(body)]}), "tool_calls")
    };
    let usage = json!({"choices": [], "usage": {"prompt_tokens": 3, "completion_tokens": 1}});
    let choice = json!({"choices": [{"delta": delta, "finish_reason": finish}]});
    stream(sse(&[("", choice), ("", usage)]) + "data: [DONE]\n\n")
}

fn model_list() -> Value {
    json!({"object": "list", "data": [
        {"id": "claude-sonnet-4.5", "name": "Claude Sonnet 4.5", "model_picker_enabled": true,
         "policy": {"state": "enabled"},
         "capabilities": {"limits": {"max_context_window_tokens": 200000,
            "max_output_tokens": 16000}, "supports": {"tool_calls": true}},
         "supported_endpoints": ["/chat/completions"]},
        {"id": "gpt-5.6-sol", "name": "GPT-5.6 Sol", "model_picker_enabled": true,
         "capabilities": {"supports": {"tool_calls": true}},
         "supported_endpoints": ["/responses"]},
        {"id": "org-blocked", "model_picker_enabled": true, "policy": {"state": "disabled"}},
        {"id": "embedding", "model_picker_enabled": false}
    ]})
}

impl GitHub {
    /// The fake GitHub and Copilot API; `mint_status` answers every Copilot token mint.
    fn start(mint_status: u16) -> Self {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        let seen = Arc::new(Mutex::new(Seen::default()));
        let listener = runtime
            .block_on(tokio::net::TcpListener::bind("127.0.0.1:0"))
            .unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let router = Self::router(seen.clone(), origin.clone(), mint_status);
        runtime.spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        Self {
            origin,
            seen,
            _runtime: runtime,
        }
    }

    fn router(seen: Arc<Mutex<Seen>>, origin: String, mint_status: u16) -> axum::Router {
        use axum::routing::{get, post};
        let codes = seen.clone();
        let (chats, responses) = (seen.clone(), seen);
        let authorized = |headers: &HeaderMap| header(headers, "authorization");
        axum::Router::new()
            .route(
                "/login/device/code",
                post(move |form: String| async move {
                    codes.lock().unwrap().codes.push(form);
                    axum::Json(
                        json!({"device_code": "device-secret-1", "user_code": "ABCD-1234",
                        "verification_uri": "https://github.com/login/device",
                        "interval": 1, "expires_in": 900}),
                    )
                }),
            )
            .route(
                "/login/oauth/access_token",
                post(|| async { axum::Json(json!({"access_token": GITHUB_TOKEN})) }),
            )
            .route(
                "/copilot_internal/v2/token",
                get(move |headers: HeaderMap| async move {
                    if authorized(&headers) != format!("token {GITHUB_TOKEN}") {
                        return StatusCode::UNAUTHORIZED.into_response();
                    }
                    if mint_status != 200 {
                        return StatusCode::from_u16(mint_status).unwrap().into_response();
                    }
                    axum::Json(
                        json!({"token": COPILOT_TOKEN, "expires_at": 4_000_000_000u64,
                        "endpoints": {"api": origin}}),
                    )
                    .into_response()
                }),
            )
            .route("/page", get(|| async { "the page says hello" }))
            .route(
                "/user",
                get(|| async { axum::Json(json!({"login": "octocat"})) }),
            )
            .route(
                "/models",
                get(move |headers: HeaderMap| async move {
                    if authorized(&headers) != format!("Bearer {COPILOT_TOKEN}") {
                        return StatusCode::UNAUTHORIZED.into_response();
                    }
                    axum::Json(model_list()).into_response()
                }),
            )
            .route(
                "/chat/completions",
                post(
                    move |headers: HeaderMap, axum::Json(body): axum::Json<Value>| async move {
                        record(&chats, "/chat/completions", &headers, &body);
                        chat(&body)
                    },
                ),
            )
            .route(
                "/responses",
                post(
                    move |headers: HeaderMap, axum::Json(body): axum::Json<Value>| async move {
                        record(&responses, "/responses", &headers, &body);
                        let usage = json!({"usage": {"input_tokens": 3, "output_tokens": 1}});
                        stream(sse(&[
                            (
                                "response.output_text.delta",
                                json!({"delta": "responses hi"}),
                            ),
                            ("response.completed", json!({"response": usage})),
                        ]))
                    },
                ),
            )
    }
}

fn record(seen: &Mutex<Seen>, path: &str, headers: &HeaderMap, body: &Value) {
    let mut seen = seen.lock().unwrap();
    if body.to_string().contains("Write a short title") {
        seen.titles.push(header(headers, "x-initiator"));
        return;
    }
    seen.calls.push((
        path.into(),
        header(headers, "x-initiator"),
        header(headers, "authorization"),
    ));
    seen.bodies.push(body.clone());
    for (name, value) in [
        ("copilot-integration-id", "vscode-chat"),
        ("openai-intent", "conversation-edits"),
    ] {
        assert_eq!(header(headers, name), value, "{name}");
    }
    assert!(header(headers, "editor-version").starts_with("cyber/"));
}

struct Env {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

impl Env {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::write(root.join("models.json"), "{}").unwrap();
        std::fs::write(root.join("notes.txt"), "copilot notes\n").unwrap();
        Self { _dir: dir, root }
    }

    fn cyber(&self, github: &str, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_cyber"))
            .args(["--format", "json"])
            .args(args)
            .env_clear()
            .env("HOME", self.root.join("home"))
            .env("CYBER_HOME", self.root.join("cyber"))
            .env("CYBER_MODELS_PATH", self.root.join("models.json"))
            .env("CYBER_COPILOT_GITHUB_URL", github)
            .env("CYBER_COPILOT_API_URL", github)
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .current_dir(&self.root)
            .output()
            .unwrap()
    }

    fn auth_file(&self) -> PathBuf {
        self.root.join("cyber/data/copilot-auth.json")
    }

    fn models_file(&self) -> PathBuf {
        self.root.join("cyber/data/copilot-models.json")
    }
}

fn stdout(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|_| panic!("{}", describe(output)))
}

fn describe(output: &Output) -> String {
    format!(
        "status: {:?}\nstdout: {}\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

/// No token or device code ever reaches the terminal.
fn assert_no_secrets(output: &Output) {
    let transcript = describe(output);
    for secret in [GITHUB_TOKEN, COPILOT_TOKEN, "device-secret-1"] {
        assert!(
            !transcript.contains(secret),
            "{secret} leaked: {transcript}"
        );
    }
}

#[cfg(unix)]
fn mode(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

#[cfg(unix)]
#[test]
fn device_login_stores_the_github_login_owner_only_and_lists_the_plans_models() {
    let (env, github) = (Env::new(), GitHub::start(200));
    let output = env.cyber(&github.origin, &["providers", "login", "copilot"]);
    assert!(output.status.success(), "{}", describe(&output));
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("Open https://github.com/login/device and enter ABCD-1234"),
        "{}",
        describe(&output)
    );
    assert_eq!(
        stdout(&output),
        json!({"provider": "copilot", "user": "octocat", "host": "github.com",
            "models": ["claude-sonnet-4.5", "gpt-5.6-sol"]})
    );
    assert_no_secrets(&output);
    let form = github.seen.lock().unwrap().codes[0].clone();
    assert!(form.contains("scope=read%3Auser"), "{form}");

    assert_eq!(mode(&env.auth_file()), 0o600);
    let stored: Value = serde_json::from_slice(&std::fs::read(env.auth_file()).unwrap()).unwrap();
    assert_eq!(
        stored,
        json!({"version": 1, "host": "github.com", "github_token": GITHUB_TOKEN,
            "user": "octocat"})
    );
    assert!(
        !String::from_utf8_lossy(&std::fs::read(env.models_file()).unwrap())
            .contains(COPILOT_TOKEN),
        "minted tokens are never written"
    );

    let listed = env.cyber(&github.origin, &["models", "copilot"]);
    let refs: Vec<_> = stdout(&listed)["models"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["ref"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(refs, ["copilot/claude-sonnet-4.5", "copilot/gpt-5.6-sol"]);

    let logout = env.cyber(&github.origin, &["providers", "logout", "copilot"]);
    assert_eq!(
        stdout(&logout),
        json!({"provider": "copilot", "removed": true})
    );
    assert!(!env.auth_file().exists() && !env.models_file().exists());
    let again = env.cyber(&github.origin, &["providers", "logout", "copilot"]);
    assert_eq!(
        stdout(&again),
        json!({"provider": "copilot", "removed": false})
    );
    let gone = env.cyber(&github.origin, &["models", "copilot"]);
    assert!(!gone.status.success(), "{}", describe(&gone));
}

#[cfg(unix)]
#[test]
fn an_account_without_a_copilot_plan_is_refused_and_nothing_is_stored() {
    let (env, github) = (Env::new(), GitHub::start(403));
    let output = env.cyber(&github.origin, &["providers", "login", "copilot"]);
    assert_eq!(output.status.code(), Some(1), "{}", describe(&output));
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("This GitHub account has no active Copilot subscription"),
        "{}",
        describe(&output)
    );
    assert_no_secrets(&output);
    assert!(!env.auth_file().exists() && !env.models_file().exists());
}

#[cfg(unix)]
#[test]
fn an_enterprise_host_is_stored_and_must_be_a_bare_hostname() {
    let (env, github) = (Env::new(), GitHub::start(200));
    // Regression: names URLs read as IPv4 addresses are usage errors, not panics.
    for host in ["https://evil.example/x", "foo.123", "1.2.3.4", "a.0x1"] {
        let bad = env.cyber(
            &github.origin,
            &["providers", "login", "copilot", "--enterprise", host],
        );
        assert_eq!(bad.status.code(), Some(2), "{host}: {}", describe(&bad));
        assert!(!String::from_utf8_lossy(&bad.stderr).contains("panicked"));
    }
    assert!(github.seen.lock().unwrap().codes.is_empty());
    assert!(!env.auth_file().exists());

    let output = env.cyber(
        &github.origin,
        &[
            "providers",
            "login",
            "copilot",
            "--enterprise",
            "Octo.GHE.com",
        ],
    );
    assert!(output.status.success(), "{}", describe(&output));
    assert_eq!(stdout(&output)["host"], "octo.ghe.com");
    let stored: Value = serde_json::from_slice(&std::fs::read(env.auth_file()).unwrap()).unwrap();
    assert_eq!(stored["host"], "octo.ghe.com");

    let chatgpt = env.cyber(
        &github.origin,
        &[
            "providers",
            "login",
            "chatgpt",
            "--enterprise",
            "octo.ghe.com",
        ],
    );
    assert_eq!(chatgpt.status.code(), Some(2), "{}", describe(&chatgpt));
}

/// `provider-catalog` → Premium requests counted once per prompt: the first model call of
/// the prompt is the user's, the call after the tool result is the agent's.
#[cfg(unix)]
#[test]
fn exec_turns_carry_the_copilot_token_and_count_one_user_request_per_prompt() {
    let (env, github) = (Env::new(), GitHub::start(200));
    let login = env.cyber(&github.origin, &["providers", "login", "copilot"]);
    assert!(login.status.success(), "{}", describe(&login));

    let exec = |model: &str| {
        env.cyber(
            &github.origin,
            &[
                "exec",
                "--embedded",
                "--ephemeral",
                "-m",
                model,
                "read my notes",
            ],
        )
    };
    let chat = exec("copilot/claude-sonnet-4.5");
    assert!(chat.status.success(), "{}", describe(&chat));
    assert!(String::from_utf8_lossy(&chat.stdout).contains("read it"));
    assert_no_secrets(&chat);
    let responses = exec("copilot/gpt-5.6-sol");
    assert!(responses.status.success(), "{}", describe(&responses));
    assert!(String::from_utf8_lossy(&responses.stdout).contains("responses hi"));

    let seen = github.seen.lock().unwrap();
    let calls: Vec<_> = seen
        .calls
        .iter()
        .map(|(path, initiator, _)| (path.as_str(), initiator.as_str()))
        .collect();
    assert_eq!(
        calls,
        [
            ("/chat/completions", "user"),
            ("/chat/completions", "agent"),
            ("/responses", "user"),
        ]
    );
    assert!(
        seen.titles.iter().all(|initiator| initiator == "agent"),
        "a title is never a new request: {:?}",
        seen.titles
    );
    for (_, _, authorization) in &seen.calls {
        assert_eq!(authorization, &format!("Bearer {COPILOT_TOKEN}"));
    }
    let tool_result = seen.bodies[1]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["role"] == "tool")
        .unwrap()
        .to_string();
    assert!(tool_result.contains("copilot notes"), "{tool_result}");
    assert_eq!(seen.bodies[2]["model"], "gpt-5.6-sol");
    assert_eq!(seen.bodies[2]["store"], false);
}

/// Regression: the model calls a Turn makes besides its own (prompt hook checks,
/// `webfetch` summaries on the `small` role, a child Session's calls) are the agent's, so
/// each prompt still counts as one premium request.
#[cfg(unix)]
#[test]
fn hook_checks_page_summaries_and_child_sessions_are_never_the_users_requests() {
    let (env, github) = (Env::new(), GitHub::start(200));
    let login = env.cyber(&github.origin, &["providers", "login", "copilot"]);
    assert!(login.status.success(), "{}", describe(&login));
    let config = env.root.join("cyber/config");
    std::fs::create_dir_all(&config).unwrap();
    let hook = json!({"type": "prompt", "id": "guard", "prompt": "Allow reading notes"});
    std::fs::write(
        config.join("cyber.jsonc"),
        json!({"small_model": "copilot/claude-sonnet-4.5",
            "hooks": {"PreToolUse": [{"matcher": "read", "hooks": [hook]}]}})
        .to_string(),
    )
    .unwrap();
    let page = format!("fetch the page PAGE={}/page", github.origin);
    for prompt in ["read my notes", page.as_str(), "delegate the work"] {
        let output = env.cyber(
            &github.origin,
            &[
                "exec",
                "--embedded",
                "--ephemeral",
                "--dangerously-skip-permissions",
                "-m",
                "copilot/claude-sonnet-4.5",
                prompt,
            ],
        );
        assert!(output.status.success(), "{prompt}: {}", describe(&output));
        assert_no_secrets(&output);
    }
    let seen = github.seen.lock().unwrap();
    let calls: Vec<_> = seen
        .bodies
        .iter()
        .zip(&seen.calls)
        .map(|(body, (_, initiator, _))| (kind(body), initiator.as_str()))
        .collect();
    for expected in ["hook", "summary", "child"] {
        assert!(
            calls.iter().any(|(kind, _)| *kind == expected),
            "{expected}: {calls:?}"
        );
    }
    let users: Vec<_> = calls.iter().filter(|(_, i)| *i == "user").collect();
    assert_eq!(users, [&("turn", "user"); 3], "{calls:?}");
}

#[cfg(windows)]
#[test]
fn copilot_login_is_refused_on_windows() {
    let env = Env::new();
    let output = env.cyber("http://127.0.0.1:9", &["providers", "login", "copilot"]);
    assert_eq!(output.status.code(), Some(2), "{}", describe(&output));
    assert!(String::from_utf8_lossy(&output.stderr).contains("not supported on Windows"));
    assert!(!env.auth_file().exists());
}

/// The `cyber` commands of the first `bash` block after `heading` in `doc` (a path from the
/// workspace root), each split into its arguments with the binary and comments dropped.
fn documented_commands(doc: &str, heading: &str) -> Vec<Vec<String>> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join(doc);
    let text = std::fs::read_to_string(&path).unwrap();
    let section = &text[text
        .find(heading)
        .unwrap_or_else(|| panic!("{heading} in {doc}"))..];
    let block = section.split("```bash\n").nth(1).unwrap();
    let block = &block[..block.find("```").unwrap()];
    block
        .lines()
        .map(|line| line.split(" #").next().unwrap().trim())
        .filter_map(|line| {
            line.strip_prefix("./target/debug/cyber ")
                .or_else(|| line.strip_prefix("cyber "))
        })
        .map(split_args)
        .collect()
}

/// Whitespace-separated words; a double-quoted word may hold spaces.
fn split_args(line: &str) -> Vec<String> {
    line.split('"')
        .enumerate()
        .flat_map(|(index, part)| {
            if index % 2 == 1 {
                vec![part.to_owned()]
            } else {
                part.split_whitespace().map(str::to_owned).collect()
            }
        })
        .collect()
}

/// Regression guard for `docs/local-core-guide.md`: every documented Copilot command runs
/// as written against the fake GitHub (exec kept in-process and ephemeral), the
/// walkthrough ends signed out, and the README (which no longer carries its own
/// walkthrough) links the guide's section.
#[cfg(unix)]
#[test]
fn the_documented_copilot_walkthroughs_run_as_written() {
    for (doc, heading) in [(
        "docs/local-core-guide.md",
        "## Signing in with a GitHub Copilot subscription",
    )] {
        let (env, github) = (Env::new(), GitHub::start(200));
        let commands = documented_commands(doc, heading);
        assert!(commands.len() >= 5, "{doc}: {commands:?}");
        for mut args in commands {
            if args[0] == "exec" {
                args.splice(1..1, ["--embedded".into(), "--ephemeral".into()]);
            }
            let args: Vec<&str> = args.iter().map(String::as_str).collect();
            let output = env.cyber(&github.origin, &args);
            assert!(
                output.status.success(),
                "{doc}: {args:?}\n{}",
                describe(&output)
            );
            assert_no_secrets(&output);
        }
        assert!(!env.auth_file().exists(), "{doc} ends signed out");
    }
    let readme =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../README.md"))
            .unwrap();
    assert!(
        readme.contains("docs/local-core-guide.md#signing-in-with-a-github-copilot-subscription")
    );
}
