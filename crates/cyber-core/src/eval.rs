//! Evaluation manifests (`harness-evaluation` → Reproducible evaluation manifest).
//!
//! A manifest pins everything that can change an outcome: fixture tree hash, environment,
//! harness version, model, decoding options, budgets, timeouts and grading. Deterministic
//! fixtures (scripted provider) are kept separate from live-provider runs, and recovery
//! manifests link each fault case to the requirement it verifies.

use std::io;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub schema_version: u32,
    pub id: String,
    pub description: String,
    pub kind: ManifestKind,
    pub harness: Harness,
    #[serde(default)]
    pub model: Option<ModelSpec>,
    #[serde(default)]
    pub fixture: Option<Fixture>,
    #[serde(default)]
    pub tasks: Vec<Task>,
    #[serde(default)]
    pub cases: Vec<RecoveryCase>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ManifestKind {
    /// Coding tasks driven by a scripted provider; reproducible byte for byte.
    Deterministic,
    /// Coding tasks against a real provider; never claimed to be deterministic.
    Live,
    /// Fault-injection scenarios linked to requirements.
    Recovery,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Harness {
    /// Minimum harness version the manifest was written for.
    pub min_version: String,
    /// Tool schema contract version (`tools.v<N>`).
    pub tools: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelSpec {
    pub provider: String,
    pub id: String,
    #[serde(default)]
    pub decoding: Value,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fixture {
    /// Repository-relative directory under `eval/fixtures/`.
    pub path: String,
    /// `sha256:<hex>` of the fixture tree (see [`tree_sha256`]).
    pub tree_sha256: String,
    pub environment: Environment,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Environment {
    /// Container image digest, when the fixture runs in a container.
    #[serde(default)]
    pub image_digest: Option<String>,
    /// Toolchain lock, for example `python3>=3.10`.
    #[serde(default)]
    pub lock: Vec<String>,
    #[serde(default)]
    pub setup: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Task {
    pub id: String,
    pub prompt: String,
    pub budget: Budget,
    pub timeout_seconds: u64,
    pub grading: Grading,
}

/// The Budget object of `observability-costs`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Budget {
    pub max_turns: Option<u32>,
    pub max_tokens: Option<u64>,
    pub max_cost_usd: Option<f64>,
    pub max_wall_seconds: Option<u64>,
}

impl Budget {
    fn is_empty(&self) -> bool {
        self.max_turns.is_none()
            && self.max_tokens.is_none()
            && self.max_cost_usd.is_none()
            && self.max_wall_seconds.is_none()
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Grading {
    /// Repository-relative directory under `eval/graders/`, outside agent-writable roots.
    pub grader_dir: String,
    /// Command run after the task. `{grader_dir}` and `{workspace}` are substituted.
    pub command: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryCase {
    pub id: String,
    /// `<capability>/<Requirement name>` this case verifies.
    pub requirement: String,
    pub fault: Fault,
    /// Test that executes the case, as `<crate>::<test target>::<test name>`.
    pub test: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Fault {
    /// SIGKILL of the writer process. Not a power-loss simulation.
    ProcessKill,
    /// In-process tasks abandoned without settlement (an async runtime torn down mid-call).
    TaskAbort,
    /// Filesystem or power-loss fault simulation.
    PowerLoss,
    DiskFull,
    WriterContention,
    ReplayGap,
    UntrustedConfig,
    SecretRead,
    PermissionCeiling,
    StaleEdit,
    RewindConflict,
    RepeatedCompaction,
}

pub fn load_manifest(path: &Path) -> Result<Manifest, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
}

/// Check a manifest against the repository it lives in. Returns every issue found.
pub fn validate(manifest: &Manifest, repo_root: &Path) -> Vec<String> {
    let mut issues = Vec::new();
    if manifest.schema_version != 1 {
        issues.push(format!(
            "schema_version {} is not supported",
            manifest.schema_version
        ));
    }
    match manifest.kind {
        ManifestKind::Recovery => validate_recovery(manifest, repo_root, &mut issues),
        kind => validate_coding(manifest, kind, repo_root, &mut issues),
    }
    issues
}

fn validate_coding(manifest: &Manifest, kind: ManifestKind, root: &Path, issues: &mut Vec<String>) {
    match (kind, &manifest.model) {
        (ManifestKind::Live, None) => issues.push("live manifests must name a model".into()),
        (ManifestKind::Deterministic, Some(_)) => issues.push(
            "deterministic manifests use the scripted provider and must not name a model".into(),
        ),
        _ => {}
    }
    if !manifest.cases.is_empty() {
        issues.push("coding manifests must not contain recovery cases".into());
    }
    let Some(fixture) = &manifest.fixture else {
        issues.push("coding manifests need a fixture".into());
        return;
    };
    validate_fixture(fixture, root, issues);
    if manifest.tasks.is_empty() {
        issues.push("coding manifests need at least one task".into());
    }
    for task in &manifest.tasks {
        validate_task(task, fixture, root, issues);
    }
}

fn validate_fixture(fixture: &Fixture, root: &Path, issues: &mut Vec<String>) {
    if !fixture.path.starts_with("eval/fixtures/") {
        issues.push(format!(
            "fixture {} must live under eval/fixtures/",
            fixture.path
        ));
    }
    match tree_sha256(&root.join(&fixture.path)) {
        Ok(actual) if actual == fixture.tree_sha256 => {}
        Ok(actual) => issues.push(format!(
            "fixture {} tree hash is {actual}, manifest says {}",
            fixture.path, fixture.tree_sha256
        )),
        Err(e) => issues.push(format!("fixture {}: {e}", fixture.path)),
    }
}

fn validate_task(task: &Task, fixture: &Fixture, root: &Path, issues: &mut Vec<String>) {
    let id = &task.id;
    if task.budget.is_empty() {
        issues.push(format!("task {id}: budget must set at least one limit"));
    }
    if task.timeout_seconds == 0 {
        issues.push(format!("task {id}: timeout_seconds must be positive"));
    }
    let grader = &task.grading.grader_dir;
    if !grader.starts_with("eval/graders/") || grader.starts_with(&fixture.path) {
        issues.push(format!(
            "task {id}: grader {grader} must live under eval/graders/, outside the fixture"
        ));
    } else if !root.join(grader).is_dir() {
        issues.push(format!(
            "task {id}: grader directory {grader} does not exist"
        ));
    }
    if task.grading.command.is_empty() {
        issues.push(format!("task {id}: grading command is empty"));
    }
}

fn validate_recovery(manifest: &Manifest, root: &Path, issues: &mut Vec<String>) {
    if manifest.model.is_some() || manifest.fixture.is_some() || !manifest.tasks.is_empty() {
        issues.push("recovery manifests contain only cases".into());
    }
    if manifest.cases.is_empty() {
        issues.push("recovery manifests need at least one case".into());
    }
    for case in &manifest.cases {
        if !requirement_exists(root, &case.requirement) {
            issues.push(format!(
                "case {}: requirement {} not found in openspec/specs",
                case.id, case.requirement
            ));
        }
        if case.test.split("::").count() != 3 {
            issues.push(format!(
                "case {}: test must be <crate>::<target>::<name>",
                case.id
            ));
        }
    }
}

fn requirement_exists(root: &Path, reference: &str) -> bool {
    let Some((capability, name)) = reference.split_once('/') else {
        return false;
    };
    let spec = root.join("openspec/specs").join(capability).join("spec.md");
    std::fs::read_to_string(spec).is_ok_and(|text| {
        text.lines()
            .any(|l| l.trim() == format!("### Requirement: {name}"))
    })
}

/// `sha256:<hex>` over every regular file under `dir`, in sorted relative-path order, as
/// `<path>\0<length>\0<bytes>`. Paths use `/` separators.
pub fn tree_sha256(dir: &Path) -> io::Result<String> {
    let mut files = Vec::new();
    collect_files(dir, dir, &mut files)?;
    files.sort();
    let mut hasher = Sha256::new();
    for rel in files {
        let bytes = std::fs::read(dir.join(&rel))?;
        let name = rel.to_string_lossy().replace('\\', "/");
        hasher.update(name.as_bytes());
        hasher.update([0]);
        hasher.update(bytes.len().to_string().as_bytes());
        hasher.update([0]);
        hasher.update(&bytes);
    }
    Ok(format!("sha256:{:x}", hasher.finalize()))
}

fn collect_files(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) -> io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name();
        if name == "__pycache__" || name == ".DS_Store" {
            continue;
        }
        if entry.file_type()?.is_dir() {
            collect_files(root, &path, out)?;
        } else {
            out.push(
                path.strip_prefix(root)
                    .map_err(io::Error::other)?
                    .to_path_buf(),
            );
        }
    }
    Ok(())
}
