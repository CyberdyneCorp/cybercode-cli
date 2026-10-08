//! Event subject/path matching and dotted-field conditions.

use globset::{Glob, GlobMatcher};
use regex::Regex;
use serde_json::Value;

use super::HookDefinition;

#[derive(Debug, Clone)]
enum SubjectMatcher {
    Any,
    Glob(GlobMatcher),
    Regex(Regex),
}

#[derive(Debug, Clone)]
pub(super) struct Selector {
    subject: SubjectMatcher,
    paths: Vec<GlobMatcher>,
    condition: Option<(Vec<String>, Regex)>,
}

impl Selector {
    pub(super) fn new(
        matcher: Option<&str>,
        paths: &[String],
        handler: &crate::config::HookHandler,
    ) -> Result<Self, String> {
        let subject = match matcher {
            None | Some("*") => SubjectMatcher::Any,
            Some(pattern)
                if pattern.starts_with('/') && pattern.ends_with('/') && pattern.len() >= 2 =>
            {
                SubjectMatcher::Regex(
                    Regex::new(&pattern[1..pattern.len() - 1])
                        .map_err(|error| error.to_string())?,
                )
            }
            Some(pattern) => SubjectMatcher::Glob(glob(pattern)?),
        };
        let paths = paths
            .iter()
            .map(|pattern| glob(pattern))
            .collect::<Result<_, _>>()?;
        let condition = handler
            .condition
            .as_ref()
            .map(|condition| {
                Ok::<_, String>((
                    condition.field.split('.').map(str::to_string).collect(),
                    Regex::new(&condition.matches).map_err(|error| error.to_string())?,
                ))
            })
            .transpose()?;
        Ok(Self {
            subject,
            paths,
            condition,
        })
    }

    fn matches(&self, subject: &str, paths: &[String], payload: &Value) -> bool {
        let subject_matches = match &self.subject {
            SubjectMatcher::Any => true,
            SubjectMatcher::Glob(pattern) => pattern.is_match(subject),
            SubjectMatcher::Regex(pattern) => pattern.is_match(subject),
        };
        subject_matches && self.matches_paths(paths) && self.matches_condition(payload)
    }

    fn matches_paths(&self, paths: &[String]) -> bool {
        self.paths.is_empty()
            || paths
                .iter()
                .filter_map(|path| relative_path(path))
                .any(|path| self.paths.iter().any(|pattern| pattern.is_match(&path)))
    }

    fn matches_condition(&self, payload: &Value) -> bool {
        let Some((fields, pattern)) = &self.condition else {
            return true;
        };
        let field = fields
            .iter()
            .try_fold(payload, |value, field| value.get(field));
        match field {
            Some(Value::String(value)) => pattern.is_match(value),
            Some(value) => pattern.is_match(&crate::config::canonical_json(value)),
            None => false,
        }
    }
}

impl HookDefinition {
    /// Paths must be relative to the event Location. No matching executes a handler.
    pub fn matches(&self, event: &str, subject: &str, paths: &[String], payload: &Value) -> bool {
        self.event == event && self.selector.matches(subject, paths, payload)
    }
}

fn glob(pattern: &str) -> Result<GlobMatcher, String> {
    Glob::new(pattern)
        .map(|glob| glob.compile_matcher())
        .map_err(|error| error.to_string())
}

fn relative_path(path: &str) -> Option<String> {
    // Normalize both client spellings, and refuse absolute/escaping paths on every host.
    let path = path.replace('\\', "/");
    if path.starts_with('/')
        || path.as_bytes().get(1) == Some(&b':')
        || path.split('/').any(|part| part == "..")
    {
        return None;
    }
    let parts: Vec<_> = path
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .collect();
    if parts.is_empty() {
        return None;
    }
    Some(parts.join("/"))
}
