//! Read-only controls for the mandatory native long-root status assertion.

use super::{Execution, OsString, Output, Path};

pub(super) fn diagnose(
    execution: &Execution,
    directory: &Path,
    args: &[OsString],
    original: &mut Output,
) {
    let Some(position) = args.iter().position(|arg| arg == "--work-tree") else {
        return;
    };
    let Some(alias) = args.get(position + 1) else {
        return;
    };
    let alias = Path::new(alias);
    let mut implicit = args.to_vec();
    implicit.drain(position..position + 2);
    let mut no_symlinks = vec!["-c".into(), "core.symlinks=false".into()];
    no_symlinks.extend_from_slice(args);
    // This override is only a labelled test control, never a product fix.
    for (label, launch, arguments) in [
        ("alias-as-launch-directory", alias, args.to_vec()),
        ("implicit-worktree-at-alias", alias, implicit),
        ("symlink-normalization-disabled", directory, no_symlinks),
    ] {
        append_probe(original, label, execution.invoke(launch, &arguments));
    }
    // These builtins initialize core configuration before their own worktree
    // setup. Keep the exact metadata/alias boundary and all execution policy.
    if let Some(command) = args.iter().position(|arg| arg == "status") {
        for (label, tail) in [
            (
                "config-before-worktree-unstaged",
                vec![
                    "diff",
                    "--no-ext-diff",
                    "--no-textconv",
                    "--name-only",
                    "-z",
                    "--",
                ],
            ),
            (
                "config-before-worktree-staged",
                vec![
                    "diff",
                    "--cached",
                    "--no-ext-diff",
                    "--no-textconv",
                    "--name-only",
                    "-z",
                    "HEAD",
                    "--",
                ],
            ),
            (
                "config-before-worktree-untracked",
                vec!["ls-files", "--others", "--exclude-standard", "-z", "--"],
            ),
        ] {
            let mut arguments = args[..command].to_vec();
            arguments.extend(tail.into_iter().map(OsString::from));
            append_probe(original, label, execution.invoke(directory, &arguments));
        }
    }
    append_probe(
        original,
        "git-version",
        execution.invoke(directory, &["--version".into()]),
    );
    if let Some(metadata) = args.windows(2).find(|pair| pair[0] == "--git-dir") {
        append_probe(
            original,
            "repository-symlink-setting",
            execution.invoke(
                directory,
                &[
                    "--git-dir".into(),
                    metadata[1].clone(),
                    "config".into(),
                    "--get".into(),
                    "core.symlinks".into(),
                ],
            ),
        );
    }
}

fn append_probe(original: &mut Output, label: &str, result: std::io::Result<Output>) {
    let diagnostic = match result {
        Ok(output) => format!(
            "\nNative status control {label}: code={:?}; stdout={:?}; stderr={:?}",
            output.status.code(),
            String::from_utf8_lossy(&output.stdout)
                .chars()
                .take(1024)
                .collect::<String>(),
            String::from_utf8_lossy(&output.stderr)
                .chars()
                .take(1024)
                .collect::<String>(),
        ),
        Err(error) => format!("\nNative status control {label}: launch error={error}"),
    };
    original.stderr.extend_from_slice(diagnostic.as_bytes());
}
