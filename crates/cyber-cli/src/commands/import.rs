//! `cyber import` (`compat-import` → Import command, Mapping report, Idempotent re-import,
//! Source detection). Detection and previews bypass configuration/bootstrap/log creation;
//! a write re-verifies every reviewed file, then replaces `cyber.jsonc` and writes the
//! mapping report.
use crate::{cli::GlobalArgs, error::CliError, output};
use cyber_core::{
    env::{EnvSource, ProcessEnv},
    import::{
        ImportPreview, PreviewOutput, SourceRoots, SourceTool, WrittenImport, detect_sources,
    },
    paths,
};
use serde::Serialize;

#[derive(Debug, clap::Args)]
pub struct ImportArgs {
    #[arg(long, conflicts_with_all=["tool", "dry_run", "yes", "include"])]
    pub detect: bool,
    #[arg(value_enum, required_unless_present = "detect")]
    pub tool: Option<ImportTool>,
    /// Print the file diff and the mapping report; never write.
    #[arg(long)]
    pub dry_run: bool,
    /// Write without asking for confirmation.
    #[arg(long)]
    pub yes: bool,
    /// Also import these items.
    #[arg(long, value_enum)]
    pub include: Vec<Include>,
    #[arg(long, value_enum, default_value = "project")]
    pub scope: Scope,
}
#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum ImportTool {
    Claude,
    Codex,
    Opencode,
    Auto,
}
#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum Scope {
    Project,
    Global,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Include {
    Sessions,
}

const SESSIONS_NOT_IMPORTED: &str =
    "sessions: not imported; session import is not available in this version";

/// What one import did, for `--format json`.
#[derive(Serialize)]
struct Outcome<'a> {
    #[serde(flatten)]
    preview: &'a PreviewOutput,
    dry_run: bool,
    nothing_to_do: bool,
    /// Where a write saves the mapping report.
    report_dir: &'a std::path::Path,
    written: Option<WrittenImport>,
    notes: Vec<&'static str>,
}

pub fn run(args: &ImportArgs, global: &GlobalArgs) -> Result<(), CliError> {
    if args.detect {
        return detect(global);
    }
    let tool = args
        .tool
        .ok_or_else(|| CliError::usage("choose a source tool"))?;
    let roots = source_roots(global)?;
    let scope = match args.scope {
        Scope::Project => cyber_core::import::ImportScope::Project,
        Scope::Global => cyber_core::import::ImportScope::Global,
    };
    let native = paths::Paths::resolve(&ProcessEnv, &roots.home).config;
    let preview = cyber_core::import::preview_import(&roots, source_tool(tool), scope, &native)
        .map_err(|e| CliError::runtime(e.reason))?;
    preview.verify().map_err(|e| CliError::runtime(e.reason))?;
    if preview.output().sources == 0 {
        return none_found(tool, &roots, args.scope, global);
    }
    let mut outcome = Outcome {
        preview: preview.output(),
        dry_run: args.dry_run,
        nothing_to_do: !preview.has_changes(),
        report_dir: preview.report_dir(),
        written: None,
        notes: Vec::new(),
    };
    if args.include.contains(&Include::Sessions) {
        outcome.notes.push(SESSIONS_NOT_IMPORTED);
    }
    if !output::is_json(global.format) {
        print_preview(&outcome);
    }
    if !outcome.nothing_to_do && !args.dry_run {
        if !args.yes && !confirmed(&preview)? {
            eprintln!("Import cancelled; no files written.");
            return Err(CliError::silent(crate::error::EXIT_RUNTIME));
        }
        outcome.written = Some(
            preview
                .write(&format!("Import report: {}", tool_name(tool)))
                .map_err(|e| CliError::runtime(e.reason))?,
        );
    }
    if output::is_json(global.format) {
        return output::json(&outcome);
    }
    print_outcome(&outcome);
    Ok(())
}
fn source_tool(tool: ImportTool) -> Option<SourceTool> {
    match tool {
        ImportTool::Claude => Some(SourceTool::Claude),
        ImportTool::Codex => Some(SourceTool::Codex),
        ImportTool::Opencode => Some(SourceTool::OpenCode),
        ImportTool::Auto => None,
    }
}
fn tool_name(tool: ImportTool) -> &'static str {
    match tool {
        ImportTool::Claude => "claude",
        ImportTool::Codex => "codex",
        ImportTool::Opencode => "opencode",
        ImportTool::Auto => "auto",
    }
}
/// Ask on stderr and read one line of stdin; anything but `y`/`yes` (EOF included)
/// declines.
fn confirmed(preview: &ImportPreview) -> Result<bool, CliError> {
    use std::io::Write;
    eprint!(
        "Write these changes to {}? [y/N] ",
        escaped(&preview.output().target.to_string_lossy())
    );
    std::io::stderr().flush()?;
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer)?;
    Ok(matches!(
        answer.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}
/// `no claude configuration found in /repo or ~/.claude`; exits 0.
fn none_found(
    tool: ImportTool,
    roots: &SourceRoots,
    scope: Scope,
    global: &GlobalArgs,
) -> Result<(), CliError> {
    let codex = roots
        .codex_home
        .as_ref()
        .map_or_else(|| "~/.codex".into(), |p| escaped(&p.to_string_lossy()));
    let homes = match tool {
        ImportTool::Claude => "~/.claude".to_string(),
        ImportTool::Codex => codex,
        ImportTool::Opencode => "~/.config/opencode".into(),
        ImportTool::Auto => format!("~/.config/opencode, {codex} or ~/.claude"),
    };
    let tools = match tool {
        ImportTool::Auto => "opencode, codex or claude",
        other => tool_name(other),
    };
    let message = match scope {
        Scope::Project => format!(
            "no {tools} configuration found in {} or {homes}",
            escaped(&roots.project_root.to_string_lossy())
        ),
        Scope::Global => format!("no {tools} configuration found in {homes}"),
    };
    if output::is_json(global.format) {
        return output::json(&serde_json::json!({ "found": false, "message": message }));
    }
    println!("{message}");
    Ok(())
}
fn print_preview(outcome: &Outcome<'_>) {
    let view = outcome.preview;
    println!("Target: {}", escaped(&view.target.to_string_lossy()));
    println!("Raw file-layer preview; profiles and substitutions are not evaluated.");
    for layer in &view.native_layers {
        println!("Native layer: {}", escaped(&layer.to_string_lossy()));
    }
    print!("{}", view.diff);
    if !outcome.nothing_to_do {
        println!(
            "New file: {} (the mapping report below)",
            escaped(
                &outcome
                    .report_dir
                    .join("import-report-<timestamp>.md")
                    .to_string_lossy()
            )
        );
    }
    for item in &view.report {
        println!(
            "{}: {} {} — {}",
            item.status,
            escaped(&item.source.to_string_lossy()),
            escaped(&item.field),
            item.reason
        );
        if let Some(comparison) = &item.comparison {
            println!(
                "  kept: {}",
                serde_json::to_string(&comparison.kept).unwrap_or_else(|_| "<unavailable>".into())
            );
            println!(
                "  ignored: {}",
                serde_json::to_string(&comparison.ignored)
                    .unwrap_or_else(|_| "<unavailable>".into())
            );
        }
        for source in &item.sources {
            println!(
                "  from: {} {}",
                escaped(&source.source.to_string_lossy()),
                escaped(&source.field)
            );
        }
    }
    for required in &view.required_environment {
        for source in &required.sources {
            println!(
                "  Credential source: {} {}",
                escaped(&source.source.to_string_lossy()),
                escaped(&source.field)
            );
        }
        println!(
            "Set {} from the source field {}",
            required.requirement.variable, required.requirement.field
        );
    }
    for note in &outcome.notes {
        println!("{note}");
    }
    println!(
        "Partial coverage: rule files, Codex agent files and requirements.toml are reported above, not converted."
    );
}
fn print_outcome(outcome: &Outcome<'_>) {
    if outcome.nothing_to_do {
        println!("nothing to do");
    } else if let Some(written) = &outcome.written {
        println!("Wrote {}", escaped(&written.target.to_string_lossy()));
        println!("Report: {}", escaped(&written.report.to_string_lossy()));
    } else {
        println!("Dry run: no files written.");
    }
}
fn source_roots(global: &GlobalArgs) -> Result<SourceRoots, CliError> {
    let env = ProcessEnv;
    let home = paths::home_dir(&env)
        .ok_or_else(|| CliError::runtime("cannot determine the home directory"))?;
    let directory = global
        .cwd
        .clone()
        .map_or_else(std::env::current_dir, Ok)?
        .canonicalize()?;
    let project_root = git_root(&directory).unwrap_or_else(|| directory.clone());
    Ok(SourceRoots {
        project_root,
        directory,
        home,
        codex_home: env.get("CODEX_HOME").map(Into::into),
    })
}
pub fn detect(global: &GlobalArgs) -> Result<(), CliError> {
    let roots = source_roots(global)?;
    let report = detect_sources(&roots).map_err(|e| CliError::runtime(e.reason))?;
    if output::is_json(global.format) {
        return output::json(&report);
    }
    if report.tools.is_empty() {
        println!("no opencode, codex or claude configuration found");
    }
    for tool in &report.tools {
        print_detected(tool);
    }
    for issue in &report.issues {
        println!(
            "issue: {}: {}",
            escaped(&issue.path.to_string_lossy()),
            issue.reason
        );
    }
    println!(
        "Coverage incomplete: sources referenced from inside configuration are not resolved. Counts include raw definitions in separate layers; read-time assumes the default compatibility settings."
    );
    Ok(())
}
/// `claude: 3 agents; read-time: no`, one line per counted item, then the files.
fn print_detected(tool: &cyber_core::import::DetectedTool) {
    let name = match tool.tool {
        SourceTool::OpenCode => "opencode",
        SourceTool::Codex => "codex",
        SourceTool::Claude => "claude",
    };
    let (c, r) = (&tool.counts, &tool.read_time);
    let items = [
        (c.agents, r.agents, "agents"),
        (c.commands, r.commands, "commands"),
        (c.skills, r.skills, "skills"),
        (c.mcp_servers, r.mcp_servers, "mcp servers"),
        (c.hooks, r.hooks, "hooks"),
    ];
    for (count, covered, label) in items.into_iter().filter(|(count, ..)| *count > 0) {
        println!(
            "{name}: {count} {label}; read-time: {}",
            coverage(count, covered)
        );
    }
    match c.sessions {
        Some(0) => (),
        Some(count) => println!(
            "{name}: {count} sessions; read-time: {}",
            coverage(count, r.sessions)
        ),
        None => println!("{name}: sessions: unknown; read-time: no"),
    }
    for file in &tool.files {
        println!("  {}", escaped(&file.path.to_string_lossy()));
    }
}
fn coverage(count: usize, covered: usize) -> String {
    match covered {
        0 => "no".into(),
        n if n >= count => "yes".into(),
        n => format!("{n} of {count}"),
    }
}
fn git_root(directory: &std::path::Path) -> Option<std::path::PathBuf> {
    // Metadata only; no hooks, helpers, subprocesses or project-id cache writes.
    cyber_core::project::worktree_root(directory)
}
fn escaped(value: &str) -> String {
    value.chars().flat_map(char::escape_debug).collect()
}
