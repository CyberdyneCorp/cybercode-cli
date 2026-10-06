//! Analyze shell source delivered through stdin without interpreting ordinary data as code.

use super::{
    RemovalRisk, RemovalScope, SHELLS, State, WrapperCommand, inspect, literal, powershell, word,
    wrapper_command,
};
use std::path::Path;
use tree_sitter::Node;

#[derive(Clone, Copy)]
enum Language {
    Bash,
    PowerShell,
}

#[derive(Clone, Copy)]
struct Consumer {
    language: Language,
    directory_changed: bool,
}

pub(super) fn inspect_input(
    node: Node<'_>,
    source: &[u8],
    scope: &RemovalScope<'_>,
    depth: usize,
    state: &State,
    risks: &mut Vec<RemovalRisk>,
) {
    if node.kind() == "command"
        && node
            .parent()
            .is_some_and(|n| n.kind() == "redirected_statement")
    {
        return;
    }
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
    let redirects = redirects(node);
    let active = redirects
        .iter()
        .rfind(|n| changes_stdin(**n, source))
        .copied();
    for redirect in redirects {
        if changes_stdin(redirect, source) && active != Some(redirect) {
            continue;
        }
        if let Some(consumer) = consumer.or_else(|| pipeline_shell(redirect, source, state)) {
            let mut child = state.shell_child(command, source, scope);
            if command_words(command, source, state)
                .first()
                .and_then(|v| v.as_deref())
                == Some("env")
            {
                child.invalidate();
            }
            if consumer.directory_changed {
                child.change_directory();
            }
            inspect_redirect(
                redirect,
                source,
                scope,
                depth,
                &child,
                consumer.language,
                risks,
            );
        }
    }
}

fn command_words(node: Node<'_>, source: &[u8], state: &State) -> Vec<Option<String>> {
    let mut cursor = node.walk();
    let mut words: Vec<_> = node
        .named_children(&mut cursor)
        .filter(|n| n.kind() != "variable_assignment" && !n.kind().ends_with("_redirect"))
        // The Bash grammar parses an adjacent here-string fd as a numeric argument.
        .filter(|n| {
            !(n.kind() == "number"
                && n.next_named_sibling().is_some_and(|next| {
                    next.kind() == "herestring_redirect" && n.end_byte() == next.start_byte()
                }))
        })
        .map(|n| word(n, source, state))
        .collect();
    if stdin_dash(node, source) {
        words.push(Some("-".into()));
    }
    words
}

/// The Bash grammar can omit a standalone `-` immediately before a heredoc.
pub(super) fn stdin_dash(command: Node<'_>, source: &[u8]) -> bool {
    let Some(owner) = command
        .parent()
        .filter(|n| n.kind() == "redirected_statement")
    else {
        return false;
    };
    let mut cursor = owner.walk();
    let redirect = owner
        .named_children(&mut cursor)
        .find(|n| n.kind() == "heredoc_redirect");
    let Some(redirect) = redirect else {
        return false;
    };
    source
        .get(command.end_byte()..redirect.start_byte())
        .and_then(|s| std::str::from_utf8(s).ok())
        .is_some_and(|gap| gap.trim_matches([' ', '\t']) == "-")
}

fn pipeline_shell(node: Node<'_>, source: &[u8], state: &State) -> Option<Consumer> {
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

fn forwarded_shell(node: Node<'_>, source: &[u8], state: &State) -> Option<Consumer> {
    let pipeline = node.parent().filter(|n| n.kind() == "pipeline")?;
    let mut cursor = pipeline.walk();
    pipeline
        .named_children(&mut cursor)
        .skip_while(|n| *n != node)
        .skip(1)
        .filter(|n| n.kind() == "command")
        .find_map(|n| stdin_shell(&command_words(n, source, state), state.directory_changed))
}

fn stdin_shell(mut words: &[Option<String>], mut changed_directory: bool) -> Option<Consumer> {
    // Each wrapper consumes words; iterate rather than recurse on untrusted input.
    loop {
        let name = words.first()?.as_deref()?;
        let name = Path::new(name).file_name()?.to_str()?;
        let args = &words[1..];
        if SHELLS.contains(&name) {
            return reads_stdin(args).then_some(Consumer {
                language: Language::Bash,
                directory_changed: changed_directory,
            });
        }
        if powershell::recognizes(name) {
            return powershell::stdin_properties(args).map(|uncertain| Consumer {
                language: Language::PowerShell,
                directory_changed: changed_directory || uncertain,
            });
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
    language: Language,
    risks: &mut Vec<RemovalRisk>,
) {
    let mut cursor = node.walk();
    let body = node.named_children(&mut cursor).find(|child| {
        (node.kind() == "herestring_redirect" && child.kind() != "file_descriptor")
            || child.kind() == "heredoc_body"
    });
    if !matches!(node.kind(), "heredoc_redirect" | "herestring_redirect") {
        if changes_stdin(node, source) {
            risks.push(RemovalRisk::Unresolved(
                "unresolved interpreter stdin redirect".into(),
            ));
        }
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
        return;
    }
    let mut cursor = node.walk();
    if node
        .named_children(&mut cursor)
        .any(|child| changes_stdin(child, source))
    {
        risks.push(RemovalRisk::Unresolved(
            "stdin redirect overrides literal source".into(),
        ));
        return;
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
    match language {
        Language::Bash => inspect(&script, scope, depth + 1, state, risks),
        Language::PowerShell => {
            powershell::inspect_program(&script, scope, depth + 1, state, risks)
        }
    }
}

fn expands_heredoc(node: Node<'_>, source: &[u8]) -> bool {
    let mut cursor = node.walk();
    node.named_children(&mut cursor)
        .find(|n| n.kind() == "heredoc_start")
        .and_then(|n| n.utf8_text(source).ok())
        .is_some_and(|s| !s.contains(['\'', '"', '\\']))
}

/// Only a literal fd-0 redirect proves the command's stdin source here.
pub(super) fn literal_stdin(command: Node<'_>, source: &[u8]) -> bool {
    let owner = command
        .parent()
        .filter(|n| n.kind() == "redirected_statement")
        .unwrap_or(command);
    let mut known = false;
    for node in redirects(owner) {
        if changes_stdin(node, source) {
            known = literal_redirect(node, source);
        }
    }
    known
}

fn redirects(owner: Node<'_>) -> Vec<Node<'_>> {
    let mut redirects = Vec::new();
    if let Some(body) = owner
        .child_by_field_name("body")
        .filter(|n| n.kind() == "command")
    {
        let mut cursor = body.walk();
        redirects.extend(
            body.named_children(&mut cursor)
                .filter(|n| n.kind().ends_with("_redirect")),
        );
    }
    let mut cursor = owner.walk();
    redirects.extend(
        owner
            .named_children(&mut cursor)
            .filter(|n| n.kind().ends_with("_redirect")),
    );
    redirects.sort_by_key(|node| node.start_byte());
    redirects
}

fn changes_stdin(node: Node<'_>, source: &[u8]) -> bool {
    if !node.kind().ends_with("_redirect") {
        return false;
    }
    if let Some(fd) = node
        .child_by_field_name("descriptor")
        .and_then(|n| n.utf8_text(source).ok())
    {
        return fd == "0";
    }
    if matches!(node.kind(), "heredoc_redirect" | "herestring_redirect") {
        return true;
    }
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .any(|n| matches!(n.kind(), "<" | "<&" | "<&-"))
}

fn literal_redirect(node: Node<'_>, source: &[u8]) -> bool {
    if !matches!(node.kind(), "heredoc_redirect" | "herestring_redirect") {
        return false;
    }
    if node
        .child_by_field_name("descriptor")
        .and_then(|n| n.utf8_text(source).ok())
        .is_some_and(|s| s != "0")
    {
        return false;
    }
    let mut cursor = node.walk();
    if node
        .named_children(&mut cursor)
        .any(|child| changes_stdin(child, source))
    {
        return false;
    }
    let mut cursor = node.walk();
    let body = node.named_children(&mut cursor).find(|child| {
        (node.kind() == "herestring_redirect" && child.kind() != "file_descriptor")
            || child.kind() == "heredoc_body"
    });
    let Some(body) = body else {
        return false;
    };
    if body.end_byte().saturating_sub(body.start_byte()) > 1024 * 1024 {
        return false;
    }
    if node.kind() == "herestring_redirect" {
        return literal(body, source).is_some();
    }
    let text = body.utf8_text(source).unwrap_or_default();
    !expands_heredoc(node, source) || !text.contains(['$', '`'])
}
