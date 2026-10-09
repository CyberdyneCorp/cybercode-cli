use std::collections::HashMap;
use std::path::PathBuf;

use cyber_core::config::{self, LoadRequest, McpSettings, Resolved};
use cyber_core::paths::Paths;
use cyber_core::trust::TrustStore;
use cyber_tools::mcp::authorize_server;
use serde_json::json;

struct Fixture {
    _root: tempfile::TempDir,
    repo: PathBuf,
    env: HashMap<String, String>,
    paths: Paths,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let repo = root.path().join("repo");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        let env = HashMap::from([(
            "CYBER_HOME".into(),
            root.path().join("home").display().to_string(),
        )]);
        let paths = Paths::resolve(&env, root.path());
        paths.ensure().unwrap();
        std::fs::write(
            paths.config.join("cyber.jsonc"),
            json!({"mcp":{"audit":{"type":"local","command":"audit-server"}}}).to_string(),
        )
        .unwrap();
        Self {
            _root: root,
            repo,
            env,
            paths,
        }
    }
    fn resolved(&self) -> Resolved {
        self.resolved_profile(None)
    }

    fn resolved_profile(&self, profile: Option<&str>) -> Resolved {
        config::load(&LoadRequest {
            location: &self.repo,
            paths: &self.paths,
            env: &self.env,
            home: self._root.path(),
            profile,
            overrides: &[],
            flags: json!({}),
        })
        .unwrap()
    }
    fn trust(&self) -> TrustStore {
        TrustStore::new(self.paths.trust_file())
    }
    fn approve_workspace(&self) {
        let loaded = self.resolved();
        self.trust()
            .approve(
                &loaded.trust.checkout_root,
                loaded.trust.digest.as_deref().unwrap(),
            )
            .unwrap();
    }
    fn project_args(&self, arg: &str) {
        std::fs::write(
            self.repo.join("cyber.jsonc"),
            json!({"mcp":{"audit":{"args":[arg]}}}).to_string(),
        )
        .unwrap();
    }
}

#[test]
fn selected_profile_preserves_global_and_project_server_field_origins() {
    let fixture = Fixture::new();
    let profile = "dev~/core";
    std::fs::write(fixture.paths.config.join("cyber.jsonc"), json!({"mcp":{"audit":{"type":"local","command":"audit-server"}},"profiles":{profile:{"mcp":{"audit":{"timeout":45}}}}}).to_string()).unwrap();
    let global = fixture.resolved_profile(Some(profile));
    assert!(
        !authorize_server(&global, &fixture.trust(), &fixture.repo, "audit")
            .unwrap()
            .requires_sandbox
    );
    std::fs::write(
        fixture.repo.join("cyber.jsonc"),
        json!({"profiles":{profile:{"mcp":{"audit":{"args":["--profile-project"]}}}}}).to_string(),
    )
    .unwrap();
    fixture.approve_workspace();
    let project = fixture.resolved_profile(Some(profile));
    assert!(project.sources["/mcp/audit/args"].starts_with("project:"));
    assert!(project.sources["/mcp/audit/timeout"].starts_with("global:"));
    assert!(authorize_server(&project, &fixture.trust(), &fixture.repo, "audit").is_err());
    let hash = McpSettings::from_config(&project.value).unwrap().servers["audit"]
        .digest("audit")
        .unwrap();
    fixture.trust().approve_mcp(&fixture.repo, &hash).unwrap();
    assert!(
        !authorize_server(&project, &fixture.trust(), &fixture.repo, "audit")
            .unwrap()
            .requires_sandbox
    );
}

#[test]
fn global_launch_selection_is_explicit_and_location_bound() {
    let fixture = Fixture::new();
    let resolved = fixture.resolved();
    let server = authorize_server(&resolved, &fixture.trust(), &fixture.repo, "audit").unwrap();
    assert!(!server.requires_sandbox);
    let other = tempfile::tempdir().unwrap();
    assert!(authorize_server(&resolved, &fixture.trust(), other.path(), "audit").is_err());
    assert!(authorize_server(&resolved, &fixture.trust(), &fixture.repo, "absent").is_err());
    let mut disabled = resolved;
    disabled.value["mcp"]["audit"]["enabled"] = json!(false);
    assert!(authorize_server(&disabled, &fixture.trust(), &fixture.repo, "audit").is_err());
}

#[test]
fn project_arguments_cannot_inherit_global_command_trust() {
    let fixture = Fixture::new();
    fixture.project_args("--project");
    fixture.approve_workspace();
    let loaded = fixture.resolved();
    let definition = &McpSettings::from_config(&loaded.value).unwrap().servers["audit"];
    let digest = definition.digest("audit").unwrap();
    fixture
        .trust()
        .approve_hook(&fixture.repo, &digest)
        .unwrap();
    assert!(authorize_server(&loaded, &fixture.trust(), &fixture.repo, "audit").is_err());
    fixture.trust().approve_mcp(&fixture.repo, &digest).unwrap();
    let server = authorize_server(&loaded, &fixture.trust(), &fixture.repo, "audit").unwrap();
    assert!(!server.requires_sandbox);
    fixture.trust().revoke_mcp(&fixture.repo, &digest).unwrap();
    assert!(authorize_server(&loaded, &fixture.trust(), &fixture.repo, "audit").is_err());
}

#[test]
fn missing_project_field_origin_cannot_downgrade_an_approved_server_to_global() {
    let fixture = Fixture::new();
    fixture.project_args("--project");
    fixture.approve_workspace();
    let loaded = fixture.resolved();
    let hash = McpSettings::from_config(&loaded.value).unwrap().servers["audit"]
        .digest("audit")
        .unwrap();
    fixture.trust().approve_mcp(&fixture.repo, &hash).unwrap();
    assert!(authorize_server(&loaded, &fixture.trust(), &fixture.repo, "audit").is_ok());
    let mut partial = loaded;
    partial.sources.remove("/mcp/audit/args");
    assert!(authorize_server(&partial, &fixture.trust(), &fixture.repo, "audit").is_err());
}

#[test]
fn changed_effective_server_and_workspace_revocation_refuse_reuse() {
    let fixture = Fixture::new();
    fixture.project_args("--first");
    fixture.approve_workspace();
    let loaded = fixture.resolved();
    let hash = McpSettings::from_config(&loaded.value).unwrap().servers["audit"]
        .digest("audit")
        .unwrap();
    fixture.trust().approve_mcp(&fixture.repo, &hash).unwrap();
    assert!(authorize_server(&loaded, &fixture.trust(), &fixture.repo, "audit").is_ok());
    fixture.project_args("--changed");
    fixture.approve_workspace();
    assert!(
        authorize_server(
            &fixture.resolved(),
            &fixture.trust(),
            &fixture.repo,
            "audit"
        )
        .is_err()
    );
    fixture.trust().revoke(&loaded.trust.checkout_root).unwrap();
    assert!(authorize_server(&loaded, &fixture.trust(), &fixture.repo, "audit").is_err());
}

#[test]
fn missing_or_unknown_provenance_and_malformed_trust_fail_closed() {
    let fixture = Fixture::new();
    let mut missing = fixture.resolved();
    missing.sources.clear();
    assert!(authorize_server(&missing, &fixture.trust(), &fixture.repo, "audit").is_err());
    let mut unknown = fixture.resolved();
    for (key, source) in &mut unknown.sources {
        if key.starts_with("/mcp/audit/") {
            *source = "unloaded-origin".into();
        }
    }
    assert!(authorize_server(&unknown, &fixture.trust(), &fixture.repo, "audit").is_err());
    fixture.project_args("--project");
    fixture.approve_workspace();
    let loaded = fixture.resolved();
    std::fs::write(fixture.paths.trust_file(), "private malformed credential").unwrap();
    let error = authorize_server(&loaded, &fixture.trust(), &fixture.repo, "audit")
        .err()
        .unwrap();
    assert!(!error.contains("private malformed credential"));
}
