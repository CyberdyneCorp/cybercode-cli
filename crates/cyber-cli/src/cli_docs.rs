//! The command-line reference (docs/cli.md), generated from the clap definition so it
//! cannot drift: `cyber help-markdown` prints it, `just docs-cli` writes it and a test
//! fails when the checked-in file is stale.

use std::fmt::Write;

use clap::{Arg, ArgAction, Command, CommandFactory};

use crate::cli::Cli;

/// The whole reference: every visible subcommand, depth first, with its arguments.
pub fn markdown() -> String {
    let mut root = Cli::command();
    root.build();
    let mut out = String::from(
        "# Command-line reference\n\n\
         <!-- Generated from the clap definition by `cyber help-markdown`. Do not edit; \
         regenerate with `just docs-cli`. -->\n\n\
         Generated from `cyber`'s argument definitions; `cyber <command> --help` prints the \
         same text. Options under `cyber` marked Global apply to every subcommand and are \
         not repeated below.\n",
    );
    section(&mut out, &mut root, true);
    out
}

fn section(out: &mut String, cmd: &mut Command, root: bool) {
    let name = cmd.get_bin_name().unwrap_or(cmd.get_name()).to_string();
    let _ = write!(out, "\n## `{name}`\n\n");
    if let Some(about) = cmd.get_long_about().or(cmd.get_about()) {
        let _ = write!(out, "{}\n\n", one_line(&about.to_string()));
    }
    let usage = cmd.render_usage().to_string();
    let usage = usage.trim().trim_start_matches("Usage:").trim();
    let _ = writeln!(out, "```text\n{usage}\n```");
    let args: Vec<&Arg> = cmd
        .get_arguments()
        .filter(|arg| !arg.is_hide_set() && arg.get_id() != "help")
        .filter(|arg| root || !arg.is_global_set())
        .collect();
    let (positionals, options): (Vec<&Arg>, Vec<&Arg>) =
        args.into_iter().partition(|arg| arg.is_positional());
    list(out, "Arguments", &positionals);
    list(out, "Options", &options);
    let subcommands: Vec<&Command> = visible(cmd).collect();
    if !subcommands.is_empty() {
        out.push_str("\n**Subcommands:**\n\n");
        for sub in subcommands {
            let full = sub.get_bin_name().unwrap_or(sub.get_name());
            let about = sub.get_about().map(|a| one_line(&a.to_string()));
            let _ = writeln!(
                out,
                "- [`{}`](#{}){}",
                sub.get_name(),
                anchor(full),
                about.map(|a| format!(": {a}")).unwrap_or_default()
            );
        }
    }
    for sub in cmd.get_subcommands_mut() {
        if !sub.is_hide_set() && sub.get_name() != "help" {
            section(out, sub, false);
        }
    }
}

fn visible(cmd: &Command) -> impl Iterator<Item = &Command> {
    cmd.get_subcommands()
        .filter(|sub| !sub.is_hide_set() && sub.get_name() != "help")
}

fn list(out: &mut String, title: &str, args: &[&Arg]) {
    if args.is_empty() {
        return;
    }
    let _ = write!(out, "\n**{title}:**\n\n");
    for arg in args {
        let mut sentences = Vec::new();
        if let Some(help) = arg.get_long_help().or(arg.get_help()) {
            let help = one_line(&help.to_string());
            let stop = if help.ends_with(['.', ')', ':']) {
                ""
            } else {
                "."
            };
            sentences.push(format!("{help}{stop}"));
        }
        sentences.extend(value_notes(arg));
        if arg.is_global_set() {
            sentences.push("Global.".into());
        }
        let _ = write!(out, "- `{}`", signature(arg));
        if !sentences.is_empty() {
            let _ = write!(out, ": {}", sentences.join(" "));
        }
        out.push('\n');
    }
}

/// "Values: …." and "Default: …." for an argument that takes a value.
fn value_notes(arg: &Arg) -> Vec<String> {
    if !takes_values(arg) {
        return Vec::new();
    }
    let mut notes = Vec::new();
    let values: Vec<String> = arg
        .get_possible_values()
        .iter()
        .filter(|value| !value.is_hide_set())
        .map(|value| format!("`{}`", value.get_name()))
        .collect();
    if !values.is_empty() {
        notes.push(format!("Values: {}.", values.join(", ")));
    }
    let defaults: Vec<String> = arg
        .get_default_values()
        .iter()
        .map(|value| value.to_string_lossy().into_owned())
        .collect();
    if !defaults.is_empty() {
        notes.push(format!("Default: `{}`.", defaults.join(",")));
    }
    notes
}

fn takes_values(arg: &Arg) -> bool {
    arg.get_action().takes_values()
}

/// `-m, --model <MODEL>`, `--worktree [<NAME>]`, `[PROMPT]...`.
fn signature(arg: &Arg) -> String {
    let value = value_name(arg);
    let range = arg.get_num_args().unwrap_or_default();
    let repeated = range.max_values() > 1 || matches!(arg.get_action(), ArgAction::Append);
    if arg.is_positional() {
        let mut s = if arg.is_required_set() {
            format!("<{value}>")
        } else {
            format!("[{value}]")
        };
        if repeated {
            s.push_str("...");
        }
        return s;
    }
    let mut names = Vec::new();
    if let Some(short) = arg.get_short() {
        names.push(format!("-{short}"));
    }
    if let Some(long) = arg.get_long() {
        names.push(format!("--{long}"));
    }
    let mut s = names.join(", ");
    if takes_values(arg) {
        if range.min_values() == 0 {
            let _ = write!(s, " [<{value}>]");
        } else {
            let _ = write!(s, " <{value}>");
        }
    }
    s
}

fn value_name(arg: &Arg) -> String {
    arg.get_value_names()
        .and_then(|names| names.first())
        .map(|name| name.to_string())
        .unwrap_or_else(|| arg.get_id().as_str().to_uppercase())
}

/// Help text on one Markdown list line.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// GitHub's heading anchor for `` `cyber exec` ``: `cyber-exec`.
fn anchor(heading: &str) -> String {
    heading
        .to_lowercase()
        .chars()
        .filter_map(|c| match c {
            ' ' => Some('-'),
            c if c.is_alphanumeric() || c == '-' || c == '_' => Some(c),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// docs/cli.md is the output of `cyber help-markdown`; a flag added without
    /// regenerating it fails here (and in CI).
    #[test]
    fn docs_cli_md_is_current() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/cli.md");
        let checked_in = std::fs::read_to_string(path)
            .unwrap_or_default()
            .replace("\r\n", "\n");
        assert!(
            checked_in == markdown(),
            "docs/cli.md is stale: run `just docs-cli` and commit the result"
        );
    }

    #[test]
    fn hidden_commands_and_global_repeats_are_left_out() {
        let doc = markdown();
        assert!(!doc.contains("## `cyber help-markdown`"));
        assert!(doc.contains("## `cyber exec`"));
        assert!(doc.contains("`--dangerously-skip-permissions`"));
        let exec = doc.split("## `cyber exec`").nth(1).unwrap();
        let exec = exec.split("\n## ").next().unwrap();
        assert!(!exec.contains("`--cwd <PATH>`"), "{exec}");
    }
}
