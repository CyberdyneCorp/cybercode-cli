//! The `/workflows` Library tab (`workflows` → Saved and bundled workflows): the Location's
//! Workflow library with origin badges. Enter opens a JSON args form prefilled from the
//! entry's `meta.args` defaults, checked against its schema before the run is posted for
//! the open Session; `o` shows the module source read-only. Tab switches back to Runs.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde_json::{Map, Value, json};

use crate::app::{Action, App, Overlay};
use crate::settings::Loadable;
use crate::workflows::{Refusal, Reply, Request, printable};

/// One library entry.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub name: String,
    pub description: String,
    /// `bundled`, `global` or `project`.
    pub origin: String,
    pub args: Option<Value>,
    pub estimate: Option<String>,
    pub shadowed: Vec<String>,
    pub source: Option<String>,
}

impl Entry {
    fn parse(value: &Value) -> Option<Self> {
        let text = |key: &str| value[key].as_str().map(printable);
        Some(Self {
            name: text("name")?,
            description: text("description").unwrap_or_default(),
            origin: value["origin"]["kind"].as_str().unwrap_or("saved").to_owned(),
            args: value.get("args").filter(|args| args.is_object()).cloned(),
            estimate: text("estimate"),
            shadowed: value["shadowed"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(printable)
                .collect(),
            source: value["source"].as_str().map(str::to_owned),
        })
    }

    /// The args every property's `default` gives, as one line of JSON.
    pub fn default_args(&self) -> String {
        let mut defaults = Map::new();
        let properties = self
            .args
            .as_ref()
            .and_then(|schema| schema["properties"].as_object());
        for (key, property) in properties.into_iter().flatten() {
            if let Some(default) = property.get("default") {
                defaults.insert(key.clone(), default.clone());
            }
        }
        Value::Object(defaults).to_string()
    }
}

/// The args form of one entry.
#[derive(Debug, Clone, PartialEq)]
pub struct Form {
    pub name: String,
    pub estimate: Option<String>,
    pub text: String,
    pub error: Option<String>,
    /// Posted; waiting for the server's answer.
    pub posting: bool,
}

/// An entry's source, read-only.
#[derive(Debug, Clone, PartialEq)]
pub struct SourceView {
    pub name: String,
    pub lines: Vec<String>,
    pub scroll: u16,
}

#[derive(Debug, Default)]
pub struct Library {
    pub entries: Loadable<Vec<Entry>>,
    /// Modules that failed to load.
    pub issues: usize,
    pub selected: usize,
    pub form: Option<Form>,
    pub source: Option<SourceView>,
}

impl Library {
    pub fn listed(&self) -> &[Entry] {
        self.entries.data.as_deref().unwrap_or_default()
    }

    fn current(&self) -> Option<&Entry> {
        self.listed().get(self.selected)
    }
}

/// `text` as run args: a JSON object that satisfies the entry's schema. The error names
/// the failing `args.<path>`.
pub fn checked_args(text: &str, schema: Option<&Value>) -> Result<Value, String> {
    let args: Value =
        serde_json::from_str(text).map_err(|error| format!("args are not JSON: {error}"))?;
    if !args.is_object() {
        return Err("args must be a JSON object".into());
    }
    let Some(schema) = schema else {
        return Ok(args);
    };
    let validator = jsonschema::validator_for(schema)
        .map_err(|error| format!("the Workflow's args schema is invalid: {error}"))?;
    let first = validator.iter_errors(&args).next().map(|error| {
        let path: String = error
            .instance_path()
            .to_string()
            .split('/')
            .skip(1)
            .map(|token| format!(".{token}"))
            .collect();
        format!("args{path}: {error}")
    });
    match first {
        None => Ok(args),
        Some(error) => Err(error),
    }
}

impl App {
    /// Tab: switch between the Runs and the Library tab; the Library loads when opened.
    pub(crate) fn toggle_library(&mut self) -> Vec<Action> {
        let w = &mut self.workflows;
        w.library_tab = !w.library_tab;
        if !w.library_tab {
            return w.reload();
        }
        let generation = w.library.entries.start();
        vec![Action::Workflow(Request::Library {
            generation,
            directory: w.directory.clone(),
        })]
    }

    pub(crate) fn library_key(&mut self, key: KeyEvent) -> Vec<Action> {
        let library = &mut self.workflows.library;
        if library.source.is_some() {
            source_key(library, key);
            return Vec::new();
        }
        if library.form.is_some() {
            return self.form_key(key);
        }
        match key.code {
            KeyCode::Tab => self.toggle_library(),
            KeyCode::Esc => {
                self.overlay = Overlay::None;
                Vec::new()
            }
            KeyCode::Enter => {
                self.open_form();
                Vec::new()
            }
            KeyCode::Char('o') => {
                self.open_source();
                Vec::new()
            }
            code => {
                let last = library.listed().len().saturating_sub(1);
                library.selected = crate::workflows::step(library.selected, last, code);
                Vec::new()
            }
        }
    }

    fn open_form(&mut self) {
        let library = &mut self.workflows.library;
        if let Some(entry) = library.current() {
            library.form = Some(Form {
                name: entry.name.clone(),
                estimate: entry.estimate.clone(),
                text: entry.default_args(),
                error: None,
                posting: false,
            });
        }
    }

    fn open_source(&mut self) {
        let library = &mut self.workflows.library;
        if let Some(entry) = library.current() {
            let lines = entry
                .source
                .as_deref()
                .unwrap_or("(the server did not send the source)")
                .lines()
                .map(printable)
                .collect();
            library.source = Some(SourceView {
                name: entry.name.clone(),
                lines,
                scroll: 0,
            });
        }
    }

    fn form_key(&mut self, key: KeyEvent) -> Vec<Action> {
        let w = &mut self.workflows;
        let Some(form) = w.library.form.as_mut() else {
            return Vec::new();
        };
        if form.posting {
            return Vec::new();
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => w.library.form = None,
            KeyCode::Char('u') if ctrl => form.text.clear(),
            KeyCode::Char(c) if !ctrl => form.text.push(c),
            KeyCode::Backspace => {
                form.text.pop();
            }
            KeyCode::Enter => return self.submit_form(),
            _ => {}
        }
        Vec::new()
    }

    /// Check the form's args and start the run, or show why not.
    fn submit_form(&mut self) -> Vec<Action> {
        let w = &mut self.workflows;
        let Some(form) = w.library.form.as_ref() else {
            return Vec::new();
        };
        let schema = w
            .library
            .listed()
            .iter()
            .find(|entry| entry.name == form.name)
            .and_then(|entry| entry.args.as_ref());
        let checked = checked_args(&form.text, schema);
        let name = form.name.clone();
        let directory = w.directory.clone();
        let Some(form) = w.library.form.as_mut() else {
            return Vec::new();
        };
        match checked {
            Ok(args) => {
                form.error = None;
                form.posting = true;
                vec![Action::Workflow(Request::StartRun {
                    directory,
                    session: self.session.id.clone(),
                    name,
                    args,
                })]
            }
            Err(error) => {
                form.error = Some(error);
                Vec::new()
            }
        }
    }

    /// The library was read.
    pub(crate) fn library_listed(&mut self, generation: u64, result: Result<(Vec<Entry>, usize), String>) {
        let library = &mut self.workflows.library;
        let result = result.map(|(entries, issues)| {
            library.issues = issues;
            entries
        });
        library.entries.apply(generation, result);
        library.selected = library.selected.min(library.listed().len().saturating_sub(1));
    }

    /// A run started from the form: back to Runs, which lists it; a refusal stays on
    /// the form.
    pub(crate) fn library_started(&mut self, name: &str, result: Result<Value, Refusal>) -> Vec<Action> {
        let w = &mut self.workflows;
        let Some(form) = w.library.form.as_mut().filter(|form| form.name == name) else {
            return Vec::new();
        };
        form.posting = false;
        match result {
            Ok(run) => {
                w.library.form = None;
                w.library_tab = false;
                let id = run["id"].as_str().unwrap_or_default();
                self.toast(format!("Started {name} ({})", printable(id)));
                self.workflows.reload()
            }
            Err(refusal) => {
                form.error = Some(printable(&refusal.message));
                Vec::new()
            }
        }
    }
}

fn source_key(library: &mut Library, key: KeyEvent) {
    let Some(view) = library.source.as_mut() else {
        return;
    };
    match key.code {
        KeyCode::Up => view.scroll = view.scroll.saturating_sub(1),
        KeyCode::Down => view.scroll = view.scroll.saturating_add(1),
        KeyCode::PageUp => view.scroll = view.scroll.saturating_sub(10),
        KeyCode::PageDown => view.scroll = view.scroll.saturating_add(10),
        KeyCode::Esc | KeyCode::Char('q') => library.source = None,
        _ => {}
    }
}

/// `GET /workflows?source=true` for `directory`: the entries and the number of issues.
pub(crate) async fn list(client: &cyber_client::Client, generation: u64, directory: &str) -> Reply {
    let result = client
        .at(directory)
        .get("/workflows?source=true")
        .await
        .map_err(crate::perform::err)
        .map(|library| {
            let entries = library["workflows"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Entry::parse)
                .collect();
            let issues = library["issues"].as_array().map_or(0, Vec::len);
            (entries, issues)
        });
    Reply::Library { generation, result }
}

/// Start `name` with `args` for `session`.
pub(crate) async fn start(
    client: &cyber_client::Client,
    directory: &str,
    session: &str,
    name: String,
    args: Value,
) -> Reply {
    let body = json!({"session_id": session, "name": name, "args": args});
    let result = client
        .at(directory)
        .post("/workflows/runs", body)
        .await
        .map(|run| run["data"].clone())
        .map_err(crate::workflows::refusal);
    Reply::Started { name, result }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry() -> Entry {
        Entry::parse(&json!({
            "name": "review", "description": "Multi-angle review",
            "origin": {"kind": "bundled", "name": "review"},
            "args": {"type": "object", "properties": {
                "lenses": {"type": "array", "items": {"enum": ["correctness", "tests"]}, "default": ["correctness"]},
                "max_findings": {"type": "integer", "maximum": 60, "default": 30},
                "target": {"type": "string"}
            }, "additionalProperties": false},
            "estimate": "lenses × groups", "shadowed": [], "source": "export const meta = {};"
        }))
        .unwrap()
    }

    #[test]
    fn the_form_is_prefilled_from_defaults_and_checked_against_the_schema() {
        let entry = entry();
        assert_eq!(entry.origin, "bundled");
        assert_eq!(
            entry.default_args(),
            r#"{"lenses":["correctness"],"max_findings":30}"#
        );
        let schema = entry.args.as_ref();
        assert_eq!(
            checked_args(r#"{"max_findings": 5}"#, schema).unwrap(),
            json!({"max_findings": 5})
        );
        let too_many = checked_args(r#"{"max_findings": 99}"#, schema).unwrap_err();
        assert!(too_many.starts_with("args.max_findings: "), "{too_many}");
        let lens = checked_args(r#"{"lenses": ["style"]}"#, schema).unwrap_err();
        assert!(lens.starts_with("args.lenses.0: "), "{lens}");
        assert!(checked_args("[1]", schema).unwrap_err().contains("object"));
        assert!(checked_args("{", schema).unwrap_err().starts_with("args are not JSON"));
        assert!(checked_args(r#"{"x": 1}"#, None).is_ok());
    }
}
