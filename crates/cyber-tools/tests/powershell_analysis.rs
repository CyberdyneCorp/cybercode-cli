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

#[test]
fn abbreviated_and_aliased_parameters_bind_like_their_full_names() {
    for code in [
        "Set-Content -Lit '/outside/a' -Val '/repo/data'",
        "Set-Content -LP '/outside/a' -Value '/repo/data'",
        "Set-Content -PSPath '/outside/a' -Value '/repo/data'",
        "Set-Content -Path:'/outside/a' -Value '/repo/data'",
        "Out-File -File '/outside/a' -InputObj '/repo/data'",
        "Set-Content -EA Stop '/outside/a' -Value '/repo/data'",
    ] {
        let (analysis, _) = inspect(code);
        assert_eq!(
            analysis.commands[0].mutates,
            vec![PathBuf::from("/outside/a")],
            "{code}"
        );
    }
    let (analysis, _) = inspect("Copy-Item '/repo/a' -Dest '/outside/b'");
    let paths = &analysis.commands[0].mutates;
    assert!(paths.contains(&PathBuf::from("/outside/b")), "{paths:?}");
}

#[test]
fn unknown_parameters_of_mutating_commands_keep_their_values_as_possible_targets() {
    let (analysis, _) = inspect("Set-Content -Bogus '/outside/a' -Value 'x'");
    assert!(
        analysis.commands[0]
            .mutates
            .contains(&PathBuf::from("/outside/a")),
        "{:?}",
        analysis.commands[0].mutates
    );
    for code in [
        "Invoke-WebRequest 'https://example.org/x' -OutFile '/outside/a'",
        "Export-Csv -InputObject $x -Path '/outside/a'",
        "Expand-Archive '/repo/a.zip' -DestinationPath '/outside/a'",
        "Start-Transcript '/outside/a'",
    ] {
        let (analysis, _) = inspect(code);
        assert!(
            analysis.commands[0]
                .mutates
                .contains(&PathBuf::from("/outside/a")),
            "{code}: {:?}",
            analysis.commands[0].mutates
        );
        assert!(
            !analysis.commands[0]
                .mutates
                .iter()
                .any(|p| p.to_string_lossy().contains("example.org")),
            "{code}"
        );
    }
}

/// Regression: abbreviations resolved against one table shared by every cmdlet, so a
/// prefix naming a value parameter of another cmdlet (`-U` → `-Uri`, `-Inc` →
/// `-Include`) swallowed the path that binds positionally after a switch of this one.
#[test]
fn abbreviations_resolve_against_the_cmdlets_own_parameters() {
    for code in [
        "Start-Transcript -U '/outside/a'",
        "Start-Transcript -Inc '/outside/a'",
        "Start-Transcript -Include '/outside/a'",
        "Export-Csv -Inc '/outside/a' -InputObject $x",
        "Compress-Archive '/repo/src' -U '/outside/a'",
        "Start-Transcript -OutputDirectory '/outside/a'",
    ] {
        let (analysis, _) = inspect(code);
        assert!(
            analysis.commands[0]
                .mutates
                .contains(&PathBuf::from("/outside/a")),
            "{code}: {:?}",
            analysis.commands[0].mutates
        );
    }
}

#[test]
fn dotnet_file_members_are_mutation_facts() {
    for code in [
        "[IO.File]::WriteAllText('/outside/a', 'x')",
        "[System.IO.File]::AppendAllText('/outside/a', 'x')",
        "[IO.File]::Copy('/repo/b', '/outside/a')",
        "[IO.Directory]::CreateDirectory('/outside/a')",
    ] {
        let (analysis, risk) = inspect(code);
        let paths: Vec<_> = analysis
            .commands
            .iter()
            .flat_map(|c| c.mutates.clone())
            .collect();
        assert!(
            paths.contains(&PathBuf::from("/outside/a")),
            "{code}: {paths:?}"
        );
        assert!(matches!(risk, Some(RemovalRisk::Unresolved(_))), "{code}");
    }
}

#[test]
fn abbreviated_removal_switches_are_understood() {
    assert_eq!(
        inspect("Remove-Item -Rec -Fo -Lit '/repo'").1,
        Some(RemovalRisk::Critical(PathBuf::from("/repo")))
    );
    assert_eq!(
        inspect("Remove-Item -LiteralPath '/repo/build[1]' -Rec -Fo").1,
        None
    );
    assert!(matches!(
        inspect("Remove-Item -Fil '*.tmp' '/repo/out'").1,
        Some(RemovalRisk::Unresolved(_))
    ));
}
