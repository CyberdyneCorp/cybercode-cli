//! Parse executable inline source with its language grammar, never by text search.

use super::{MAX_NESTING, RemovalRisk, RemovalScope, State, inspect, inspect_path, word};
use std::collections::BTreeMap;
use tree_sitter::{Node, Parser};

mod bindings;
mod values;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Language {
    Python,
    JavaScript,
}

pub(super) fn language(name: &str) -> Option<Language> {
    let name = name.strip_suffix(".exe").unwrap_or(name);
    if matches!(name, "node" | "nodejs") {
        return Some(Language::JavaScript);
    }
    let suffix = name.strip_prefix("python")?;
    suffix
        .chars()
        .all(|c| c.is_ascii_digit() || c == '.')
        .then_some(Language::Python)
}

pub(super) fn invocation(
    name: &str,
    args: &[Node<'_>],
    source: &[u8],
    scope: &RemovalScope<'_>,
    depth: usize,
    state: &State,
    risks: &mut Vec<RemovalRisk>,
) {
    let lang = language(name).expect("recognized interpreter");
    let words: Vec<_> = args.iter().map(|n| word(*n, source, state)).collect();
    let mut index = 0;
    while let Some(arg) = words.get(index).and_then(|v| v.as_deref()) {
        if code_flag(lang, arg) {
            match words.get(index + 1).and_then(|v| v.as_deref()) {
                Some(code) => {
                    inspect_program(lang, code, scope, depth + 1, &mut Frame::new(state), risks)
                }
                None => unresolved(risks, "dynamic inline source"),
            }
            return;
        }
        if let Some(code) = attached_code(lang, arg) {
            inspect_program(lang, code, scope, depth + 1, &mut Frame::new(state), risks);
            return;
        }
        if matches!(arg, "--help" | "--version" | "-h" | "-V") {
            return;
        }
        if !arg.starts_with('-') || arg == "-" {
            break;
        }
        index += interpreter_option(lang, arg, risks);
    }
    unresolved(risks, "unresolved interpreter source or invocation");
}

fn code_flag(lang: Language, arg: &str) -> bool {
    match lang {
        Language::Python => python_code(arg).is_some_and(str::is_empty),
        Language::JavaScript => matches!(arg, "-e" | "--eval" | "-p" | "--print" | "-pe"),
    }
}

fn attached_code(lang: Language, arg: &str) -> Option<&str> {
    if lang == Language::Python {
        return python_code(arg).filter(|s| !s.is_empty());
    }
    if arg.starts_with("-pe") || arg.starts_with("-ep") {
        return None;
    }
    let prefixes: &[&str] = match lang {
        Language::Python => &[],
        Language::JavaScript => &["--eval=", "--print=", "-e", "-p"],
    };
    prefixes
        .iter()
        .find_map(|p| arg.strip_prefix(p).filter(|v| !v.is_empty()))
}

fn python_code(arg: &str) -> Option<&str> {
    let short = arg.strip_prefix('-')?;
    let index = short.find('c')?;
    short[..index]
        .chars()
        .all(|c| "bBdEiIOqRsSuvx".contains(c))
        .then_some(&short[index + 1..])
}

fn interpreter_option(lang: Language, arg: &str, risks: &mut Vec<RemovalRisk>) -> usize {
    if lang == Language::Python {
        if matches!(arg, "-W" | "-X") {
            return 2;
        }
        if arg.starts_with("-W")
            || arg.starts_with("-X")
            || arg
                .strip_prefix('-')
                .is_some_and(|s| !s.is_empty() && s.chars().all(|c| "bBdEiIOqRsSuvx".contains(c)))
        {
            return 1;
        }
    }
    if lang == Language::JavaScript
        && matches!(arg, "--input-type=module" | "--input-type=commonjs")
    {
        return 1;
    }
    unresolved(risks, "unsupported interpreter option");
    if matches!(arg, "-r" | "--require" | "--import" | "--loader" | "-m") {
        2
    } else {
        1
    }
}

#[derive(Clone)]
struct Frame {
    shell: State,
    values: BTreeMap<String, Option<String>>,
    aliases: BTreeMap<String, String>,
    syntax_depth: usize,
}

impl Frame {
    fn new(shell: &State) -> Self {
        Self {
            shell: shell.clone(),
            values: BTreeMap::new(),
            aliases: BTreeMap::new(),
            syntax_depth: 0,
        }
    }
    fn merge_uncertain(&mut self, branch: &Self) {
        for name in branch.values.keys().chain(branch.aliases.keys()) {
            self.values.insert(name.clone(), None);
        }
        self.invalidate();
    }
    fn invalidate(&mut self) {
        for value in self.values.values_mut() {
            *value = None;
        }
        self.aliases.clear();
    }
}

fn unresolved(risks: &mut Vec<RemovalRisk>, reason: &str) {
    let risk = RemovalRisk::Unresolved(reason.into());
    if !risks.contains(&risk) {
        risks.push(risk);
    }
}

fn inspect_program(
    lang: Language,
    code: &str,
    scope: &RemovalScope<'_>,
    depth: usize,
    frame: &mut Frame,
    risks: &mut Vec<RemovalRisk>,
) {
    if depth > MAX_NESTING || code.len() > 1024 * 1024 || frame.shell.exhausted() {
        unresolved(risks, "inline analysis limit exceeded");
        return;
    }
    let mut parser = Parser::new();
    let grammar = match lang {
        Language::Python => tree_sitter_python::LANGUAGE,
        Language::JavaScript => tree_sitter_javascript::LANGUAGE,
    };
    let tree = parser
        .set_language(&grammar.into())
        .ok()
        .and_then(|()| parser.parse(code, None));
    let Some(tree) = tree.filter(|t| !t.root_node().has_error()) else {
        unresolved(risks, "unparseable inline source");
        return;
    };
    walk(
        tree.root_node(),
        code.as_bytes(),
        lang,
        scope,
        depth,
        frame,
        risks,
    );
}

fn walk(
    node: Node<'_>,
    src: &[u8],
    lang: Language,
    scope: &RemovalScope<'_>,
    depth: usize,
    frame: &mut Frame,
    risks: &mut Vec<RemovalRisk>,
) {
    if frame.syntax_depth >= 128 {
        unresolved(risks, "inline syntax nesting limit exceeded");
        return;
    }
    frame.syntax_depth += 1;
    walk_body(node, src, lang, scope, depth, frame, risks);
    frame.syntax_depth -= 1;
}

fn walk_body(
    node: Node<'_>,
    src: &[u8],
    lang: Language,
    scope: &RemovalScope<'_>,
    depth: usize,
    frame: &mut Frame,
    risks: &mut Vec<RemovalRisk>,
) {
    if !frame.shell.visit() {
        unresolved(risks, "inline analysis budget exhausted");
        return;
    }
    if isolated(node.kind()) || conditional_operator(node, src) {
        let mut branch = frame.clone();
        branch.invalidate();
        children(node, src, lang, scope, depth, &mut branch, risks);
        frame.merge_uncertain(&branch);
        frame.shell.directory_changed |= branch.shell.directory_changed;
        return;
    }
    bindings::update(node, src, scope, frame, risks);
    if matches!(node.kind(), "call" | "call_expression") {
        inspect_call(node, src, lang, scope, depth, frame, risks);
    }
    children(node, src, lang, scope, depth, frame, risks);
}

fn isolated(kind: &str) -> bool {
    matches!(
        kind,
        "function_definition"
            | "function_declaration"
            | "function_expression"
            | "arrow_function"
            | "class_definition"
            | "class_declaration"
            | "if_statement"
            | "conditional_expression"
            | "ternary_expression"
            | "lambda"
            | "do_statement"
            | "for_statement"
            | "for_in_statement"
            | "while_statement"
            | "try_statement"
            | "with_statement"
            | "list_comprehension"
            | "dictionary_comprehension"
            | "set_comprehension"
            | "generator_expression"
    )
}

fn conditional_operator(node: Node<'_>, src: &[u8]) -> bool {
    node.kind() == "boolean_operator"
        || node
            .child_by_field_name("operator")
            .and_then(|n| n.utf8_text(src).ok())
            .is_some_and(|s| matches!(s, "&&" | "||" | "??"))
}

fn children(
    node: Node<'_>,
    src: &[u8],
    lang: Language,
    scope: &RemovalScope<'_>,
    depth: usize,
    frame: &mut Frame,
    risks: &mut Vec<RemovalRisk>,
) {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if frame.shell.exhausted() {
            unresolved(risks, "inline analysis budget exhausted");
            break;
        }
        walk(child, src, lang, scope, depth, frame, risks);
    }
}

fn inspect_call(
    node: Node<'_>,
    src: &[u8],
    lang: Language,
    scope: &RemovalScope<'_>,
    depth: usize,
    frame: &mut Frame,
    risks: &mut Vec<RemovalRisk>,
) {
    let Some(function) = node.child_by_field_name("function") else {
        return;
    };
    let name = values::name(function, src, frame).unwrap_or_default();
    let method = name.rsplit('.').next().unwrap_or_default();
    let args = values::arguments(node);
    if known_removal(&name, function, src, scope, frame) {
        removal(function, &args, src, scope, frame, risks);
        if method == "removedirs" {
            removal_ancestors(&args, src, scope, frame, risks);
        }
    } else if matches!(name.as_str(), "eval" | "exec") {
        nested_program(&args, src, lang, scope, depth, frame, risks);
    } else if matches!(
        name.as_str(),
        "os.system" | "child_process.exec" | "child_process.execSync"
    ) {
        nested_shell(&args, src, scope, depth, frame, risks);
    } else if matches!(name.as_str(), "os.chdir" | "process.chdir") {
        frame.shell.change_directory();
        unresolved(risks, "inline working-directory change");
    } else if !values::known_call(node, &name, src, scope, frame) {
        unresolved(risks, "unresolved inline call dispatch");
    }
}

fn known_removal(
    name: &str,
    function: Node<'_>,
    src: &[u8],
    scope: &RemovalScope<'_>,
    frame: &Frame,
) -> bool {
    if matches!(
        name,
        "shutil.rmtree"
            | "os.remove"
            | "os.unlink"
            | "os.rmdir"
            | "os.removedirs"
            | "fs.rm"
            | "fs.rmSync"
            | "fs.rmdir"
            | "fs.rmdirSync"
            | "fs.unlink"
            | "fs.unlinkSync"
            | "fs.promises.rm"
            | "fs.promises.rmdir"
            | "fs.promises.unlink"
    ) {
        return true;
    }
    matches!(name.rsplit('.').next(), Some("unlink" | "rmdir"))
        && function
            .child_by_field_name("object")
            .and_then(|n| values::value(n, src, scope, frame, 0))
            .is_some()
}

fn removal(
    function: Node<'_>,
    args: &[Node<'_>],
    src: &[u8],
    scope: &RemovalScope<'_>,
    frame: &Frame,
    risks: &mut Vec<RemovalRisk>,
) {
    let receiver = function
        .child_by_field_name("object")
        .and_then(|n| values::value(n, src, scope, frame, 0));
    let argument =
        values::removal_target(args, src).and_then(|n| values::value(n, src, scope, frame, 0));
    match receiver.or(argument) {
        Some(path) => inspect_path(&path, false, scope, &frame.shell, risks),
        None => unresolved(risks, "dynamic inline removal argument"),
    }
    if args.iter().any(|n| {
        n.kind() == "keyword_argument"
            && n.child_by_field_name("name")
                .is_some_and(|n| n.utf8_text(src).ok() == Some("dir_fd"))
    }) {
        unresolved(risks, "inline removal uses a directory descriptor");
    }
}

fn removal_ancestors(
    args: &[Node<'_>],
    src: &[u8],
    scope: &RemovalScope<'_>,
    frame: &Frame,
    risks: &mut Vec<RemovalRisk>,
) {
    if let Some(path) =
        values::removal_target(args, src).and_then(|n| values::value(n, src, scope, frame, 0))
    {
        let joined = scope.workdir.join(path);
        for ancestor in joined.ancestors().skip(1) {
            inspect_path(
                &ancestor.to_string_lossy(),
                false,
                scope,
                &frame.shell,
                risks,
            );
        }
    }
}

fn nested_program(
    args: &[Node<'_>],
    src: &[u8],
    lang: Language,
    scope: &RemovalScope<'_>,
    depth: usize,
    frame: &mut Frame,
    risks: &mut Vec<RemovalRisk>,
) {
    let mut child = frame.clone();
    match args
        .first()
        .and_then(|n| values::value(*n, src, scope, frame, 0))
    {
        Some(code) => inspect_program(lang, &code, scope, depth + 1, &mut child, risks),
        None => unresolved(risks, "dynamic nested inline source"),
    }
    frame.merge_uncertain(&child);
    frame.shell.change_directory();
}

fn nested_shell(
    args: &[Node<'_>],
    src: &[u8],
    scope: &RemovalScope<'_>,
    depth: usize,
    frame: &mut Frame,
    risks: &mut Vec<RemovalRisk>,
) {
    match args
        .first()
        .and_then(|n| values::value(*n, src, scope, frame, 0))
    {
        Some(code) => inspect(&code, scope, depth + 1, &frame.shell, risks),
        None => unresolved(risks, "dynamic inline shell source"),
    }
}
