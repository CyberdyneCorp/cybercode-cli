//! PowerShell command-source analysis; unsupported dispatch retains manual confirmation.

use super::{MAX_NESTING, RemovalRisk, RemovalScope, State, inspect_path, word};
use base64::Engine as _;
use tree_sitter::{Node, Parser};

mod command_line;
use command_line::Source;

pub(super) fn recognizes(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "pwsh" | "pwsh.exe" | "powershell" | "powershell.exe"
    )
}

fn unresolved(risks: &mut Vec<RemovalRisk>, reason: &str) {
    let risk = RemovalRisk::Unresolved(format!("PowerShell: {reason}"));
    if !risks.contains(&risk) {
        risks.push(risk);
    }
}

pub(super) fn invocation(
    command: Node<'_>,
    args: &[Node<'_>],
    source: &[u8],
    scope: &RemovalScope<'_>,
    depth: usize,
    state: &State,
    risks: &mut Vec<RemovalRisk>,
) {
    let words: Option<Vec<_>> = args.iter().map(|n| word(*n, source, state)).collect();
    let Some(mut words) = words else {
        unresolved(risks, "dynamic invocation");
        return;
    };
    if super::input::stdin_dash(command, source) {
        words.push("-".into());
    }
    let invocation = command_line::parse(&words);
    if matches!(invocation.source, Source::Help) {
        return;
    }
    if !invocation.no_profile {
        unresolved(risks, "profile loading may change command resolution");
    }
    if !invocation.startup_certain {
        unresolved(risks, "unsupported or conflicting startup options");
    }
    let mut child = state.clone();
    if !invocation.no_profile || !invocation.startup_certain {
        child.change_directory();
    }
    let code = match invocation.source {
        Source::Text(words) => join_source(words),
        Source::Encoded(text) => decode_source(text),
        Source::Stdin => {
            if !super::input::literal_stdin(command, source) {
                unresolved(risks, "unresolved stdin source");
            }
            return;
        }
        Source::Missing | Source::Help => None,
    };
    match code {
        Some(code) => inspect_program(&code, scope, depth + 1, &child, risks),
        None => unresolved(risks, "missing, malformed or oversized source"),
    }
}

fn join_source(words: &[String]) -> Option<String> {
    if words.is_empty() {
        return None;
    }
    let size = words.iter().fold(0usize, |size, word| {
        size.saturating_add(word.len()).saturating_add(1)
    });
    (size <= 1024 * 1024).then(|| words.join(" "))
}

fn decode_source(text: &str) -> Option<String> {
    if text.len() > 1024 * 1024 {
        return None;
    }
    let text: String = text.chars().filter(|c| !c.is_ascii_whitespace()).collect();
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(text)
        .ok()?;
    let (pairs, remainder) = bytes.as_chunks::<2>();
    if !remainder.is_empty() {
        return None;
    }
    let units: Vec<_> = pairs
        .iter()
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect();
    String::from_utf16(&units)
        .ok()
        .filter(|code| !code.is_empty())
}

pub(super) fn stdin_properties(args: &[Option<String>]) -> Option<bool> {
    let words = args.iter().cloned().collect::<Option<Vec<_>>>()?;
    let invocation = command_line::parse(&words);
    matches!(invocation.source, Source::Stdin)
        .then_some(!invocation.no_profile || !invocation.startup_certain)
}

pub(super) fn inspect_program(
    code: &str,
    scope: &RemovalScope<'_>,
    depth: usize,
    state: &State,
    risks: &mut Vec<RemovalRisk>,
) {
    if depth > MAX_NESTING || code.len() > 1024 * 1024 {
        unresolved(risks, "source or nesting limit exceeded");
        return;
    }
    let mut parser = Parser::new();
    let tree = parser
        .set_language(&tree_sitter_powershell::LANGUAGE.into())
        .ok()
        .and_then(|()| parser.parse(code, None));
    let Some(tree) = tree.filter(|t| !t.root_node().has_error()) else {
        unresolved(risks, "unparseable source");
        return;
    };
    let mut state = state.clone();
    state.variables.clear();
    state
        .variables
        .insert("$home".into(), Some(scope.home.display().to_string()));
    state.variables.insert(
        "$pwd".into(),
        (!state.directory_changed).then(|| scope.workdir.display().to_string()),
    );
    walk(
        tree.root_node(),
        code.as_bytes(),
        scope,
        0,
        &mut state,
        risks,
    );
}

fn walk(
    node: Node<'_>,
    source: &[u8],
    scope: &RemovalScope<'_>,
    depth: usize,
    state: &mut State,
    risks: &mut Vec<RemovalRisk>,
) {
    if depth > 128 || !state.visit() {
        unresolved(risks, "analysis budget exhausted");
        return;
    }
    match node.kind() {
        "function_statement" | "class_statement" => {
            unresolved(risks, "custom command or class resolution");
            state.invalidate();
            return;
        }
        "assignment_expression" => assignment(node, source, state, risks),
        "command" => command(node, source, scope, state, risks),
        "invokation_expression" | "invokation_foreach_expression" => {
            member_removal(node, source, scope, state, risks)
        }
        "script_block_expression"
            if node
                .parent()
                .is_none_or(|p| p.kind() != "command_name_expr") =>
        {
            unresolved(risks, "deferred script block");
            return;
        }
        "if_statement" | "switch_statement" | "for_statement" | "foreach_statement"
        | "while_statement" | "try_statement" => {
            state.invalidate();
            state.change_directory();
            unresolved(risks, "uncertain control flow");
        }
        _ => {}
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if state.exhausted() {
            break;
        }
        walk(child, source, scope, depth + 1, state, risks);
    }
    if state.exhausted() {
        unresolved(risks, "analysis budget exhausted");
    }
    if matches!(
        node.kind(),
        "if_statement"
            | "switch_statement"
            | "for_statement"
            | "foreach_statement"
            | "while_statement"
            | "try_statement"
    ) {
        state.invalidate();
        state.change_directory();
    }
}

fn assignment(node: Node<'_>, source: &[u8], state: &mut State, risks: &mut Vec<RemovalRisk>) {
    if node.named_child(1).and_then(|n| n.utf8_text(source).ok()) != Some("=") {
        state.invalidate();
        unresolved(risks, "compound assignment");
        return;
    }
    let name = node
        .named_child(0)
        .and_then(|n| scalar(n, source, state, 0, true));
    let value = node
        .child_by_field_name("value")
        .and_then(|n| scalar(n, source, state, 0, false));
    if let Some(name) = name.filter(|n| n.starts_with('$') && !n.contains(':')) {
        state.variables.insert(name.to_ascii_lowercase(), value);
    } else {
        state.invalidate();
        unresolved(risks, "unsupported assignment");
    }
}

fn command(
    node: Node<'_>,
    source: &[u8],
    scope: &RemovalScope<'_>,
    state: &mut State,
    risks: &mut Vec<RemovalRisk>,
) {
    let Some(name) = node
        .child_by_field_name("command_name")
        .and_then(|n| scalar(n, source, state, 0, false))
    else {
        unresolved(risks, "dynamic command name");
        return;
    };
    let name = name.to_ascii_lowercase();
    if matches!(
        name.as_str(),
        "set-location"
            | "cd"
            | "chdir"
            | "sl"
            | "push-location"
            | "pop-location"
            | "pushd"
            | "popd"
    ) {
        state.change_directory();
        state.variables.insert("$pwd".into(), None);
        unresolved(risks, "directory change");
        return;
    }
    if matches!(
        name.as_str(),
        "remove-item" | "ri" | "rm" | "rmdir" | "rd" | "del" | "erase"
    ) || name == "microsoft.powershell.management\\remove-item"
    {
        removal(node, source, scope, state, risks);
    } else if !matches!(
        name.as_str(),
        "write-output" | "write-host" | "echo" | "get-location" | "pwd"
    ) {
        state.invalidate();
        unresolved(risks, "unresolved command dispatch");
    } else if has_parameters(node) {
        state.invalidate();
        unresolved(risks, "command parameters may mutate bindings");
    }
}

fn has_parameters(node: Node<'_>) -> bool {
    let Some(elements) = node.child_by_field_name("command_elements") else {
        return false;
    };
    let mut cursor = elements.walk();
    elements
        .named_children(&mut cursor)
        .any(|n| n.kind() == "command_parameter")
}

fn removal(
    node: Node<'_>,
    source: &[u8],
    scope: &RemovalScope<'_>,
    state: &State,
    risks: &mut Vec<RemovalRisk>,
) {
    let Some(elements) = node.child_by_field_name("command_elements") else {
        unresolved(risks, "missing removal target");
        return;
    };
    let mut cursor = elements.walk();
    let mut literal_path = false;
    let mut seen = false;
    for arg in elements.named_children(&mut cursor) {
        if arg.kind() == "command_argument_sep" {
            continue;
        }
        if arg.kind() == "command_parameter" {
            match arg
                .utf8_text(source)
                .unwrap_or_default()
                .to_ascii_lowercase()
                .as_str()
            {
                "-literalpath" => literal_path = true,
                "-path" => literal_path = false,
                "-recurse" | "-force" => {}
                _ => unresolved(risks, "unsupported removal option"),
            }
            continue;
        }
        seen = true;
        inspect_targets(arg, source, scope, state, literal_path, 0, risks);
    }
    if !seen {
        unresolved(risks, "missing removal target");
    }
}

fn inspect_targets(
    node: Node<'_>,
    source: &[u8],
    scope: &RemovalScope<'_>,
    state: &State,
    literal_path: bool,
    depth: usize,
    risks: &mut Vec<RemovalRisk>,
) {
    if depth > 16 || !state.visit() {
        unresolved(risks, "target analysis limit exceeded");
        return;
    }
    if node.kind() == "array_literal_expression" && node.named_child_count() > 1 {
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            inspect_targets(child, source, scope, state, literal_path, depth + 1, risks);
        }
        return;
    }
    let Some(path) = scalar(node, source, state, 0, false) else {
        unresolved(risks, "dynamic removal target");
        return;
    };
    if path.is_empty()
        || path.contains(':') && !windows_drive_path(&path)
        || !literal_path && path.contains(['*', '?', '[', ']'])
    {
        unresolved(risks, "provider or wildcard removal target");
        return;
    }
    inspect_path(&path, path.starts_with('~'), scope, state, risks);
}

fn windows_drive_path(path: &str) -> bool {
    let path = std::path::Path::new(path);
    path.is_absolute()
        && matches!(path.components().next(), Some(std::path::Component::Prefix(prefix)) if matches!(prefix.kind(), std::path::Prefix::Disk(_) | std::path::Prefix::VerbatimDisk(_)))
}

fn member_removal(
    node: Node<'_>,
    source: &[u8],
    scope: &RemovalScope<'_>,
    state: &State,
    risks: &mut Vec<RemovalRisk>,
) {
    let owner = node
        .named_child(0)
        .and_then(|n| n.utf8_text(source).ok())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let member = node
        .named_child(1)
        .and_then(|n| n.utf8_text(source).ok())
        .unwrap_or_default();
    if !matches!(
        owner.as_str(),
        "[system.io.directory]" | "[io.directory]" | "[system.io.file]" | "[io.file]"
    ) || !member.eq_ignore_ascii_case("delete")
    {
        unresolved(risks, "unresolved member invocation");
        return;
    }
    let target = node
        .named_child(2)
        .and_then(|n| n.child_by_field_name("argument_expression_list"))
        .and_then(|n| n.named_child(0));
    match target {
        Some(target) => inspect_targets(target, source, scope, state, true, 0, risks),
        None => unresolved(risks, "missing member removal target"),
    }
}

fn scalar(
    node: Node<'_>,
    source: &[u8],
    state: &State,
    depth: usize,
    variable_name: bool,
) -> Option<String> {
    if depth > 16 || !state.visit() {
        return None;
    }
    let text = node.utf8_text(source).ok()?;
    if text.len() > 64 * 1024 {
        return None;
    }
    match node.kind() {
        "variable" => {
            if variable_name {
                Some(text.into())
            } else {
                state
                    .variables
                    .get(&text.to_ascii_lowercase())
                    .cloned()
                    .flatten()
            }
        }
        "verbatim_string_characters" => Some(
            text.strip_prefix('\'')?
                .strip_suffix('\'')?
                .replace("''", "'"),
        ),
        "expandable_string_literal"
            if node.named_child_count() == 0 && !text.contains(['$', '`']) =>
        {
            Some(
                text.strip_prefix('"')?
                    .strip_suffix('"')?
                    .replace("\"\"", "\""),
            )
        }
        "generic_token" | "command_name"
            if node.named_child_count() == 0 && !text.contains(['$', '`', '\'', '"']) =>
        {
            Some(text.into())
        }
        "command_name_expr"
        | "path_command_name"
        | "left_assignment_expression"
        | "pipeline"
        | "pipeline_chain"
        | "argument_expression"
        | "logical_argument_expression"
        | "bitwise_argument_expression"
        | "comparison_argument_expression"
        | "additive_argument_expression"
        | "multiplicative_argument_expression"
        | "format_argument_expression"
        | "range_argument_expression"
        | "logical_expression"
        | "bitwise_expression"
        | "comparison_expression"
        | "additive_expression"
        | "multiplicative_expression"
        | "format_expression"
        | "range_expression"
        | "unary_expression"
        | "string_literal"
        | "array_literal_expression"
            if node.named_child_count() == 1 =>
        {
            scalar(
                node.named_child(0)?,
                source,
                state,
                depth + 1,
                variable_name,
            )
        }
        _ => None,
    }
}
