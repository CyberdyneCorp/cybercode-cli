//! Output of the source adapters that sit beside the settings converters: hooks, Markdown
//! agents, Claude model/env settings and Codex profiles. Each returns a native fragment,
//! the sources of its entries and the items it left behind, and [`absorb`] folds it into
//! a family's proposal.
use super::preview::MappingRecord;
use super::provenance::{self, Origins, SourceReference};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// A native fragment with the sources of each of its entries.
#[derive(Debug, Default)]
pub(super) struct Adapted {
    pub config: Value,
    /// Native entry pointer → the source fields it was built from. Raw-document adapters
    /// name raw pointers (see [`Adapted::raw`]); file adapters name the file itself.
    pub fields: Vec<(String, Vec<SourceReference>)>,
    pub pending: Vec<Pending>,
}

/// A source item an adapter did not convert, with the report status it belongs under.
#[derive(Debug)]
pub(super) struct Pending {
    pub field: String,
    /// `None`: the raw pointer `field` names its own source.
    pub source: Option<PathBuf>,
    pub status: &'static str,
    pub reason: &'static str,
}

impl Adapted {
    pub fn new() -> Self {
        Self {
            config: json!({}),
            ..Self::default()
        }
    }

    /// Record a native entry built from raw-document pointers.
    pub fn raw(&mut self, native: String, raw: &[&str]) {
        let references = raw
            .iter()
            .map(|field| SourceReference {
                source: PathBuf::new(),
                field: (*field).to_owned(),
            })
            .collect();
        self.fields.push((native, references));
    }

    /// A raw-document item left behind.
    pub fn skip(&mut self, field: impl Into<String>, status: &'static str, reason: &'static str) {
        self.pending.push(Pending {
            field: field.into(),
            source: None,
            status,
            reason,
        });
    }

    /// Where the entries and pending items recorded so far end.
    pub fn mark(&self) -> (usize, usize) {
        (self.fields.len(), self.pending.len())
    }

    /// Attribute every raw-pointer entry and item recorded since `mark` to `file`: for
    /// adapters over one file's document rather than the merged settings.
    pub fn attribute(&mut self, mark: (usize, usize), file: &Path) {
        for (_, references) in &mut self.fields[mark.0..] {
            for reference in references
                .iter_mut()
                .filter(|r| r.source.as_os_str().is_empty())
            {
                reference.source = file.into();
            }
        }
        for item in &mut self.pending[mark.1..] {
            item.source.get_or_insert_with(|| file.into());
        }
    }

    /// A file-level item left behind.
    pub fn skip_file(
        &mut self,
        file: &Path,
        field: &str,
        status: &'static str,
        reason: &'static str,
    ) {
        self.pending.push(Pending {
            field: field.into(),
            source: Some(file.into()),
            status,
            reason,
        });
    }
}

/// Fold `adapted` into `config`: `permissions.rules` entries append, every other
/// second-level entry (an agent, a hook event, a profile) replaces the earlier one whole.
/// Raw-pointer sources resolve through `raw_origins`; items left behind become report
/// records attributed to their source, or to `fallback`.
pub(super) fn absorb(
    config: &mut Value,
    origins: &mut Origins,
    report: &mut Vec<MappingRecord>,
    adapted: Adapted,
    raw_origins: &Origins,
    fallback: &Path,
) {
    let Adapted {
        config: fragment,
        fields,
        pending,
    } = adapted;
    let offset = append_rules(config, &fragment);
    place_entries(config, origins, fragment);
    for (pointer, references) in fields {
        let pointer = shift_rule(&pointer, offset);
        let resolved = resolve(&references, raw_origins, fallback);
        origins.insert(pointer, resolved);
    }
    for item in pending {
        let source = item.source.clone().unwrap_or_else(|| {
            provenance::references(raw_origins, &item.field)
                .first()
                .map_or_else(|| fallback.to_path_buf(), |r| r.source.clone())
        });
        report.push(MappingRecord {
            sources: vec![SourceReference {
                source: source.clone(),
                field: item.field.clone(),
            }],
            source,
            field: item.field,
            status: item.status,
            reason: item.reason,
            comparison: None,
        });
    }
}

/// Append fragment rules after the existing ones; returns where they start.
fn append_rules(config: &mut Value, fragment: &Value) -> usize {
    let Some(rules) = fragment
        .pointer("/permissions/rules")
        .and_then(Value::as_array)
    else {
        return 0;
    };
    if !config.get("permissions").is_some_and(Value::is_object) {
        config["permissions"] = json!({});
    }
    let existing = config["permissions"]
        .as_object_mut()
        .unwrap()
        .entry("rules")
        .or_insert_with(|| json!([]));
    if !existing.is_array() {
        *existing = json!([]);
    }
    let list = existing.as_array_mut().unwrap();
    let offset = list.len();
    list.extend(rules.iter().cloned());
    offset
}

fn place_entries(config: &mut Value, origins: &mut Origins, fragment: Value) {
    let Value::Object(fragment) = fragment else {
        return;
    };
    for (key, value) in fragment {
        if key == "permissions" {
            continue;
        }
        let top = provenance::child("", &key);
        match (config.get_mut(&key), value) {
            (Some(Value::Object(existing)), Value::Object(entries)) => {
                for (name, entry) in entries {
                    forget(origins, &provenance::child(&top, &name));
                    existing.insert(name, entry);
                }
            }
            (_, value) => {
                forget(origins, &top);
                config[&key] = value;
            }
        }
    }
}

fn forget(origins: &mut Origins, pointer: &str) {
    let prefix = format!("{pointer}/");
    origins.retain(|key, _| key != pointer && !key.starts_with(&prefix));
}

fn shift_rule(pointer: &str, offset: usize) -> String {
    let Some(rest) = pointer.strip_prefix("/permissions/rules/") else {
        return pointer.to_owned();
    };
    let (index, tail) = rest.split_once('/').unwrap_or((rest, ""));
    match index.parse::<usize>() {
        Ok(index) if tail.is_empty() => format!("/permissions/rules/{}", index + offset),
        Ok(index) => format!("/permissions/rules/{}/{tail}", index + offset),
        Err(_) => pointer.to_owned(),
    }
}

fn resolve(
    references: &[SourceReference],
    raw_origins: &Origins,
    fallback: &Path,
) -> Vec<SourceReference> {
    let mut out = Vec::new();
    for reference in references {
        if !reference.source.as_os_str().is_empty() {
            out.push(reference.clone());
            continue;
        }
        let found = provenance::references(raw_origins, &reference.field);
        if found.is_empty() {
            out.push(SourceReference {
                source: fallback.to_path_buf(),
                field: reference.field.clone(),
            });
        }
        out.extend(found);
    }
    out.sort();
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_replace_whole_and_rules_append_with_shifted_sources() {
        let mut config = json!({
            "agents":{"a":{"description":"inline","steps":3}},
            "permissions":{"rules":[{"action":"bash","resource":"*","effect":"allow"}]}
        });
        let mut origins = Origins::new();
        origins.insert("/agents/a/steps".into(), vec![]);
        let mut adapted = Adapted::new();
        adapted.config = json!({
            "agents":{"a":{"description":"markdown"}},
            "permissions":{"rules":[{"action":"skill","resource":"*","effect":"deny"}]},
            "fallback_models":["x/y"]
        });
        adapted.raw("/permissions/rules/0".into(), &["/approval_policy"]);
        adapted.fields.push((
            "/agents/a".into(),
            vec![SourceReference {
                source: "a.md".into(),
                field: "body".into(),
            }],
        ));
        adapted.skip("/apiKeyHelper", "not imported", "reason");
        let mut report = Vec::new();
        absorb(
            &mut config,
            &mut origins,
            &mut report,
            adapted,
            &Origins::new(),
            Path::new("settings.json"),
        );
        assert_eq!(config["agents"]["a"], json!({"description":"markdown"}));
        assert_eq!(config["permissions"]["rules"][1]["action"], "skill");
        assert_eq!(config["fallback_models"], json!(["x/y"]));
        assert!(!origins.contains_key("/agents/a/steps"));
        assert_eq!(
            origins["/permissions/rules/1"][0].source,
            PathBuf::from("settings.json")
        );
        assert_eq!(origins["/agents/a"][0].source, PathBuf::from("a.md"));
        assert_eq!(report[0].field, "/apiKeyHelper");
        assert_eq!(report[0].source, PathBuf::from("settings.json"));
    }
}
