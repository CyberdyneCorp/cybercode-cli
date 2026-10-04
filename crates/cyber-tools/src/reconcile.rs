//! Read-only reconciliation of file mutations whose outcome became unknown
//! (`tool-registry` → Tool recovery contract). Shell commands always stay unknown.

use std::path::{Path, PathBuf};

use cyber_server::runtime::{CallState, Reconciliation};
use serde_json::Value;

use crate::tools::patch::{self, Op};

#[derive(Debug, Clone, Copy, PartialEq)]
enum State {
    Applied,
    NotApplied,
    Unknown,
}

pub(crate) fn reconcile(location: &Path, home: &Path, call: &CallState) -> Reconciliation {
    let Some(input) = &call.input else {
        return Reconciliation::Unknown;
    };
    let resolve = |p: &str| crate::host::resolve_path(location, home, p);
    let str_of = |k: &str| input.get(k).and_then(Value::as_str).unwrap_or_default();
    let (state, evidence) = match call.name.as_str() {
        "write" => {
            let path = resolve(str_of("path"));
            (
                write(&path, str_of("content")),
                format!("{} content", path.display()),
            )
        }
        "edit" => {
            let path = resolve(str_of("path"));
            (
                edit(&path, str_of("old_string"), str_of("new_string")),
                format!("{} content", path.display()),
            )
        }
        "apply_patch" => match patch::parse(str_of("patch")) {
            Ok(ops) => apply_patch(&ops, &resolve),
            Err(_) => (State::Unknown, String::new()),
        },
        _ => (State::Unknown, String::new()),
    };
    match state {
        State::Applied => Reconciliation::Succeeded {
            evidence: format!("{evidence} already reflects the change"),
        },
        State::NotApplied => Reconciliation::NotApplied {
            evidence: format!("{evidence} does not reflect the change"),
        },
        State::Unknown => Reconciliation::Unknown,
    }
}

/// File text with a BOM stripped and CRLF normalized, as the edit tools compare it.
fn text(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    let raw = String::from_utf8(bytes).ok()?;
    Some(
        raw.strip_prefix('\u{feff}')
            .unwrap_or(&raw)
            .replace("\r\n", "\n"),
    )
}

fn write(path: &Path, content: &str) -> State {
    match text(path) {
        Some(current) if current == content.replace("\r\n", "\n") => State::Applied,
        // The pre-image is not recorded, so a different file proves nothing either way.
        _ => State::Unknown,
    }
}

fn edit(path: &Path, old: &str, new: &str) -> State {
    let Some(current) = text(path) else {
        return State::Unknown;
    };
    match (current.contains(old), current.contains(new)) {
        (false, true) => State::Applied,
        (true, false) => State::NotApplied,
        _ => State::Unknown,
    }
}

fn apply_patch(ops: &[Op], resolve: &dyn Fn(&str) -> PathBuf) -> (State, String) {
    let states: Vec<State> = ops.iter().map(|op| op_state(op, resolve)).collect();
    let all = |s: State| states.iter().all(|x| *x == s);
    let state = if all(State::Applied) {
        State::Applied
    } else if all(State::NotApplied) {
        State::NotApplied
    } else {
        State::Unknown
    };
    (state, format!("every file in the patch ({})", ops.len()))
}

fn op_state(op: &Op, resolve: &dyn Fn(&str) -> PathBuf) -> State {
    match op {
        Op::Add { path, content } => match text(&resolve(path)) {
            Some(current) if current == *content => State::Applied,
            Some(_) => State::Unknown,
            None => State::NotApplied,
        },
        Op::Delete { path } => {
            if resolve(path).exists() {
                State::NotApplied
            } else {
                State::Applied
            }
        }
        Op::Update {
            path,
            move_to,
            chunks,
        } => {
            let source = resolve(path);
            let target = move_to.as_deref().map_or_else(|| source.clone(), resolve);
            let moved = target != source && !source.exists();
            let current = text(if moved { &target } else { &source });
            let Some(current) = current else {
                return State::Unknown;
            };
            let lines: Vec<String> = current.lines().map(str::to_string).collect();
            let new_present = chunks
                .iter()
                .all(|c| c.new.is_empty() || patch::find(&lines, &c.new, 0).is_some());
            let old_applies = !moved && patch::apply_chunks(&current, chunks, path).is_ok();
            match (new_present, old_applies) {
                (true, false) => State::Applied,
                (false, true) => State::NotApplied,
                _ => State::Unknown,
            }
        }
    }
}
