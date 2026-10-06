//! Static guard regressions; dangerous commands are never executed.
use cyber_tools::permissions::{RemovalRisk, RemovalScope, bash_removal};
use std::path::{Path, PathBuf};

fn risk(command: &str) -> Option<RemovalRisk> {
    bash_removal(
        command,
        &RemovalScope {
            location: Path::new("/repo"),
            project: Path::new("/repo"),
            home: Path::new("/home/user"),
            workdir: Path::new("/repo"),
        },
    )
}

#[test]
fn inline_powershell_critical_removals_are_guarded() {
    for command in [
        "pwsh -NoProfile -Command 'Remove-Item -LiteralPath /repo -Recurse'",
        "powershell.exe -Command 'Remove-Item -Path /repo -Force'",
        "pwsh -c 'Microsoft.PowerShell.Management\\Remove-Item /repo'",
    ] {
        assert_eq!(
            risk(command),
            Some(RemovalRisk::Critical(PathBuf::from("/repo"))),
            "{command}"
        );
    }
}

#[test]
fn powershell_aliases_bindings_arrays_and_static_delete_apis_preserve_roots() {
    for source in [
        "rEmOvE-iTeM -LiteralPath /repo -Recurse -Force",
        "ri /repo",
        "rm /repo",
        "rmdir /repo",
        "rd /repo",
        "del /repo",
        "erase /repo",
        "$target=\"/repo\"; Remove-Item $TARGET",
        "Remove-Item -Path \"safe\", \"/repo\"",
        "Remove-Item -LiteralPath $HOME",
        "Remove-Item -LiteralPath $PWD",
        "[System.IO.Directory]::Delete(\"/repo\", $true)",
        "[IO.File]::Delete(\"/repo\")",
        "& { Remove-Item /repo }",
    ] {
        let command = format!("pwsh -NoProfile -Command '{source}'");
        assert!(
            matches!(risk(&command), Some(RemovalRisk::Critical(_))),
            "{command}: {:?}",
            risk(&command)
        );
    }
}

#[test]
fn powershell_quoted_data_and_specific_literal_files_are_not_critical_roots() {
    for source in [
        "Write-Output \"Remove-Item /repo -Recurse\"",
        "# Remove-Item /repo\nWrite-Output \"hello\"",
        "Remove-Item -LiteralPath /repo/build/file",
        "Remove-Item -LiteralPath \"/repo/a[b]\"",
        "$target=\"/repo/file\"; Remove-Item $target",
    ] {
        let command = format!("pwsh -NoProfile -Command '{source}'");
        assert_eq!(risk(&command), None, "{command}");
    }
}

#[test]
fn uncertain_powershell_source_options_and_dispatch_require_confirmation() {
    for source in [
        "pwsh -Command 'Write-Output hello'",
        "pwsh -NoProfile -Command 'Remove-Item $unknown'",
        "pwsh -NoProfile -Command 'Remove-Item -Path /repo/*'",
        "pwsh -NoProfile -Command 'Remove-Item -LiteralPath Registry::HKCU'",
        "pwsh -NoProfile -Command 'Invoke-Expression $source'",
        "pwsh -NoProfile -EncodedCommand AA==",
        "pwsh -NoProfile -File untrusted.ps1",
        "pwsh -NoProfile -Command 'Set-Location somewhere; Remove-Item .'",
        "pwsh -NoProfile -Command '$p=\"/repo\"; if ($maybe) {$p=\"safe\"}; Remove-Item $p'",
        "pwsh -NoProfile -Command '$p=\"/repo\"; $p+=\"safe\"; Remove-Item $p'",
        "pwsh -NoProfile -Command '$block={Remove-Item /repo}; Write-Output $block'",
        "pwsh -NoProfile -Command 'Remove-Item -UnknownFlag safe'",
        "pwsh -NoProfile -Command '$p=\"safe\"; Write-Output /repo -OutVariable p; Remove-Item $p'",
        "pwsh -NoProfile -Command 'Write-Output \"unterminated'",
    ] {
        assert!(
            matches!(risk(source), Some(RemovalRisk::Unresolved(_))),
            "{source}: {:?}",
            risk(source)
        );
    }
}

#[cfg(windows)]
#[test]
fn powershell_guard_resolves_native_drive_and_verbatim_paths() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("repo with spaces");
    std::fs::create_dir(&root).unwrap();
    let canonical = std::fs::canonicalize(&root).unwrap();
    for target in [&root, &canonical] {
        let command = format!(
            "pwsh -NoProfile -Command 'Remove-Item -LiteralPath \"{}\" -Recurse'",
            target.display()
        );
        assert_eq!(
            bash_removal(
                &command,
                &RemovalScope {
                    location: &root,
                    project: &root,
                    home: temp.path(),
                    workdir: &root
                }
            ),
            Some(RemovalRisk::Critical(canonical.clone())),
            "{command}"
        );
    }
}

#[test]
fn direct_inline_python_and_javascript_removals_are_critical() {
    for command in [
        "python3 -c 'import shutil; shutil.rmtree(\".\")'",
        "python3 -c 'import os; os.unlink(\"/\")'",
        "python3 -c 'from pathlib import Path; Path(\".\").rmdir()'",
        "node -e 'require(\"fs\").rmSync(\".\", {recursive:true})'",
    ] {
        assert_eq!(
            risk(command),
            Some(RemovalRisk::Critical(if command.contains("os.unlink") {
                PathBuf::from("/")
            } else {
                PathBuf::from("/repo")
            })),
            "{command}"
        );
    }
}

#[test]
fn aliases_bindings_and_literal_path_constructors_preserve_critical_targets() {
    for command in [
        "python3 -Ic 'import shutil as s; ROOT=\"/repo\"; s.rmtree(ROOT)'",
        "python3 -c 'from shutil import rmtree as delete; delete(\".\")'",
        "python3 -c 'import shutil; delete=shutil.rmtree; delete(\".\")'",
        "python3 -c 'from pathlib import Path as P; P(\".\").unlink(missing_ok=True)'",
        "python3 -c 'import os; ROOT=\"safe\"; (ROOT:=\".\"); os.rmdir(ROOT)'",
        "python3 -c 'import os; os.rmdir(os.path.join(\"/repo\", \".\"))'",
        "node --input-type=module -e 'import {rmSync as remove} from \"node:fs\"; remove(\".\")'",
        "node -e 'const {rmSync: remove} = require(\"fs\"); const ROOT=\"/repo\"; remove(ROOT)'",
        "node -e 'const fs=require(\"node:fs\"); const ROOT=require(\"path\").join(\"/repo\", \".\"); fs.rmSync(ROOT)'",
        "node -e 'require(\"node:fs/promises\").rm(process.cwd())'",
        "node --eval='require(\"fs\").unlinkSync(\"/repo\")'",
    ] {
        assert!(
            matches!(risk(command), Some(RemovalRisk::Critical(_))),
            "{command}: {:?}",
            risk(command)
        );
    }
}

#[test]
fn inline_nested_execution_and_escaped_literals_are_analyzed() {
    for command in [
        "python3 -c 'import os; os.system(\"rm -rf /repo\")'",
        "python3 -c 'exec(\"import shutil; shutil.rmtree(\\\"/repo\\\")\")'",
        "node -e 'require(\"child_process\").execSync(\"rm -rf /repo\")'",
        "node -e 'eval(\"require(\\\"fs\\\").rmSync(\\\"/repo\\\")\")'",
        "python3 -c 'import os; os.rmdir(\"\\x2f\")'",
        "node -e 'require(\"fs\").rmSync(\"\\u002f\")'",
        "env python3 -c 'import shutil; shutil.rmtree(\".\")'",
        "bash -c \"python3 -c 'import shutil; shutil.rmtree(\\\".\\\")'\"",
    ] {
        assert!(
            matches!(risk(command), Some(RemovalRisk::Critical(_))),
            "{command}: {:?}",
            risk(command)
        );
    }
}

#[test]
fn quoted_program_data_and_noncritical_literal_removals_are_not_critical() {
    for command in [
        "python3 -c 'print(\"shutil.rmtree(/repo)\")'",
        "node -e 'console.log(\"require(fs).rmSync(/repo)\")'",
        "python3 -c 'import shutil; shutil.rmtree(\"build\")'",
        "node -e 'require(\"fs\").unlinkSync(\"build/file\")'",
    ] {
        assert_eq!(risk(command), None, "{command}");
    }
}

#[test]
fn unresolved_dispatch_context_and_control_flow_cannot_approve_removals() {
    for command in [
        "python3 -c 'import shutil; shutil.rmtree(input())'",
        "python3 -c 'getattr(__import__(\"shutil\"), \"rmtree\")(\".\")'",
        "python3 -c 'import os; os.chdir(\"/tmp\"); os.rmdir(\"child\")'",
        "python3 -c 'import shutil; ROOT=\".\"; ROOT+=\"/sub\"; shutil.rmtree(ROOT)'",
        "node -e 'const fs=require(\"fs\"); let root=\".\"; false && (root=\"file\"); fs.rmSync(root)'",
        "node -e 'require(\"fs\")[method](target)'",
        "node -e 'const fs=require(\"fs\"); fs.rmSync(process.argv[1])'",
        "python3 -c '$SOURCE'",
        "python3 -c 'import shutil; shutil.rmtree('\"$SOURCE\"')'",
        "python3 -c 'print('",
    ] {
        assert!(
            matches!(risk(command), Some(RemovalRisk::Unresolved(_))),
            "{command}: {:?}",
            risk(command)
        );
    }
}

#[test]
fn removedirs_includes_implicit_parent_removals() {
    assert!(matches!(
        risk("python3 -c 'import os; os.removedirs(\"build/nested\")'"),
        Some(RemovalRisk::Critical(_))
    ));
}

#[test]
fn option_clusters_home_paths_and_named_removal_arguments_are_checked() {
    for command in [
        "python3 -W ignore -c 'import shutil; shutil.rmtree(ignore_errors=True, path=\".\")'",
        "python3 -c 'from pathlib import Path; Path.home().rmdir()'",
        "node -pe 'require(\"fs\").rmSync(\".\")'",
    ] {
        assert!(
            matches!(risk(command), Some(RemovalRisk::Critical(_))),
            "{command}: {:?}",
            risk(command)
        );
    }
}

#[test]
fn inline_source_and_syntax_budgets_fail_closed() {
    let nested = format!(
        "node -e '{}\"literal\"{}'",
        "(".repeat(300),
        ")".repeat(300)
    );
    assert!(matches!(risk(&nested), Some(RemovalRisk::Unresolved(_))));
    let wide = format!("python3 -c '{}'", "a=1\n".repeat(10_000));
    assert!(matches!(risk(&wide), Some(RemovalRisk::Unresolved(_))));
    let mut nested_eval = "console.log(\"data\")".to_string();
    for _ in 0..10 {
        nested_eval = format!("eval({})", serde_json::to_string(&nested_eval).unwrap());
    }
    let command = format!("node -e '{nested_eval}'");
    assert!(matches!(risk(&command), Some(RemovalRisk::Unresolved(_))));
}

#[test]
fn path_constructor_and_javascript_join_semantics_preserve_roots() {
    for command in [
        "python3 -c 'from pathlib import Path; Path(\"build\", \"..\").rmdir()'",
        "python3 -c 'from pathlib import Path; Path().rmdir()'",
        "node -e 'require(\"fs\").rmSync(require(\"path\").join(\"/repo\", \"/.git\"))'",
        "node -e 'require(\"fs\").rmSync(require(\"path\").resolve(\".\"))'",
    ] {
        assert!(
            matches!(risk(command), Some(RemovalRisk::Critical(_))),
            "{command}: {:?}",
            risk(command)
        );
    }
    assert!(matches!(
        risk("node -ep 'console.log(1)'"),
        Some(RemovalRisk::Unresolved(_))
    ));
}

#[test]
fn uncertain_shadowing_cannot_restore_builtin_dispatch_proof() {
    for command in [
        "python3 -c 'import shutil; condition=True; condition and (print:=shutil.rmtree); print(\"/repo\")'",
        "python3 -c 'exec(\"import shutil; print=shutil.rmtree\"); print(\"/repo\")'",
        "node -e 'const fs=require(\"fs\"); if (true) { console=fs; }; console.log(\"/repo\")'",
        "node -e 'require(\"fs\").rmSync(require(\"path\").resolve(process.argv[1]))'",
    ] {
        assert!(
            matches!(risk(command), Some(RemovalRisk::Unresolved(_))),
            "{command}: {:?}",
            risk(command)
        );
    }
}

#[test]
fn an_unknown_remove_method_is_not_assumed_to_be_a_filesystem_api() {
    let command = "python3 -c 'items=[\".\"]; items.remove(\".\")'";
    assert!(matches!(risk(command), Some(RemovalRisk::Unresolved(_))));
}
