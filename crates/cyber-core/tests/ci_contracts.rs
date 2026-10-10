//! Native CI must retain an earlier command failure when later commands would pass.
use serde_yaml_ng::Value;

fn windows_native_batches() -> Vec<(String, String)> {
    let workflow: Value =
        serde_yaml_ng::from_str(include_str!("../../../.github/workflows/ci.yml")).unwrap();
    let mut batches = Vec::new();
    for job in workflow["jobs"].as_mapping().unwrap().values() {
        if job["runs-on"].as_str() != Some("windows-2025") {
            continue;
        }
        for step in job["steps"].as_sequence().unwrap() {
            let Some(run) = step["run"].as_str() else {
                continue;
            };
            if run
                .lines()
                .filter(|line| line.trim_start().starts_with("cargo "))
                .count()
                > 1
            {
                batches.push((step["name"].as_str().unwrap().into(), run.into()));
            }
        }
    }
    batches
}

#[test]
fn every_batched_windows_cargo_command_preserves_its_failure() {
    let batches = windows_native_batches();
    assert!(
        !batches.is_empty(),
        "expected native multi-command coverage"
    );
    for (name, run) in batches {
        let lines: Vec<_> = run.lines().map(str::trim).collect();
        for (i, line) in lines.iter().enumerate() {
            if line.starts_with("cargo ") {
                assert_eq!(
                    lines.get(i + 1).copied(),
                    Some("if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }"),
                    "missing native exit propagation in {name}: {line}",
                );
            }
        }
    }
}

#[cfg(windows)]
#[test]
fn native_powershell_batches_stop_on_first_failure_even_when_later_commands_succeed() {
    for (name, run) in windows_native_batches() {
        let mut first = true;
        let probe = run
            .lines()
            .map(|line| {
                if !line.trim_start().starts_with("cargo ") {
                    return line.to_owned();
                }
                let code = if first { 7 } else { 0 };
                first = false;
                format!("cmd /C exit {code}")
            })
            .collect::<Vec<_>>()
            .join("\n");
        let output = std::process::Command::new("pwsh")
            .args(["-NoProfile", "-NonInteractive", "-Command", &probe])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(7), "{name}: {output:?}");
    }
}

#[test]
fn windows_snapshot_logs_are_published_immediately_without_hiding_test_failures() {
    let workflow: Value =
        serde_yaml_ng::from_str(include_str!("../../../.github/workflows/ci.yml")).unwrap();
    let job = workflow["jobs"]
        .as_mapping()
        .unwrap()
        .values()
        .find(|job| job["runs-on"].as_str() == Some("windows-2025"))
        .unwrap();
    let steps = job["steps"].as_sequence().unwrap();
    let index = steps
        .iter()
        .position(|step| step["name"].as_str() == Some("Native migration source snapshots"))
        .unwrap();
    let run = steps[index]["run"].as_str().unwrap();
    assert!(
        run.contains("--test import_snapshot 2>&1 | Tee-Object -FilePath migration-snapshot.log")
    );
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
