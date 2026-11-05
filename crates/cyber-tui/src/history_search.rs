//! Incremental search over the prompt history (`tui` → Prompt history search).
//!
//! The draft stays in the composer while searching; the view shows the selected
//! entry instead, so cancelling, quitting or switching Session keeps the draft.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::{Action, App};

impl App {
    /// `composer.history_search`: open the search with an empty query.
    pub(crate) fn open_history_search(&mut self) {
        self.history_search = Some(HistorySearch::new(self.composer.history()));
    }

    /// Keys while the search is open; every key is taken.
    pub(crate) fn history_search_key(&mut self, key: KeyEvent) -> Vec<Action> {
        let Some(search) = self.history_search.as_mut() else {
            return Vec::new();
        };
        let history = self.composer.history();
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if self.keymap.action("composer", &key, false) == Some("history_search") {
            search.older();
            return Vec::new();
        }
        match key.code {
            KeyCode::Esc => self.history_search = None,
            KeyCode::Char('g' | 'c') if ctrl => self.history_search = None,
            KeyCode::Enter | KeyCode::Tab | KeyCode::Left | KeyCode::Right => {
                let chosen = search.current(history).map(str::to_string);
                self.history_search = None;
                if let Some(text) = chosen {
                    self.composer.recall(&text);
                }
            }
            KeyCode::Up => search.older(),
            KeyCode::Down => search.newer(),
            KeyCode::Backspace => search.pop(history),
            KeyCode::Char(c) if !ctrl && !key.modifiers.contains(KeyModifiers::ALT) => {
                search.push(c, history);
            }
            _ => {}
        }
        Vec::new()
    }
}

/// An open search: the query and the matching history indices, newest first.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HistorySearch {
    pub query: String,
    matches: Vec<usize>,
    selected: usize,
}

impl HistorySearch {
    pub fn new(history: &[String]) -> Self {
        let mut search = Self::default();
        search.refresh(history);
        search
    }

    pub fn push(&mut self, c: char, history: &[String]) {
        self.query.push(c);
        self.refresh(history);
    }

    pub fn pop(&mut self, history: &[String]) {
        self.query.pop();
        self.refresh(history);
    }

    /// Select the next older match; past the oldest the selection stays.
    pub fn older(&mut self) {
        if self.selected + 1 < self.matches.len() {
            self.selected += 1;
        }
    }

    /// Select the next newer match; past the newest the selection stays.
    pub fn newer(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    /// The selected entry, or `None` when nothing matches.
    pub fn current<'a>(&self, history: &'a [String]) -> Option<&'a str> {
        let index = *self.matches.get(self.selected)?;
        history.get(index).map(String::as_str)
    }

    /// `(N, M)`: the selected match, counted from the newest, and how many match.
    pub fn position(&self) -> Option<(usize, usize)> {
        (!self.matches.is_empty()).then(|| (self.selected + 1, self.matches.len()))
    }

    fn refresh(&mut self, history: &[String]) {
        self.matches = matches(history, &self.query);
        self.selected = 0;
    }
}

/// Indices of the entries containing `query`, newest first, each text listed once.
fn matches(history: &[String], query: &str) -> Vec<usize> {
    let mut seen = std::collections::HashSet::new();
    (0..history.len())
        .rev()
        .filter(|&i| seen.insert(history[i].as_str()))
        .filter(|&i| query.is_empty() || !marks(&history[i], query).is_empty())
        .collect()
}

/// Byte ranges of every occurrence of `query` in `text`, without overlaps. Case is
/// ignored unless the query has an uppercase letter (smart case).
pub fn marks(text: &str, query: &str) -> Vec<std::ops::Range<usize>> {
    if query.is_empty() {
        return Vec::new();
    }
    let fold = !query.chars().any(char::is_uppercase);
    let mut ranges = Vec::new();
    let mut from = 0;
    while from < text.len() {
        match match_at(&text[from..], query, fold) {
            Some(len) => {
                ranges.push(from..from + len);
                from += len;
            }
            None => from += text[from..].chars().next().map_or(1, char::len_utf8),
        }
    }
    ranges
}

/// The byte length of `query` matched at the start of `text`, if it matches there.
fn match_at(text: &str, query: &str, fold: bool) -> Option<usize> {
    let mut chars = text.char_indices();
    for q in query.chars() {
        let (_, c) = chars.next()?;
        let same = if fold {
            c.to_lowercase().eq(q.to_lowercase())
        } else {
            c == q
        };
        if !same {
            return None;
        }
    }
    Some(chars.next().map_or(text.len(), |(at, _)| at))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn history(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn matches_newest_first_and_moves_both_ways() {
        let h = history(&["run the tests", "fix the login bug", "run the linter"]);
        let mut s = HistorySearch::new(&h);
        assert_eq!(s.position(), Some((1, 3)));
        for c in "run".chars() {
            s.push(c, &h);
        }
        assert_eq!(s.current(&h), Some("run the linter"));
        assert_eq!(s.position(), Some((1, 2)));
        s.older();
        assert_eq!(s.current(&h), Some("run the tests"));
        s.older();
        assert_eq!(s.current(&h), Some("run the tests"));
        s.newer();
        s.newer();
        assert_eq!(s.current(&h), Some("run the linter"));
    }

    #[test]
    fn duplicates_are_listed_once_and_typing_reselects_the_newest() {
        let h = history(&["deploy", "build", "deploy"]);
        let mut s = HistorySearch::new(&h);
        assert_eq!(s.position(), Some((1, 2)));
        s.older();
        s.push('p', &h);
        assert_eq!(s.position(), Some((1, 1)));
        s.pop(&h);
        assert_eq!(s.current(&h), Some("deploy"));
    }

    #[test]
    fn no_match_has_no_entry() {
        let h = history(&["one"]);
        let mut s = HistorySearch::new(&h);
        s.push('z', &h);
        assert_eq!((s.current(&h), s.position()), (None, None));
        assert_eq!(HistorySearch::new(&[]).position(), None);
    }

    #[test]
    fn smart_case() {
        let h = history(&["Deploy staging", "deploy prod"]);
        let mut s = HistorySearch::new(&h);
        for c in "deploy".chars() {
            s.push(c, &h);
        }
        assert_eq!(s.position(), Some((1, 2)));
        s.query.clear();
        for c in "Deploy".chars() {
            s.push(c, &h);
        }
        assert_eq!(s.position(), Some((1, 1)));
        assert_eq!(s.current(&h), Some("Deploy staging"));
    }

    #[test]
    fn marks_every_occurrence_by_byte_range() {
        assert_eq!(marks("run, RUN, run", "run"), vec![0..3, 5..8, 10..13]);
        assert_eq!(marks("ÉCOLE école", "école"), vec![0..6, 7..13]);
        assert_eq!(marks("aaaa", "aa"), vec![0..2, 2..4]);
        assert!(marks("abc", "").is_empty());
    }
}
