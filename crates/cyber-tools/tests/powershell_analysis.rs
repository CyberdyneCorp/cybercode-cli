//! Native PowerShell permission facts without executing script source.

use std::path::{Path, PathBuf};

use cyber_tools::permissions::{RemovalRisk, RemovalScope, powershell_analysis};

fn inspect(source: &str) -> (cyber_tools::bash_analysis::Analysis, Option<RemovalRisk>) {
    powershell_analysis(
        source,
        &RemovalScope {
            location: Path::new("/repo"),
            project: Path::new("/repo"),
            home: Path::new("/home/dev"),
            workdir: Path::new("/repo"),
        },
    )
}

#[test]
fn native_resources_preserve_commands_and_keep_printed_code_as_data() {
    let (analysis, _) =
        inspect("git status; Write-Output 'Remove-Item /repo'; git commit -m 'change'");
    assert!(!analysis.unparseable);
    assert_eq!(analysis.commands.len(), 3);
    assert_eq!(analysis.commands[0].always, "git status *");
    assert_eq!(
        analysis.commands[1].text,
        "Write-Output 'Remove-Item /repo'"
    );
    assert_eq!(analysis.commands[1].always, "Write-Output *");
    assert_eq!(analysis.commands[2].always, "git commit *");
    assert_eq!(inspect("Write-Output 'Remove-Item /repo'").1, None);
}

#[test]
fn named_and_positional_operands_exclude_content_and_common_parameter_values() {
    for code in [
        "Set-Content -LiteralPath '/outside/a' -Value '/repo/data'",
        "Set-Content -LiteralPath '/outside/a' '/repo/data'",
        "Set-Content -ErrorAction Stop '/outside/a' -Value '/repo/data'",
        "Add-Content '/outside/a' '/repo/data'",
        "Microsoft.PowerShell.Management\\Set-Content -Path '/outside/a' -Value '/repo/data'",
        "Out-File -FilePath '/outside/a' -InputObject '/repo/data'",
    ] {
        let (analysis, risk) = inspect(code);
        assert_eq!(
            analysis.commands[0].mutates,
            vec![PathBuf::from("/outside/a")],
            "{code}"
        );
        assert!(matches!(risk, Some(RemovalRisk::Unresolved(_))), "{code}");
    }
}

#[test]
fn named_destinations_bind_their_actual_position() {
    for code in [
        "Copy-Item '/repo/a' -Destination '/outside/b'",
        "Copy-Item -Destination '/outside/b' '/repo/a'",
        "Move-Item -LiteralPath '/repo/a' -Destination '/outside/b'",
    ] {
        let (analysis, _) = inspect(code);
        let paths = &analysis.commands[0].mutates;
        assert_eq!(paths.len(), 2, "{code}: {paths:?}");
        assert!(paths.contains(&PathBuf::from("/repo/a")));
        assert!(paths.contains(&PathBuf::from("/outside/b")));
    }
}

#[test]
fn literal_arrays_redirects_and_item_names_produce_filesystem_facts() {
    let (analysis, _) = inspect("Remove-Item -LiteralPath '/outside/a','/outside/b'");
    assert_eq!(
        analysis.commands[0].mutates,
        vec![PathBuf::from("/outside/a"), PathBuf::from("/outside/b")]
    );
    let (analysis, _) = inspect("Write-Output 'data' > '/outside/output'");
    assert_eq!(
        analysis.commands[0].mutates,
        vec![PathBuf::from("/outside/output")]
    );
    let (analysis, _) = inspect("New-Item -Path '/repo/.cyber' -Name 'hooks.jsonc' -ItemType File");
    assert!(
        analysis.commands[0]
            .mutates
            .contains(&PathBuf::from("/repo/.cyber/hooks.jsonc"))
    );
    for code in [
        "Rename-Item '/repo/a' 'cyber.jsonc'",
        "Rename-Item -Path '/repo/a' -NewName 'cyber.jsonc'",
    ] {
        assert!(
            inspect(code).0.commands[0]
                .mutates
                .contains(&PathBuf::from("/repo/cyber.jsonc")),
            "{code}"
        );
    }
}

#[test]
fn scoped_literals_are_shared_with_removal_analysis_and_unknown_flow_cannot_restore_proof() {
    let (analysis, risk) = inspect("$target='/repo'; Remove-Item -LiteralPath $TARGET");
    assert_eq!(analysis.commands[0].mutates, vec![PathBuf::from("/repo")]);
    assert!(matches!(risk, Some(RemovalRisk::Critical(_))));
    let (analysis, risk) = inspect(
        "$target='/outside/a'; Set-Content -LiteralPath $target -Value 'x'; Remove-Item -LiteralPath $target",
    );
    assert_eq!(
        analysis.commands[0].mutates,
        vec![PathBuf::from("/outside/a")]
    );
    assert!(analysis.commands[1].mutates.is_empty());
    assert!(matches!(risk, Some(RemovalRisk::Unresolved(_))));
    let (analysis, risk) = inspect(
        "Set-Location '/outside'; Set-Content './a' -Value 'x'; Set-Content '/other/a' -Value 'x'",
    );
    assert!(analysis.commands[1].mutates.is_empty());
    assert_eq!(
        analysis.commands[2].mutates,
        vec![PathBuf::from("/other/a")]
    );
    assert!(matches!(risk, Some(RemovalRisk::Unresolved(_))));
}

#[test]
fn uncertain_names_providers_globs_and_syntax_do_not_gain_literal_prefix_proof() {
    let (analysis, risk) = inspect("& $unknown -LiteralPath '/outside/a'");
    assert_eq!(analysis.commands[0].always, analysis.commands[0].text);
    assert!(matches!(risk, Some(RemovalRisk::Unresolved(_))));
    for code in [
        "Set-Content -Path './*.txt' -Value x",
        "Set-Content -LiteralPath 'env:SECRET' -Value x",
    ] {
        assert!(inspect(code).0.commands[0].mutates.is_empty(), "{code}");
    }
    assert_eq!(
        inspect("Set-Content -LiteralPath './[*].txt' -Value x")
            .0
            .commands[0]
            .mutates,
        vec![PathBuf::from("/repo/[*].txt")]
    );
    let source = "Write-Output 'unterminated";
    let (analysis, risk) = inspect(source);
    assert!(analysis.unparseable);
    assert_eq!(analysis.commands.len(), 1);
    assert_eq!(analysis.commands[0].always, source);
    assert!(matches!(risk, Some(RemovalRisk::Unresolved(_))));
}

#[cfg(windows)]
#[test]
fn native_drive_paths_produce_external_and_protected_facts() {
    let root = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(root.path()).unwrap();
    let repo = root.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let outside = root.join("outside.txt");
    let protected = repo.join("cyber.jsonc");
    let quote = |path: &Path| path.display().to_string().replace('\'', "''");
    let source = format!(
        "Set-Content -LiteralPath '{}' -Value 'data'; Write-Output 'data' > '{}'; Remove-Item -LiteralPath '{}'",
        quote(&outside),
        quote(&protected),
        quote(&repo)
    );
    let (analysis, risk) = powershell_analysis(
        &source,
        &RemovalScope {
            location: &repo,
            project: &repo,
            home: &root,
            workdir: &repo,
        },
    );
    assert_eq!(analysis.commands[0].mutates, vec![outside]);
    assert_eq!(analysis.commands[1].mutates, vec![protected]);
    assert!(matches!(risk, Some(RemovalRisk::Critical(path)) if path == repo));
}
