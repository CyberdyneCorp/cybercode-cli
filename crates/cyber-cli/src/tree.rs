//! The authoritative top-level command registry (`cli-commands` → Command tree).

use crate::cli::Format;
use crate::error::CliError;

/// Every top-level command, in `--help` group order. Kept equal to the spec by a test.
pub const COMMANDS: &[&str] = &[
    // Core
    "exec",
    "web",
    "attach",
    "sessions",
    "models",
    "providers",
    "agents",
    "skills",
    "commands",
    "memory",
    "worktree",
    "review",
    "pr",
    "git",
    // Orchestration
    "workflows",
    "goals",
    "loops",
    "teams",
    "routines",
    "handoff",
    "teleport",
    "apply",
    // Connectivity
    "serve",
    "service",
    "api",
    "login",
    "logout",
    "whoami",
    "tokens",
    "peers",
    "remote",
    "runners",
    "runner",
    "messages",
    "channels",
    "orchestrator",
    "relay",
    "share",
    "github",
    "acp",
    "ide",
    // Extensibility
    "mcp",
    "plugins",
    "hooks",
    "permissions",
    "trust",
    "sandbox",
    "features",
    "lsp",
    "fmt",
    "env",
    "import",
    // Maintenance
    "doctor",
    "debug",
    "db",
    "stats",
    "telemetry",
    "eval",
    "upgrade",
    "uninstall",
    "completion",
];

/// Error for a subcommand that clap did not match.
pub fn external(args: &[String]) -> CliError {
    let name = args.first().map(String::as_str).unwrap_or_default();
    if COMMANDS.contains(&name) {
        return CliError::unavailable(&format!("`cyber {name}`"), "P0 or later");
    }
    match suggest(name) {
        Some(s) => CliError::usage(format!("unknown command \"{name}\". Did you mean \"{s}\"?")),
        None => {
            CliError::usage(format!("unknown command \"{name}\"")).with_hint("run \"cyber --help\"")
        }
    }
}

/// `--format` given after an unmatched subcommand, which clap leaves unparsed.
pub fn format_in(args: &[String]) -> Option<Format> {
    let value = args.iter().enumerate().find_map(|(i, arg)| {
        arg.strip_prefix("--format=")
            .map(str::to_string)
            .or_else(|| {
                (arg == "--format")
                    .then(|| args.get(i + 1).cloned())
                    .flatten()
            })
    })?;
    <Format as clap::ValueEnum>::from_str(&value, false).ok()
}

/// The closest command within edit distance 2.
pub fn suggest(name: &str) -> Option<&'static str> {
    COMMANDS
        .iter()
        .map(|c| (strsim::levenshtein(name, c), *c))
        .filter(|(d, _)| *d <= 2)
        .min_by_key(|(d, _)| *d)
        .map(|(_, c)| c)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn format_after_external_command() {
        let args = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            format_in(&args(&["exec", "--format", "json"])),
            Some(Format::Json)
        );
        assert_eq!(
            format_in(&args(&["exec", "--format=json"])),
            Some(Format::Json)
        );
        assert_eq!(format_in(&args(&["exec", "hi"])), None);
    }

    #[test]
    fn suggestions_within_distance_two() {
        assert_eq!(suggest("sesions"), Some("sessions"));
        assert_eq!(suggest("dbug"), Some("debug"));
        assert_eq!(suggest("zzzzzzzz"), None);
    }

    #[test]
    fn registry_matches_the_spec_command_tree() {
        let spec = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../openspec/specs/cli-commands/spec.md"
        ))
        .unwrap();
        let section = spec
            .split("### Requirement: Command tree")
            .nth(1)
            .and_then(|s| s.split("### Requirement:").next())
            .unwrap();
        let from_spec: BTreeSet<String> = section
            .lines()
            .filter(|l| l.starts_with("- **") && l.contains("**:"))
            .flat_map(|l| {
                l.split('`')
                    .skip(1)
                    .step_by(2)
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .filter(|c| c != "cyber [project]")
            .collect();
        let in_code: BTreeSet<String> = COMMANDS.iter().map(|c| c.to_string()).collect();
        assert_eq!(in_code, from_spec);
    }
}
