//! `/loop` in the TUI (`loops-scheduling` → Fixed-interval loop, Self-paced loop, Noop
//! collapsing, Loop status visibility): the command posts to the server, `/loop list`
//! opens a picker whose Enter stops a loop, iteration prompts render as `↻` lines and runs
//! of quiet iterations collapse into one `N quiet checks` line.

use std::collections::{HashMap, HashSet};

use cyber_core::loops::LoopCommand;
use serde_json::{Value, json};

use cyber_client::Client;

use crate::app::{Action, App};
use crate::model::{Choice, Item};
use crate::perform::{Msg, err};

/// What `/loop` asks the server.
#[derive(Debug, Clone, PartialEq)]
pub enum LoopRequest {
    Create {
        session_id: String,
        command: String,
    },
    List {
        session_id: String,
    },
    Stop {
        session_id: String,
        loop_id: Option<String>,
    },
}

/// The Session's loops as the transcript and footer use them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Loops {
    /// Iteration message ID → whether it reported nothing new.
    noop: HashMap<String, bool>,
    /// Next due times of the active loops.
    next_due: Vec<Option<i64>>,
}

impl Loops {
    /// Replace with `GET /sessions/{id}/loops`.
    pub fn set(&mut self, loops: &[Value]) {
        self.noop = loops
            .iter()
            .flat_map(|l| l["history"].as_array().into_iter().flatten())
            .filter_map(|r| Some((r["message_id"].as_str()?.to_owned(), r["noop"].as_bool()?)))
            .collect();
        self.next_due = loops
            .iter()
            .filter(|l| l["status"] == "active")
            .map(|l| l["next_due_ms"].as_i64())
            .collect();
    }

    /// Replace with a snapshot's loops; `None` (no iterations in the transcript) keeps them.
    pub fn replace(&mut self, loops: Option<Vec<Value>>) {
        if let Some(loops) = loops {
            self.set(&loops);
        }
    }

    pub fn noop(&self, message_id: &str) -> bool {
        self.noop.get(message_id).copied().unwrap_or(false)
    }

    /// `↻ loop · next in 4m`, `↻ loop · running`, `↻ 2 loops`; `None` without active loops.
    pub fn footer(&self, now_ms: i64) -> Option<String> {
        match self.next_due.as_slice() {
            [] => None,
            [Some(due)] => Some(format!("↻ loop · next in {}", until(*due - now_ms))),
            [None] => Some("↻ loop · running".into()),
            many => Some(format!("↻ {} loops", many.len())),
        }
    }
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

fn until(ms: i64) -> String {
    let seconds = (ms.max(0) + 999) / 1000;
    match seconds {
        0..=59 => format!("{seconds}s"),
        60..=3599 => format!("{}m", seconds / 60),
        _ => format!("{}h", seconds / 3600),
    }
}

/// Whether `id` is a loop iteration's inbox ID (`loopi_<loop>_<n>`).
pub fn is_iteration(id: &str) -> bool {
    id.starts_with("loopi_")
}

/// `(short loop id, n)` of an iteration ID.
pub fn iteration_label(id: &str) -> Option<(String, String)> {
    let rest = id.strip_prefix("loopi_")?;
    let (ulid, n) = rest.rsplit_once('_')?;
    let short = ulid.get(ulid.len().saturating_sub(4)..)?.to_lowercase();
    Some((short, n.to_owned()))
}

/// Runs of quiet iterations: the item index that draws `N quiet checks` with its count
/// and loop label, and every other index of those runs, which draws nothing. An
/// iteration's run covers its prompt and the replies up to the next user item.
pub fn quiet_runs(
    items: &[Item],
    loops: &Loops,
) -> (HashMap<usize, (usize, String)>, HashSet<usize>) {
    let mut starts = HashMap::new();
    let mut hidden = HashSet::new();
    let mut i = 0;
    while i < items.len() {
        let Some(label) = quiet(&items[i], loops) else {
            i += 1;
            continue;
        };
        let start = i;
        let mut count = 0;
        while i < items.len() && quiet(&items[i], loops).as_ref() == Some(&label) {
            count += 1;
            i += 1;
            while i < items.len() && !matches!(items[i], Item::User { .. }) {
                i += 1;
            }
        }
        starts.insert(start, (count, label));
        hidden.extend(start + 1..i);
    }
    (starts, hidden)
}

/// The loop label of a quiet iteration prompt.
fn quiet(item: &Item, loops: &Loops) -> Option<String> {
    let Item::User { id, .. } = item else {
        return None;
    };
    (is_iteration(id) && loops.noop(id)).then(|| iteration_label(id).map(|(short, _)| short))?
}

impl App {
    /// `/loop list` answered: a picker, or a hint when the Session has no loops.
    pub(crate) fn loops_listed(&mut self, session_id: &str, choices: Vec<Choice>) {
        if session_id != self.session.id {
            return;
        }
        if choices.is_empty() {
            self.toast("No loops in this Session; start one with /loop <interval> <prompt>");
            return;
        }
        self.open_picker(
            crate::app::PickerKind::Loops,
            "Loops · Enter stops the selected active loop",
            choices,
        );
    }

    /// `/loop …`: checked here first, so a typo is reported without a server round trip.
    pub(crate) fn loop_command(&mut self, args: &str) -> Vec<Action> {
        let session_id = self.session.id.clone();
        match cyber_core::loops::parse(args) {
            Err(message) => {
                self.toast(message);
                Vec::new()
            }
            Ok(LoopCommand::Create(_)) => vec![Action::Loop(LoopRequest::Create {
                session_id,
                command: args.trim().to_owned(),
            })],
            Ok(LoopCommand::List) => vec![Action::Loop(LoopRequest::List { session_id })],
            Ok(LoopCommand::Stop(loop_id)) => {
                vec![Action::Loop(LoopRequest::Stop {
                    session_id,
                    loop_id,
                })]
            }
        }
    }
}

/// Run a `/loop` request; creation and stops answer with a toast, the listing with the
/// picker rows.
pub async fn perform(client: &Client, request: LoopRequest) -> Result<Msg, String> {
    match request {
        LoopRequest::Create {
            session_id,
            command,
        } => {
            let path = format!("/sessions/{session_id}/loops");
            let created = client
                .post(&path, json!({ "command": command }))
                .await
                .map_err(err)?;
            Ok(Msg::Toast(started(&created["data"])))
        }
        LoopRequest::Stop {
            session_id,
            loop_id,
        } => {
            let path = match &loop_id {
                Some(id) => format!("/sessions/{session_id}/loops/{id}"),
                None => format!("/sessions/{session_id}/loops"),
            };
            let stopped = client.delete(&path).await.map_err(err)?;
            let count = stopped["data"].as_array().map_or(0, Vec::len);
            Ok(Msg::Toast(match count {
                0 => "No active loops".into(),
                1 => "Stopped 1 loop".into(),
                n => format!("Stopped {n} loops"),
            }))
        }
        LoopRequest::List { session_id } => {
            let listed = client
                .get(&format!("/sessions/{session_id}/loops"))
                .await
                .map_err(err)?;
            let loops = listed["data"].as_array().cloned().unwrap_or_default();
            Ok(Msg::Loops {
                session_id,
                choices: loops.iter().map(choice).collect(),
            })
        }
    }
}

/// `↻ loop 7xk2 · every 5m · first iteration now`.
fn started(data: &Value) -> String {
    let short = short_id(data["loop_id"].as_str().unwrap_or_default());
    match data["interval_seconds"].as_u64() {
        Some(seconds) => format!(
            "↻ loop {short} · every {} · first iteration now",
            every(seconds)
        ),
        None => format!("↻ loop {short} · self-paced · first iteration now"),
    }
}

fn every(seconds: u64) -> String {
    match seconds {
        s if s % 86_400 == 0 => format!("{}d", s / 86_400),
        s if s % 3600 == 0 => format!("{}h", s / 3600),
        s if s % 60 == 0 => format!("{}m", s / 60),
        s => format!("{s}s"),
    }
}

fn short_id(loop_id: &str) -> String {
    let ulid = loop_id.trim_start_matches("lop_");
    ulid.get(ulid.len().saturating_sub(4)..)
        .unwrap_or(ulid)
        .to_lowercase()
}

/// A picker row: the active ones stop on Enter; ended ones only inform.
fn choice(l: &Value) -> Choice {
    let pace = l["interval_seconds"]
        .as_u64()
        .map_or("self-paced".into(), |s| format!("every {}", every(s)));
    let status = l["status"].as_str().unwrap_or("active");
    let iterations = l["iterations"].as_u64().unwrap_or(0);
    let quiet = l["noop_streak"].as_u64().unwrap_or(0);
    let mut detail = format!("{status} · {iterations} iterations");
    if quiet > 0 {
        detail.push_str(&format!(" · {quiet} quiet"));
    }
    if let Some(reason) = l["stop_reason"].as_str() {
        detail.push_str(&format!(" · {reason}"));
    }
    Choice {
        key: if status == "active" {
            l["loop_id"].as_str().unwrap_or_default().to_owned()
        } else {
            String::new()
        },
        label: format!(
            "↻ {} · {pace} · {}",
            short_id(l["loop_id"].as_str().unwrap_or_default()),
            l["prompt"].as_str().unwrap_or_default()
        ),
        detail,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(id: &str) -> Item {
        Item::User {
            id: id.into(),
            text: "check".into(),
            promoted_ms: None,
        }
    }

    fn reply(id: &str) -> Item {
        Item::System {
            id: id.into(),
            text: "reply".into(),
        }
    }

    #[test]
    fn consecutive_quiet_iterations_of_one_loop_collapse() {
        let mut loops = Loops::default();
        loops.set(&[json!({"status": "active", "next_due_ms": 0, "history": [
            {"message_id": "loopi_01ABCD_1", "noop": true},
            {"message_id": "loopi_01ABCD_2", "noop": true},
            {"message_id": "loopi_01ABCD_3", "noop": false},
        ]})]);
        let items = vec![
            user("loopi_01ABCD_1"),
            reply("a1"),
            user("loopi_01ABCD_2"),
            reply("a2"),
            reply("a2b"),
            user("loopi_01ABCD_3"),
            reply("a3"),
        ];
        let (starts, hidden) = quiet_runs(&items, &loops);
        assert_eq!(starts.get(&0), Some(&(2, "abcd".to_string())));
        assert_eq!(hidden, (1..5).collect());
    }

    #[test]
    fn labels_footer_and_rows() {
        assert_eq!(
            iteration_label("loopi_01ABCDEF_12"),
            Some(("cdef".into(), "12".into()))
        );
        let mut loops = Loops::default();
        assert_eq!(loops.footer(0), None);
        loops.set(&[json!({"status": "active", "next_due_ms": 240_000, "history": []})]);
        assert_eq!(loops.footer(0).unwrap(), "↻ loop · next in 4m");
        assert_eq!(
            started(&json!({"loop_id": "lop_01XYZ7K2", "interval_seconds": 300})),
            "↻ loop z7k2 · every 5m · first iteration now"
        );
        let row = choice(
            &json!({"loop_id": "lop_01ABCD", "interval_seconds": 3600, "status": "stopped",
            "iterations": 3, "noop_streak": 2, "prompt": "check CI", "stop_reason": "stopped by the user"}),
        );
        assert_eq!(row.key, "");
        assert_eq!(row.label, "↻ abcd · every 1h · check CI");
        assert_eq!(
            row.detail,
            "stopped · 3 iterations · 2 quiet · stopped by the user"
        );
    }
}
