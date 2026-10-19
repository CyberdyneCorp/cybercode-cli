//! `peers` configuration and `cyber peers` through the real binary
//! (`cross-session-messaging` → Direct peer transport, Peer grants). Nothing here
//! reaches another machine; children end on their own, so the tests also run on Windows.

use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

use serde_json::{Value, json};

const FINGERPRINT: &str = "sha256:9f2c000000000000000000000000000000000000000000000000000000000001";
const TOKEN: &str = "cyp_pgr01HZX0ABCDEF_Qm9vbGVhbi1zZWNyZXQtdmFsdWUtNDMtY2hhcnMtbG9uZw";

struct Env {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

impl Env {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join("cyber-home/config")).unwrap();
        std::fs::create_dir_all(root.join("repo/.git")).unwrap();
        Self { _dir: dir, root }
    }

    fn user_config(&self, value: &Value) {
        let path = self.root.join("cyber-home/config/cyber.jsonc");
        std::fs::write(path, value.to_string()).unwrap();
    }

    fn project_config(&self, file: &str, value: &Value) {
        let path = self.root.join("repo").join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, value.to_string()).unwrap();
    }

    fn cyber(&self, args: &[&str]) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_cyber"));
        command
            .args(args)
            .env_clear()
            .env("HOME", self.root.join("home"))
            .env("CYBER_HOME", self.root.join("cyber-home"))
            .env("DESK_PEER_TOKEN", TOKEN)
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .current_dir(self.root.join("repo"))
            .stdin(Stdio::null());
        #[cfg(windows)]
        if let Some(system) = std::env::var_os("SystemRoot") {
            command.env("SystemRoot", system);
        }
        command.output().unwrap()
    }
}

fn peers() -> Value {
    json!({
        "listen": {"address": "0.0.0.0:4748"},
        "machine": "laptop",
        "remotes": [{"name": "desk", "url": "https://desk.local:4748",
            "fingerprint": FINGERPRINT, "token": "{env:DESK_PEER_TOKEN}"}],
    })
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// User config holds `peers`; the resolved configuration shows it with the token
/// redacted, and the token never reaches the output.
#[test]
fn user_peers_config_resolves_with_the_token_redacted() {
    let env = Env::new();
    env.user_config(&json!({ "peers": peers() }));
    let output = env.cyber(&["debug", "config"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!stdout.contains(TOKEN), "{stdout}");
    let value: Value = serde_json::from_str(&stdout).unwrap();
    let remote = &value["peers"]["remotes"][0];
    assert_eq!(
        (&remote["name"], &remote["token"]),
        (&json!("desk"), &json!("***"))
    );
    assert_eq!(value["peers"]["listen"]["address"], "0.0.0.0:4748");
}

/// Project and local config may not set `peers`, approved or not: the command fails with
/// exit 2 and a message naming the file and `cyber peers add`.
#[test]
fn project_and_local_peers_config_is_refused() {
    for file in [
        ".cyber/cyber.jsonc",
        ".cyber/cyber.local.jsonc",
        "cyber.json",
    ] {
        let env = Env::new();
        env.project_config(file, &json!({ "peers": peers() }));
        let name = file.rsplit('/').next().unwrap();
        let output = env.cyber(&["debug", "config"]);
        assert_eq!(output.status.code(), Some(2), "{file}: {}", stderr(&output));
        let error = stderr(&output);
        assert!(
            error.contains("ConfigInvalidError")
                && error.contains("peers: accepted only in user config"),
            "{error}"
        );
        assert!(
            error.contains("not in project:") && error.contains(&format!("{name}; remove")),
            "{error}"
        );
        assert!(error.contains("cyber peers add"), "{error}");
    }
}

/// Each peer rule of user config fails loading with a message naming the path, and the
/// remote rules name `cyber peers add`.
#[test]
fn invalid_user_peers_config_is_refused_with_messages() {
    let remote =
        |name: &str, url: &str| json!({"name": name, "url": url, "fingerprint": FINGERPRINT});
    for (peers, path, names_add) in [
        (
            json!({"listen": {"address": "0.0.0.0:4748", "tls": false}}),
            "peers.listen.tls:",
            false,
        ),
        (
            json!({"remotes": [remote("desk", "http://desk.local:4748")]}),
            "peers.remotes[0].url:",
            true,
        ),
        (
            json!({"remotes": [remote("desk", "https://a:1"), remote("desk", "https://b:1")]}),
            "peers.remotes[1].name:",
            true,
        ),
        (json!([remote("desk", "https://desk:4748")]), "peers:", true),
    ] {
        let env = Env::new();
        env.user_config(&json!({ "peers": peers }));
        let output = env.cyber(&["debug", "config"]);
        assert_eq!(
            output.status.code(),
            Some(2),
            "{peers}: {}",
            stderr(&output)
        );
        let error = stderr(&output);
        assert!(error.contains(path), "{path}: {error}");
        assert_eq!(error.contains("cyber peers add"), names_add, "{error}");
    }
}

/// One machine of a two-machine test: its own home, config, state and background server.
struct Machine {
    env: Env,
    vars: Vec<(String, String)>,
}

impl Machine {
    fn new(peers: Value) -> Self {
        let env = Env::new();
        env.user_config(&json!({ "peers": peers }));
        Self {
            env,
            vars: Vec::new(),
        }
    }

    fn cyber(&self, args: &[&str]) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_cyber"));
        command
            .args(args)
            .env_clear()
            .env("HOME", self.env.root.join("home"))
            .env("CYBER_HOME", self.env.root.join("cyber-home"))
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .envs(self.vars.iter().map(|(k, v)| (k, v)))
            .current_dir(self.env.root.join("repo"))
            .stdin(Stdio::null());
        #[cfg(windows)]
        if let Some(system) = std::env::var_os("SystemRoot") {
            command.env("SystemRoot", system);
        }
        command.output().unwrap()
    }

    fn user_config_text(&self) -> String {
        std::fs::read_to_string(self.env.root.join("cyber-home/config/cyber.jsonc")).unwrap()
    }
}

impl Drop for Machine {
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

fn stdout_json(output: &Output) -> Value {
    exits(output, 0);
    serde_json::from_slice(&output.stdout).unwrap()
}

/// The pairing code line of `cyber peers invite`.
fn pairing_code(output: &str) -> String {
    let start = output.find("cyberpeer://").expect(output);
    output[start..]
        .split_whitespace()
        .next()
        .unwrap()
        .to_string()
}

fn code_field<'a>(code: &'a str, key: &str) -> &'a str {
    let query = code.split_once("/?").unwrap().1;
    query
        .split('&')
        .find_map(|pair| pair.strip_prefix(&format!("{key}=")))
        .unwrap()
}

/// Task 4.3, scenario "Pair two machines", through two real `cyber` binaries with separate
/// homes: `invite` on desk (its peer listener on 127.0.0.1 with TLS) prints a pairing code
/// once; `add` on laptop tests it before storing anything, writes the remote and prints the
/// reverse-pairing hint; `test` prints desk, its version and scope `message`; desk's
/// database holds the token hash, not the token; `list` on desk flags the grant that cannot
/// reply and on laptop shows desk online. A wrong pin stores nothing; `revoke` makes the
/// next `test` fail with `PeerAuthError` `unauthorized`; `invite --rotate-key` swaps the
/// running listener's key so a stale pin fails with `fingerprint_mismatch`; `rm` removes the
/// remote. The token goes through `{env:NAME}` here so the test never writes the user's
/// keyring; the file fallback runs where it is the platform's store (Linux).
#[test]
fn two_binaries_pair_and_test() {
    let desk = Machine::new(json!({"listen": {"address": "127.0.0.1:0"}, "machine": "desk"}));
    let mut laptop = Machine::new(json!({"machine": "laptop"}));
    exits(&desk.cyber(&["service", "start"]), 0);
    let code = invite(&desk);
    refuse_another_pin(&laptop, &code);
    let token = add_with_env_token(&mut laptop, &code);
    test_and_inspect(&desk, &laptop, &token);
    listings(&desk, &laptop);
    send_needs_a_from_session(&desk, &laptop);
    file_fallback(&laptop, &code);
    revoke_rotate_and_remove(&desk, &laptop, &code);
}

/// `cyber peers invite laptop` on desk: its pairing code.
fn invite(desk: &Machine) -> String {
    let invited = exits(&desk.cyber(&["peers", "invite", "laptop"]), 0);
    assert!(
        invited.contains("on laptop: cyber peers add desk '<code>'"),
        "{invited}"
    );
    let code = pairing_code(&invited);
    assert!(code.starts_with("cyberpeer://127.0.0.1:"), "{code}");
    assert_eq!(code_field(&code, "machine"), "desk");
    let token = code_field(&code, "t").to_string();
    assert!(token.starts_with("cyp_pgr"), "{code}");
    code
}

/// A code with another pin fails the handshake and stores nothing.
fn refuse_another_pin(laptop: &Machine, code: &str) {
    let other_pin = format!("sha256:{}", "0".repeat(64));
    let forged = code.replace(code_field(code, "fp"), &other_pin);
    let refused = exits(&laptop.cyber(&["peers", "add", "desk", &forged]), 1);
    assert!(
        refused.contains("PeerAuthError") && refused.contains("fingerprint_mismatch"),
        "{refused}"
    );
    // Regression: the hint said `cyber peers rm desk` for a remote that was never added.
    assert!(
        refused.contains("nothing was stored") && !refused.contains("peers rm"),
        "{refused}"
    );
    assert!(!laptop.user_config_text().contains("remotes"));
}

/// `cyber peers add desk '<code>' --token-env`: tested, written to user config as
/// `{env:NAME}`; returns the token.
fn add_with_env_token(laptop: &mut Machine, code: &str) -> String {
    let token = code_field(code, "t").to_string();
    laptop.vars.push(("DESK_PEER_TOKEN".into(), token.clone()));
    let added = exits(
        &laptop.cyber(&[
            "peers",
            "add",
            "desk",
            code,
            "--token-env",
            "DESK_PEER_TOKEN",
        ]),
        0,
    );
    assert!(
        added.contains("added desk") && added.contains("scopes message"),
        "{added}"
    );
    assert!(
        added.contains(
            "run `cyber peers invite desk` here and `cyber peers add laptop '<code>'` on desk"
        ),
        "{added}"
    );
    let config = laptop.user_config_text();
    assert!(
        config.contains("{env:DESK_PEER_TOKEN}") && !config.contains(&token),
        "{config}"
    );

    token
}

/// `test` prints desk, its version and scopes; desk's database holds only the hash.
fn test_and_inspect(desk: &Machine, laptop: &Machine, token: &str) {
    let tested = exits(&laptop.cyber(&["peers", "test", "desk"]), 0);
    let version = String::from_utf8_lossy(&laptop.cyber(&["--version"]).stdout).into_owned();
    assert!(tested.contains("machine     desk"), "{tested}");
    assert!(tested.contains("scopes      message"), "{tested}");
    let reported = tested
        .lines()
        .find_map(|line| line.strip_prefix("version"))
        .unwrap()
        .trim()
        .to_string();
    assert!(version.contains(&reported), "{reported} / {version}");

    // desk's database holds the hash of the secret, never the token.
    let rows = stdout_json(&desk.cyber(&[
        "db",
        "query",
        "SELECT name, token_sha256, scopes FROM peer_grant",
        "--format",
        "json",
    ]));
    let stored = rows.to_string();
    let secret = &token[token.len() - 43..];
    assert!(
        stored.contains("laptop") && !stored.contains(secret),
        "{stored}"
    );
}

/// desk can receive from laptop but has no remote to reply to; laptop sees desk online.
fn listings(desk: &Machine, laptop: &Machine) {
    let listed = stdout_json(&desk.cyber(&["peers", "list", "--json"]));
    let grant = &listed["grants"][0];
    assert_eq!(
        (&grant["name"], &grant["can_reply"]),
        (&json!("laptop"), &json!(false))
    );
    let text = exits(&desk.cyber(&["peers", "list"]), 0);
    assert!(
        text.contains("can receive, cannot reply: on laptop: cyber peers invite desk"),
        "{text}"
    );
    let listed = stdout_json(&laptop.cyber(&["peers", "list", "--json"]));
    assert_eq!(listed["remotes"][0]["state"], "online", "{listed}");
    assert_eq!(listed["remotes"][0]["hello"]["machine"], "desk");
}

/// Create a root Session titled `title` through `cyber api`; its ID.
fn api_session(machine: &Machine, title: &str) -> String {
    let body = json!({ "title": title }).to_string();
    let created = stdout_json(&machine.cyber(&["api", "POST", "/sessions", "-d", &body]));
    created["data"]["id"]
        .as_str()
        .expect("a Session ID")
        .to_string()
}

/// Regression: the README's `cyber messages send migration@desk "…"` failed (exit 2)
/// because a peer target needs a sending Session; the walkthrough
/// and the help now pass `--from`. Without it the error names `--from`; with it the
/// message crosses to desk and is held there.
fn send_needs_a_from_session(desk: &Machine, laptop: &Machine) {
    let migration = api_session(desk, "migration");
    api_session(laptop, "review");
    let text = "is the schema migration done?";
    let refused = exits(
        &laptop.cyber(&["messages", "send", "migration@desk", text]),
        2,
    );
    assert!(
        refused.contains("needs a sending Session") && refused.contains("--from <session>"),
        "{refused}"
    );
    let sent = exits(
        &laptop.cyber(&[
            "messages",
            "send",
            "migration@desk",
            text,
            "--from",
            "review",
            "--format",
            "json",
        ]),
        0,
    );
    let sent: Value = serde_json::from_str(sent.trim()).expect(&sent);
    assert_eq!(sent["outcome"], "held", "{sent}");
    let history = stdout_json(&desk.cyber(&[
        "messages",
        "list",
        "--session",
        &migration,
        "--format",
        "json",
    ]));
    assert!(history.to_string().contains(text), "{history}");
}

/// `revoke`, `invite --rotate-key` and `rm`, each seen by the next `test`.
fn revoke_rotate_and_remove(desk: &Machine, laptop: &Machine, code: &str) {
    // Revoked on desk: the next request is refused.
    exits(&desk.cyber(&["peers", "revoke", "laptop"]), 0);
    let failed = exits(&laptop.cyber(&["peers", "test", "desk"]), 1);
    assert!(
        failed.contains("PeerAuthError") && failed.contains("unauthorized"),
        "{failed}"
    );
    // Regression: a send named the tag twice (`PeerAuthError: PeerAuthError: …`).
    let refused = exits(
        &laptop.cyber(&[
            "messages",
            "send",
            "migration@desk",
            "x",
            "--from",
            "review",
        ]),
        1,
    );
    assert_eq!(refused.matches("PeerAuthError").count(), 1, "{refused}");
    assert!(refused.contains("unauthorized"), "{refused}");

    // A new key on the running listener: the old pin no longer matches.
    let rotated = exits(
        &desk.cyber(&["peers", "invite", "tablet", "--rotate-key"]),
        0,
    );
    assert_ne!(
        code_field(&pairing_code(&rotated), "fp"),
        code_field(code, "fp")
    );
    let failed = exits(&laptop.cyber(&["peers", "test", "desk"]), 1);
    assert!(failed.contains("fingerprint_mismatch"), "{failed}");

    exits(&laptop.cyber(&["peers", "rm", "desk"]), 0);
    let failed = exits(&laptop.cyber(&["peers", "test", "desk"]), 1);
    assert!(failed.contains("PeerNotConfiguredError"), "{failed}");
}

/// Without `--token-env` the token goes to the platform store: on Linux the owner-only
/// file, with the warning.
#[cfg(target_os = "linux")]
fn file_fallback(laptop: &Machine, code: &str) {
    use std::os::unix::fs::PermissionsExt;
    let added = exits(&laptop.cyber(&["peers", "add", "desk2", code]), 0);
    assert!(
        added.contains("warning: peer tokens stored in file (no OS keyring)"),
        "{added}"
    );
    let file = laptop.env.root.join("cyber-home/data/peers-secrets.json");
    let mode = std::fs::metadata(&file).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600);
    assert!(!laptop.user_config_text().contains(code_field(code, "t")));
    exits(&laptop.cyber(&["peers", "test", "desk2"]), 0);
    exits(&laptop.cyber(&["peers", "rm", "desk2"]), 0);
    assert!(
        !std::fs::read_to_string(&file)
            .unwrap()
            .contains(code_field(code, "t"))
    );
}

/// macOS and Windows keep tokens in the OS keyring, which a test must not write.
#[cfg(not(target_os = "linux"))]
fn file_fallback(_laptop: &Machine, _code: &str) {}

/// Why mDNS cannot be exercised on this host, if it cannot: a probe one responder
/// publishes on the IPv4 interfaces (loopback off, as the server publishes) must be
/// resolved by another responder within 5 s. CI runners often have no multicast.
fn mdns_unavailable() -> Option<String> {
    use mdns_sd::{IfKind, ServiceDaemon, ServiceEvent, ServiceInfo};
    const PROBE: &str = "_cyberprobe._tcp.local.";
    let name = format!("probe-{}", unique());
    let publisher = match ServiceDaemon::new() {
        Ok(daemon) => daemon,
        Err(error) => return Some(error.to_string()),
    };
    let _ = publisher.disable_interface(vec![IfKind::LoopbackV4, IfKind::LoopbackV6]);
    let mut info = ServiceInfo::new(PROBE, &name, "cyber-probe.local.", "", 9, &[("v", "1")][..])
        .unwrap()
        .enable_addr_auto();
    info.set_interfaces(vec![IfKind::IPv4]);
    if let Err(error) = publisher.register(info) {
        return Some(error.to_string());
    }
    let browser = ServiceDaemon::new().unwrap();
    let events = browser.browse(PROBE).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let mut found = false;
    while let Some(left) = deadline.checked_duration_since(std::time::Instant::now()) {
        match events.recv_timeout(left) {
            Ok(ServiceEvent::ServiceResolved(service)) if service.fullname.starts_with(&name) => {
                found = true;
                break;
            }
            Ok(_) => {}
            Err(_) => break,
        }
    }
    let _ = browser.shutdown();
    let _ = publisher.shutdown();
    (!found).then(|| "a published mDNS probe was not resolved within 5 s".to_string())
}

/// A name no other run on this network uses.
fn unique() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("{}-{nanos}", std::process::id())
}

/// The entry of `cyber peers discover --json` for `machine`.
fn discover_entry(laptop: &Machine, machine: &str) -> Value {
    let found = stdout_json(&laptop.cyber(&["peers", "discover", "--timeout", "5s", "--json"]));
    found["peers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|peer| peer["machine"] == machine)
        .cloned()
        .unwrap_or_else(|| panic!("{machine} is not discovered: {found}"))
}

/// Task 8.2, scenario "Discover a desktop", through two real `cyber` binaries: desk's
/// background server publishes its peer listener (`peers.mdns`, bound to all interfaces)
/// and laptop's `cyber peers discover` lists it with its address, fingerprint and `not
/// added`. Discovery grants nothing: a code naming that address with another pin is refused
/// by `add` before any request; once desk's own code is added, discovery shows `added as
/// desk`. Skipped, with the reason, where multicast
/// does not work.
#[test]
fn discover_lists_a_published_peer() {
    if let Some(reason) = mdns_unavailable() {
        eprintln!("skipping discover_lists_a_published_peer: {reason}");
        return;
    }
    let machine = format!("desk-{}", unique());
    let desk =
        Machine::new(json!({"listen": {"address": "0.0.0.0:0"}, "machine": machine, "mdns": true}));
    let mut laptop = Machine::new(json!({"machine": "laptop"}));
    exits(&desk.cyber(&["service", "start"]), 0);
    let listener = stdout_json(&desk.cyber(&["peers", "list", "--json"]))["listener"].clone();
    let fingerprint = listener["fingerprint"].as_str().unwrap().to_string();
    let port = listener["url"]
        .as_str()
        .unwrap()
        .rsplit(':')
        .next()
        .unwrap()
        .to_string();

    let listed = exits(&laptop.cyber(&["peers", "discover", "--timeout", "5s"]), 0);
    let line = listed
        .lines()
        .find(|line| line.trim_start().starts_with(&machine))
        .unwrap_or_else(|| panic!("{machine} is not listed: {listed}"));
    assert!(
        line.contains(&fingerprint) && line.contains(&format!(":{port}")),
        "{line}"
    );
    assert!(line.ends_with("not added"), "{line}");
    let entry = discover_entry(&laptop, &machine);
    assert_eq!(entry["fingerprint"], fingerprint.as_str());
    assert_eq!(entry["remote"], Value::Null);
    let address = entry["addresses"][0].as_str().unwrap().to_string();
    assert!(address.ends_with(&format!(":{port}")), "{entry}");

    // Another pin at the discovered address is refused before anything is called.
    let other_pin = format!("sha256:{}", "0".repeat(64));
    let forged = format!("cyberpeer://{address}/?machine={machine}&fp={other_pin}&t={TOKEN}");
    let refused = exits(&laptop.cyber(&["peers", "add", "desk", &forged]), 1);
    assert!(
        refused.contains(&format!("{machine} published {fingerprint} at {address}")),
        "{refused}"
    );
    assert!(!refused.contains("PeerAuthError"), "{refused}");
    assert!(!laptop.user_config_text().contains("remotes"));

    // desk's own code pairs, and discovery then names the remote that pins its key. The
    // code names loopback: a host firewall may keep a test binary from accepting
    // connections on the network address (macOS does).
    let invited = exits(
        &desk.cyber(&["peers", "invite", "laptop", "--host", "127.0.0.1"]),
        0,
    );
    let code = pairing_code(&invited);
    assert_eq!(code_field(&code, "fp"), fingerprint);
    add_with_env_token(&mut laptop, &code);
    assert_eq!(discover_entry(&laptop, &machine)["remote"], "desk");
}

/// `cyber attach <session>@<peer>` (`cli-commands` → Attach command) through the binary:
/// an address naming no enabled remote is `PeerNotConfiguredError` (exit 2, with the
/// pairing hint) before any request, a configured peer still needs a terminal for the
/// TUI, and a server URL or local Session is not attached this way.
#[test]
fn attach_needs_a_configured_peer_and_a_terminal() {
    let env = Env::new();
    env.user_config(&json!({ "peers": peers() }));
    let output = env.cyber(&["attach", "migration@nas"]);
    let text = exits(&output, 2);
    assert!(text.contains("PeerNotConfiguredError"), "{text}");
    assert!(text.contains("cyber peers add nas"), "{text}");

    let mut disabled = peers();
    disabled["remotes"][0]["enabled"] = json!(false);
    env.user_config(&json!({ "peers": disabled }));
    let text = exits(&env.cyber(&["attach", "migration@desk"]), 2);
    assert!(text.contains("PeerNotConfiguredError"), "{text}");

    env.user_config(&json!({ "peers": peers() }));
    let text = exits(&env.cyber(&["attach", "migration@desk"]), 2);
    assert!(text.contains("the TUI needs a terminal"), "{text}");
    assert!(!text.contains(TOKEN), "{text}");

    for target in ["http://10.0.0.5:4747", "ses_01HZX0ABCDEF"] {
        let text = exits(&env.cyber(&["attach", target]), 2);
        assert!(
            text.contains("not available in this build yet"),
            "{target}: {text}"
        );
    }
    let text = exits(&env.cyber(&["attach", "a@b@c"]), 2);
    assert!(text.contains("is not a <session>@<peer> address"), "{text}");
}

/// The `peers:*` rows of `cyber doctor --format json`.
fn doctor_peers(machine: &Machine) -> Vec<Value> {
    let output = machine.cyber(&["doctor", "--format", "json"]);
    let report: Value = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|error| panic!("{error}: {}", stderr(&output)));
    report["checks"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["check"].as_str().unwrap().starts_with("peers"))
        .cloned()
        .collect()
}

/// Task 10.2, `cyber doctor`'s peer section through the real binary: before the server
/// runs it names the configured listener address and TLS; with the server running, the
/// listener's URL, TLS, fingerprint (the one pairing codes pin) and active grant count;
/// and the remotes, warning about those whose token is in the file fallback (a remote
/// with its token in config is not one of them).
#[test]
fn doctor_reports_the_peer_listener_and_file_tokens() {
    let desk = Machine::new(json!({
        "listen": {"address": "127.0.0.1:0"},
        "machine": "desk",
        "remotes": [
            {"name": "laptop", "url": "https://127.0.0.1:9", "fingerprint": FINGERPRINT},
            {"name": "nas", "url": "https://127.0.0.1:9", "fingerprint": FINGERPRINT, "token": TOKEN},
        ],
    }));
    let rows = doctor_peers(&desk);
    assert_eq!(rows.len(), 2, "{rows:?}");
    assert_eq!(rows[0]["check"], "peers:listener");
    assert_eq!(
        rows[0]["detail"], "127.0.0.1:0 · TLS on · opens when the server starts",
        "{rows:?}"
    );
    assert_eq!(
        (&rows[1]["check"], &rows[1]["status"], &rows[1]["detail"]),
        (
            &json!("peers:remotes"),
            &json!("pass"),
            &json!("laptop, nas")
        )
    );
    assert!(!rows.iter().any(|r| r.to_string().contains(TOKEN)));

    exits(&desk.cyber(&["service", "start"]), 0);
    let code = invite(&desk);
    let rows = doctor_peers(&desk);
    let listener = rows[0]["detail"].as_str().unwrap();
    assert_eq!(rows[0]["status"], "pass", "{rows:?}");
    assert!(listener.starts_with("https://127.0.0.1:"), "{listener}");
    assert!(
        listener.contains(&format!(
            "· TLS on · fingerprint {} · 1 active grant",
            code_field(&code, "fp")
        )),
        "{listener}"
    );
    assert!(!listener.contains(code_field(&code, "t")), "{listener}");
    file_tokens_are_flagged(&desk);
}

/// A token of `laptop` in the owner-only file: doctor names it, not `nas` (config token).
#[cfg(unix)]
fn file_tokens_are_flagged(desk: &Machine) {
    use std::os::unix::fs::PermissionsExt;
    let file = desk.env.root.join("cyber-home/data/peers-secrets.json");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, json!({ "peer:laptop": TOKEN }).to_string()).unwrap();
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
    let rows = doctor_peers(desk);
    assert_eq!(rows[1]["status"], "warn", "{rows:?}");
    let detail = rows[1]["detail"].as_str().unwrap();
    assert!(
        detail.starts_with("laptop, nas; tokens in the file fallback (no OS keyring): laptop in "),
        "{detail}"
    );
    assert!(!detail.contains(TOKEN), "{detail}");
}

/// Windows keeps no token file.
#[cfg(not(unix))]
fn file_tokens_are_flagged(_desk: &Machine) {}
