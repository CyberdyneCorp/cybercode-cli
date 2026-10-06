//! Literal bindings and structural import aliases; uncertain mutations invalidate proof.

use super::{Frame, RemovalRisk, RemovalScope, unresolved, values};
use tree_sitter::Node;

pub(super) fn update(
    node: Node<'_>,
    src: &[u8],
    scope: &RemovalScope<'_>,
    frame: &mut Frame,
    risks: &mut Vec<RemovalRisk>,
) {
    match node.kind() {
        "import_statement" | "import_from_statement" => imports(node, src, frame, risks),
        "assignment" | "assignment_expression" | "variable_declarator" | "named_expression" => {
            assignment(node, src, scope, frame, risks)
        }
        "augmented_assignment"
        | "augmented_assignment_expression"
        | "update_expression"
        | "delete_statement" => frame.invalidate(),
        _ => {}
    }
}

fn assignment(
    node: Node<'_>,
    src: &[u8],
    scope: &RemovalScope<'_>,
    frame: &mut Frame,
    risks: &mut Vec<RemovalRisk>,
) {
    let left = node
        .child_by_field_name("left")
        .or_else(|| node.child_by_field_name("name"));
    let right = node
        .child_by_field_name("right")
        .or_else(|| node.child_by_field_name("value"));
    let (Some(left), Some(right)) = (left, right) else {
        frame.invalidate();
        unresolved(risks, "unsupported inline binding mutation");
        return;
    };
    let alias = values::name(right, src, frame);
    if left.kind() == "object_pattern" {
        destructure(left, src, alias.as_deref(), frame);
        return;
    }
    if left.kind() != "identifier" {
        unresolved(risks, "unsupported inline binding mutation");
        frame.invalidate();
        return;
    }
    let key = left.utf8_text(src).unwrap_or_default().to_string();
    let value = values::value(right, src, scope, frame, 0);
    frame.values.insert(key.clone(), value);
    frame.aliases.remove(&key);
    if let Some(alias) = alias {
        frame.aliases.insert(key, alias);
    }
}

fn imports(node: Node<'_>, src: &[u8], frame: &mut Frame, risks: &mut Vec<RemovalRisk>) {
    if let Some(source) = node.child_by_field_name("source") {
        let module =
            values::string(source, src).and_then(|s| values::module(&s).map(str::to_string));
        if module.is_none() {
            unresolved(risks, "unresolved inline import");
        }
        let mut cursor = node.walk();
        for clause in node
            .named_children(&mut cursor)
            .filter(|n| n.kind() == "import_clause")
        {
            js_import(clause, src, module.as_deref(), frame);
        }
        return;
    }
    let module = node
        .child_by_field_name("module_name")
        .and_then(|n| n.utf8_text(src).ok());
    let mut cursor = node.walk();
    for import in node.children_by_field_name("name", &mut cursor) {
        let original = import
            .child_by_field_name("name")
            .unwrap_or(import)
            .utf8_text(src)
            .unwrap_or_default();
        let alias = import
            .child_by_field_name("alias")
            .and_then(|n| n.utf8_text(src).ok())
            .unwrap_or(original);
        if !matches!(module.unwrap_or(original), "os" | "shutil" | "pathlib") {
            unresolved(risks, "unresolved inline import");
        }
        let qualified = module.map_or_else(|| original.to_string(), |m| format!("{m}.{original}"));
        frame.values.insert(alias.into(), None);
        frame.aliases.insert(alias.into(), qualified);
    }
}

fn js_import(node: Node<'_>, src: &[u8], module: Option<&str>, frame: &mut Frame) {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        match child.kind() {
            "identifier" => bind_alias(child, src, module, frame),
            "import_specifier" => {
                let original = child.child_by_field_name("name");
                let alias = child.child_by_field_name("alias").or(original);
                if let (Some(original), Some(alias)) = (original, alias) {
                    let qualified = module
                        .map(|m| format!("{m}.{}", original.utf8_text(src).unwrap_or_default()));
                    bind_alias(alias, src, qualified.as_deref(), frame);
                }
            }
            _ => js_import(child, src, module, frame),
        }
    }
}

fn destructure(node: Node<'_>, src: &[u8], module: Option<&str>, frame: &mut Frame) {
    let mut cursor = node.walk();
    for item in node.named_children(&mut cursor) {
        let (property, alias) = if item.kind() == "pair_pattern" {
            (
                item.child_by_field_name("key"),
                item.child_by_field_name("value"),
            )
        } else {
            (Some(item), Some(item))
        };
        if let (Some(property), Some(alias)) = (property, alias) {
            let qualified =
                module.map(|m| format!("{m}.{}", property.utf8_text(src).unwrap_or_default()));
            bind_alias(alias, src, qualified.as_deref(), frame);
        }
    }
}

fn bind_alias(node: Node<'_>, src: &[u8], value: Option<&str>, frame: &mut Frame) {
    let key = node.utf8_text(src).unwrap_or_default().to_string();
    frame.values.insert(key.clone(), None);
    frame.aliases.remove(&key);
    if let Some(value) = value {
        frame.aliases.insert(key, value.into());
    }
}
