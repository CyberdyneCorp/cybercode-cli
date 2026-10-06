//! Literal command resources and filesystem operands for native PowerShell.

use std::path::{Path, PathBuf};

use tree_sitter::Node;

use super::{RemovalScope, State, scalar};
use crate::bash_analysis::{SimpleCommand, normalize};

pub(super) fn command(
    node: Node<'_>,
    source: &[u8],
    scope: &RemovalScope<'_>,
    state: &State,
) -> SimpleCommand {
    let text = node
        .utf8_text(source)
        .unwrap_or_default()
        .trim()
        .to_string();
    let name_node = node.child_by_field_name("command_name");
    let name = name_node.and_then(|n| scalar(n, source, state, 0, false));
    let args = arguments(node);
    let literal_name = name_node.is_some_and(|n| n.kind() == "command_name");
    let always = if literal_name {
        prefix(name.as_deref().unwrap_or_default(), &args, source, state)
            .map(|prefix| format!("{prefix} *"))
            .unwrap_or_else(|| text.clone())
    } else {
        text.clone()
    };
    let mutates = operands(
        name.as_deref().unwrap_or_default(),
        &args,
        source,
        scope,
        state,
    );
    SimpleCommand {
        text,
        always,
        mutates,
    }
}

fn arguments(node: Node<'_>) -> Vec<Node<'_>> {
    let Some(elements) = node.child_by_field_name("command_elements") else {
        return Vec::new();
    };
    let mut cursor = elements.walk();
    elements
        .named_children(&mut cursor)
        .filter(|n| n.kind() != "command_argument_sep")
        .collect()
}

fn prefix(name: &str, args: &[Node<'_>], source: &[u8], state: &State) -> Option<String> {
    if name.is_empty() {
        return None;
    }
    if !matches!(
        name.to_ascii_lowercase().as_str(),
        "git"
            | "npm"
            | "pnpm"
            | "yarn"
            | "bun"
            | "cargo"
            | "go"
            | "docker"
            | "kubectl"
            | "gh"
            | "pip"
            | "pip3"
            | "uv"
            | "poetry"
            | "deno"
            | "dotnet"
            | "helm"
            | "terraform"
            | "make"
            | "just"
            | "rustup"
    ) {
        return Some(name.into());
    }
    let argument = args.first()?;
    let subcommand = scalar(*argument, source, state, 0, false)?;
    if subcommand.starts_with('-') || subcommand.chars().any(char::is_whitespace) {
        return None;
    }
    Some(format!("{name} {}", argument.utf8_text(source).ok()?))
}

#[derive(Clone, Copy)]
enum Argument {
    Path { literal: bool, slot: usize },
    Name,
    Skip,
}

fn mutator(name: &str) -> Option<usize> {
    match name.to_ascii_lowercase().as_str() {
        "set-content" | "sc" | "add-content" | "ac" | "clear-content" | "clc" | "out-file"
        | "tee-object" | "tee" | "new-item" | "ni" | "remove-item" | "ri" | "rm" | "rmdir"
        | "rd" | "del" | "erase" => Some(1),
        "copy-item" | "cpi" | "cp" | "copy" | "move-item" | "mi" | "mv" | "move"
        | "rename-item" | "rni" | "ren" => Some(2),
        _ => None,
    }
}

fn parameter(text: &str) -> Option<Argument> {
    match text.to_ascii_lowercase().as_str() {
        "-literalpath" => Some(Argument::Path {
            literal: true,
            slot: 0,
        }),
        "-path" | "-filepath" => Some(Argument::Path {
            literal: false,
            slot: 0,
        }),
        "-destination" => Some(Argument::Path {
            literal: false,
            slot: 1,
        }),
        "-name" | "-newname" => Some(Argument::Name),
        "-force" | "-recurse" | "-append" | "-noclobber" | "-nonewline" | "-whatif"
        | "-confirm" | "-passthru" => None,
        _ => Some(Argument::Skip),
    }
}

fn operands(
    name: &str,
    args: &[Node<'_>],
    source: &[u8],
    scope: &RemovalScope<'_>,
    state: &State,
) -> Vec<PathBuf> {
    let lower = name.to_ascii_lowercase();
    let name = match lower.split_once('\\') {
        Some(("microsoft.powershell.management" | "microsoft.powershell.utility", name)) => name,
        _ => lower.as_str(),
    };
    let arity = mutator(name);
    let mut pending = None;
    let mut bound = [false; 2];
    let mut paths = Vec::new();
    let mut item_name = None;
    for argument in args {
        if argument.kind() == "redirection" {
            paths.extend(redirection(*argument, source, scope, state));
            continue;
        }
        let Some(arity) = arity else { continue };
        if argument.kind() == "command_parameter" {
            pending = parameter(argument.utf8_text(source).unwrap_or_default());
            continue;
        }
        let selection = pending.take().unwrap_or_else(|| {
            let slot = bound.iter().position(|b| !b).unwrap_or(2);
            if slot == 1 && matches!(name, "rename-item" | "rni" | "ren") {
                Argument::Name
            } else if slot < arity {
                Argument::Path {
                    literal: false,
                    slot,
                }
            } else {
                Argument::Skip
            }
        });
        match selection {
            Argument::Path { literal, slot } => {
                bound[slot] = true;
                paths.extend(path_values(*argument, source, scope, state, literal, 0))
            }
            Argument::Name => {
                bound[1] = true;
                item_name = scalar(*argument, source, state, 0, false);
            }
            Argument::Skip => {}
        }
    }
    append_name(name, &mut paths, item_name.as_deref());
    paths
}

fn append_name(command: &str, paths: &mut Vec<PathBuf>, name: Option<&str>) {
    let Some(name) = name.filter(|name| !name.is_empty() && !name.contains(['*', '?', '[', ']']))
    else {
        return;
    };
    let rename = matches!(
        command.to_ascii_lowercase().as_str(),
        "rename-item" | "rni" | "ren"
    );
    let create = matches!(command.to_ascii_lowercase().as_str(), "new-item" | "ni");
    if !rename && !create {
        return;
    }
    let destinations: Vec<_> = paths
        .iter()
        .filter_map(|path| {
            let root = if rename {
                path.parent()?
            } else {
                path.as_path()
            };
            Some(normalize(&crate::host::canonical(&root.join(name))))
        })
        .collect();
    paths.extend(destinations);
}

fn redirection(
    node: Node<'_>,
    source: &[u8],
    scope: &RemovalScope<'_>,
    state: &State,
) -> Vec<PathBuf> {
    let mut cursor = node.walk();
    let Some(file) = node
        .named_children(&mut cursor)
        .find(|n| n.kind() == "redirected_file_name")
    else {
        return Vec::new();
    };
    let mut cursor = file.walk();
    file.named_children(&mut cursor)
        .filter(|n| n.kind() != "command_argument_sep")
        .flat_map(|n| path_values(n, source, scope, state, true, 0))
        .collect()
}

fn path_values(
    node: Node<'_>,
    source: &[u8],
    scope: &RemovalScope<'_>,
    state: &State,
    literal: bool,
    depth: usize,
) -> Vec<PathBuf> {
    if depth > 16 || !state.visit() {
        return Vec::new();
    }
    if node.kind() == "array_literal_expression" && node.named_child_count() > 1 {
        let mut cursor = node.walk();
        return node
            .named_children(&mut cursor)
            .flat_map(|n| path_values(n, source, scope, state, literal, depth + 1))
            .collect();
    }
    scalar(node, source, state, 0, false)
        .and_then(|text| path(&text, scope, state, literal))
        .into_iter()
        .collect()
}

fn path(text: &str, scope: &RemovalScope<'_>, state: &State, literal: bool) -> Option<PathBuf> {
    let path = Path::new(text);
    if text.is_empty()
        || (!literal && text.contains(['*', '?', '[', ']']))
        || (text.contains(':') && !path.is_absolute())
        || (state.directory_changed && !path.is_absolute())
    {
        return None;
    }
    Some(normalize(&crate::host::canonical(
        &scope.workdir.join(path),
    )))
}
