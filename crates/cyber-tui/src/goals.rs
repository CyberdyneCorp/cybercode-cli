//! `/goal` in the TUI (`goals` → Goal management commands, tui → Goal and loop
//! indicators): commands post to the server; `/goal` opens a panel with the current Goal
//! (turns against budget, cost, evaluator totals, last verdict), the queue and recent
//! Goals, where `e` edits, `p` pauses, `r` resumes and `c` clears the selected Goal and
//! Shift+↑/↓ moves a queued one. Goal prompts render as `◎` lines, the footer names the
//! active Goal and the sidebar has a Goal section.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use cyber_client::Client;
use cyber_core::goals::GoalCommand;
use serde_json::{Value, json};

use crate::app::{Action, App, Overlay, PickerKind};
use crate::model::Choice;
use crate::perform::{Msg, err};

/// The panel title, naming its keys.
const PANEL_TITLE: &str = "Goals · e edit · p pause · r resume · c clear · shift+↑↓ reorder";

/// What `/goal` asks the server.
#[derive(Debug, Clone, PartialEq)]
pub enum GoalRequest {
    Show {
        session_id: String,
    },
    Set {
        session_id: String,
        condition: String,
        check: Option<String>,
        add: bool,
    },
    Pause {
        session_id: String,
    },
    Resume {
        session_id: String,
    },
    Skip {
        session_id: String,
    },
    Clear {
        session_id: String,
        goal_id: Option<String>,
    },
    /// Put the condition of `goal_id` (else the current Goal) into the composer.
    Edit {
        session_id: String,
        goal_id: Option<String>,
    },
    EditCondition {
        session_id: String,
        goal_id: String,
        condition: String,
    },
    /// `/goal fork`: a new background Session pursues the Goal.
    Fork {
        session_id: String,
        condition: String,
        check: Option<String>,
    },
    /// `/goal use <template> [text]`.
    Use {
        session_id: String,
        template: String,
        args: String,
    },
    /// The queue in its new order; the panel reopens with it.
    Reorder {
        session_id: String,
        goal_ids: Vec<String>,
    },
}

/// The current Goal as the footer and the sidebar show it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Current {
    pub status: String,
    pub condition: String,
    pub turns: u64,
    pub max_turns: u64,
    pub cost_usd: f64,
    /// `met`, `not met` or `impossible`, with its reason.
    pub verdict: Option<(String, String)>,
}

/// The Session's Goals as the footer and the sidebar show them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Goals {
    pub current: Option<Current>,
    pub queued: usize,
}

impl Goals {
    /// Replace with `GET /sessions/{id}/goals`; `None` keeps the last value.
    pub fn replace(&mut self, view: Option<Value>) {
        let Some(view) = view else {
            return;
        };
        self.queued = view["queue"].as_array().map_or(0, Vec::len);
        self.current = view["current"]
            .as_object()
            .map(|_| current(&view["current"]));
    }

    /// `◎ goal <condition>`, `◎ goal paused <condition>`; `None` without a current Goal.
    pub fn footer(&self) -> Option<String> {
        let goal = self.current.as_ref()?;
        let short = truncate(goal.condition.lines().next().unwrap_or_default(), 40);
        Some(match goal.status.as_str() {
            "active" => format!("◎ goal {short}"),
            other => format!("◎ goal {other} {short}"),
        })
    }

    /// The sidebar's Goal section without its header; empty without a current Goal.
    pub fn sidebar(&self) -> Vec<String> {
        let Some(goal) = &self.current else {
            return Vec::new();
        };
        let mut lines = vec![
            format!("◎ {}", goal.condition.lines().next().unwrap_or_default()),
            format!(
                "{} · turn {}/{} · ${:.2}",
                goal.status, goal.turns, goal.max_turns, goal.cost_usd
            ),
        ];
        if let Some((verdict, reason)) = &goal.verdict {
            lines.push(format!("{verdict}: {reason}"));
        }
        if self.queued > 0 {
            lines.push(format!("+{} queued", self.queued));
        }
        lines
    }
}

fn current(g: &Value) -> Current {
    let text = |key: &str| g[key].as_str().unwrap_or_default().to_owned();
    let verdict = g["last_verdict"]
        .as_str()
        .map(|v| (v.replace('_', " "), text("reason")));
    Current {
        status: g["status"].as_str().unwrap_or("active").to_owned(),
        condition: text("condition"),
        turns: g["turns"].as_u64().unwrap_or(0),
        max_turns: g["budget"]["max_turns"].as_u64().unwrap_or(0),
        cost_usd: g["cost_usd"].as_f64().unwrap_or(0.0),
        verdict,
    }
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut cut: String = text.chars().take(max - 1).collect();
    cut.push('…');
    cut
}

/// A goal prompt or continuation, as the server words it: the line to draw.
pub fn prompt_line(text: &str) -> Option<String> {
    let body = |tag: &str| {
        let start = text.find('>')? + 1;
        let end = text.rfind(&format!("</{tag}>"))?;
        Some(text[start..end].trim().to_owned())
    };
    if text.starts_with("<goal id=") {
        let inner = body("goal")?;
        let condition = inner.lines().nth(1).unwrap_or_default();
        return Some(format!("◎ goal: {}", truncate(condition, 200)));
    }
    if text.starts_with("<goal_not_met") {
        let inner = body("goal_not_met")?;
        let reason = inner
            .lines()
            .find_map(|l| l.strip_prefix("Why: "))
            .unwrap_or("not met yet");
        return Some(format!("◎ not met: {}", truncate(reason, 200)));
    }
    None
}

/// Whether the transcript holds a goal prompt, so the snapshot reads the Goals.
pub fn mentioned(text: &str) -> bool {
    text.starts_with("<goal id=") || text.starts_with("<goal_not_met")
}

impl App {
    /// `/goal …`: checked here first, so a typo is reported without a server round trip.
    pub(crate) fn goal_command(&mut self, args: &str) -> Vec<Action> {
        let session_id = self.session.id.clone();
        let request = match cyber_core::goals::parse(args) {
            Err(message) => {
                self.toast(message);
                return Vec::new();
            }
            Ok(GoalCommand::Show | GoalCommand::List) => GoalRequest::Show { session_id },
            Ok(GoalCommand::Set { condition, check }) => GoalRequest::Set {
                session_id,
                condition,
                check,
                add: false,
            },
            Ok(GoalCommand::Add { condition, check }) => GoalRequest::Set {
                session_id,
                condition,
                check,
                add: true,
            },
            Ok(GoalCommand::Pause) => GoalRequest::Pause { session_id },
            Ok(GoalCommand::Resume) => GoalRequest::Resume { session_id },
            Ok(GoalCommand::Skip) => GoalRequest::Skip { session_id },
            Ok(GoalCommand::Clear(goal_id)) => GoalRequest::Clear {
                session_id,
                goal_id,
            },
            Ok(GoalCommand::Edit(goal_id)) => GoalRequest::Edit {
                session_id,
                goal_id,
            },
            Ok(GoalCommand::EditCondition { goal_id, condition }) => GoalRequest::EditCondition {
                session_id,
                goal_id,
                condition,
            },
            Ok(GoalCommand::Fork { condition, check }) => GoalRequest::Fork {
                session_id,
                condition,
                check,
            },
            Ok(GoalCommand::Use { template, args }) => GoalRequest::Use {
                session_id,
                template,
                args,
            },
        };
        vec![Action::Goal(request)]
    }

    /// `/goal` answered: the picker, or a hint when the Session has no Goals.
    pub(crate) fn goals_listed(&mut self, session_id: &str, choices: Vec<Choice>) {
        if session_id != self.session.id {
            return;
        }
        if choices.is_empty() {
            self.toast("No goals; set one with /goal <condition>");
            return;
        }
        self.open_picker(PickerKind::Goals, PANEL_TITLE, choices);
    }

    /// A key in the `/goal` panel: `e`, `p`, `r`, `c` and Shift+↑/↓; `None` for keys
    /// the picker handles itself (moving, Enter, Esc).
    pub(crate) fn goal_panel_key(&mut self, key: KeyEvent) -> Option<Vec<Action>> {
        let Overlay::Picker(picker) = &self.overlay else {
            return None;
        };
        if picker.kind != PickerKind::Goals {
            return None;
        }
        let session_id = self.session.id.clone();
        let selected = picker.current().map(|choice| choice.key.clone());
        let request = match (key.code, key.modifiers) {
            (KeyCode::Char('e'), KeyModifiers::NONE) => GoalRequest::Edit {
                session_id,
                goal_id: selected,
            },
            (KeyCode::Char('p'), KeyModifiers::NONE) => GoalRequest::Pause { session_id },
            (KeyCode::Char('r'), KeyModifiers::NONE) => GoalRequest::Resume { session_id },
            (KeyCode::Char('c'), KeyModifiers::NONE) => GoalRequest::Clear {
                session_id,
                goal_id: Some(selected?),
            },
            (KeyCode::Up | KeyCode::Down, KeyModifiers::SHIFT) => {
                let goal_ids = moved(&picker.items, selected.as_deref()?, key.code == KeyCode::Up)?;
                GoalRequest::Reorder {
                    session_id,
                    goal_ids,
                }
            }
            _ => return None,
        };
        self.overlay = Overlay::None;
        Some(vec![Action::Goal(request)])
    }
}

/// The queued Goal IDs with `selected` moved one place up or down; `None` when it is
/// not queued or cannot move.
fn moved(rows: &[Choice], selected: &str, up: bool) -> Option<Vec<String>> {
    let mut queue: Vec<String> = rows
        .iter()
        .filter(|row| row.detail.starts_with("queued "))
        .map(|row| row.key.clone())
        .collect();
    let at = queue.iter().position(|id| id == selected)?;
    let to = if up { at.checked_sub(1)? } else { at + 1 };
    if to >= queue.len() {
        return None;
    }
    queue.swap(at, to);
    Some(queue)
}

/// Run a `/goal` request.
pub async fn perform(client: &Client, request: GoalRequest) -> Result<Msg, String> {
    match request {
        GoalRequest::Show { session_id } => show(client, session_id).await,
        GoalRequest::Set {
            session_id,
            condition,
            check,
            add,
        } => set(client, &session_id, condition, check, add).await,
        GoalRequest::Pause { session_id } => control(client, &session_id, "pause").await,
        GoalRequest::Resume { session_id } => control(client, &session_id, "resume").await,
        GoalRequest::Skip { session_id } => control(client, &session_id, "skip").await,
        GoalRequest::Clear {
            session_id,
            goal_id,
        } => clear(client, &session_id, goal_id).await,
        GoalRequest::Edit {
            session_id,
            goal_id,
        } => edit(client, &session_id, goal_id).await,
        GoalRequest::EditCondition {
            session_id,
            goal_id,
            condition,
        } => {
            let path = format!("{}/{goal_id}", base(&session_id));
            client
                .patch(&path, json!({"condition": condition}))
                .await
                .map_err(err)?;
            Ok(Msg::Toast("◎ goal updated".into()))
        }
        GoalRequest::Fork {
            session_id,
            condition,
            check,
        } => fork(client, &session_id, condition, check).await,
        GoalRequest::Use {
            session_id,
            template,
            args,
        } => {
            let body = json!({"template": template, "args": args, "mode": "set"});
            client.post(&base(&session_id), body).await.map_err(err)?;
            Ok(Msg::Toast(format!(
                "◎ goal set from {template}; working toward it"
            )))
        }
        GoalRequest::Reorder {
            session_id,
            goal_ids,
        } => {
            let path = format!("{}/order", base(&session_id));
            client
                .patch(&path, json!({"goal_ids": goal_ids}))
                .await
                .map_err(err)?;
            show(client, session_id).await
        }
    }
}

/// `/goal fork`: report the new Session and where it works.
async fn fork(
    client: &Client,
    session_id: &str,
    condition: String,
    check: Option<String>,
) -> Result<Msg, String> {
    let mut body = json!({"condition": condition});
    if let Some(check) = check {
        body["check"] = check.into();
    }
    let path = format!("{}/fork", base(session_id));
    let fork = client.post(&path, body).await.map_err(err)?;
    let data = &fork["data"];
    let id = data["session_id"].as_str().unwrap_or_default();
    Ok(Msg::Toast(match data["branch"].as_str() {
        Some(branch) => format!("◎ goal forked to {id} on {branch}"),
        None => format!("◎ goal forked to {id}"),
    }))
}

fn base(session_id: &str) -> String {
    format!("/sessions/{session_id}/goals")
}

async fn show(client: &Client, session_id: String) -> Result<Msg, String> {
    let view = client.get(&base(&session_id)).await.map_err(err)?;
    Ok(Msg::Goals {
        choices: choices(&view["data"]),
        session_id,
    })
}

async fn set(
    client: &Client,
    session_id: &str,
    condition: String,
    check: Option<String>,
    add: bool,
) -> Result<Msg, String> {
    let mut body = json!({"condition": condition, "mode": if add { "add" } else { "set" }});
    if let Some(check) = check {
        body["check"] = check.into();
    }
    let goal = client.post(&base(session_id), body).await.map_err(err)?;
    let active = goal["data"]["status"] == "active";
    Ok(Msg::Toast(if active {
        "◎ goal set; working toward it".into()
    } else {
        "◎ goal queued".into()
    }))
}

/// `pause`, `resume` or `skip`.
async fn control(client: &Client, session_id: &str, verb: &str) -> Result<Msg, String> {
    let path = format!("{}/{verb}", base(session_id));
    let answer = client.post(&path, json!({})).await.map_err(err)?;
    Ok(Msg::Toast(match verb {
        "skip" if answer["data"].is_null() => "◎ goal skipped; the queue is empty".into(),
        "skip" => "◎ goal skipped; working on the next one".into(),
        "pause" => "◎ goal paused".into(),
        _ => "◎ goal resumed".into(),
    }))
}

async fn clear(client: &Client, session_id: &str, goal_id: Option<String>) -> Result<Msg, String> {
    let path = match &goal_id {
        Some(id) => format!("{}/{id}", base(session_id)),
        None => base(session_id),
    };
    let cleared = client.delete(&path).await.map_err(err)?;
    let n = cleared["data"].as_array().map_or(0, Vec::len);
    Ok(Msg::Toast(format!(
        "◎ cleared {n} goal{}",
        if n == 1 { "" } else { "s" }
    )))
}

/// Prefill the composer with `/goal edit <id> <condition>`.
async fn edit(client: &Client, session_id: &str, goal_id: Option<String>) -> Result<Msg, String> {
    let view = client.get(&base(session_id)).await.map_err(err)?;
    let goal = find(&view["data"], goal_id.as_deref()).ok_or("no such goal to edit")?;
    Ok(Msg::Compose(format!(
        "/goal edit {} {}",
        goal["goal_id"].as_str().unwrap_or_default(),
        goal["condition"].as_str().unwrap_or_default()
    )))
}

/// The current Goal, or the live one named `goal_id`.
fn find(view: &Value, goal_id: Option<&str>) -> Option<Value> {
    let Some(id) = goal_id else {
        return Some(view["current"].clone()).filter(|g| !g.is_null());
    };
    view["queue"]
        .as_array()
        .into_iter()
        .flatten()
        .chain(std::iter::once(&view["current"]))
        .find(|g| g["goal_id"] == id)
        .cloned()
}

/// Picker rows: the current Goal with its totals, then the queue, then recent Goals.
pub(crate) fn choices(view: &Value) -> Vec<Choice> {
    let mut rows = Vec::new();
    if !view["current"].is_null() {
        rows.push(row(&view["current"], "current"));
    }
    for g in view["queue"].as_array().into_iter().flatten() {
        rows.push(row(g, "queued"));
    }
    for g in view["ended"].as_array().into_iter().flatten().take(5) {
        rows.push(row(g, "ended"));
    }
    rows
}

fn row(g: &Value, place: &str) -> Choice {
    let status = g["status"].as_str().unwrap_or_default();
    let turns = g["turns"].as_u64().unwrap_or(0);
    let max = g["budget"]["max_turns"].as_u64().unwrap_or(0);
    let mut detail = format!(
        "{place} · {status} · turn {turns}/{max} · ${:.2}",
        g["cost_usd"].as_f64().unwrap_or(0.0)
    );
    let evals = g["eval_count"].as_u64().unwrap_or(0);
    if evals > 0 {
        detail.push_str(&format!(
            " · evaluator {evals}× {} tokens ${:.2}",
            g["eval_tokens"].as_u64().unwrap_or(0),
            g["eval_cost_usd"].as_f64().unwrap_or(0.0)
        ));
    }
    if let Some(reason) = g["reason"].as_str() {
        detail.push_str(&format!(" · {}", truncate(reason, 80)));
    }
    Choice {
        key: g["goal_id"].as_str().unwrap_or_default().to_owned(),
        label: format!(
            "◎ {}",
            truncate(g["condition"].as_str().unwrap_or_default(), 70)
        ),
        detail,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompts_render_as_goal_lines() {
        let set = "<goal id=\"gol_1\">\nWork toward this goal until it is met:\ncargo test passes\n</goal>";
        assert_eq!(prompt_line(set).unwrap(), "◎ goal: cargo test passes");
        let not_met = "<goal_not_met id=\"gol_1\">\nThe goal is not met yet:\ncargo test passes\n\nWhy: 3 tests failing\n\nKeep working toward it.\n</goal_not_met>";
        assert_eq!(prompt_line(not_met).unwrap(), "◎ not met: 3 tests failing");
        assert_eq!(prompt_line("hello"), None);
    }

    #[test]
    fn footer_and_rows() {
        let mut goals = Goals::default();
        assert_eq!(goals.footer(), None);
        goals.replace(Some(
            json!({"current": {"status": "active", "condition": "ship the release notes"}}),
        ));
        assert_eq!(goals.footer().unwrap(), "◎ goal ship the release notes");
        goals.replace(Some(
            json!({"current": {"status": "paused", "condition": "x"}}),
        ));
        assert_eq!(goals.footer().unwrap(), "◎ goal paused x");
        let rows = choices(
            &json!({"current": {"goal_id": "gol_1", "condition": "A", "status": "active",
            "turns": 3, "budget": {"max_turns": 50}, "cost_usd": 0.5, "eval_count": 3, "eval_tokens": 900,
            "eval_cost_usd": 0.01, "reason": "2 left"}, "queue": [{"goal_id": "gol_2", "condition": "B",
            "status": "pending", "turns": 0, "budget": {"max_turns": 50}}], "ended": []}),
        );
        assert_eq!(rows.len(), 2);
        assert_eq!(
            rows[0].detail,
            "current · active · turn 3/50 · $0.50 · evaluator 3× 900 tokens $0.01 · 2 left"
        );
        assert_eq!(rows[1].label, "◎ B");
    }
}
