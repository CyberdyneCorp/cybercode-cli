//! The Session recap line (`tui` → Session recap line): `※ recap: <text>` at the top of
//! the transcript, from `POST /sessions/:id/recap`. With `tui.recap: auto` it is asked
//! for when an old Session is opened and when the terminal regains focus after a long
//! absence; `/recap` asks at once whatever the setting.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use serde_json::Value;

use crate::app::{Action, App};
use crate::model::Item;

/// The idle time, or time away, after which a recap is asked for.
pub const RETURN_MS: i64 = 30 * 60 * 1000;
/// The most rows the text wraps to.
const MAX_ROWS: usize = 2;
/// The longest text kept from the server.
const MAX_CHARS: usize = 400;
const PREFIX: &str = "※ recap: ";

/// A recap from the server.
#[derive(Debug, Clone, PartialEq)]
pub struct Recap {
    pub text: String,
    pub through_seq: i64,
    pub generated_ms: i64,
}

impl Recap {
    /// `{text, through_seq, generated_ms}`, with control characters removed.
    pub fn parse(value: &Value) -> Option<Self> {
        let text: String = crate::sidebar::clean(value["text"].as_str()?)
            .chars()
            .take(MAX_CHARS)
            .collect();
        let text = text.trim().to_owned();
        (!text.is_empty()).then(|| Self {
            text,
            through_seq: value["through_seq"].as_i64().unwrap_or(0),
            generated_ms: value["generated_ms"].as_i64().unwrap_or(0),
        })
    }
}

#[derive(Debug, Default)]
pub struct RecapState {
    /// The recap being shown.
    pub shown: Option<Recap>,
    /// The latest recap received, shown or not.
    pub last: Option<Recap>,
    /// The Session whose open was already considered.
    pub(crate) opened: Option<String>,
    /// When the terminal lost focus.
    pub(crate) away_since: Option<i64>,
    /// A fixed wall clock (ms since the epoch) for tests.
    pub clock_ms: Option<i64>,
}

impl App {
    fn wall_ms(&self) -> i64 {
        self.recap.clock_ms.unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_millis() as i64)
        })
    }

    fn recap_auto(&self) -> bool {
        self.prefs.get("recap") != "off"
    }

    /// The time of the latest finished run, if any.
    fn last_finished_ms(&self) -> Option<i64> {
        self.items
            .iter()
            .filter_map(|item| match item {
                Item::Assistant { step, .. } => step.ended_ms,
                _ => None,
            })
            .max()
    }

    fn ask_recap(&self, manual: bool) -> Vec<Action> {
        vec![Action::Recap {
            session_id: self.session.id.clone(),
            manual,
        }]
    }

    /// After a Session's messages load: ask once per Session when its last activity is
    /// at least [`RETURN_MS`] old.
    pub(crate) fn recap_on_open(&mut self) -> Vec<Action> {
        if self.recap.opened.as_deref() == Some(self.session.id.as_str()) {
            return Vec::new();
        }
        self.recap.opened = Some(self.session.id.clone());
        self.recap.shown = None;
        self.recap.last = None;
        let old = self
            .last_finished_ms()
            .is_some_and(|last| self.wall_ms() - last >= RETURN_MS);
        if self.recap_auto() && old && !self.session.running {
            self.ask_recap(false)
        } else {
            Vec::new()
        }
    }

    pub(crate) fn recap_focus_lost(&mut self) {
        self.recap.away_since = Some(self.wall_ms());
    }

    /// Back after at least [`RETURN_MS`] away, with a run finished since the last recap.
    pub(crate) fn recap_focus_gained(&mut self) -> Vec<Action> {
        let Some(away) = self.recap.away_since.take() else {
            return Vec::new();
        };
        let since = self.recap.last.as_ref().map_or(0, |r| r.generated_ms);
        let active = self.last_finished_ms().is_some_and(|last| last > since);
        if self.recap_auto() && active && self.wall_ms() - away >= RETURN_MS {
            self.ask_recap(false)
        } else {
            Vec::new()
        }
    }

    /// `/recap`.
    pub(crate) fn recap_command(&mut self) -> Vec<Action> {
        self.ask_recap(true)
    }

    /// The server's answer; a failure shows nothing, except a toast for `/recap`.
    pub(crate) fn apply_recap(
        &mut self,
        session_id: &str,
        manual: bool,
        result: Result<Value, String>,
    ) {
        if session_id != self.session.id {
            return;
        }
        match result.map(|value| Recap::parse(&value)) {
            Ok(Some(recap)) => {
                self.recap.last = Some(recap.clone());
                self.recap.shown = Some(recap);
            }
            Ok(None) if manual => self.toast("recap: the model returned no recap"),
            Err(error) if manual => self.toast(format!("recap: {error}")),
            _ => {}
        }
    }

    /// The next prompt hides the recap, and Esc dismisses it.
    pub(crate) fn hide_recap(&mut self) -> bool {
        self.recap.shown.take().is_some()
    }
}

/// The block's rows at `width`: the wrapped text (at most two rows, ending with `…` when
/// cut) and the rule; empty when no recap is shown.
pub fn rows(app: &App, width: usize) -> Vec<Line<'static>> {
    let Some(recap) = &app.recap.shown else {
        return Vec::new();
    };
    let t = &app.theme;
    let text_style = Style::default()
        .fg(t.text_muted)
        .add_modifier(Modifier::ITALIC);
    let width = width.max(PREFIX.chars().count() + 4);
    let mut rows: Vec<String> = Vec::new();
    let mut row = PREFIX.to_owned();
    let mut cut = false;
    for word in recap.text.split_whitespace() {
        let fits = row.chars().count() + 1 + word.chars().count() <= width;
        if !fits && row.chars().count() > PREFIX.chars().count() {
            if rows.len() + 1 == MAX_ROWS {
                cut = true;
                break;
            }
            rows.push(std::mem::replace(
                &mut row,
                " ".repeat(PREFIX.chars().count()),
            ));
        }
        if !row.ends_with(' ') {
            row.push(' ');
        }
        row.push_str(word);
    }
    if cut {
        let keep: String = row.chars().take(width.saturating_sub(1)).collect();
        row = format!("{}…", keep.trim_end());
    }
    rows.push(row);
    let mut out: Vec<Line<'static>> = Vec::new();
    for (i, row) in rows.into_iter().enumerate() {
        let row: String = row.chars().take(width).collect();
        if i == 0 {
            let rest: String = row.chars().skip(PREFIX.chars().count()).collect();
            out.push(Line::from(vec![
                Span::styled(
                    PREFIX.trim_end().to_owned(),
                    text_style.add_modifier(Modifier::BOLD),
                ),
                Span::styled(format!(" {}", rest.trim_start()), text_style),
            ]));
        } else {
            out.push(Line::from(Span::styled(row, text_style)));
        }
    }
    out.push(Line::from(Span::styled(
        "─".repeat(width),
        Style::default().fg(t.border_active),
    )));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn app_with(text: &str) -> App {
        let mut app = App::new(crate::tests::session(false), Vec::new(), "cyber");
        app.apply_recap(
            &app.session.id.clone(),
            false,
            Ok(json!({ "text": text, "through_seq": 3, "generated_ms": 1 })),
        );
        app
    }

    fn text(app: &App, width: usize) -> Vec<String> {
        rows(app, width).iter().map(ToString::to_string).collect()
    }

    #[test]
    fn wraps_to_two_rows_with_an_ellipsis_and_a_rule() {
        let app = app_with(&"word ".repeat(60));
        let rows = text(&app, 40);
        assert_eq!(rows.len(), 3, "{rows:?}");
        assert!(rows[0].starts_with("※ recap: word"));
        assert!(rows[1].ends_with('…'), "{rows:?}");
        assert!(rows.iter().all(|r| r.chars().count() <= 40));
        assert_eq!(rows[2], "─".repeat(40));
        let short = text(&app_with("I fixed it."), 40);
        assert_eq!(short, ["※ recap: I fixed it.", &"─".repeat(40)]);
    }

    #[test]
    fn control_characters_are_removed_and_failures_are_quiet() {
        let app = app_with("done\u{1b}]52;c;aGk=\u{7}");
        assert!(
            text(&app, 60)
                .iter()
                .all(|r| !r.contains('\u{1b}') && !r.contains('\u{7}'))
        );
        let mut app = App::new(crate::tests::session(false), Vec::new(), "cyber");
        let id = app.session.id.clone();
        app.apply_recap(&id, false, Err("boom".into()));
        assert!(app.toast.is_none());
        app.apply_recap(&id, true, Err("boom".into()));
        assert!(app.toast.as_ref().unwrap().0.contains("recap: boom"));
        app.apply_recap("other", true, Ok(json!({ "text": "x" })));
        assert!(app.recap.shown.is_none());
    }

    fn finished(app: &mut App, ended_ms: i64) {
        app.items.push(Item::Assistant {
            id: format!("msg_{ended_ms}"),
            text: "Done.".into(),
            reasoning: String::new(),
            tools: Vec::new(),
            error: None,
            step: crate::model::Step {
                ended_ms: Some(ended_ms),
                ..Default::default()
            },
        });
    }

    #[test]
    fn focus_return_needs_thirty_minutes_away_and_activity_since_the_last_recap() {
        let mut app = App::new(crate::tests::session(false), Vec::new(), "cyber");
        let id = app.session.id.clone();
        finished(&mut app, 1_000);
        app.recap.clock_ms = Some(2_000);
        app.recap_focus_lost();
        app.recap.clock_ms = Some(2_000 + RETURN_MS - 1);
        assert!(app.recap_focus_gained().is_empty(), "29:59 away");
        app.recap_focus_lost();
        app.recap.clock_ms = Some(2_000 + RETURN_MS - 1 + RETURN_MS);
        let asked = app.recap_focus_gained();
        assert_eq!(
            asked,
            [Action::Recap {
                session_id: id.clone(),
                manual: false
            }]
        );
        // A recap newer than the last run: no activity since, so no request.
        app.apply_recap(
            &id,
            false,
            Ok(json!({ "text": "I did it.", "through_seq": 9, "generated_ms": 5_000 })),
        );
        app.recap_focus_lost();
        app.recap.clock_ms = Some(10 * RETURN_MS);
        assert!(app.recap_focus_gained().is_empty());
        finished(&mut app, 6_000);
        app.recap_focus_lost();
        app.recap.clock_ms = Some(20 * RETURN_MS);
        assert_eq!(app.recap_focus_gained().len(), 1);
        // Gaining focus without having lost it asks nothing.
        assert!(app.recap_focus_gained().is_empty());
    }
}
