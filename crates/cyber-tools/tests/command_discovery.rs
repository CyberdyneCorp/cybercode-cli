mod support;
use serde_json::json;
use support::Fixture;

#[test]
fn host_command_discovery_obeys_project_disable_without_running_templates() {
    let f = Fixture::new();
    let global = f.dir.path().join("home/.config/cyber/commands");
    let local = f.repo.join(".cyber/commands");
    std::fs::create_dir_all(&global).unwrap();
    std::fs::create_dir_all(&local).unwrap();
    std::fs::write(global.join("global.md"), "Global $ARGUMENTS").unwrap();
    std::fs::write(local.join("project.md"), "Project $ARGUMENTS").unwrap();
    f.set_config(json!({}));
    assert_eq!(
        f.host.expand_command(&f.repo, "project", "src").as_deref(),
        Some("Project src")
    );
    f.env.set("CYBER_DISABLE_PROJECT_CONFIG", "1");
    assert!(f.host.expand_command(&f.repo, "project", "src").is_none());
    assert_eq!(
        f.host.expand_command(&f.repo, "global", "src").as_deref(),
        Some("Global src")
    );
}
