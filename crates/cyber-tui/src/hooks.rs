//! Committed hook observations; this viewer never reconciles or replays commands.
use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Clone, Deserialize, PartialEq)]
struct Receipt {
    id: String,
    session_id: String,
    hook_id: String,
    event: String,
    status: String,
    started_ms: i64,
    duration_ms: Option<u64>,
    outcome: Option<String>,
    acknowledged: Option<bool>,
    must_stop: bool,
    call_id: Option<String>,
    tool_name: Option<String>,
}
#[derive(Debug, Clone, Deserialize, PartialEq)]
struct Cursor {
    next: Option<String>,
}
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Page {
    data: Vec<Receipt>,
    cursor: Cursor,
}
impl Page {
    pub fn parse(value: &Value, session: &str) -> Result<Self, String> {
        let page: Self =
            Self::deserialize(value).map_err(|_| "Malformed hook receipt page".to_string())?;
        if page.data.len() > 50
            || page.data.iter().any(|row| {
                row.session_id != session
                    || row.id.is_empty()
                    || row.started_ms < 0
                    || !matches!(row.status.as_str(), "running" | "completed" | "unknown")
            })
        {
            return Err("Invalid or foreign hook receipt page".into());
        }
        Ok(page)
    }
    fn lines(&self) -> Vec<String> {
        if self.data.is_empty() {
            return vec!["No recorded hook executions.".into()];
        }
        self.data
            .iter()
            .flat_map(|row| {
                let status = match row.status.as_str() {
                    "running" => "running · live state unverified",
                    "unknown" => "unknown · recovery required",
                    _ => "completed",
                };
                vec![
                    format!("{} · {}", status, safe(&row.id)),
                    format!("  hook={}", safe(&row.hook_id)),
                    format!(
                        "  {} · {} · started={} · duration_ms={}",
                        safe(&row.event),
                        row.outcome
                            .as_deref()
                            .map(safe)
                            .unwrap_or_else(|| "pending".into()),
                        row.started_ms,
                        row.duration_ms
                            .map_or("pending".into(), |ms| ms.to_string())
                    ),
                    format!(
                        "  call={} tool={}",
                        row.call_id
                            .as_deref()
                            .map(safe)
                            .unwrap_or_else(|| "none".into()),
                        row.tool_name
                            .as_deref()
                            .map(safe)
                            .unwrap_or_else(|| "none".into())
                    ),
                    format!(
                        "  Stop acknowledged: {} · must stop: {}",
                        row.acknowledged.map_or("unverified", flag),
                        flag(row.must_stop)
                    ),
                    String::new(),
                ]
            })
            .collect()
    }
}
fn flag(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}
fn safe(value: &str) -> String {
    value.escape_debug().to_string()
}
#[derive(Debug, Default)]
pub struct View {
    generation: u64,
    pending: bool,
    result: Option<Result<Page, String>>,
    pub scroll: u16,
}
impl View {
    pub fn load(&mut self) -> u64 {
        self.generation = self.generation.wrapping_add(1);
        self.pending = true;
        self.result = None;
        self.scroll = 0;
        self.generation
    }
    pub fn invalidate(&mut self) {
        self.load();
        self.pending = false;
    }
    pub fn apply(&mut self, generation: u64, result: Result<Page, String>) {
        if generation != self.generation || !self.pending {
            return;
        }
        self.pending = false;
        self.result = Some(result);
    }
    pub fn next(&self) -> Option<String> {
        self.result.as_ref()?.as_ref().ok()?.cursor.next.clone()
    }
    pub fn lines(&self) -> Vec<String> {
        let mut lines = vec!["Recorded hook execution observations".into(), String::new()];
        match &self.result {
            Some(Ok(page)) => lines.extend(page.lines()),
            Some(Err(error)) => lines.push(format!("Could not load receipts: {}", safe(error))),
            None => lines.push("Loading hook receipts…".into()),
        }
        lines.push(format!(
            "R refresh first page · {} · ↑/↓ scroll · Esc close",
            if self.next().is_some() {
                "N next page"
            } else {
                "No further page"
            }
        ));
        lines
    }
}
