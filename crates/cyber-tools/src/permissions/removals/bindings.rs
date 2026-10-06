use std::collections::BTreeMap;
use std::{cell::Cell, rc::Rc};
use tree_sitter::Node;

use super::{RemovalScope, decode_escape};

const MAX_WORD_BYTES: usize = 64 * 1024;

#[derive(Clone)]
pub(super) struct State {
    remaining_nodes: Rc<Cell<u32>>,
    pub directory_changed: bool,
    pub variables: BTreeMap<String, Option<String>>,
    pub functions: BTreeMap<String, Vec<String>>,
}

impl State {
    pub fn new(scope: &RemovalScope<'_>, directory_changed: bool) -> Self {
        let mut state = Self {
            remaining_nodes: Rc::new(Cell::new(20_000)),
            directory_changed,
            variables: BTreeMap::new(),
            functions: BTreeMap::new(),
        };
        state
            .variables
            .insert("HOME".into(), Some(scope.home.display().to_string()));
        state.variables.insert(
            "PWD".into(),
            (!directory_changed).then(|| scope.workdir.display().to_string()),
        );
        state
    }

    pub fn exhausted(&self) -> bool {
        self.remaining_nodes.get() == 0
    }

    pub fn visit(&self) -> bool {
        let remaining = self.remaining_nodes.get();
        self.remaining_nodes.set(remaining.saturating_sub(1));
        remaining > 0
    }

    pub fn register_function(&mut self, node: Node<'_>, source: &[u8]) {
        if node.kind() != "function_definition" {
            return;
        }
        let name = node
            .child_by_field_name("name")
            .and_then(|n| n.utf8_text(source).ok());
        let body = node
            .child_by_field_name("body")
            .and_then(|n| n.utf8_text(source).ok());
        if let (Some(name), Some(body)) = (name, body) {
            self.functions.insert(name.into(), vec![body.into()]);
        }
    }

    pub fn merge_uncertainty(&mut self, branch: Self) {
        if branch.directory_changed {
            self.change_directory();
        }
        for (name, value) in branch.variables {
            if self.variables.get(&name) != Some(&value) {
                self.variables.insert(name, None);
            }
        }
        self.merge_functions(branch.functions);
    }

    pub fn merge_functions(&mut self, functions: BTreeMap<String, Vec<String>>) {
        for (name, bodies) in functions {
            let known = self.functions.entry(name).or_default();
            for body in bodies {
                if !known.contains(&body) {
                    known.push(body);
                }
            }
        }
    }

    pub fn prefix_child(&self, command: Node<'_>, source: &[u8], scope: &RemovalScope<'_>) -> Self {
        let mut child = self.clone();
        child.apply_prefix(command, source, scope);
        child
    }

    fn apply_prefix(&mut self, command: Node<'_>, source: &[u8], scope: &RemovalScope<'_>) {
        let mut cursor = command.walk();
        for assignment in command
            .named_children(&mut cursor)
            .filter(|n| n.kind() == "variable_assignment")
        {
            self.assignment(assignment, source, scope);
        }
    }

    pub fn shell_child(&self, command: Node<'_>, source: &[u8], scope: &RemovalScope<'_>) -> Self {
        let mut child = self.clone();
        child.invalidate();
        for name in ["HOME", "PWD"] {
            child
                .variables
                .insert(name.into(), self.variables.get(name).cloned().flatten());
        }
        child.apply_prefix(command, source, scope);
        child
    }

    pub fn invalidate(&mut self) {
        for value in self.variables.values_mut() {
            *value = None;
        }
    }

    pub fn change_directory(&mut self) {
        self.directory_changed = true;
        self.variables.insert("PWD".into(), None);
    }

    pub fn assignment(&mut self, node: Node<'_>, source: &[u8], scope: &RemovalScope<'_>) {
        let Some(name) = node.child_by_field_name("name") else {
            return;
        };
        if name.kind() != "variable_name" {
            self.invalidate();
            return;
        }
        let Ok(name) = name.utf8_text(source) else {
            return;
        };
        let value = node.child_by_field_name("value");
        let mut result = value
            .filter(|n| {
                !matches!(
                    n.kind(),
                    "array" | "binary_expression" | "unary_expression" | "postfix_expression"
                )
            })
            .and_then(|n| word(n, source, self));
        if node.utf8_text(source).is_ok_and(|s| s.contains("+=")) {
            result = None;
        }
        if let Some(raw) = value.and_then(|n| n.utf8_text(source).ok()) {
            if raw == "~" {
                result = Some(scope.home.display().to_string());
            } else if let Some(rest) = raw.strip_prefix("~/") {
                result = Some(scope.home.join(rest).display().to_string());
            }
        }
        self.variables.insert(name.into(), result);
    }
}

pub(super) fn word(node: Node<'_>, source: &[u8], state: &State) -> Option<String> {
    let raw = node.utf8_text(source).ok()?;
    let mut chars = raw.chars().peekable();
    let mut quote = None;
    let mut out = String::new();
    while let Some(c) = chars.next() {
        if out.len() >= MAX_WORD_BYTES {
            return None;
        }
        match (quote, c) {
            (Some('\''), '\'') | (Some('"'), '"') => quote = None,
            (Some('\''), c) => out.push(c),
            (_, '\\') => decode_escape(&mut out, quote, chars.next()?),
            (None, '\'' | '"') => quote = Some(c),
            (_, '$') => expand(&mut chars, state, quote.is_some(), &mut out)?,
            (_, '`') | (None, '*' | '?' | '[' | '{') => return None,
            (_, c) => out.push(c),
        }
    }
    (quote.is_none() && out.len() <= MAX_WORD_BYTES).then_some(out)
}

fn expand(
    chars: &mut std::iter::Peekable<std::str::Chars<'_>>,
    state: &State,
    quoted: bool,
    out: &mut String,
) -> Option<()> {
    let brace = chars.peek() == Some(&'{');
    if brace {
        chars.next();
    }
    let mut name = String::new();
    while chars
        .peek()
        .is_some_and(|c| c.is_ascii_alphanumeric() || *c == '_')
    {
        name.push(chars.next()?);
    }
    if name.is_empty() || (brace && chars.next()? != '}') {
        return None;
    }
    let value = state.variables.get(&name)?.as_ref()?;
    // Unquoted expansion may split into several operands or glob across critical roots.
    if !quoted
        && (value.is_empty()
            || value
                .chars()
                .any(|c| c.is_whitespace() || "*?[".contains(c)))
    {
        return None;
    }
    if out.len().saturating_add(value.len()) > MAX_WORD_BYTES {
        return None;
    }
    out.push_str(value);
    Some(())
}
