//! Analyze shell source delivered through stdin without interpreting ordinary data as code.

use super::{
    RemovalRisk, RemovalScope, SHELLS, State, WrapperCommand, inspect, literal, word,
    wrapper_command,
};
use std::path::Path;
use tree_sitter::Node;

pub(super) fn inspect_input(
    node: Node<'_>,
    source: &[u8],
    scope: &RemovalScope<'_>,
    depth: usize,
    state: &State,
    risks: &mut Vec<RemovalRisk>,
) {
    let command = match node.kind() {
        "redirected_statement" => node.child_by_field_name("body"),
        "command" => Some(node),
        _ => None,
    };
    let Some(command) = command.filter(|n| n.kind() == "command") else {
        return;
    };
    let consumer = stdin_shell(
        &command_words(command, source, state),
        state.directory_changed,
    )
    .or_else(|| forwarded_shell(node, source, state));
    let mut cursor = node.walk();
    for redirect in node.named_children(&mut cursor) {
        if let Some(directory_change) = consumer.or_else(|| pipeline_shell(redirect, source, state))
        {
            let mut child = state.shell_child(command, source, scope);
            if command_words(command, source, state)
                .first()
                .and_then(|v| v.as_deref())
                == Some("env")
            {
                child.invalidate();
            }
            if directory_change {
                child.change_directory();
            }
            inspect_redirect(redirect, source, scope, depth, &child, risks);
        }
    }
}

fn command_words(node: Node<'_>, source: &[u8], state: &State) -> Vec<Option<String>> {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .filter(|n| n.kind() != "variable_assignment" && !n.kind().ends_with("_redirect"))
        // The Bash grammar parses an adjacent here-string fd as a numeric argument.
        .filter(|n| {
            !(n.kind() == "number"
                && n.next_named_sibling().is_some_and(|next| {
                    next.kind() == "herestring_redirect" && n.end_byte() == next.start_byte()
                }))
        })
        .map(|n| word(n, source, state))
        .collect()
}

fn pipeline_shell(node: Node<'_>, source: &[u8], state: &State) -> Option<bool> {
    if node.kind() != "heredoc_redirect" {
        return None;
    }
    let mut cursor = node.walk();
    let pipeline = node
        .named_children(&mut cursor)
        .find(|n| n.kind() == "pipeline")?;
    let mut cursor = pipeline.walk();
    pipeline
        .named_children(&mut cursor)
        .filter(|n| n.kind() == "command")
        .find_map(|n| stdin_shell(&command_words(n, source, state), state.directory_changed))
}

fn forwarded_shell(node: Node<'_>, source: &[u8], state: &State) -> Option<bool> {
    let pipeline = node.parent().filter(|n| n.kind() == "pipeline")?;
    let mut cursor = pipeline.walk();
    pipeline
        .named_children(&mut cursor)
        .filter(|n| n.kind() == "command")
        .find_map(|n| stdin_shell(&command_words(n, source, state), state.directory_changed))
}

fn stdin_shell(mut words: &[Option<String>], mut changed_directory: bool) -> Option<bool> {
    // Each wrapper consumes words; iterate rather than recurse on untrusted input.
    loop {
        let name = words.first()?.as_deref()?;
        let name = Path::new(name).file_name()?.to_str()?;
        let args = &words[1..];
        if SHELLS.contains(&name) {
            return reads_stdin(args).then_some(changed_directory);
        }
        if !matches!(name, "command" | "builtin" | "exec" | "env" | "nohup") {
            return None;
        }
        match wrapper_command(name, args) {
            WrapperCommand::Run {
                index,
                changes_directory,
            } => {
                words = &args[index..];
                changed_directory |= changes_directory;
            }
            _ => return None,
        }
    }
}

fn reads_stdin(args: &[Option<String>]) -> bool {
    let mut index = 0;
    let mut stdin = false;
    while let Some(value) = args.get(index) {
        let Some(value) = value.as_deref() else {
            return true;
        };
        match value {
            "--help" | "--version" => return false,
            "--" => return stdin || index + 1 == args.len(),
            "-o" | "+o" | "-O" | "+O" | "--rcfile" | "--init-file" => index += 2,
            v if v.starts_with('-') && !v.starts_with("--") => {
                if v.contains('c') {
                    return false;
                }
                stdin |= v.contains('s');
                index += 1;
            }
            v if v.starts_with("--") => index += 1,
            _ => return stdin,
        }
    }
    true
}

fn inspect_redirect(
    node: Node<'_>,
    source: &[u8],
    scope: &RemovalScope<'_>,
    depth: usize,
    state: &State,
    risks: &mut Vec<RemovalRisk>,
) {
    let mut cursor = node.walk();
    let body = node.named_children(&mut cursor).find(|child| {
        (node.kind() == "herestring_redirect" && child.kind() != "file_descriptor")
            || child.kind() == "heredoc_body"
    });
    if !matches!(node.kind(), "heredoc_redirect" | "herestring_redirect") {
        return;
    }
    if node
        .child_by_field_name("descriptor")
        .and_then(|n| n.utf8_text(source).ok())
        .is_some_and(|s| s != "0")
    {
        risks.push(RemovalRisk::Unresolved(
            "shell input on another file descriptor".into(),
        ));
    }
    let Some(body) = body else { return };
    let script = if node.kind() == "herestring_redirect" {
        literal(body, source)
    } else {
        body.utf8_text(source).ok().map(str::to_owned)
    };
    let Some(script) = script else {
        risks.push(RemovalRisk::Unresolved("dynamic shell stdin".into()));
        return;
    };
    // Expansion can insert syntax before the receiving shell parses the stream.
    if node.kind() == "heredoc_redirect"
        && expands_heredoc(node, source)
        && (script.contains('$') || script.contains('`'))
    {
        risks.push(RemovalRisk::Unresolved("expanded shell heredoc".into()));
    }
    inspect(&script, scope, depth + 1, state, risks);
}

fn expands_heredoc(node: Node<'_>, source: &[u8]) -> bool {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .find(|n| n.kind() == "heredoc_start")
        .and_then(|n| n.utf8_text(source).ok())
        .is_some_and(|s| !s.contains(['\'', '"', '\\']))
}
