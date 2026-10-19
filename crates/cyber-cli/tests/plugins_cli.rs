//! `cyber plugins` through the real binary (`plugins-marketplace` → Plugin manifest,
//! Component discovery defaults, Install scopes, Plugin sources, Plugin CLI, Pure mode,
//! Trust and signing): installs land in the scope files and the lockfile, and what a
//! loaded plugin contributes shows up in `hooks list`, `commands list`, `skills list` and
//! `mcp definitions` of the same binary.
use serde_json::{Value, json};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        // Without a `\\?\` prefix on Windows, as a user types paths for git and npm.
        let root = dir.path().canonicalize().unwrap();
        let text = root.display().to_string();
        let root = PathBuf::from(text.strip_prefix(r"\\?\").unwrap_or(&text));
        std::fs::create_dir_all(root.join("repo/.git")).unwrap();
        std::fs::create_dir_all(root.join("home")).unwrap();
        Self { _dir: dir, root }
    }

    fn repo(&self) -> PathBuf {
        self.root.join("repo")
    }

    /// `cyber` for one user (`CYBER_HOME` `<root>/<user>`) at the repository.
    fn cyber_as(&self, user: &str, args: &[&str], envs: &[(&str, &str)], stdin: &str) -> Output {
        self.cyber_in(&self.repo(), user, args, envs, stdin)
    }

    /// `cyber` for one user at `location`.
    fn cyber_in(
        &self,
        location: &Path,
        user: &str,
        args: &[&str],
        envs: &[(&str, &str)],
        stdin: &str,
    ) -> Output {
        let mut child = Command::new(env!("CARGO_BIN_EXE_cyber"))
            .args(["--cwd", location.to_str().unwrap()])
            .args(args)
            .env_clear()
            .env("HOME", self.root.join("home"))
            .env("CYBER_HOME", self.root.join(user))
            .env("CYBER_DB", "test.db")
            .env("CYBER_MANAGED_POLICY_DIR", self.root.join("no-policy"))
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .envs(windows_env(&self.root))
            .envs(envs.iter().copied())
            .current_dir(&self.root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(stdin.as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
    }

    fn cyber(&self, args: &[&str]) -> Output {
        self.cyber_as("alice", args, &[], "")
    }

    fn json_as(&self, user: &str, args: &[&str], envs: &[(&str, &str)]) -> Value {
        let mut args = args.to_vec();
        args.extend(["--format", "json"]);
        let output = self.cyber_as(user, &args, envs, "");
        assert!(output.status.success(), "{args:?}: {}", explain(&output));
        serde_json::from_slice(&output.stdout).unwrap()
    }

    fn json(&self, args: &[&str]) -> Value {
        self.json_as("alice", args, &[])
    }
}

/// What git, npm and PowerShell need on Windows; nothing elsewhere.
fn windows_env(root: &Path) -> Vec<(String, std::ffi::OsString)> {
    if !cfg!(windows) {
        return Vec::new();
    }
    let mut env: Vec<_> = [
        "SystemRoot",
        "TEMP",
        "TMP",
        "ProgramFiles",
        "ComSpec",
        "PATHEXT",
    ]
    .into_iter()
    .filter_map(|name| Some((name.to_owned(), std::env::var_os(name)?)))
    .collect();
    let home = root.join("home");
    env.push(("USERPROFILE".into(), home.clone().into()));
    env.push(("APPDATA".into(), home.join("AppData/Roaming").into()));
    env.push(("LOCALAPPDATA".into(), home.join("AppData/Local").into()));
    env
}

fn explain(output: &Output) -> String {
    format!(
        "status {:?}\nstdout: {}\nstderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

/// Write `files` under `dir`, creating parents.
fn write_tree(dir: &Path, files: &[(&str, String)]) {
    for (path, text) in files {
        let path = dir.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
}

fn hook(id: &str) -> String {
    json!({"hooks":{"PostToolUse":[{"matcher":"read","hooks":[
        {"type":"command","id":id,"command":"cat >/dev/null"}]}]}})
    .to_string()
}

/// A plugin with a hook, a command colliding with the built-in `review`, a free command,
/// a skill, an agent and an MCP server.
fn review_kit(dir: &Path) {
    write_tree(
        dir,
        &[
            (
                "cyber-plugin.json",
                json!({"name":"review-kit","version":"1.0.0","description":"Review helpers"})
                    .to_string(),
            ),
            ("hooks/hooks.json", hook("kit-guard")),
            (
                "commands/review.md",
                "---\ndescription: Kit review\n---\nReview with the kit: $ARGUMENTS\n".into(),
            ),
            (
                "commands/tidy.md",
                "---\ndescription: Tidy\n---\nTidy up.\n".into(),
            ),
            (
                "skills/lint/SKILL.md",
                "---\nname: lint\ndescription: Lint the change\n---\nRun the linter.\n".into(),
            ),
            (
                "agents/helper.md",
                "---\ndescription: Helps\n---\nHelp out.\n".into(),
            ),
            (
                ".mcp.json",
                json!({"mcpServers":{"kit-docs":{"command":"${CYBER_PLUGIN_ROOT}/docs-server"}}})
                    .to_string(),
            ),
        ],
    );
}

fn plugin_hooks(listed: &Value) -> Vec<&Value> {
    listed["hooks"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|hook| hook["scope"] == "plugin")
        .collect()
}

fn state(listed: &Value, name: &str) -> Value {
    listed["plugins"]
        .as_array()
        .unwrap()
        .iter()
        .find(|plugin| plugin["name"] == name)
        .map(|plugin| plugin["state"].clone())
        .unwrap_or(Value::Null)
}

/// The entry of `list[key]` whose `name` is `name`, or `null`.
fn named(list: &Value, key: &str, name: &str) -> Value {
    list[key]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["name"] == name)
        .cloned()
        .unwrap_or(Value::Null)
}

fn succeeded(output: &Output) {
    assert!(output.status.success(), "{}", explain(output));
}

#[test]
fn install_path_plugin_registers_hooks_and_namespaces_colliding_command() {
    let f = Fixture::new();
    let kit = f.root.join("review-kit");
    review_kit(&kit);
    let output = f.cyber(&["plugins", "install", kit.to_str().unwrap()]);
    succeeded(&output);
    assert!(
        String::from_utf8_lossy(&output.stdout)
            .contains("Installed review-kit@1.0.0 (user scope, unsigned) sha256:"),
        "{}",
        explain(&output)
    );
    let registry = read_json(&f.root.join("alice/config/plugins.json"));
    assert_eq!(registry["plugins"]["review-kit"]["source"]["type"], "path");
    let listed = f.json(&["plugins", "list"]);
    assert_eq!(state(&listed, "review-kit"), "active", "{listed:#}");
    assert_eq!(listed["plugins"][0]["unsigned"], true);

    assert_components_load(&f, &kit);
    assert_changed_tree_needs_update(&f, &kit);
    assert_policy_disable_and_remove(&f);
}

/// Hooks, a namespaced command, a free command, a skill, an MCP server and an agent.
fn assert_components_load(f: &Fixture, kit: &Path) {
    let hooks = f.json(&["hooks", "list"]);
    let plugin = plugin_hooks(&hooks);
    assert_eq!(plugin.len(), 1, "{hooks:#}");
    assert_eq!(plugin[0]["handler"]["id"], "kit-guard");
    let source = plugin[0]["source"].as_str().unwrap();
    assert!(source.starts_with("plugin:review-kit@1.0.0:"), "{hooks:#}");

    let commands = f.json(&["commands", "list"]);
    assert_eq!(named(&commands, "commands", "review")["source"], "builtin");
    let namespaced = named(&commands, "commands", "review-kit:review");
    assert_eq!(namespaced["source"], "command", "{commands:#}");
    assert_eq!(namespaced["namespace"], "review-kit");
    assert_eq!(namespaced["provenance"]["winner"]["scope"], "plugin");
    assert_eq!(named(&commands, "commands", "tidy")["description"], "Tidy");

    let skills = f.json(&["skills", "list"]);
    assert_eq!(
        named(&skills, "skills", "lint")["scope"],
        "plugin",
        "{skills:#}"
    );

    let servers = f.json(&["mcp", "definitions"]);
    // `${CYBER_PLUGIN_ROOT}/docs-server`: the root substituted, the manifest's `/` kept.
    let root = kit.display().to_string();
    let command = format!(
        "{}/docs-server",
        root.strip_prefix(r"\\?\").unwrap_or(&root)
    );
    let server = named(&servers, "servers", "kit-docs");
    assert_eq!(
        server["definition"]["command"],
        json!(command),
        "{servers:#}"
    );
    let config = f.json(&["debug", "config"]);
    let system = config["agents"]["helper"]["system"].as_str().map(str::trim);
    assert_eq!(system, Some("Help out."), "{config:#}");

    let info = f.json(&["plugins", "info", "review-kit"]);
    let summary = &info["entries"][0]["manifest"]["summary"];
    assert_eq!(summary["hooks"], json!(["PostToolUse"]), "{info:#}");
    assert_eq!(summary["mcp"], json!(["kit-docs"]), "{info:#}");
    assert_eq!(summary["commands"], 2, "{info:#}");
}

/// A changed tree no longer loads until it is updated and so re-approved.
fn assert_changed_tree_needs_update(f: &Fixture, kit: &Path) {
    write_tree(kit, &[("hooks/hooks.json", hook("kit-guard-2"))]);
    let listed = f.json(&["plugins", "list"]);
    assert_eq!(state(&listed, "review-kit"), "changed", "{listed:#}");
    assert!(plugin_hooks(&f.json(&["hooks", "list"])).is_empty());
    succeeded(&f.cyber(&["plugins", "update", "review-kit"]));
    let hooks = f.json(&["hooks", "list"]);
    assert_eq!(plugin_hooks(&hooks)[0]["handler"]["id"], "kit-guard-2");
}

/// A managed `disabled` entry overrides the user scope; disabled and removed plugins
/// contribute nothing.
fn assert_policy_disable_and_remove(f: &Fixture) {
    let policy = f.root.join("policy");
    write_tree(
        &policy,
        &[(
            "cyber-policy.json",
            json!({"policy":{"plugins":{"disabled":["review-kit"]}}}).to_string(),
        )],
    );
    let managed = [("CYBER_MANAGED_POLICY_DIR", policy.to_str().unwrap())];
    let listed = f.json_as("alice", &["plugins", "list"], &managed);
    assert_eq!(state(&listed, "review-kit"), "policy", "{listed:#}");
    let reason = &listed["plugins"][0]["reason"];
    assert_eq!(reason, "blocked by organization policy");
    assert!(plugin_hooks(&f.json_as("alice", &["hooks", "list"], &managed)).is_empty());

    succeeded(&f.cyber(&["plugins", "disable", "review-kit"]));
    assert!(plugin_hooks(&f.json(&["hooks", "list"])).is_empty());
    succeeded(&f.cyber(&["plugins", "remove", "review-kit"]));
    let listed = f.json(&["plugins", "list"]);
    assert_eq!(listed["plugins"], json!([]), "{listed:#}");
}

#[test]
fn invalid_manifest_name_fails_naming_the_field() {
    let f = Fixture::new();
    let plugin = f.root.join("lint-guard");
    write_tree(
        &plugin,
        &[(
            "cyber-plugin.json",
            json!({"name":"Lint Guard","version":"1.0.0","hooks":"hooks.json"}).to_string(),
        )],
    );
    for args in [
        vec!["plugins", "install", plugin.to_str().unwrap()],
        vec!["plugins", "validate", plugin.to_str().unwrap()],
    ] {
        let output = f.cyber(&args);
        assert_eq!(
            output.status.code(),
            Some(2),
            "{args:?}: {}",
            explain(&output)
        );
        assert!(
            String::from_utf8_lossy(&output.stderr)
                .contains("cyber-plugin.json: name: invalid plugin name \"Lint Guard\""),
            "{args:?}: {}",
            explain(&output)
        );
    }
    assert!(!f.root.join("alice/config/plugins.json").exists());

    write_tree(
        &plugin,
        &[(
            "cyber-plugin.json",
            json!({"name":"lint-guard","version":"1.0.0","hooks":"hooks.json"}).to_string(),
        )],
    );
    let validated = f.json(&["plugins", "validate", plugin.to_str().unwrap()]);
    assert_eq!(validated["valid"], true, "{validated:#}");
    // The declared `hooks` path replaces the `hooks/hooks.json` default.
    write_tree(&plugin, &[("hooks.json", hook("declared-hooks"))]);
    succeeded(&f.cyber(&["plugins", "install", plugin.to_str().unwrap()]));
    assert_eq!(first_plugin_hook(&f), "declared-hooks");
}

#[test]
fn engine_mismatch_refuses_install_without_force() {
    let f = Fixture::new();
    let plugin = f.root.join("future");
    write_tree(
        &plugin,
        &[
            (
                "cyber-plugin.json",
                json!({"name":"future","version":"1.0.0","engines":{"cyber":">=99.0.0"}})
                    .to_string(),
            ),
            ("hooks/hooks.json", hook("future-guard")),
        ],
    );
    let output = f.cyber(&["plugins", "install", plugin.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(2), "{}", explain(&output));
    let stderr = String::from_utf8_lossy(&output.stderr);
    let expected = format!(
        "requires cyber >=99.0.0, running {}",
        env!("CARGO_PKG_VERSION")
    );
    assert!(stderr.contains(&expected), "{}", explain(&output));
    assert!(stderr.contains("--force"), "{}", explain(&output));
    assert!(!f.root.join("alice/config/plugins.json").exists());

    let installed = f.json(&["plugins", "install", plugin.to_str().unwrap(), "--force"]);
    assert_eq!(installed["forced"], true, "{installed:#}");
    assert_eq!(
        plugin_hooks(&f.json(&["hooks", "list"]))[0]["handler"]["id"],
        "future-guard"
    );
}

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@example.test"])
        .args(args)
        .output()
        .unwrap();
    assert!(output.status.success(), "git {args:?}: {output:?}");
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

fn read_json(path: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// A git repository holding plugin `kit` under `kit/`; returns its first commit.
fn kit_repository(source: &Path) -> String {
    std::fs::create_dir_all(source).unwrap();
    git(source, &["init", "--quiet", "--initial-branch=main"]);
    write_tree(
        source,
        &[
            (
                "kit/cyber-plugin.json",
                json!({"name":"kit","version":"1.0.0"}).to_string(),
            ),
            ("kit/hooks/hooks.json", hook("kit-first")),
        ],
    );
    git(source, &["add", "."]);
    git(source, &["commit", "--quiet", "-m", "first"]);
    git(source, &["rev-parse", "HEAD"])
}

fn locked(f: &Fixture) -> Value {
    read_json(&f.repo().join(".cyber/plugins.lock"))["plugins"]["kit"].clone()
}

fn first_plugin_hook(f: &Fixture) -> Value {
    plugin_hooks(&f.json(&["hooks", "list"]))[0]["handler"]["id"].clone()
}

/// The lockfile pins `commit` and the digest, the scope file only the request, and the
/// tree is cached without git metadata; returns the digest.
fn assert_pinned(f: &Fixture, source: &Path, first: &str, installed: &Value) -> String {
    let lock = locked(f);
    assert_eq!(lock["commit"], json!(first), "{lock:#}");
    assert_eq!(lock["version"], "1.0.0");
    let digest = lock["digest"].as_str().unwrap().to_owned();
    assert!(
        digest.starts_with("sha256:") && digest.len() == 71,
        "{lock:#}"
    );
    assert_eq!(installed["digest"], json!(digest));
    let shared = read_json(&f.repo().join(".cyber/plugins.json"));
    assert_eq!(
        shared["plugins"]["kit"],
        json!({"source":{"type":"git","url":source.display().to_string(),"ref":"main","path":"kit"}}),
        "the committed scope file holds the request, the lockfile the resolution"
    );
    let [cached] = cached_trees(f).try_into().unwrap();
    let hex = &digest["sha256:".len()..][..32];
    assert_eq!(cached.file_name().unwrap(), format!("1.0.0-{hex}").as_str());
    assert!(cached.join("cyber-plugin.json").is_file());
    assert!(!cached.join(".git").exists());
    digest
}

/// The cached trees of plugin `kit`, content-addressed as `<version>-<digest>`.
fn cached_trees(f: &Fixture) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(f.root.join("alice/cache/plugins/kit")) else {
        return Vec::new();
    };
    let mut trees: Vec<PathBuf> = entries
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.is_dir())
        .collect();
    trees.sort();
    trees
}

/// The plugin cache is shared by every checkout: removing a plugin from one project
/// keeps the tree another project still records, a declined update leaves the trusted
/// tree in place, and an enable refused for a digest mismatch changes nothing.
#[test]
fn shared_plugin_cache_keeps_trees_other_projects_and_declined_changes_need() {
    let f = Fixture::new();
    let source = f.root.join("kit-src");
    kit_repository(&source);
    let other = f.root.join("other");
    std::fs::create_dir_all(other.join(".git")).unwrap();
    let spec = format!("git+{}", source.display());
    let install = [
        "plugins", "install", &spec, "--ref", "main", "--path", "kit", "--scope", "project",
    ];
    for location in [f.repo(), other.clone()] {
        succeeded(&f.cyber_in(&location, "alice", &install, &[], ""));
    }
    let at_other = |args: &[&str], stdin: &str| f.cyber_in(&other, "alice", args, &[], stdin);
    let other_json = |args: &[&str]| -> Value {
        let mut args = args.to_vec();
        args.extend(["--format", "json"]);
        let output = at_other(&args, "");
        succeeded(&output);
        serde_json::from_slice(&output.stdout).unwrap()
    };
    let other_hook = || plugin_hooks(&other_json(&["hooks", "list"]))[0]["handler"]["id"].clone();
    assert_eq!(cached_trees(&f).len(), 1, "both projects share one tree");

    succeeded(&f.cyber(&["plugins", "remove", "kit"]));
    assert_eq!(state(&other_json(&["plugins", "list"]), "kit"), "active");
    assert_eq!(other_hook(), "kit-first");
    let trusted = cached_trees(&f);
    assert_eq!(trusted.len(), 1);

    // Upstream changes the content without bumping the version; the update is declined.
    write_tree(&source, &[("kit/hooks/hooks.json", hook("kit-second"))]);
    git(&source, &["commit", "--quiet", "-am", "second"]);
    let second = git(&source, &["rev-parse", "HEAD"]);
    let declined = at_other(&["plugins", "update", "kit"], "n\n");
    assert_eq!(declined.status.code(), Some(1), "{}", explain(&declined));
    assert_eq!(state(&other_json(&["plugins", "list"]), "kit"), "active");
    assert_eq!(other_hook(), "kit-first");
    assert_eq!(
        cached_trees(&f),
        trusted,
        "the declined fetch was discarded"
    );

    // A lockfile pinning other content is refused and leaves the cache as it was.
    let lockfile = other.join(".cyber/plugins.lock");
    let pinned = std::fs::read_to_string(&lockfile).unwrap();
    let mut tampered = read_json(&lockfile);
    tampered["plugins"]["kit"]["commit"] = json!(second);
    std::fs::write(&lockfile, tampered.to_string()).unwrap();
    let refused = at_other(&["plugins", "enable", "kit", "--yes"], "");
    assert_eq!(refused.status.code(), Some(2), "{}", explain(&refused));
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("does not match the recorded"),
        "{}",
        explain(&refused)
    );
    assert_eq!(cached_trees(&f), trusted);
    std::fs::write(&lockfile, pinned).unwrap();
    assert_eq!(other_hook(), "kit-first");

    // The last project to remove it deletes the tree.
    succeeded(&at_other(&["plugins", "remove", "kit"], ""));
    assert!(cached_trees(&f).is_empty());
}

#[test]
fn git_install_writes_lockfile_commit_and_digest() {
    let f = Fixture::new();
    let source = f.root.join("kit-src");
    let first = kit_repository(&source);
    let spec = format!("git+{}", source.display());
    let install = [
        "plugins", "install", &spec, "--ref", "main", "--path", "kit", "--scope", "project",
    ];
    let installed = f.json(&install);
    assert_eq!(installed["commit"], json!(first), "{installed:#}");
    let digest = assert_pinned(&f, &source, &first, &installed);
    assert_eq!(first_plugin_hook(&f), "kit-first");

    // A new commit on `main` is not picked up by a repeated install: the lock pins it.
    write_tree(&source, &[("kit/hooks/hooks.json", hook("kit-second"))]);
    git(&source, &["commit", "--quiet", "-am", "second"]);
    let second = git(&source, &["rev-parse", "HEAD"]);
    assert_eq!(f.json(&install)["commit"], json!(first));
    assert_eq!(locked(&f)["commit"], json!(first));

    // `cyber plugins update` moves the pin and records the new digest.
    let updated = f.json(&["plugins", "update", "kit", "--yes"]);
    assert_eq!(updated["commit"], json!(second), "{updated:#}");
    assert_eq!(locked(&f)["commit"], json!(second));
    assert_ne!(locked(&f)["digest"], json!(digest));
    assert_eq!(first_plugin_hook(&f), "kit-second");
}

#[test]
fn project_plugin_requires_trust_before_enable() {
    let f = Fixture::new();
    let guard = f.repo().join("plugins/guard");
    write_tree(
        &guard,
        &[
            (
                "cyber-plugin.json",
                json!({"name":"guard","version":"2.0.0"}).to_string(),
            ),
            ("hooks/hooks.json", hook("guard-hook")),
        ],
    );
    // Alice installs it for the project; her install is her approval.
    let install = [
        "plugins",
        "install",
        guard.to_str().unwrap(),
        "--scope",
        "project",
    ];
    succeeded(&f.cyber(&install));
    let shared = read_json(&f.repo().join(".cyber/plugins.json"));
    assert_eq!(
        shared["plugins"]["guard"]["source"]["path"],
        "plugins/guard"
    );
    assert_eq!(plugin_hooks(&f.json(&["hooks", "list"])).len(), 1);

    assert_teammate_must_trust(&f);
    assert_local_scope_is_per_user(&f);

    // A changed tree is refused for everyone until someone reviews an update.
    write_tree(&guard, &[("hooks/hooks.json", hook("guard-swapped"))]);
    let bob = |args: &[&str]| f.json_as("bob", args, &[]);
    assert_eq!(state(&bob(&["plugins", "list"]), "guard"), "changed");
    assert!(plugin_hooks(&bob(&["hooks", "list"])).is_empty());
    let refused = f.cyber_as("bob", &["plugins", "enable", "guard", "--yes"], &[], "");
    assert_eq!(refused.status.code(), Some(2), "{}", explain(&refused));
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert!(
        stderr.contains("does not match the recorded"),
        "{}",
        explain(&refused)
    );
}

/// Bob opens the same checkout: nothing loads until he reviews and trusts it.
fn assert_teammate_must_trust(f: &Fixture) {
    let bob = |args: &[&str]| f.json_as("bob", args, &[]);
    assert_eq!(state(&bob(&["plugins", "list"]), "guard"), "untrusted");
    assert!(plugin_hooks(&bob(&["hooks", "list"])).is_empty());
    let declined = f.cyber_as("bob", &["plugins", "enable", "guard"], &[], "n\n");
    assert_eq!(declined.status.code(), Some(1), "{}", explain(&declined));
    let prompt = String::from_utf8_lossy(&declined.stderr);
    for needle in [
        "Plugin guard@2.0.0 (project scope, unsigned)",
        "digest: sha256:",
        "hooks: PostToolUse",
        "Trust and enable guard? [y/N]",
    ] {
        assert!(prompt.contains(needle), "{needle}\n{}", explain(&declined));
    }
    assert_eq!(state(&bob(&["plugins", "list"]), "guard"), "untrusted");
    succeeded(&f.cyber_as("bob", &["plugins", "enable", "guard"], &[], "y\n"));
    assert_eq!(state(&bob(&["plugins", "list"]), "guard"), "active");
    assert_eq!(plugin_hooks(&bob(&["hooks", "list"])).len(), 1);
}

/// A local-scope install is Bob's alone: the file sits in the checkout, but Alice's
/// trust store never approved it.
fn assert_local_scope_is_per_user(f: &Fixture) {
    let notes = f.repo().join("plugins/notes");
    write_tree(
        &notes,
        &[(
            "cyber-plugin.json",
            json!({"name":"notes","version":"0.1.0"}).to_string(),
        )],
    );
    let install = [
        "plugins",
        "install",
        notes.to_str().unwrap(),
        "--scope",
        "local",
    ];
    succeeded(&f.cyber_as("bob", &install, &[], ""));
    let listed = read_json(&f.repo().join(".cyber/plugins.local.json"));
    assert_eq!(
        listed["plugins"]["notes"]["source"]["path"],
        "plugins/notes"
    );
    assert_eq!(
        state(&f.json_as("bob", &["plugins", "list"], &[]), "notes"),
        "active"
    );
    assert_eq!(state(&f.json(&["plugins", "list"]), "notes"), "untrusted");
}

#[test]
fn pure_flag_disables_plugins_project_hooks_and_project_mcp() {
    let f = Fixture::new();
    let kit = f.root.join("review-kit");
    review_kit(&kit);
    assert!(
        f.cyber(&["plugins", "install", kit.to_str().unwrap()])
            .status
            .success()
    );
    let policy = f.root.join("policy");
    write_tree(
        &policy,
        &[(
            "cyber-policy.json",
            json!({"hooks":{"PostToolUse":[{"matcher":"read","hooks":[
                {"type":"command","id":"managed-guard","command":"cat >/dev/null"}]}]}})
            .to_string(),
        )],
    );
    write_tree(
        &f.root.join("alice/config"),
        &[(
            "cyber.jsonc",
            json!({"hooks":{"PostToolUse":[{"matcher":"read","hooks":[
            {"type":"command","id":"global-guard","command":"cat >/dev/null"}]}]}})
            .to_string(),
        )],
    );
    write_tree(
        &f.repo(),
        &[(
            "cyber.jsonc",
            json!({
                "hooks":{"PostToolUse":[{"matcher":"read","hooks":[
                    {"type":"command","id":"project-guard","command":"cat >/dev/null"}]}]},
                "mcp":{"project-docs":{"type":"local","command":"docs"}}
            })
            .to_string(),
        )],
    );
    let inspected = f.json(&["trust", "inspect"]);
    let digest = inspected["digest"].as_str().unwrap();
    assert!(
        f.cyber(&["trust", "approve", "--digest", digest])
            .status
            .success()
    );

    let policy_dir = policy.to_str().unwrap();
    let managed = [("CYBER_MANAGED_POLICY_DIR", policy_dir)];
    let ids = |listed: &Value| -> Vec<String> {
        listed["hooks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|hook| hook["handler"]["id"].as_str().unwrap().to_owned())
            .collect()
    };
    let servers = |listed: &Value| -> Vec<String> {
        listed["servers"]
            .as_array()
            .unwrap()
            .iter()
            .map(|server| server["name"].as_str().unwrap().to_owned())
            .collect()
    };
    let normal = f.json_as("alice", &["hooks", "list"], &managed);
    assert_eq!(
        ids(&normal),
        [
            "managed-guard",
            "global-guard",
            "project-guard",
            "kit-guard"
        ],
        "managed, global, project, plugin\n{normal:#}"
    );
    let mut all = servers(&f.json_as("alice", &["mcp", "definitions"], &managed));
    all.sort();
    assert_eq!(all, ["kit-docs", "project-docs"]);

    let pure_env = [
        ("CYBER_MANAGED_POLICY_DIR", policy_dir),
        ("CYBER_PURE", "1"),
    ];
    for (flags, envs) in [(&["--pure"][..], &managed[..]), (&[][..], &pure_env[..])] {
        let with = |args: &[&str]| {
            let mut args = args.to_vec();
            args.extend(flags);
            f.json_as("alice", &args, envs)
        };
        assert_eq!(
            ids(&with(&["hooks", "list"])),
            ["managed-guard"],
            "{flags:?}"
        );
        assert!(
            servers(&with(&["mcp", "definitions"])).is_empty(),
            "{flags:?}"
        );
        let listed = with(&["plugins", "list"]);
        assert_eq!(listed["pure"], true);
        assert_eq!(state(&listed, "review-kit"), "pure", "{listed:#}");
        assert_eq!(listed["plugins"][0]["reason"], "disabled by --pure");
        let commands = with(&["commands", "list"]);
        assert!(
            !commands.to_string().contains("review-kit:review"),
            "{commands:#}"
        );
        let mut args = vec!["debug", "info"];
        args.extend(flags);
        let info = f.cyber_as("alice", &args, envs, "");
        assert!(
            String::from_utf8_lossy(&info.stderr).contains("external extensions disabled (--pure)"),
            "{}",
            explain(&info)
        );
    }
}

#[test]
fn npm_install_packs_without_lifecycle_scripts() {
    let f = Fixture::new();
    let package = f.root.join("npm-kit");
    let marker = |name: &str| package.join(name);
    let script = |name: &str| format!("node -e \"require('fs').writeFileSync('{name}','')\"");
    write_tree(
        &package,
        &[
            (
                "package.json",
                json!({"name":"npm-kit","version":"1.2.0","scripts":{
                    "prepack":script("PREPACK_RAN"),
                    "prepare":script("PREPARE_RAN"),
                    "postpack":script("POSTPACK_RAN")
                }})
                .to_string(),
            ),
            (
                "cyber-plugin.json",
                json!({"name":"npm-kit","version":"1.2.0"}).to_string(),
            ),
            ("hooks/hooks.json", hook("npm-guard")),
        ],
    );
    let spec = format!("npm:{}", package.display());
    // npm before 11 runs a folder's prepare script despite --ignore-scripts: refused.
    let npm = std::process::Command::new(if cfg!(windows) { "npm.cmd" } else { "npm" })
        .arg("--version")
        .output()
        .unwrap();
    let major: u32 = String::from_utf8_lossy(&npm.stdout)
        .trim()
        .split('.')
        .next()
        .and_then(|major| major.parse().ok())
        .unwrap_or(0);
    if major < 11 {
        let output = f.cyber(&["plugins", "install", &spec, "--scope", "project"]);
        assert!(!output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("install npm 11 or later"), "{stderr}");
        for name in ["PREPACK_RAN", "PREPARE_RAN", "POSTPACK_RAN"] {
            assert!(!marker(name).exists(), "{name} ran");
        }
        return;
    }
    let installed = f.json(&["plugins", "install", &spec, "--scope", "project"]);
    assert_eq!(installed["version"], "1.2.0", "{installed:#}");
    for name in ["PREPACK_RAN", "PREPARE_RAN", "POSTPACK_RAN"] {
        assert!(!marker(name).exists(), "{name} ran");
    }
    let digest = installed["digest"].as_str().unwrap();
    let cached = f
        .root
        .join("alice/cache/plugins/npm-kit")
        .join(format!("1.2.0-{}", &digest["sha256:".len()..][..32]));
    assert!(cached.join("hooks/hooks.json").is_file());
    let lock = read_json(&f.repo().join(".cyber/plugins.lock"));
    assert_eq!(
        lock["plugins"]["npm-kit"]["source"]["type"], "npm",
        "{lock:#}"
    );
    assert_eq!(lock["plugins"]["npm-kit"]["digest"], installed["digest"]);
    assert_eq!(
        plugin_hooks(&f.json(&["hooks", "list"]))[0]["handler"]["id"],
        "npm-guard"
    );
}

/// alice's global `cyber.jsonc`.
fn user_config(f: &Fixture) -> Value {
    let path = f.root.join("alice/config/cyber.jsonc");
    let text = std::fs::read_to_string(&path).unwrap();
    cyber_core::config::parse_jsonc(&path.display().to_string(), &text).unwrap()
}

/// A plugin that runs a host (`main`), with capabilities and required settings.
fn host_plugin(dir: &Path, secret: bool) {
    let mut properties = json!({
        "site": {"type": "string", "description": "Jira site"},
        "limit": {"type": "integer"},
    });
    let mut required = vec!["site"];
    if secret {
        properties["api_token"] = json!({"type": "string", "secret": true});
        required.push("api_token");
    }
    let manifest = json!({"name": "jira-kit", "version": "1.0.0", "main": "host.py",
        "capabilities": {"network": {"hosts": ["acme.atlassian.net"]}, "secrets": secret},
        "userConfig": {"type": "object", "properties": properties, "required": required}});
    write_tree(
        dir,
        &[
            ("cyber-plugin.json", manifest.to_string()),
            ("host.py", "import sys\n".into()),
        ],
    );
}

#[test]
fn install_shows_capabilities_and_asks_for_required_settings() {
    let f = Fixture::new();
    let source = f.root.join("jira-kit");
    host_plugin(&source, false);
    let output = f.cyber_as(
        "alice",
        &["plugins", "install", source.to_str().unwrap()],
        &[],
        "acme\n",
    );
    assert!(output.status.success(), "{}", explain(&output));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("host: python host.py"), "{stderr}");
    assert!(
        stderr.contains("capabilities: network (acme.atlassian.net)"),
        "{stderr}"
    );
    assert!(stderr.contains("jira-kit site (Jira site): "), "{stderr}");
    // Ordinary values land under `plugins.<name>.config` of the user's configuration.
    let shown = &user_config(&f)["plugins"];
    assert_eq!(shown["jira-kit"]["config"]["site"], "acme", "{shown}");
    let set = f.cyber(&["plugins", "configure", "jira-kit", "--set", "limit=5"]);
    assert!(set.status.success(), "{}", explain(&set));
    let shown = &user_config(&f)["plugins"];
    assert_eq!(shown["jira-kit"]["config"]["limit"], 5, "{shown}");
    let unknown = f.cyber(&["plugins", "configure", "jira-kit", "--set", "colour=red"]);
    assert!(!unknown.status.success());
    assert!(String::from_utf8_lossy(&unknown.stderr).contains("has no setting colour"));
    let info = f.json(&["plugins", "info", "jira-kit"]);
    let host = &info["entries"][0]["host"];
    assert_eq!(host["state"], "not_started", "{info}");
    assert_eq!(
        host["capabilities"]["network"],
        json!({"kind": "hosts", "hosts": ["acme.atlassian.net"]})
    );
}

/// The file backend: the release builds have no Linux keyring.
#[cfg(target_os = "linux")]
#[test]
fn secret_settings_are_stored_outside_configuration_files() {
    let f = Fixture::new();
    let source = f.root.join("jira-kit");
    host_plugin(&source, true);
    let output = f.cyber_as(
        "alice",
        &["plugins", "install", source.to_str().unwrap()],
        &[],
        "acme\ns3cret-token\n",
    );
    assert!(output.status.success(), "{}", explain(&output));
    let refused = f.cyber(&["plugins", "configure", "jira-kit", "--set", "api_token=x"]);
    assert!(!refused.status.success());
    assert!(String::from_utf8_lossy(&refused.stderr).contains("is secret"));
    let config = std::fs::read_to_string(f.root.join("alice/config/cyber.jsonc")).unwrap();
    assert!(
        config.contains("acme") && !config.contains("s3cret-token"),
        "{config}"
    );
    let secrets = f.root.join("alice/data/plugin-secrets.json");
    let stored: Value = serde_json::from_str(&std::fs::read_to_string(&secrets).unwrap()).unwrap();
    assert_eq!(
        user_secrets(&stored, "jira-kit")["api_token"],
        "s3cret-token"
    );
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(&secrets).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600);
    let removed = f.cyber(&["plugins", "remove", "jira-kit"]);
    assert!(removed.status.success(), "{}", explain(&removed));
    let stored: Value = serde_json::from_str(&std::fs::read_to_string(&secrets).unwrap()).unwrap();
    assert!(user_secrets(&stored, "jira-kit").is_null(), "{stored}");
}

/// The file backend's entry of user-scope plugin `name` (`<name>.user.<digest>`).
#[cfg(target_os = "linux")]
fn user_secrets(stored: &Value, name: &str) -> Value {
    let prefix = format!("{name}.user.");
    stored
        .as_object()
        .unwrap()
        .iter()
        .find(|(owner, _)| owner.starts_with(&prefix))
        .map(|(_, entry)| entry.clone())
        .unwrap_or(Value::Null)
}

/// On a terminal, a secret setting is read without echo. `script` gives the install a
/// pseudo-terminal; the secret is typed only once its prompt has appeared.
#[cfg(target_os = "linux")]
#[test]
fn secret_settings_are_read_without_echo_on_a_terminal() {
    use std::io::Read;
    use std::sync::{Arc, Mutex};
    let f = Fixture::new();
    let source = f.root.join("jira-kit");
    host_plugin(&source, true);
    let command = format!(
        "{} --cwd {} plugins install {}",
        env!("CARGO_BIN_EXE_cyber"),
        f.repo().display(),
        source.display()
    );
    let mut child = Command::new("script")
        .args(["-qec", &command, "/dev/null"])
        .env_clear()
        .env("HOME", f.root.join("home"))
        .env("CYBER_HOME", f.root.join("alice"))
        .env("CYBER_DB", "test.db")
        .env("CYBER_MANAGED_POLICY_DIR", f.root.join("no-policy"))
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("TERM", "xterm")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let screen = Arc::new(Mutex::new(String::new()));
    let mut stdout = child.stdout.take().unwrap();
    let reader = {
        let screen = Arc::clone(&screen);
        std::thread::spawn(move || {
            let mut buffer = [0; 1024];
            while let Ok(n) = stdout.read(&mut buffer) {
                if n == 0 {
                    break;
                }
                screen
                    .lock()
                    .unwrap()
                    .push_str(&String::from_utf8_lossy(&buffer[..n]));
            }
        })
    };
    let wait_for = |text: &str| {
        for _ in 0..500 {
            if screen.lock().unwrap().contains(text) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        panic!("never saw {text:?}: {}", screen.lock().unwrap());
    };
    let mut stdin = child.stdin.take().unwrap();
    wait_for("jira-kit site");
    stdin.write_all(b"acme\r").unwrap();
    wait_for("jira-kit api_token");
    std::thread::sleep(std::time::Duration::from_millis(300));
    stdin.write_all(b"s3cret-token\r").unwrap();
    assert!(
        child.wait().unwrap().success(),
        "{}",
        screen.lock().unwrap()
    );
    drop(stdin);
    reader.join().unwrap();
    let screen = screen.lock().unwrap().clone();
    assert!(screen.contains("acme"), "{screen}");
    assert!(
        !screen.contains("s3cret-token"),
        "the secret was echoed: {screen}"
    );
    let secrets = f.root.join("alice/data/plugin-secrets.json");
    let stored: Value = serde_json::from_str(&std::fs::read_to_string(secrets).unwrap()).unwrap();
    assert_eq!(
        user_secrets(&stored, "jira-kit")["api_token"],
        "s3cret-token"
    );
}
