//! The Turn footer marker (`tui` → Turn footer marker), built only from durable
//! message data so it reads the same after a reload, on another client or after a
//! server restart.

use crate::model::Item;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Done,
    Interrupted,
    Failed,
}

/// One finished run: the assistant entries that answer one promoted user input.
#[derive(Debug, Clone, PartialEq)]
pub struct Marker {
    /// Index in the items of the run's last assistant entry; the marker goes below it.
    pub after: usize,
    pub agent: String,
    /// `provider/model` of the last step.
    pub model: String,
    pub duration_ms: i64,
    pub ended_ms: i64,
    pub outcome: Outcome,
}

/// Markers for every finished run. The last run has none while `running`, and a run
/// with a step that has not ended has none.
pub fn markers(items: &[Item], running: bool) -> Vec<Marker> {
    let mut out = Vec::new();
    let mut run: Vec<usize> = Vec::new();
    for (i, item) in items.iter().enumerate() {
        match item {
            Item::Assistant { .. } => run.push(i),
            Item::User { .. } => out.extend(marker(items, &std::mem::take(&mut run))),
            Item::System { .. } => {}
        }
    }
    if !running {
        out.extend(marker(items, &run));
    }
    out
}

fn marker(items: &[Item], run: &[usize]) -> Option<Marker> {
    let first = step_of(items, *run.first()?)?;
    let last_index = *run.last()?;
    let Item::Assistant {
        step, error, tools, ..
    } = &items[last_index]
    else {
        return None;
    };
    let ended_ms = run
        .iter()
        .map(|&i| step_of(items, i).and_then(|s| s.ended_ms))
        .collect::<Option<Vec<_>>>()?
        .into_iter()
        .max()?;
    let interrupted = step.error_kind.as_deref() == Some("interrupted")
        || tools.iter().any(|t| t.status == "interrupted");
    let outcome = match error {
        _ if interrupted => Outcome::Interrupted,
        Some(_) => Outcome::Failed,
        None => Outcome::Done,
    };
    let model = match (step.provider.is_empty(), step.model.is_empty()) {
        (_, true) => String::new(),
        (true, false) => step.model.clone(),
        (false, false) => format!("{}/{}", step.provider, step.model),
    };
    Some(Marker {
        after: last_index,
        agent: step.agent.clone(),
        model,
        duration_ms: ended_ms - first.started_ms?,
        ended_ms,
        outcome,
    })
}

fn step_of(items: &[Item], i: usize) -> Option<&crate::model::Step> {
    match &items[i] {
        Item::Assistant { step, .. } => Some(step),
        _ => None,
    }
}

/// `850ms`, `11.3s`, `4m 05s` or `1h 02m`.
pub fn duration(ms: i64) -> String {
    let ms = ms.max(0);
    if ms < 1_000 {
        return format!("{ms}ms");
    }
    if ms < 60_000 {
        // Tenths, rounded down, so 59,999 ms never reads `60.0s`.
        return format!("{}.{}s", ms / 1_000, ms % 1_000 / 100);
    }
    let seconds = ms / 1_000;
    if seconds < 3_600 {
        return format!("{}m {:02}s", seconds / 60, seconds % 60);
    }
    format!("{}h {:02}m", seconds / 3_600, seconds % 3_600 / 60)
}

/// `build` → `Build`.
pub fn title_case(name: &str) -> String {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// The local wall-clock `HH:MM` of `ms`; `offset_minutes` overrides the local zone.
pub fn clock(ms: i64, offset_minutes: Option<i32>) -> String {
    use chrono::{FixedOffset, Local, TimeZone};
    let Some(utc) = chrono::DateTime::from_timestamp_millis(ms) else {
        return String::new();
    };
    match offset_minutes.and_then(|m| FixedOffset::east_opt(m * 60)) {
        Some(zone) => zone
            .from_utc_datetime(&utc.naive_utc())
            .format("%H:%M")
            .to_string(),
        None => Local
            .from_utc_datetime(&utc.naive_utc())
            .format("%H:%M")
            .to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Step, Tool};

    fn user(id: &str) -> Item {
        Item::User {
            id: id.into(),
            text: "go".into(),
            promoted_ms: Some(0),
        }
    }

    fn assistant(id: &str, started: i64, ended: Option<i64>, error_kind: Option<&str>) -> Item {
        Item::Assistant {
            id: id.into(),
            text: String::new(),
            reasoning: String::new(),
            tools: Vec::new(),
            error: error_kind.map(|k| format!("{k} error")),
            step: Step {
                agent: "build".into(),
                provider: "test".into(),
                model: "main".into(),
                started_ms: Some(started),
                ended_ms: ended,
                error_kind: error_kind.map(str::to_string),
            },
        }
    }

    #[test]
    fn durations_use_the_specified_units() {
        assert_eq!(duration(850), "850ms");
        assert_eq!(duration(11_300), "11.3s");
        assert_eq!(duration(59_999), "59.9s");
        assert_eq!(duration(245_000), "4m 05s");
        assert_eq!(duration(3_720_000), "1h 02m");
        assert_eq!(duration(-5), "0ms");
    }

    #[test]
    fn runs_span_their_steps_and_the_active_run_has_no_marker() {
        let items = vec![
            user("u1"),
            assistant("a1", 1_000, Some(4_000), None),
            assistant("a2", 4_100, Some(12_300), None),
            user("u2"),
            assistant("a3", 20_000, Some(24_200), Some("interrupted")),
            user("u3"),
            assistant("a4", 30_000, Some(30_500), Some("rate_limit")),
            user("u4"),
            assistant("a5", 40_000, None, None),
        ];
        let done = markers(&items, true);
        assert_eq!(done.len(), 3);
        assert_eq!((done[0].after, done[0].duration_ms), (2, 11_300));
        assert_eq!(done[0].model, "test/main");
        assert_eq!(done[1].outcome, Outcome::Interrupted);
        assert_eq!(done[2].outcome, Outcome::Failed);
        assert_eq!(
            markers(&items, false).len(),
            3,
            "a step that never ended has no marker"
        );
    }

    #[test]
    fn interrupted_tools_mark_the_run_interrupted() {
        let mut items = vec![user("u1"), assistant("a1", 0, Some(4_200), None)];
        if let Item::Assistant { tools, .. } = &mut items[1] {
            tools.push(Tool {
                call_id: "c".into(),
                name: "bash".into(),
                input: serde_json::Value::Null,
                status: "interrupted".into(),
                output: String::new(),
                redactions: 0,
            });
        }
        assert_eq!(markers(&items, false)[0].outcome, Outcome::Interrupted);
    }

    #[test]
    fn clock_uses_the_given_offset() {
        assert_eq!(clock(14 * 3_600_000 + 2 * 60_000, Some(0)), "14:02");
        assert_eq!(clock(0, Some(-180)), "21:00");
        assert_eq!(title_case("build"), "Build");
    }
}
