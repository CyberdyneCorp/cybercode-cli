//! Native Windows PowerShell dispatch with the shared shell authorization and owner.
#![cfg_attr(not(windows), allow(dead_code))]

use std::path::{Path, PathBuf};

use base64::Engine as _;
use cyber_server::runtime::{RetrySafety, ToolDef};
use futures::future::BoxFuture;
use serde_json::{Value, json};

use super::{Tool, ToolError, bash, def, failed, number, text};
use crate::host::Ctx;
use crate::permissions::{RemovalRisk, RemovalScope, powershell_analysis};

pub(crate) struct PowerShell;

impl Tool for PowerShell {
    fn def(&self) -> ToolDef {
        def(
            "powershell",
            "Run a native PowerShell command in the project. stdout and stderr are combined. timeout_ms defaults to 120000 (max 600000).",
            json!({
                "type": "object", "required": ["command"], "properties": {
                    "command": {"type": "string"}, "timeout_ms": {"type": "integer"}, "workdir": {"type": "string"},
                    "description": {"type": "string"}, "background": {"type": "boolean"}
                }
            }),
            RetrySafety::Never,
            false,
        )
    }

    fn run<'a>(&'a self, ctx: &'a Ctx<'a>) -> BoxFuture<'a, Result<String, ToolError>> {
        Box::pin(async move {
            let input = &ctx.inv.input;
            let command = text(input, "command");
            if command.trim().is_empty() {
                return Err(failed("command is empty"));
            }
            if command.len() > 1024 * 1024 {
                return Err(failed("PowerShell source exceeds the 1 MiB analysis limit"));
            }
            if input.get("background").and_then(Value::as_bool) == Some(true) {
                return Err(failed(
                    "background: true starts a background Job, which arrives in P1; run in the foreground",
                ));
            }
            let program = installed(&ctx.location).ok_or_else(|| {
                failed("Neither pwsh.exe nor powershell.exe is installed outside the checkout")
            })?;
            let raw = text(input, "workdir");
            let workdir = ctx.resolve(if raw.is_empty() { "." } else { raw });
            let project = cyber_core::config::project_root(&ctx.location);
            let native_home = std::env::var_os("USERPROFILE")
                .map(PathBuf::from)
                .unwrap_or_else(|| ctx.policy.home.clone());
            let scope = RemovalScope {
                location: &ctx.location,
                project: &project,
                home: &native_home,
                workdir: &workdir,
            };
            let (analysis, mut risk) = powershell_analysis(command, &scope);
            if native_home != ctx.policy.home {
                let configured = RemovalScope {
                    home: &ctx.policy.home,
                    ..scope
                };
                let (_, other) = powershell_analysis(command, &configured);
                if risk.is_none()
                    || matches!(other, Some(RemovalRisk::Critical(_)))
                        && !matches!(risk, Some(RemovalRisk::Critical(_)))
                {
                    risk = other;
                }
            }
            let mutates = analysis
                .commands
                .iter()
                .flat_map(|c| c.mutates.clone())
                .collect();
            bash::authorize_facts(ctx, command, &workdir, &analysis, risk, false, mutates).await?;
            let prepared = crate::sandboxing::prepare_command(
                ctx,
                &program.display().to_string(),
                &arguments(command),
            )
            .await?;
            let timeout = number(input, "timeout_ms")
                .unwrap_or(bash::DEFAULT_TIMEOUT_MS)
                .clamp(1, bash::MAX_TIMEOUT_MS);
            bash::execute_prepared(ctx, prepared, &workdir, timeout).await
        })
    }
}

fn arguments(source: &str) -> Vec<String> {
    let bytes: Vec<u8> = source.encode_utf16().flat_map(u16::to_le_bytes).collect();
    vec![
        "-NoLogo".into(),
        "-NoProfile".into(),
        "-NonInteractive".into(),
        "-OutputFormat".into(),
        "Text".into(),
        "-EncodedCommand".into(),
        base64::engine::general_purpose::STANDARD.encode(bytes),
    ]
}

pub(crate) fn installed(location: &Path) -> Option<PathBuf> {
    let root = crate::host::canonical(&cyber_core::config::project_root(location));
    let mut dirs: Vec<_> = std::env::var_os("PATH")
        .into_iter()
        .flat_map(|p| std::env::split_paths(&p).collect::<Vec<_>>())
        .collect();
    if let Some(programs) = std::env::var_os("ProgramFiles") {
        dirs.push(PathBuf::from(programs).join("PowerShell/7"));
    }
    if let Some(windows) = std::env::var_os("SystemRoot") {
        dirs.push(PathBuf::from(windows).join("System32/WindowsPowerShell/v1.0"));
    }
    select(&dirs, &root)
}

fn select(dirs: &[PathBuf], checkout: &Path) -> Option<PathBuf> {
    for name in ["pwsh.exe", "powershell.exe"] {
        for dir in dirs.iter().filter(|p| p.is_absolute()) {
            let path = crate::host::canonical(&dir.join(name));
            if !path.starts_with(checkout) && path.is_file() {
                return Some(path);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_encoding_preserves_original_syntax_and_unicode() {
        let source = "param($x); Write-Output 'quotes \" & 💡'; # trailing comment";
        let args = arguments(source);
        assert_eq!(
            &args[..6],
            [
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-OutputFormat",
                "Text",
                "-EncodedCommand"
            ]
        );
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&args[6])
            .unwrap();
        let units: Vec<_> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| u16::from_le_bytes(*b))
            .collect();
        assert_eq!(String::from_utf16(&units).unwrap(), source);
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn legacy_interpreter_preserves_encoded_source_and_exit_status() {
        let root = tempfile::tempdir().unwrap();
        let program = PathBuf::from(std::env::var_os("SystemRoot").unwrap())
            .join("System32/WindowsPowerShell/v1.0/powershell.exe");
        assert!(
            program.is_file(),
            "legacy PowerShell is required for native fallback proof"
        );
        let source =
            "param(); [IO.File]::WriteAllText('encoded-source.txt', 'quotes \" & 💡'); exit 23";
        let helper = cyber_sandbox::find_helper().expect("native process owner helper");
        let mut command = cyber_sandbox::windows_process::OwnedCommand::new(
            &helper,
            &program,
            &arguments(source),
        );
        command
            .command_mut()
            .current_dir(root.path())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        let mut child = command.spawn().await.unwrap();
        let status = tokio::time::timeout(std::time::Duration::from_secs(10), child.wait())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(status.code(), Some(23));
        assert_eq!(
            std::fs::read_to_string(root.path().join("encoded-source.txt")).unwrap(),
            "quotes \" & 💡"
        );
    }

    #[test]
    fn discovery_prefers_pwsh_and_excludes_relative_and_checkout_candidates() {
        let root = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(root.path()).unwrap();
        let repo = root.join("repo");
        let first = root.join("first");
        let second = root.join("second");
        for dir in [&repo, &first, &second] {
            std::fs::create_dir_all(dir).unwrap();
        }
        std::fs::write(repo.join("pwsh.exe"), "untrusted").unwrap();
        std::fs::write(first.join("powershell.exe"), "legacy").unwrap();
        std::fs::write(second.join("pwsh.exe"), "modern").unwrap();
        let dirs = [
            PathBuf::from("."),
            repo.clone(),
            first.clone(),
            second.clone(),
        ];
        assert_eq!(select(&dirs, &repo), Some(second.join("pwsh.exe")));
        std::fs::remove_file(second.join("pwsh.exe")).unwrap();
        assert_eq!(select(&dirs, &repo), Some(first.join("powershell.exe")));
        std::fs::remove_file(first.join("powershell.exe")).unwrap();
        assert_eq!(select(&dirs, &repo), None);
    }
}
