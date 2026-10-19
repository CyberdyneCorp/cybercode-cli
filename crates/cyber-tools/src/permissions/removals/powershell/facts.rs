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

#[derive(Clone, Copy, PartialEq, Eq)]
enum Argument {
    /// A target path; `slot` is the positional parameter it also binds.
    Path {
        literal: bool,
        slot: Option<usize>,
    },
    Name,
    /// A switch without a value.
    Flag,
    /// A value that is never a filesystem target.
    Skip,
}

fn mutator(name: &str) -> Option<usize> {
    match name.to_ascii_lowercase().as_str() {
        "set-content" | "sc" | "add-content" | "ac" | "clear-content" | "clc" | "out-file"
        | "tee-object" | "tee" | "new-item" | "ni" | "remove-item" | "ri" | "rm" | "rmdir"
        | "rd" | "del" | "erase" | "export-csv" | "epcsv" | "export-clixml" | "set-item" | "si"
        | "start-transcript" => Some(1),
        "copy-item" | "cpi" | "cp" | "copy" | "move-item" | "mi" | "mv" | "move"
        | "rename-item" | "rni" | "ren" | "expand-archive" | "compress-archive" => Some(2),
        // The positional parameter is a URI; only `-OutFile` names a target.
        "invoke-webrequest" | "iwr" | "invoke-restmethod" | "irm" | "curl" | "wget" => Some(0),
        _ => None,
    }
}

/// The parameters of the mutating cmdlets above, beyond the common ones.
const PARAMETERS: &[(&str, Argument)] = &[
    ("literalpath", PATH_LITERAL),
    ("path", PATH),
    ("filepath", PATH),
    ("destination", DESTINATION),
    ("destinationpath", DESTINATION),
    (
        "outfile",
        Argument::Path {
            literal: false,
            slot: None,
        },
    ),
    ("name", Argument::Name),
    ("newname", Argument::Name),
    ("force", Argument::Flag),
    ("recurse", Argument::Flag),
    ("append", Argument::Flag),
    ("noclobber", Argument::Flag),
    ("nonewline", Argument::Flag),
    ("passthru", Argument::Flag),
    ("asbytestream", Argument::Flag),
    ("container", Argument::Flag),
    ("notypeinformation", Argument::Flag),
    ("nti", Argument::Flag),
    ("includetypeinformation", Argument::Flag),
    ("iti", Argument::Flag),
    ("useculture", Argument::Flag),
    ("noheader", Argument::Flag),
    ("includeinvocationheader", Argument::Flag),
    ("useminimalheader", Argument::Flag),
    ("update", Argument::Flag),
    (
        "outputdirectory",
        Argument::Path {
            literal: false,
            slot: None,
        },
    ),
    ("variable", Argument::Skip),
    ("quotefields", Argument::Skip),
    ("qf", Argument::Skip),
    ("usequotes", Argument::Skip),
    ("uq", Argument::Skip),
    ("fromsession", Argument::Skip),
    ("tosession", Argument::Skip),
    ("value", Argument::Skip),
    ("encoding", Argument::Skip),
    ("inputobject", Argument::Skip),
    ("itemtype", Argument::Skip),
    ("type", Argument::Skip),
    ("credential", Argument::Skip),
    ("filter", Argument::Skip),
    ("include", Argument::Skip),
    ("exclude", Argument::Skip),
    ("stream", Argument::Skip),
    ("width", Argument::Skip),
    ("delimiter", Argument::Skip),
    ("uri", Argument::Skip),
    ("method", Argument::Skip),
    ("body", Argument::Skip),
    ("headers", Argument::Skip),
    ("compressionlevel", Argument::Skip),
    ("depth", Argument::Skip),
];

const PATH: Argument = Argument::Path {
    literal: false,
    slot: Some(0),
};
const PATH_LITERAL: Argument = Argument::Path {
    literal: true,
    slot: Some(0),
};
const DESTINATION: Argument = Argument::Path {
    literal: false,
    slot: Some(1),
};

fn kind(name: &str) -> Argument {
    PARAMETERS.iter().find(|(known, _)| *known == name).map_or(
        // Common parameters: switches or values that never name a target.
        if matches!(name, "verbose" | "debug" | "whatif" | "confirm") {
            Argument::Flag
        } else {
            Argument::Skip
        },
        |(_, kind)| *kind,
    )
}

/// What a parameter token of `cmdlet` binds, resolved against that cmdlet's own
/// parameters when they are known. An unknown or ambiguous parameter keeps its value as a
/// possible target, so an unrecognized spelling never hides a mutated path.
fn parameter(cmdlet: &str, text: &str) -> Argument {
    const NAMES: [&str; PARAMETERS.len()] = {
        let mut names = [""; PARAMETERS.len()];
        let mut index = 0;
        while index < PARAMETERS.len() {
            names[index] = PARAMETERS[index].0;
            index += 1;
        }
        names
    };
    if text == "--" {
        return Argument::Flag;
    }
    let names = super::parameters::of_cmdlet(cmdlet).unwrap_or(&NAMES);
    let kinds: Vec<_> = super::parameters::candidates(text, names)
        .into_iter()
        .map(kind)
        .collect();
    match kinds.split_first() {
        Some((first, rest)) if rest.iter().all(|kind| kind == first) => *first,
        _ => Argument::Path {
            literal: false,
            slot: None,
        },
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
            pending = match parameter(name, argument.utf8_text(source).unwrap_or_default()) {
                Argument::Flag => None,
                selected => Some(selected),
            };
            continue;
        }
        let selection = pending.take().unwrap_or_else(|| {
            let slot = bound.iter().position(|b| !b).unwrap_or(2);
            if slot == 1 && matches!(name, "rename-item" | "rni" | "ren") {
                Argument::Name
            } else if slot < arity {
                Argument::Path {
                    literal: false,
                    slot: Some(slot),
                }
            } else {
                Argument::Skip
            }
        });
        match selection {
            Argument::Path { literal, slot } => {
                if let Some(slot) = slot {
                    bound[slot] = true;
                }
                paths.extend(path_values(*argument, source, scope, state, literal, 0))
            }
            Argument::Name => {
                bound[1] = true;
                item_name = scalar(*argument, source, state, 0, false);
            }
            Argument::Flag | Argument::Skip => {}
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

/// The targets of a .NET file-system member call such as `[IO.File]::WriteAllText(p, x)`.
/// Unknown members return nothing; removal analysis still marks them unresolved.
pub(super) fn member(
    node: Node<'_>,
    source: &[u8],
    scope: &RemovalScope<'_>,
    state: &State,
) -> Option<SimpleCommand> {
    let owner = node
        .named_child(0)?
        .utf8_text(source)
        .ok()?
        .to_ascii_lowercase();
    let member = node
        .named_child(1)?
        .utf8_text(source)
        .ok()?
        .to_ascii_lowercase();
    let file = matches!(owner.as_str(), "[system.io.file]" | "[io.file]");
    let directory = matches!(owner.as_str(), "[system.io.directory]" | "[io.directory]");
    let targets: &[usize] = match member.as_str() {
        "copy" if file => &[1],
        "move" if file || directory => &[0, 1],
        "replace" if file => &[0, 1, 2],
        "writealltext" | "writealllines" | "writeallbytes" | "appendalltext" | "appendalllines"
        | "create" | "createtext" | "appendtext" | "openwrite" | "delete" | "setattributes"
        | "encrypt" | "decrypt"
            if file =>
        {
            &[0]
        }
        "createdirectory" | "delete" if directory => &[0],
        _ => return None,
    };
    let list = node
        .named_child(2)?
        .child_by_field_name("argument_expression_list")?;
    let mut cursor = list.walk();
    let arguments: Vec<_> = list.named_children(&mut cursor).collect();
    let mutates: Vec<_> = targets
        .iter()
        .filter_map(|index| arguments.get(*index))
        .flat_map(|argument| path_values(*argument, source, scope, state, true, 0))
        .collect();
    let text = node.utf8_text(source).ok()?.trim().to_string();
    (!mutates.is_empty()).then(|| SimpleCommand {
        always: text.clone(),
        text,
        mutates,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every parameter a cmdlet declares has a known kind: a name missing from
    /// [`PARAMETERS`] would silently bind as a common value parameter.
    #[test]
    fn every_cmdlet_parameter_has_a_kind() {
        for cmdlet in [
            "set-content",
            "clear-content",
            "out-file",
            "tee-object",
            "new-item",
            "remove-item",
            "export-csv",
            "export-clixml",
            "set-item",
            "start-transcript",
            "copy-item",
            "move-item",
            "rename-item",
            "expand-archive",
            "compress-archive",
        ] {
            assert!(mutator(cmdlet).is_some(), "{cmdlet}");
            for name in super::super::parameters::of_cmdlet(cmdlet).unwrap() {
                assert!(
                    PARAMETERS.iter().any(|(known, _)| known == name),
                    "{cmdlet} -{name}"
                );
            }
        }
    }
}
