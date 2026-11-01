//! Direct owner-client hex entry. Resolves names only; byte access stays on the server.
use crate::{
    cli::{GlobalArgs, LaunchArgs},
    context::Context,
    error::CliError,
};
use clap::Args;
use std::path::PathBuf;

#[derive(Debug, Args)]
pub struct HexArgs {
    /// File, resolved against --cwd and confined to its project.
    pub path: PathBuf,
    /// Initial byte offset, as decimal or 0x hexadecimal.
    #[arg(long,default_value="0",value_parser=offset)]
    pub offset: u64,
    /// Open an existing Session without rebinding its Location.
    #[arg(long)]
    pub session: Option<String>,
    /// Inspect bytes without entering an edit draft.
    #[arg(long)]
    pub read_only: bool,
}
fn offset(value: &str) -> Result<u64, String> {
    match value.strip_prefix("0x") {
        Some(hex) => u64::from_str_radix(hex, 16),
        None => value.parse(),
    }
    .map_err(|_| "expected a nonnegative decimal or 0x hexadecimal byte offset".into())
}

pub fn run(args: HexArgs, mut launch: LaunchArgs, global: &GlobalArgs) -> Result<(), CliError> {
    if launch.prompt.is_some()
        || launch.worktree.is_some()
        || launch.fork
        || launch.from_pr.is_some()
        || launch.resume.is_some()
        || launch.resume_last
    {
        return Err(CliError::usage(
            "hex uses --session and cannot combine with prompt, worktree, fork or resume launch options",
        ));
    }
    let ctx = Context::new(global)?;
    let root = cyber_core::config::project_root(&ctx.location)
        .canonicalize()
        .map_err(|_| CliError::usage("project root is unavailable"))?;
    let absolute = if args.path.is_absolute() {
        args.path
    } else {
        ctx.location.join(args.path)
    };
    let mut target = PathBuf::new();
    for component in absolute.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                if !target.pop() {
                    return Err(CliError::usage("path traversal outside project"));
                }
            }
            component => target.push(component.as_os_str()),
        }
    }
    let relative = target.strip_prefix(&root).map_err(|_| {
        CliError::usage("hex target is outside the project; select its --cwd explicitly")
    })?;
    let path = relative
        .to_str()
        .filter(|p| !p.is_empty())
        .ok_or_else(|| CliError::usage("hex requires a UTF-8 file path"))?;
    launch.resume = args.session;
    launch.hex = Some(cyber_tui::HexStart {
        path: cyber_core::vcs::slash_path(std::path::Path::new(path)),
        root: root.display().to_string(),
        offset: args.offset,
        read_only: args.read_only,
    });
    super::tui::run(&launch, global, None)
}

#[cfg(test)]
mod tests {
    use clap::Parser;
    #[test]
    fn hex_entry_parses_exact_offsets_and_rejects_invalid_sizes_before_admission() {
        let cli = crate::cli::Cli::try_parse_from([
            "cyber",
            "hex",
            "fixture.bin",
            "--offset",
            "0x20000000000001",
            "--session",
            "ses_one",
            "--read-only",
        ])
        .unwrap();
        let Some(crate::cli::Command::Hex(args)) = cli.command else {
            panic!("hex command")
        };
        assert_eq!(args.offset, 9007199254740993);
        assert!(args.read_only);
        assert_eq!(args.session.as_deref(), Some("ses_one"));
        for value in ["-1", "18446744073709551616", "1.2"] {
            assert!(
                crate::cli::Cli::try_parse_from(["cyber", "hex", "f", "--offset", value]).is_err()
            );
        }
    }
}
