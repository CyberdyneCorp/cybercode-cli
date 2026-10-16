//! Typed Workflow diagnostics (`workflows` → Diagnostics).

use std::fmt;

use serde::{Deserialize, Serialize};

/// The single kind every Workflow failure is reported with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
pub enum DiagnosticKind {
    ParseError,
    UnsupportedSyntax,
    ExecutionFailure,
    AgentFailed,
    SchemaMismatch,
    BudgetExceeded,
    ModelUnavailable,
    NoInteractiveUser,
    Timeout,
    PermissionDenied,
    WorkflowReplayMismatch,
}

impl DiagnosticKind {
    pub const ALL: [DiagnosticKind; 11] = [
        Self::ParseError,
        Self::UnsupportedSyntax,
        Self::ExecutionFailure,
        Self::AgentFailed,
        Self::SchemaMismatch,
        Self::BudgetExceeded,
        Self::ModelUnavailable,
        Self::NoInteractiveUser,
        Self::Timeout,
        Self::PermissionDenied,
        Self::WorkflowReplayMismatch,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::ParseError => "ParseError",
            Self::UnsupportedSyntax => "UnsupportedSyntax",
            Self::ExecutionFailure => "ExecutionFailure",
            Self::AgentFailed => "AgentFailed",
            Self::SchemaMismatch => "SchemaMismatch",
            Self::BudgetExceeded => "BudgetExceeded",
            Self::ModelUnavailable => "ModelUnavailable",
            Self::NoInteractiveUser => "NoInteractiveUser",
            Self::Timeout => "Timeout",
            Self::PermissionDenied => "PermissionDenied",
            Self::WorkflowReplayMismatch => "WorkflowReplayMismatch",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == name)
    }
}

impl fmt::Display for DiagnosticKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A 1-based source position in the Workflow script.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Position {
    pub line: u32,
    pub column: u32,
}

impl fmt::Display for Position {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.line, self.column)
    }
}

/// One Workflow failure: a kind, a message and, for script errors, where it happened.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Diagnostic {
    pub kind: DiagnosticKind,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position: Option<Position>,
}

impl Diagnostic {
    pub fn new(kind: DiagnosticKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            position: None,
        }
    }

    pub fn at(mut self, position: Option<Position>) -> Self {
        self.position = position;
        self
    }

    pub fn parse(message: impl Into<String>, position: Position) -> Self {
        Self::new(DiagnosticKind::ParseError, message).at(Some(position))
    }

    pub fn execution(message: impl Into<String>) -> Self {
        Self::new(DiagnosticKind::ExecutionFailure, message)
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.kind, self.message)?;
        match self.position {
            Some(position) => write!(f, " ({position})"),
            None => Ok(()),
        }
    }
}

impl std::error::Error for Diagnostic {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_round_trip_and_display_with_location() {
        for kind in DiagnosticKind::ALL {
            assert_eq!(DiagnosticKind::parse(kind.as_str()), Some(kind));
        }
        assert_eq!(DiagnosticKind::parse("Other"), None);
        let diagnostic = Diagnostic::parse(
            "meta must be a pure literal",
            Position { line: 3, column: 9 },
        );
        assert_eq!(
            diagnostic.to_string(),
            "ParseError: meta must be a pure literal (3:9)"
        );
        let json = serde_json::to_value(&diagnostic).unwrap();
        assert_eq!(json["kind"], "ParseError");
        assert_eq!(json["position"]["line"], 3);
    }
}
