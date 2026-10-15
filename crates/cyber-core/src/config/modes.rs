//! Mode autonomy ranking and the non-interactive Session rules (`mcp-server` → Session
//! creation over MCP, `exec-mode` → Never blocking on interaction).
//!
//! The ranking here bounds the Modes MCP serve may give the Sessions it creates. It is
//! separate from `gate::WIDE_MODES`, which decides which project-configured Modes need
//! workspace trust; the two answer different questions and change independently.

use serde_json::{Value, json};

/// Modes from least to most autonomous.
const RANKED: [&str; 6] = [
    "plan",
    "dont-ask",
    "default",
    "accept-edits",
    "auto",
    "bypass",
];

/// The position of `mode` in the autonomy ranking; `None` for an unknown Mode.
pub fn mode_rank(mode: &str) -> Option<usize> {
    RANKED.iter().position(|ranked| *ranked == mode)
}

/// The MCP serve Mode ceiling: `max_mode`, else the global `mode`, else `default`;
/// never `bypass` (a `bypass` default gives `auto`).
pub fn mode_ceiling(max_mode: Option<&str>, global_mode: Option<&str>) -> String {
    match max_mode.or(global_mode).unwrap_or("default") {
        "bypass" => "auto".into(),
        mode => mode.into(),
    }
}

/// Bound a requested Mode by `ceiling`. A Mode at or below the ceiling is kept; above it
/// (or unknown) it is clamped to the ceiling, unless the caller asked for it `explicit`ly,
/// which is refused (`None`). `bypass` never results.
pub fn bound_mode(requested: &str, ceiling: &str, explicit: bool) -> Option<String> {
    let limit = mode_rank(ceiling)
        .unwrap_or(0)
        .min(mode_rank("auto").unwrap_or_default());
    match mode_rank(requested) {
        Some(rank) if rank <= limit => Some(requested.into()),
        _ if explicit => None,
        _ => Some(RANKED[limit].into()),
    }
}

/// Session rules that keep a Session from ever waiting on a human answer: deny
/// `question`, `plan_enter` and `plan_exit`. Used by `cyber exec` and MCP serve.
pub fn never_ask_rules() -> Value {
    json!({ "question": "deny", "plan_enter": "deny", "plan_exit": "deny" })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes_rank_from_plan_to_bypass() {
        let ranks: Vec<_> = [
            "plan",
            "dont-ask",
            "default",
            "accept-edits",
            "auto",
            "bypass",
        ]
        .iter()
        .map(|mode| mode_rank(mode).unwrap())
        .collect();
        assert_eq!(ranks, [0, 1, 2, 3, 4, 5]);
        assert_eq!(mode_rank("yolo"), None);
    }

    #[test]
    fn the_ceiling_defaults_to_the_global_mode_and_never_reaches_bypass() {
        assert_eq!(mode_ceiling(None, None), "default");
        assert_eq!(mode_ceiling(None, Some("bypass")), "auto");
        assert_eq!(mode_ceiling(None, Some("plan")), "plan");
        assert_eq!(
            mode_ceiling(Some("accept-edits"), Some("plan")),
            "accept-edits"
        );
        assert_eq!(mode_ceiling(Some("auto"), Some("bypass")), "auto");
    }

    #[test]
    fn explicit_modes_above_the_ceiling_are_refused_and_implicit_ones_clamped() {
        assert_eq!(bound_mode("auto", "default", true), None);
        assert_eq!(bound_mode("auto", "default", false).unwrap(), "default");
        assert_eq!(bound_mode("bypass", "auto", true), None);
        assert_eq!(bound_mode("bypass", "bypass", false).unwrap(), "auto");
        assert_eq!(bound_mode("dont-ask", "default", true).unwrap(), "dont-ask");
        assert_eq!(
            bound_mode("accept-edits", "default", false).unwrap(),
            "default"
        );
        assert_eq!(bound_mode("plan", "plan", true).unwrap(), "plan");
        assert_eq!(bound_mode("unknown", "default", true), None);
        assert_eq!(bound_mode("unknown", "default", false).unwrap(), "default");
    }

    #[test]
    fn never_ask_rules_deny_the_asking_tools() {
        let rules = never_ask_rules();
        for action in ["question", "plan_enter", "plan_exit"] {
            assert_eq!(rules[action], "deny", "{action}");
        }
        assert_eq!(rules.as_object().unwrap().len(), 3);
    }
}
