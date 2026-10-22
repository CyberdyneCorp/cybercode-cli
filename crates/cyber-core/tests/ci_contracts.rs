//! Native CI contracts: the Windows jobs build the test binaries once and keep a failing
//! command's exit status.
use serde_yaml_ng::Value;

fn workflow() -> Value {
    serde_yaml_ng::from_str(include_str!("../../../.github/workflows/ci.yml")).unwrap()
}

/// `(step name, run script)` of every step of the native Windows jobs.
fn windows_steps() -> Vec<(String, String)> {
    let workflow = workflow();
    let mut steps = Vec::new();
    for job in workflow["jobs"].as_mapping().unwrap().values() {
        if job["runs-on"].as_str() != Some("windows-2025") {
            continue;
        }
        for step in job["steps"].as_sequence().unwrap() {
            if let Some(run) = step["run"].as_str() {
                let name = step["name"].as_str().unwrap_or_default();
                steps.push((name.into(), run.into()));
            }
        }
    }
    steps
}

fn cargo_lines(run: &str) -> impl Iterator<Item = &str> {
    run.lines()
        .map(str::trim)
        .filter(|line| line.starts_with("cargo "))
}

/// A `cargo` command that selects other packages resolves dependency features differently
/// and rebuilds the crate stack; with one per subset the Windows job outgrew its 6-hour
/// limit. Only the feature-gated sandbox tests build separately.
#[test]
fn every_native_windows_cargo_command_selects_the_whole_workspace() {
    let steps = windows_steps();
    let commands: Vec<_> = steps
        .iter()
        .flat_map(|(name, run)| cargo_lines(run).map(move |line| (name, line)))
        .collect();
    assert!(
        commands
            .iter()
            .any(|(_, line)| line.contains("nextest run") && line.contains("--profile ci-windows")),
        "expected the native Windows test run"
    );
    for (name, line) in commands {
        if line.contains("--features") {
            continue;
        }
        assert!(
            line.contains(" --workspace"),
            "{name} rebuilds for its own package selection: {line}"
        );
    }
}

#[test]
fn piped_windows_cargo_commands_preserve_their_failure() {
    for (name, run) in windows_steps() {
        let lines: Vec<_> = run.lines().map(str::trim).collect();
        for (i, line) in lines.iter().enumerate() {
            if line.starts_with("cargo ") && line.contains(" | ") {
                assert_eq!(
                    lines.get(i + 1).copied(),
                    Some("if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }"),
                    "missing native exit propagation in {name}: {line}",
                );
            }
        }
    }
}

#[test]
fn windows_snapshot_logs_are_published_immediately_without_hiding_test_failures() {
    let workflow = workflow();
    let steps = workflow["jobs"]["windows"]["steps"].as_sequence().unwrap();
    let index = steps
        .iter()
        .position(|step| step["name"].as_str() == Some("Native migration source snapshots"))
        .unwrap();
    let run = steps[index]["run"].as_str().unwrap();
    assert!(run.contains(
        "-E 'binary_id(cyber-core::import_snapshot)' 2>&1 | Tee-Object -FilePath migration-snapshot.log"
    ));
    assert_eq!(
        run.lines().nth(1),
        Some("if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }")
    );
    let upload = &steps[index + 1];
    assert_eq!(upload["uses"].as_str(), Some("actions/upload-artifact@v4"));
    assert_eq!(upload["if"].as_str(), Some("${{ !cancelled() }}"));
    assert_eq!(
        upload["with"]["path"].as_str(),
        Some("migration-snapshot.log")
    );
}
