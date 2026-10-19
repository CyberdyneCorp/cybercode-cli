//! Plugin resolution through `config::load` (`plugins-marketplace` → Install scopes, Pure
//! mode; `hooks` → Hook scopes and merge order): what the loader contributes for loaded
//! plugins, and what it leaves out.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use cyber_core::config::{self, LoadRequest, Resolved};
use cyber_core::hooks::{HookCatalog, HookScope};
use cyber_core::paths::Paths;
use cyber_core::plugins::{Locations, Manager, PluginScope, PluginState, Source};
use serde_json::{Value, json};

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    paths: Paths,
    env: HashMap<String, String>,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join("repo/.git")).unwrap();
        let env = HashMap::from([
            (
                "CYBER_HOME".to_string(),
                root.join("cyber").display().to_string(),
            ),
            (
                "CYBER_MANAGED_POLICY_DIR".to_string(),
                root.join("policy").display().to_string(),
            ),
        ]);
        let paths = Paths::resolve(&env, &root.join("home"));
        Self {
            _dir: dir,
            root,
            paths,
            env,
        }
    }

    fn repo(&self) -> PathBuf {
        self.root.join("repo")
    }

    fn write(&self, path: &Path, value: &Value) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, value.to_string()).unwrap();
    }

    /// A plugin directory `name` with one PostToolUse hook and an MCP server.
    fn plugin(&self, dir: &Path, name: &str) {
        self.write(
            &dir.join("cyber-plugin.json"),
            &json!({"name":name,"version":"1.0.0"}),
        );
        self.write(
            &dir.join("hooks/hooks.json"),
            &json!({"PostToolUse":[{"hooks":[{"type":"command","id":name,"command":"true"}]}]}),
        );
        self.write(
            &dir.join(".mcp.json"),
            &json!({"mcpServers":{format!("{name}-docs"):{"command":"docs"}}}),
        );
    }

    fn install(&self, dir: &Path, scope: PluginScope) {
        let manager = Manager::new(Locations::new(&self.paths, &self.repo()), "0.1.0");
        let source = Source::Path {
            path: dir.display().to_string(),
        };
        let pending = manager.prepare_install(source, scope, false).unwrap();
        manager.commit(pending).unwrap();
    }

    fn load_with(&self, extra: &[(&str, &str)]) -> Resolved {
        let mut env = self.env.clone();
        env.extend(extra.iter().map(|(k, v)| (k.to_string(), v.to_string())));
        let repo = self.repo();
        config::load(&LoadRequest {
            location: &repo,
            paths: &self.paths,
            env: &env,
            home: &self.root.join("home"),
            profile: None,
            overrides: &[],
            flags: json!({}),
        })
        .unwrap()
    }
}

fn state(resolved: &Resolved, name: &str) -> Option<PluginState> {
    resolved
        .plugins
        .iter()
        .find(|plugin| plugin.name == name)
        .map(|plugin| plugin.state)
}

#[test]
fn plugin_hooks_run_last_and_project_scopes_follow_project_config() {
    let f = Fixture::new();
    f.plugin(&f.root.join("user-kit"), "user-kit");
    f.install(&f.root.join("user-kit"), PluginScope::User);
    f.plugin(&f.repo().join("plugins/team-kit"), "team-kit");
    f.install(&f.repo().join("plugins/team-kit"), PluginScope::Project);
    f.write(
        &f.paths.config.join("cyber.json"),
        &json!({"hooks":{"PostToolUse":[{"hooks":[{"type":"command","id":"global","command":"true"}]}]}}),
    );

    let resolved = f.load_with(&[]);
    let catalog = HookCatalog::from_config(&resolved).unwrap();
    let order: Vec<_> = catalog
        .definitions
        .iter()
        .map(|definition| (definition.scope, definition.handler.id.clone().unwrap()))
        .collect();
    assert_eq!(order[0], (HookScope::Global, "global".into()));
    let plugins: Vec<_> = order[1..].iter().map(|(scope, _)| *scope).collect();
    assert_eq!(plugins, [HookScope::Plugin, HookScope::Plugin], "{order:?}");
    assert!(resolved.value["mcp"]["team-kit-docs"].is_object());

    // CYBER_DISABLE_PROJECT_CONFIG leaves the project and local plugin scopes unread.
    let resolved = f.load_with(&[("CYBER_DISABLE_PROJECT_CONFIG", "1")]);
    assert_eq!(state(&resolved, "user-kit"), Some(PluginState::Active));
    assert_eq!(state(&resolved, "team-kit"), None);
    assert!(resolved.value["mcp"].get("team-kit-docs").is_none());
}

#[test]
fn pure_mode_keeps_only_managed_hooks_and_servers() {
    let f = Fixture::new();
    f.plugin(&f.root.join("user-kit"), "user-kit");
    f.install(&f.root.join("user-kit"), PluginScope::User);
    f.write(
        &f.paths.config.join("cyber.json"),
        &json!({
            "hooks":{"PostToolUse":[{"hooks":[{"type":"command","id":"global","command":"true"}]}]},
            "mcp":{"mine":{"type":"local","command":"mine"}},
            "profiles":{"ci":{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"true"}]}]}}}
        }),
    );
    f.write(
        &f.root.join("policy/cyber-policy.json"),
        &json!({
            "hooks":{"PostToolUse":[{"hooks":[{"type":"command","id":"managed","command":"true"}]}]},
            "mcp":{"org":{"type":"local","command":"org"}}
        }),
    );
    let resolved = f.load_with(&[("CYBER_PURE", "1")]);
    assert!(resolved.pure);
    assert_eq!(state(&resolved, "user-kit"), Some(PluginState::Pure));
    let ids: Vec<_> = HookCatalog::from_config(&resolved)
        .unwrap()
        .definitions
        .iter()
        .map(|definition| definition.handler.id.clone().unwrap())
        .collect();
    assert_eq!(ids, ["managed"]);
    let servers: Vec<_> = resolved.value["mcp"]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    assert_eq!(servers, ["org"]);
    assert!(resolved.value["profiles"]["ci"].get("hooks").is_none());
    assert!(
        resolved
            .value
            .get(cyber_core::plugins::COMPONENTS_KEY)
            .is_none()
    );
}

#[test]
fn only_the_loader_writes_plugin_components() {
    let f = Fixture::new();
    f.write(
        &f.repo().join("cyber.json"),
        &json!({"plugin_components":[{"name":"evil","skills":["/etc"],"commands":["/etc"]}]}),
    );
    let resolved = f.load_with(&[]);
    assert!(
        resolved
            .value
            .get(cyber_core::plugins::COMPONENTS_KEY)
            .is_none(),
        "{:#}",
        resolved.value
    );
    assert!(
        !resolved
            .sources
            .keys()
            .any(|pointer| pointer.starts_with("/plugin_components"))
    );
}
