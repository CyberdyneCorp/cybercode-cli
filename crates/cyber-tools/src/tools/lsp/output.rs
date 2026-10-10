use super::{Input, Operation, ToolError, failed, source};
use crate::lsp::{DiagnosticPosition, DiagnosticRange, LspError};
use serde_json::{Value, json};
use std::{
    collections::{BTreeSet, VecDeque},
    path::{Path, PathBuf},
};

const LIMIT: usize = 50;

fn range(value: &Value) -> Option<DiagnosticRange> {
    let range: DiagnosticRange = serde_json::from_value(value.clone()).ok()?;
    let valid =
        |p: &DiagnosticPosition| p.line <= i32::MAX as u32 && p.character <= i32::MAX as u32;
    (valid(&range.start) && valid(&range.end) && range.start <= range.end).then_some(range)
}

fn location_range(value: &Value) -> Option<DiagnosticRange> {
    if value.get("targetUri").is_some() {
        let outer = range(value.get("targetRange")?)?;
        let selected = range(value.get("targetSelectionRange")?)?;
        return (outer.start <= selected.start && selected.end <= outer.end).then_some(selected);
    }
    range(value.get("range")?)
}

fn file_uri(uri: &str) -> Option<PathBuf> {
    let url = reqwest::Url::parse(uri).ok()?;
    if url.scheme() != "file"
        || url.query().is_some()
        || url.fragment().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.host_str().is_some_and(|host| host != "localhost")
    {
        return None;
    }
    url.to_file_path().ok()
}

fn scoped(path: &Path, root: &Path, missing: bool, directory: bool) -> Option<PathBuf> {
    let canonical = match path.canonicalize() {
        Ok(path) if path.is_file() || (directory && path.is_dir()) => path,
        Err(error) if missing && error.kind() == std::io::ErrorKind::NotFound => {
            path.parent()?.canonicalize().ok()?.join(path.file_name()?)
        }
        _ => return None,
    };
    canonical.starts_with(root).then_some(canonical)
}

fn relative(path: &Path, root: &Path) -> String {
    let text = path
        .strip_prefix(root)
        .unwrap()
        .components()
        .map(|part| part.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/");
    if text.is_empty() { ".".into() } else { text }
}

struct Results {
    root: PathBuf,
    rows: Vec<Value>,
    seen: BTreeSet<String>,
    omitted: usize,
    server: Option<Value>,
}

impl Results {
    fn push(
        &mut self,
        mut row: Value,
        path: PathBuf,
        position: Option<DiagnosticPosition>,
        missing: bool,
    ) {
        let Some(path) = scoped(&path, &self.root, missing, missing && position.is_none()) else {
            self.omitted += 1;
            return;
        };
        row["path"] = json!(relative(&path, &self.root));
        if let Some(position) = position {
            row["line"] = json!(u64::from(position.line) + 1);
            row["character"] = json!(u64::from(position.character) + 1);
            row["location"] = json!(format!(
                "{}:{}:{}",
                row["path"].as_str().unwrap(),
                u64::from(position.line) + 1,
                u64::from(position.character) + 1
            ));
        }
        if let Some(server) = &self.server {
            row["server"] = server["id"].clone();
            row["server_root"] = server["root"].clone();
        }
        let key = row.to_string();
        if !self.seen.insert(key) {
            return;
        }
        if self.rows.len() == LIMIT {
            self.omitted += 1;
        } else {
            self.rows.push(row);
        }
    }
    fn location(&mut self, value: &Value) {
        let uri = value
            .get("targetUri")
            .or_else(|| value.get("uri"))
            .and_then(Value::as_str);
        let range = location_range(value);
        if let (Some(path), Some(range)) = (uri.and_then(file_uri), range) {
            self.push(json!({}), path, Some(range.start), false);
        } else {
            self.omitted += 1;
        }
    }
    fn locations(&mut self, value: &Value) {
        match value {
            Value::Null => {}
            Value::Array(values) => {
                for value in values {
                    self.location(value);
                }
            }
            _ => self.location(value),
        }
    }
    fn symbols(&mut self, value: &Value, file: Option<&Path>) {
        let Some(values) = value.as_array() else {
            return;
        };
        let mut queue: VecDeque<_> = values.iter().collect();
        while let Some(value) = queue.pop_front() {
            if let Some(children) = value.get("children").and_then(Value::as_array) {
                queue.extend(children);
            }
            let Some(name) = value.get("name").and_then(Value::as_str) else {
                self.omitted += 1;
                continue;
            };
            let kind = value
                .get("kind")
                .and_then(Value::as_u64)
                .filter(|kind| (1..=26).contains(kind));
            let row = json!({"name":name,"kind":kind});
            if let Some(location) = value.get("location") {
                let Some(path) = location
                    .get("uri")
                    .and_then(Value::as_str)
                    .and_then(file_uri)
                else {
                    self.omitted += 1;
                    continue;
                };
                let position = location
                    .get("range")
                    .and_then(range)
                    .map(|range| range.start);
                if location.get("range").is_some() && position.is_none() {
                    self.omitted += 1;
                    continue;
                }
                self.push(row, path, position, false);
            } else if let (Some(file), Some(range)) = (
                file,
                value
                    .get("selectionRange")
                    .or_else(|| value.get("range"))
                    .and_then(range),
            ) {
                self.push(row, file.into(), Some(range.start), false);
            } else {
                self.omitted += 1;
            }
        }
    }
    fn edits(&mut self, path: PathBuf, value: &Value, version: Option<&Value>) {
        let Some(edits) = value.as_array() else {
            return;
        };
        for edit in edits {
            let Some(range) = edit.get("range").and_then(range) else {
                self.omitted += 1;
                continue;
            };
            let Some(text) = edit.get("newText").and_then(Value::as_str) else {
                self.omitted += 1;
                continue;
            };
            self.push(json!({"new_text":text,"document_version":version,"end_line":u64::from(range.end.line)+1,"end_character":u64::from(range.end.character)+1}),path.clone(),Some(range.start),true);
        }
    }
    fn rename(&mut self, value: &Value) {
        if let Some(changes) = value.get("documentChanges") {
            match changes.as_array() {
                Some(changes) => {
                    for change in changes {
                        self.document_change(change);
                    }
                }
                None => self.omitted += 1,
            }
            return;
        }
        if let Some(changes) = value.get("changes").and_then(Value::as_object) {
            for (uri, edits) in changes {
                if let Some(path) = file_uri(uri) {
                    self.edits(path, edits, None);
                } else {
                    self.omitted += 1;
                }
            }
        }
    }
    fn document_change(&mut self, value: &Value) {
        if let Some(uri) = value.pointer("/textDocument/uri").and_then(Value::as_str) {
            if let Some(path) = file_uri(uri) {
                self.edits(
                    path,
                    &value["edits"],
                    value.pointer("/textDocument/version"),
                );
            } else {
                self.omitted += 1;
            }
            return;
        }
        let Some(kind) = value.get("kind").and_then(Value::as_str) else {
            self.omitted += 1;
            return;
        };
        let uri = if kind == "rename" { "oldUri" } else { "uri" };
        let Some(path) = value.get(uri).and_then(Value::as_str).and_then(file_uri) else {
            self.omitted += 1;
            return;
        };
        let mut row = json!({"proposed_operation":kind});
        if kind == "rename" {
            let Some(new_path) = value
                .get("newUri")
                .and_then(Value::as_str)
                .and_then(file_uri)
                .and_then(|path| scoped(&path, &self.root, true, true))
            else {
                self.omitted += 1;
                return;
            };
            row["new_path"] = json!(relative(&new_path, &self.root));
        } else if !matches!(kind, "create" | "delete") {
            self.omitted += 1;
            return;
        }
        self.push(row, path, None, true);
    }
    fn diagnostics(&mut self, value: &Value) {
        let Some(snapshots) = value.as_array() else {
            return;
        };
        for snapshot in snapshots {
            let Some(path) = snapshot.get("path").and_then(Value::as_str) else {
                continue;
            };
            let Some(diagnostics) = snapshot.get("diagnostics").and_then(Value::as_array) else {
                continue;
            };
            for diagnostic in diagnostics {
                if let Some(range) = diagnostic.get("range").and_then(range) {
                    self.push(json!({"message":diagnostic["message"],"severity":diagnostic["severity"],"server_version":snapshot["version"],"observed_document_version":snapshot["document_version"]}),PathBuf::from(path),Some(range.start),false);
                }
            }
        }
    }
    fn hover(&mut self, value: &Value, file: Option<&Path>, input: &Input) {
        let Some(file) = file else {
            return;
        };
        let Some(contents) = value.get("contents") else {
            return;
        };
        let text = hover_text(contents);
        if text.is_empty() {
            return;
        }
        let position = match value.get("range") {
            Some(value) => {
                let Some(range) = range(value) else {
                    self.omitted += 1;
                    return;
                };
                Some(range.start)
            }
            None => Some(DiagnosticPosition {
                line: (input.line.unwrap() - 1) as u32,
                character: (input.character.unwrap() - 1) as u32,
            }),
        };
        self.push(json!({"text":text}), file.into(), position, false);
    }
}

fn hover_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Array(values) => values.iter().map(hover_text).collect::<Vec<_>>().join("\n"),
        Value::Object(object) => object
            .get("value")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .into(),
        _ => String::new(),
    }
}

pub(super) async fn render(
    root: &Path,
    input: &Input,
    responses: Vec<(crate::lsp::ServerStatus, Result<Value, LspError>)>,
) -> Result<String, ToolError> {
    let file = input.path.as_ref().map(|path| root.join(path));
    let mut results = Results {
        root: root.into(),
        rows: vec![],
        seen: BTreeSet::new(),
        omitted: 0,
        server: None,
    };
    let mut successes = 0;
    let mut failures = 0;
    for (server, response) in responses {
        results.server = (input.operation == Operation::RenamePreview).then(|| json!({"id":server.id,"root":server.root.strip_prefix(root).map(|_|relative(&server.root, root)).unwrap_or_else(|_|"<outside Location>".into())}));
        let value = match response {
            Ok(value) => {
                successes += 1;
                value
            }
            Err(_) => {
                failures += 1;
                continue;
            }
        };
        match input.operation {
            Operation::Definition | Operation::References | Operation::Implementation => {
                results.locations(&value)
            }
            Operation::DocumentSymbols | Operation::WorkspaceSymbols => {
                results.symbols(&value, file.as_deref())
            }
            Operation::RenamePreview => results.rename(&value),
            Operation::Diagnostics => results.diagnostics(&value),
            Operation::Hover => results.hover(&value, file.as_deref(), input),
        }
    }
    if successes == 0 {
        return Err(failed(
            "All matching language servers failed or do not support this operation",
        ));
    }
    for row in &mut results.rows {
        if let (Some(path), Some(line)) = (row["path"].as_str(), row["line"].as_u64()) {
            row["preview"] = json!(preview(&root.join(path), line).await);
        }
    }
    serde_json::to_string_pretty(&json!({"operation":input.operation,"results":results.rows,"omitted":results.omitted,"servers_failed":failures})).map_err(|_|failed("LSP output serialization failed"))
}

async fn preview(path: &Path, line: u64) -> String {
    match source(path).await {
        Ok(text) => text
            .lines()
            .nth((line - 1) as usize)
            .map(|line| line.chars().take(500).collect())
            .unwrap_or_else(|| "<line unavailable>".into()),
        Err(_) => "<source preview unavailable>".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_symbols_validate_ranges_and_project_only_numeric_kinds() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let file = root.join("source.txt");
        std::fs::write(&file, "source").unwrap();
        let uri = reqwest::Url::from_file_path(file).unwrap().to_string();
        let mut results = Results {
            root,
            rows: Vec::new(),
            seen: BTreeSet::new(),
            omitted: 0,
            server: None,
        };
        results.symbols(&json!([
            {"name":"valid","kind":12,"location":{"uri":uri}},
            {"name":"opaque","kind":{"secret":"withheld"},"location":{"uri":uri}},
            {"name":"unknown","kind":27,"location":{"uri":uri}},
            {"name":"invalid","kind":12,"location":{"uri":uri,"range":{"start":{"line":2,"character":0},"end":{"line":1,"character":0}}}}
        ]), None);
        assert_eq!(results.omitted, 1);
        assert_eq!(results.rows.len(), 3);
        assert_eq!(results.rows[0]["kind"], 12);
        assert!(results.rows[1]["kind"].is_null());
        assert!(results.rows[2]["kind"].is_null());
        assert!(results.rows.iter().all(|row| row.get("line").is_none()));
        assert!(!json!(results.rows).to_string().contains("withheld"));
    }

    #[tokio::test]
    async fn portable_definition_output_matches_the_tool_golden() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().canonicalize().unwrap();
        let path = directory.join("target.txt");
        std::fs::write(&path, "target\n").unwrap();
        let input = Input::parse(
            &json!({"operation":"definition","path":"source.txt","line":1,"character":2}),
        )
        .unwrap();
        let response = json!([{"uri":reqwest::Url::from_file_path(path).unwrap().as_str(),"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":3}}}]);
        let server = crate::lsp::ServerStatus {
            id: "fixture".into(),
            root: directory.clone(),
            status: crate::lsp::ServerState::Connected,
        };
        let output = render(&directory, &input, vec![(server, Ok(response))])
            .await
            .unwrap();
        assert_eq!(
            output.trim(),
            include_str!("../../../tests/goldens/lsp.txt").trim()
        );
    }
    #[tokio::test]
    async fn invalid_remote_external_and_malformed_locations_are_omitted() {
        let tmp = tempfile::tempdir().unwrap();
        let directory = tmp.path().join("root");
        std::fs::create_dir(&directory).unwrap();
        let directory = directory.canonicalize().unwrap();
        let file = directory.join("file.txt");
        std::fs::write(&file, "valid source").unwrap();
        let outside = tmp.path().join("outside.txt");
        std::fs::write(&outside, "private external content").unwrap();
        let uri = reqwest::Url::from_file_path(&file).unwrap().to_string();
        let valid_range = json!({"start":{"line":0,"character":0},"end":{"line":0,"character":1}});
        let response = json!([
            {"uri":uri,"range":valid_range},
            {"uri":"https://example.invalid/file","range":valid_range},
            {"uri":reqwest::Url::from_file_path(&outside).unwrap().as_str(),"range":valid_range},
            {"uri":format!("{uri}?secret"),"range":valid_range},
            {"uri":uri,"range":{"start":{"line":1,"character":0},"end":{"line":0,"character":0}}},
            {"uri":uri,"range":{"start":{"line":2147483648_u64,"character":0},"end":{"line":2147483648_u64,"character":1}}},
            {"targetUri":uri,"targetRange":valid_range,"targetSelectionRange":{"start":{"line":1,"character":0},"end":{"line":1,"character":1}}},
            {"targetUri":uri,"targetSelectionRange":valid_range}
        ]);
        let input = Input::parse(
            &json!({"operation":"definition","path":"file.txt","line":1,"character":1}),
        )
        .unwrap();
        let server = crate::lsp::ServerStatus {
            id: "fixture".into(),
            root: directory.clone(),
            status: crate::lsp::ServerState::Connected,
        };
        let output = render(&directory, &input, vec![(server, Ok(response))])
            .await
            .unwrap();
        assert!(!output.contains("private external content"));
        let result: Value = serde_json::from_str(&output).unwrap();
        assert_eq!(result["results"].as_array().unwrap().len(), 1);
        assert_eq!(result["omitted"], 7);
    }
}
