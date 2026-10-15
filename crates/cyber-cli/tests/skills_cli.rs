//! `cyber skills list|show`, `cyber commands list` and `cyber doctor` print skill and command
//! diagnostics through the real app (`skills-commands` → Skill and command inspection).

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;

struct Env {
    _dir: tempfile::TempDir,
    root: PathBuf,
    repo: PathBuf,
}

impl Env {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let repo = root.join("repo");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        Self {
            _dir: dir,
            root,
            repo,
        }
    }

    fn cyber(&self, args: &[&str]) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_cyber"));
        command
            .args(args)
            .env_clear()
            .env("HOME", self.root.join("home"))
            .env("CYBER_HOME", self.root.join("cyber-home"))
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .current_dir(&self.repo);
        #[cfg(windows)]
        if let Some(system) = std::env::var_os("SystemRoot") {
            command.env("SystemRoot", system);
        }
        command.output().unwrap()
    }

    fn write(&self, rel: &str, text: &str) -> PathBuf {
        let path = self.repo.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
        path
    }
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

fn json(o: &Output) -> Value {
    serde_json::from_slice(&o.stdout).unwrap_or_else(|e| panic!("{e}: {}", stdout(o)))
}

fn display(path: &Path) -> String {
    path.display().to_string()
}

/// A project with a valid skill, a skill with invalid YAML, an unsupported command and a
/// command whose hint is derived from its placeholders.
fn fixture() -> (Env, PathBuf, PathBuf) {
    let env = Env::new();
    env.write(
        ".cyber/skills/deploy/SKILL.md",
        "---\nname: deploy\ndescription: Project deploy\nargument-hint: <env>\n---\nDeploy\n",
    );
    let broken = env.write(
        ".cyber/skills/broken/SKILL.md",
        "---\nname: [unclosed\ndescription: Broken\n---\nB\n",
    );
    let bad_command = env.write(
        ".cyber/commands/bad.md",
        "---\nagent: missing-agent\n---\nX",
    );
    env.write(".cyber/commands/ship.md", "Ship $1 to $2");
    (env, broken, bad_command)
}

#[test]
fn skills_list_and_show_print_scope_permission_and_diagnostics() {
    let (env, broken, _) = fixture();
    let list = env.cyber(&["skills", "list"]);
    assert!(list.status.success(), "{}", stderr(&list));
    let text = stdout(&list);
    assert!(text.contains("deploy  [project] loaded allow"), "{text}");
    assert!(text.contains("broken  [project] invalid -"), "{text}");
    assert!(text.contains(&display(&broken)), "{text}");
    assert!(text.contains("invalid frontmatter"), "{text}");

    let show = env.cyber(&["skills", "show", "deploy", "--format", "json"]);
    assert!(show.status.success(), "{}", stderr(&show));
    let entry = &json(&show)["skills"][0];
    assert_eq!(entry["scope"], "project");
    assert_eq!(entry["argument_hint"], "<env>");
    assert_eq!(
        entry["source"],
        display(&env.repo.join(".cyber/skills/deploy"))
    );
    let text = stdout(&env.cyber(&["skills", "show", "broken"]));
    assert!(text.contains("status: invalid"), "{text}");
    assert!(text.contains("invalid frontmatter"), "{text}");
    let missing = env.cyber(&["skills", "show", "nope"]);
    assert!(!missing.status.success());
    assert!(stderr(&missing).contains("deploy"), "{}", stderr(&missing));
}

#[test]
fn commands_list_prints_derived_hints_and_discovery_issues() {
    let (env, _, bad_command) = fixture();
    let commands = env.cyber(&["commands", "list"]);
    assert!(commands.status.success(), "{}", stderr(&commands));
    let text = stdout(&commands);
    assert!(text.contains("/ship $1 $2  [command]"), "{text}");
    assert!(text.contains("/skills "), "{text}");
    assert!(
        text.contains(&format!("issue: {}", display(&bad_command))),
        "{text}"
    );
}

#[test]
fn doctor_reports_skill_and_command_diagnostics() {
    let (env, broken, bad_command) = fixture();
    let doctor = json(&env.cyber(&["doctor", "--format", "json"]));
    let checks = doctor["checks"].as_array().unwrap();
    let find = |name: &str| checks.iter().find(|c| c["check"] == name).unwrap().clone();
    let skills = find("skills");
    assert_eq!(skills["status"], "warn");
    let detail = skills["detail"].as_str().unwrap();
    assert!(detail.contains(&display(&broken)), "{detail}");
    assert!(detail.contains("invalid frontmatter"), "{detail}");
    let commands = find("commands");
    assert_eq!(commands["status"], "warn");
    let detail = commands["detail"].as_str().unwrap();
    assert!(detail.contains(&display(&bad_command)), "{detail}");
}

/// `cyber commands trust` approves the exact current digest of a project template
/// that runs shell commands.
#[test]
fn commands_trust_approves_the_exact_project_template_digest() {
    let env = Env::new();
    let template = "Status: !`git status --short`";
    env.write(".cyber/commands/status.md", template);
    let output = env.cyber(&["--format", "json", "commands", "trust", "status"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let approved = json(&output);
    let digest = cyber_core::commands::template_digest("command", "status", template);
    assert_eq!(approved["digest"], digest.as_str());
    let store = cyber_core::trust::TrustStore::new(
        cyber_core::paths::Paths::resolve(
            &std::collections::HashMap::from([(
                "CYBER_HOME".to_string(),
                env.root.join("cyber-home").display().to_string(),
            )]),
            &env.root.join("home"),
        )
        .trust_file(),
    );
    assert!(store.is_template_approved(&env.repo, &digest).unwrap());
    let missing = env.cyber(&["commands", "trust", "absent"]);
    assert!(!missing.status.success());
    assert!(stderr(&missing).contains("No command or skill named absent"));
}

/// Serves one in-memory remote skill source; never the network.
struct MemorySource(std::collections::HashMap<String, Vec<u8>>);

impl cyber_tools::RemoteSkillFetcher for MemorySource {
    fn fetch(
        &self,
        request: cyber_tools::FetchRequest,
    ) -> futures::future::BoxFuture<'_, Result<Vec<u8>, String>> {
        let body = self.0.get(request.url.as_str()).cloned();
        Box::pin(async move { body.ok_or_else(|| "missing".to_string()) })
    }
}

/// `cyber skills trust <url>` approves a project-configured remote skill source bound to
/// its URL and cached content; afterwards `cyber skills list` loads it from the cache.
#[test]
fn skills_trust_approves_a_cached_project_remote_source() {
    let env = Env::new();
    let url = "https://skills.example.test/project";
    env.write(
        ".cyber/cyber.json",
        &serde_json::json!({"skills":[url]}).to_string(),
    );
    let uncached = env.cyber(&["skills", "trust", url]);
    assert!(!uncached.status.success());
    assert!(
        stderr(&uncached).contains("not cached"),
        "{}",
        stderr(&uncached)
    );
    let unknown = env.cyber(&["skills", "trust", "https://other.example.test/x"]);
    assert!(stderr(&unknown).contains("not a configured skill source"));

    let site = MemorySource(std::collections::HashMap::from([
        (
            format!("{url}/index.json"),
            br#"{"skills":[{"name":"proj-tool","files":["SKILL.md"]}]}"#.to_vec(),
        ),
        (
            format!("{url}/proj-tool/SKILL.md"),
            b"---\nname: proj-tool\ndescription: Project tool\n---\nRun\n".to_vec(),
        ),
    ]));
    let remote = cyber_tools::remote_skills::RemoteSkills::new(
        env.root.join("cyber-home/cache/skills"),
        std::sync::Arc::new(site),
    );
    tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(remote.refresh(
            &cyber_tools::remote_skills::parse_source_url(url).unwrap(),
            &cyber_tools::remote_skills::Network {
                mode: cyber_sandbox::NetworkMode::On,
                allowed_domains: Vec::new(),
            },
        ))
        .unwrap();
    let listed = |env: &Env| {
        let output = env.cyber(&["--format", "json", "skills", "list"]);
        assert!(output.status.success(), "{}", stderr(&output));
        json(&output)["skills"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["name"] == "proj-tool" && s["status"] == "loaded")
    };
    assert!(!listed(&env));
    let output = env.cyber(&["--format", "json", "skills", "trust", url]);
    assert!(output.status.success(), "{}", stderr(&output));
    let digest = json(&output)["digest"].as_str().unwrap().to_string();
    assert_eq!(digest, remote.cached_approval_digest(url).unwrap());
    assert!(listed(&env));
}
