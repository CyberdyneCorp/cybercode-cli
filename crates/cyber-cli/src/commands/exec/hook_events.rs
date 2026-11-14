//! `--include-hook-events` (`exec-mode` → Streaming input and partial output): `hook_event`
//! lines read from the Session's durable hook receipts.
//!
//! Receipts are not on the Session event stream, so the driver pages the authenticated
//! `GET /sessions/{id}/hook-executions` (newest first) after an owned tool call settles and
//! when the Session goes idle. A receipt is reported once it has settled and only when its
//! `call_id` belongs to a tool call of a step this driver owns; Session-scoped receipts
//! without a `call_id` cannot be attributed to an input and are not reported. Lines carry
//! the hook id, event, outcome and decision kind, never hook IO or hook-supplied text.

use std::collections::{HashMap, HashSet, VecDeque};
use std::future::Future;

use serde_json::{Value, json};

/// Receipts per page (the endpoint's default).
pub(super) const PAGE: u32 = 50;
/// Pages read per poll: at most `PAGE * MAX_PAGES` receipts are examined each time.
const MAX_PAGES: usize = 4;
/// Reported receipt IDs remembered for deduplication.
const MAX_SEEN: usize = 1024;
/// Owned tool calls remembered for attribution.
const MAX_CALLS: usize = 1024;
/// Owned receipts seen running, whose settlement keeps paging past reported receipts.
const MAX_PENDING: usize = 64;
/// Allowance for a server clock behind this process when bounding pages by run start.
const CLOCK_SKEW_MS: i64 = 60_000;

/// A bounded set that forgets its oldest entries.
#[derive(Default)]
struct Bounded {
    set: HashSet<String>,
    order: VecDeque<String>,
}

impl Bounded {
    fn insert(&mut self, id: &str, limit: usize) {
        if !self.set.insert(id.to_owned()) {
            return;
        }
        self.order.push_back(id.to_owned());
        while self.order.len() > limit {
            if let Some(old) = self.order.pop_front() {
                self.set.remove(&old);
            }
        }
    }

    fn contains(&self, id: &str) -> bool {
        self.set.contains(id)
    }
}

pub(super) struct HookEvents {
    /// Receipts that started before this (run start less clock skew) end a poll.
    since_ms: i64,
    /// Owned call ID → the inputs (`id`, `message_id`) its step was answering.
    calls: HashMap<String, Value>,
    call_order: VecDeque<String>,
    seen: Bounded,
    pending: HashSet<String>,
}

impl HookEvents {
    pub fn new(started_ms: i64) -> Self {
        Self {
            since_ms: started_ms.saturating_sub(CLOCK_SKEW_MS),
            calls: HashMap::new(),
            call_order: VecDeque::new(),
            seen: Bounded::default(),
            pending: HashSet::new(),
        }
    }

    /// Attribute `call_id` to the inputs of the owned step that called it.
    pub fn own(&mut self, call_id: &str, inputs: Value) {
        if self.calls.insert(call_id.to_owned(), inputs).is_none() {
            self.call_order.push_back(call_id.to_owned());
        }
        while self.call_order.len() > MAX_CALLS {
            if let Some(old) = self.call_order.pop_front() {
                self.calls.remove(&old);
            }
        }
    }

    pub fn owns(&self, call_id: &str) -> bool {
        self.calls.contains_key(call_id)
    }

    /// Page receipts newest first through `fetch(cursor)` and return the `hook_event`
    /// payloads of newly settled owned receipts, oldest first. Paging stops at a receipt
    /// older than the run, at an already reported one (unless an owned receipt is still
    /// running), when there are no more pages, or after `MAX_PAGES` pages.
    pub async fn poll<F, Fut>(&mut self, mut fetch: F) -> Result<Vec<Value>, String>
    where
        F: FnMut(Option<String>) -> Fut,
        Fut: Future<Output = Result<Value, String>>,
    {
        let mut lines = Vec::new();
        let mut cursor = None;
        for _ in 0..MAX_PAGES {
            let page = fetch(cursor.take()).await?;
            let rows = page["data"].as_array().ok_or("invalid hook receipt page")?;
            if self.scan(rows, &mut lines) {
                break;
            }
            match page["cursor"]["next"]
                .as_str()
                .filter(|next| valid_cursor(next))
            {
                Some(next) => cursor = Some(next.to_owned()),
                None => break,
            }
        }
        lines.reverse();
        Ok(lines)
    }

    /// Collect lines from one page; true when paging should stop.
    fn scan(&mut self, rows: &[Value], lines: &mut Vec<Value>) -> bool {
        for row in rows {
            if row["started_ms"]
                .as_i64()
                .is_some_and(|at| at < self.since_ms)
            {
                return true;
            }
            let Some(id) = row["id"].as_str() else {
                continue;
            };
            if self.seen.contains(id) {
                if self.pending.is_empty() {
                    return true;
                }
                continue;
            }
            if let Some(line) = self.receive(id, row) {
                lines.push(line);
            }
        }
        false
    }

    /// The line of an owned, settled receipt; an owned running one is remembered.
    fn receive(&mut self, id: &str, row: &Value) -> Option<Value> {
        let call_id = row["call_id"].as_str()?;
        let inputs = self.calls.get(call_id)?;
        if row["status"] == "running" {
            if self.pending.len() < MAX_PENDING {
                self.pending.insert(id.to_owned());
            }
            return None;
        }
        let outcome = match &row["outcome"] {
            Value::Null => row["status"].clone(),
            outcome => outcome.clone(),
        };
        let line = json!({
            "receipt_id": id,
            "hook_id": row["hook_id"],
            "event": row["event"],
            "call_id": call_id,
            "tool_name": row["tool_name"],
            "outcome": outcome,
            "decision": row["decision"]["decision"],
            "duration_ms": row["duration_ms"],
            "inputs": inputs,
        });
        self.pending.remove(id);
        self.seen.insert(id, MAX_SEEN);
        Some(line)
    }
}

/// A server cursor (`<ms>:<session>:<receipt>`) safe to put in a query string.
fn valid_cursor(cursor: &str) -> bool {
    !cursor.is_empty()
        && cursor
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_-:".contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn receipt(n: usize, call: Option<&str>, status: &str) -> Value {
        json!({
            "id": format!("hke_{n:04}"),
            "hook_id": "guard",
            "event": "PreToolUse",
            "call_id": call,
            "tool_name": "read",
            "started_ms": 1_000_000 + n as i64,
            "status": status,
            "outcome": (status == "completed").then_some("blocked"),
            "decision": {"decision": "deny", "reason": "SECRET REASON"},
            "duration_ms": 3,
            "io": {"stdin": "RAW STDIN", "stdout": "", "stderr": "", "truncated": false},
        })
    }

    /// Serve `rows` (newest first) in pages of `PAGE`, counting fetches.
    async fn poll(events: &mut HookEvents, rows: &[Value], fetches: &mut usize) -> Vec<Value> {
        events
            .poll(|cursor| {
                *fetches += 1;
                let start: usize =
                    cursor.map_or(0, |c| c.rsplit('_').next().unwrap().parse().unwrap());
                let end = (start + PAGE as usize).min(rows.len());
                let next = (end < rows.len()).then(|| format!("1:ses_a:hke_{end}"));
                let page = json!({"data": rows[start..end], "cursor": {"next": next}});
                async move { Ok(page) }
            })
            .await
            .unwrap()
    }

    fn newest_first(count: usize, call: &str) -> Vec<Value> {
        (0..count)
            .rev()
            .map(|n| receipt(n, Some(call), "completed"))
            .collect()
    }

    #[tokio::test]
    async fn reports_owned_settled_receipts_once_without_hook_text_or_io() {
        let mut events = HookEvents::new(1_000_000 + CLOCK_SKEW_MS);
        events.own("call_a", json!([{"id": "a", "message_id": "msg_a"}]));
        let mut rows = vec![
            receipt(3, None, "completed"),
            receipt(2, Some("call_other"), "completed"),
            receipt(1, Some("call_a"), "completed"),
        ];
        let mut fetches = 0;
        let lines = poll(&mut events, &rows, &mut fetches).await;
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert_eq!(lines[0]["receipt_id"], "hke_0001");
        assert_eq!(lines[0]["outcome"], "blocked");
        assert_eq!(lines[0]["decision"], "deny");
        assert_eq!(lines[0]["inputs"][0]["id"], "a");
        let text = lines[0].to_string();
        assert!(!text.contains("SECRET REASON") && !text.contains("RAW STDIN"));

        rows.insert(0, receipt(4, Some("call_a"), "completed"));
        let lines = poll(&mut events, &rows, &mut fetches).await;
        assert_eq!(lines.len(), 1, "only the new receipt: {lines:?}");
        assert_eq!(lines[0]["receipt_id"], "hke_0004");
        assert_eq!(fetches, 2);
    }

    #[tokio::test]
    async fn a_running_receipt_is_reported_when_it_settles() {
        let mut events = HookEvents::new(1_000_000 + CLOCK_SKEW_MS);
        events.own("call_a", json!([]));
        let mut fetches = 0;
        let rows = vec![
            receipt(2, Some("call_a"), "completed"),
            receipt(1, Some("call_a"), "running"),
        ];
        let lines = poll(&mut events, &rows, &mut fetches).await;
        assert_eq!(lines.len(), 1);
        let rows = vec![
            receipt(2, Some("call_a"), "completed"),
            receipt(1, Some("call_a"), "completed"),
        ];
        let lines = poll(&mut events, &rows, &mut fetches).await;
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert_eq!(lines[0]["receipt_id"], "hke_0001");
        assert!(events.pending.is_empty());
    }

    #[tokio::test]
    async fn paging_is_bounded_and_stops_at_the_run_start() {
        let mut events = HookEvents::new(1_000_000 + CLOCK_SKEW_MS);
        events.own("call_a", json!([]));
        let rows = newest_first(PAGE as usize * MAX_PAGES * 2, "call_a");
        let mut fetches = 0;
        let lines = poll(&mut events, &rows, &mut fetches).await;
        assert_eq!(fetches, MAX_PAGES);
        assert_eq!(lines.len(), PAGE as usize * MAX_PAGES);
        let ids: Vec<_> = lines
            .iter()
            .map(|line| line["receipt_id"].clone())
            .collect();
        let mut sorted = ids.clone();
        sorted.sort_by_key(|id| id.as_str().unwrap().to_owned());
        assert_eq!(ids, sorted, "oldest first");

        let mut late = HookEvents::new(1_000_000 + CLOCK_SKEW_MS + 60);
        late.own("call_a", json!([]));
        let rows = newest_first(100, "call_a");
        let mut fetches = 0;
        let lines = poll(&mut late, &rows, &mut fetches).await;
        assert_eq!(fetches, 1, "receipts before the run end paging");
        assert_eq!(lines.len(), 40);
        assert_eq!(lines[0]["receipt_id"], "hke_0060");
    }

    #[test]
    fn seen_and_owned_calls_stay_bounded() {
        let mut events = HookEvents::new(0);
        for n in 0..MAX_CALLS + 10 {
            events.own(&format!("call_{n}"), json!([]));
        }
        assert_eq!(events.calls.len(), MAX_CALLS);
        assert!(!events.owns("call_0") && events.owns(&format!("call_{MAX_CALLS}")));
        for n in 0..MAX_SEEN + 10 {
            events.seen.insert(&format!("hke_{n}"), MAX_SEEN);
        }
        assert_eq!(events.seen.set.len(), MAX_SEEN);
        assert!(!events.seen.contains("hke_0"));
    }

    #[test]
    fn cursors_are_query_safe() {
        assert!(valid_cursor("1:ses_a:hke_b"));
        assert!(!valid_cursor("1:ses_a&limit=500"));
        assert!(!valid_cursor(""));
    }
}
