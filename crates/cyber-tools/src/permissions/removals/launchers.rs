//! Commands that launch another command (`sudo`, `nice`, `timeout`, `xargs`, `find
//! -exec`): the launched command is inspected as if it ran directly
//! (`permissions-modes` → Critical-path removal guard, wrappers and shell flow).

use std::path::Path;

use tree_sitter::Node;

use super::bindings::{State, word};
use super::{RemovalRisk, RemovalScope, SHELLS, inline, inspect, powershell};

/// Launchers whose own options come first and whose first operand that names a command
/// is the command they run. Option values that happen to name a command only make the
/// analysis more cautious.
pub(super) const LAUNCHERS: &[&str] = &[
    "sudo",
    "doas",
    "nice",
    "ionice",
    "timeout",
    "stdbuf",
    "time",
    "chrt",
    "taskset",
    "setsid",
    "flock",
    "unbuffer",
    "caffeinate",
    "watch",
    "xargs",
    "parallel",
];

/// Launchers that append arguments read from their input to the launched command.
const FROM_INPUT: &[&str] = &["xargs", "parallel"];

const REMOVERS: &[&str] = &["rm", "rmdir", "unlink"];

/// Commands whose arguments can remove files or run more shell source.
fn relevant(name: &str) -> bool {
    REMOVERS.contains(&name)
        || matches!(
            name,
            "find" | "eval" | "command" | "builtin" | "exec" | "env" | "nohup"
        )
        || SHELLS.contains(&name)
        || LAUNCHERS.contains(&name)
        || inline::language(name).is_some()
        || powershell::recognizes(name)
}

fn base(name: &str) -> &str {
    Path::new(name)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(name)
}

/// Inspect the command a launcher runs. A dynamic word before it is unresolved, and a
/// removal fed from input (`xargs rm`) cannot be proven safe.
pub(super) fn inspect_launcher(
    name: &str,
    args: &[Node<'_>],
    source: &[u8],
    scope: &RemovalScope<'_>,
    depth: usize,
    state: &State,
    risks: &mut Vec<RemovalRisk>,
) {
    let values: Vec<_> = args.iter().map(|n| word(*n, source, state)).collect();
    let Some(index) = values
        .iter()
        .position(|value| value.as_deref().is_none_or(|v| relevant(base(v))))
    else {
        return;
    };
    let Some(launched) = values[index].as_deref().map(base) else {
        risks.push(RemovalRisk::Unresolved("dynamic launched command".into()));
        return;
    };
    if FROM_INPUT.contains(&name) && (REMOVERS.contains(&launched) || launched == "find") {
        risks.push(RemovalRisk::Unresolved(
            "removal targets read from input".into(),
        ));
    }
    let Some(last) = args.last() else {
        return;
    };
    // Reparse the source span so quoting is preserved.
    if let Ok(text) = std::str::from_utf8(&source[args[index].start_byte()..last.end_byte()]) {
        inspect(text, scope, depth + 1, state, risks);
    }
}

/// How `find` removes: `-delete`, or `-exec`/`-execdir`/`-ok`/`-okdir` with a remover.
/// `Err` when it runs a shell or a command that cannot be read.
pub(super) fn find_removes(
    args: &[Node<'_>],
    source: &[u8],
    state: &State,
) -> Result<bool, &'static str> {
    let mut removes = false;
    for (index, node) in args.iter().enumerate() {
        match word(*node, source, state).as_deref() {
            Some("-delete") => removes = true,
            Some("-exec" | "-execdir" | "-ok" | "-okdir") => {
                let command = args
                    .get(index + 1)
                    .and_then(|n| word(*n, source, state))
                    .ok_or("dynamic find command")?;
                let command = base(&command);
                if REMOVERS.contains(&command) {
                    removes = true;
                } else if relevant(command) {
                    return Err("find runs a shell or launcher");
                }
            }
            _ => {}
        }
    }
    Ok(removes)
}
