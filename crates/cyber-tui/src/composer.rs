//! The multiline prompt editor (`tui` → Prompt editor).

/// Text with a cursor, as lines of chars.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Composer {
    lines: Vec<Vec<char>>,
    row: usize,
    col: usize,
    history: Vec<String>,
    /// Position while browsing history; `None` when editing the draft.
    browsing: Option<usize>,
    stash: String,
}

impl Composer {
    pub fn new(history: Vec<String>) -> Self {
        Self {
            lines: vec![Vec::new()],
            history,
            ..Self::default()
        }
    }

    pub fn text(&self) -> String {
        self.lines
            .iter()
            .map(|l| l.iter().collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn set_text(&mut self, text: &str) {
        self.lines = text.split('\n').map(|l| l.chars().collect()).collect();
        self.row = self.lines.len() - 1;
        self.col = self.lines[self.row].len();
    }

    pub fn is_empty(&self) -> bool {
        self.lines.len() == 1 && self.lines[0].is_empty()
    }

    pub fn lines(&self) -> Vec<String> {
        self.lines.iter().map(|l| l.iter().collect()).collect()
    }

    pub fn cursor(&self) -> (usize, usize) {
        (self.row, self.col)
    }

    pub fn insert(&mut self, c: char) {
        if c == '\n' {
            return self.newline();
        }
        self.lines[self.row].insert(self.col, c);
        self.col += 1;
        self.browsing = None;
    }

    pub fn insert_str(&mut self, s: &str) {
        for c in s.chars() {
            self.insert(if c == '\r' { '\n' } else { c });
        }
    }

    pub fn newline(&mut self) {
        let rest = self.lines[self.row].split_off(self.col);
        self.row += 1;
        self.lines.insert(self.row, rest);
        self.col = 0;
    }

    pub fn backspace(&mut self) {
        if self.col > 0 {
            self.col -= 1;
            self.lines[self.row].remove(self.col);
        } else if self.row > 0 {
            let line = self.lines.remove(self.row);
            self.row -= 1;
            self.col = self.lines[self.row].len();
            self.lines[self.row].extend(line);
        }
    }

    pub fn delete(&mut self) {
        if self.col < self.lines[self.row].len() {
            self.lines[self.row].remove(self.col);
        } else if self.row + 1 < self.lines.len() {
            let next = self.lines.remove(self.row + 1);
            self.lines[self.row].extend(next);
        }
    }

    pub fn left(&mut self) {
        if self.col > 0 {
            self.col -= 1;
        } else if self.row > 0 {
            self.row -= 1;
            self.col = self.lines[self.row].len();
        }
    }

    pub fn right(&mut self) {
        if self.col < self.lines[self.row].len() {
            self.col += 1;
        } else if self.row + 1 < self.lines.len() {
            self.row += 1;
            self.col = 0;
        }
    }

    pub fn home(&mut self) {
        self.col = 0;
    }

    pub fn end(&mut self) {
        self.col = self.lines[self.row].len();
    }

    /// Delete the word before the cursor.
    pub fn delete_word(&mut self) {
        while self.col > 0 && self.lines[self.row][self.col - 1] == ' ' {
            self.backspace();
        }
        while self.col > 0 && self.lines[self.row][self.col - 1] != ' ' {
            self.backspace();
        }
    }

    pub fn clear(&mut self) {
        *self = Self::new(std::mem::take(&mut self.history));
    }

    /// Up: move within the text, or to older history from the first line.
    pub fn up(&mut self) {
        if self.row > 0 {
            self.row -= 1;
            self.col = self.col.min(self.lines[self.row].len());
            return;
        }
        let next = match self.browsing {
            None if !self.history.is_empty() => {
                self.stash = self.text();
                self.history.len() - 1
            }
            Some(i) if i > 0 => i - 1,
            _ => return,
        };
        self.browsing = Some(next);
        let text = self.history[next].clone();
        self.set_text(&text);
    }

    /// Down: move within the text, or to newer history from the last line.
    pub fn down(&mut self) {
        if self.row + 1 < self.lines.len() {
            self.row += 1;
            self.col = self.col.min(self.lines[self.row].len());
            return;
        }
        let Some(i) = self.browsing else { return };
        if i + 1 < self.history.len() {
            self.browsing = Some(i + 1);
            let text = self.history[i + 1].clone();
            self.set_text(&text);
        } else {
            self.browsing = None;
            let stash = std::mem::take(&mut self.stash);
            self.set_text(&stash);
        }
    }

    /// Submit: return the text, record it in history and clear.
    pub fn take(&mut self) -> String {
        let text = self.text();
        if !text.trim().is_empty() && self.history.last() != Some(&text) {
            self.history.push(text.clone());
        }
        let history = std::mem::take(&mut self.history);
        *self = Self::new(history);
        text
    }

    /// The `@word` or `/word` being typed at the cursor, with its start column.
    pub fn trigger(&self) -> Option<(char, usize, String)> {
        let line = &self.lines[self.row];
        let start = line[..self.col]
            .iter()
            .rposition(|c| c.is_whitespace())
            .map_or(0, |i| i + 1);
        let word: String = line[start..self.col].iter().collect();
        let first = word.chars().next()?;
        let slash_ok = first == '/' && self.row == 0 && start == 0;
        (first == '@' || slash_ok).then(|| (first, start, word[1..].to_string()))
    }

    /// Replace the trigger word with `replacement`.
    pub fn complete(&mut self, start: usize, replacement: &str) {
        let line = &mut self.lines[self.row];
        line.splice(start..self.col, replacement.chars());
        self.col = start + replacement.chars().count();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editing_and_newlines() {
        let mut c = Composer::new(Vec::new());
        c.insert_str("hello");
        c.newline();
        c.insert_str("world");
        assert_eq!(c.text(), "hello\nworld");
        c.home();
        c.backspace();
        assert_eq!(c.text(), "helloworld");
        c.delete_word();
        assert_eq!(c.text(), "world");
    }

    #[test]
    fn history_browsing_restores_the_draft() {
        let mut c = Composer::new(vec!["first".into(), "second".into()]);
        c.insert_str("draft");
        c.up();
        assert_eq!(c.text(), "second");
        c.up();
        assert_eq!(c.text(), "first");
        c.down();
        c.down();
        assert_eq!(c.text(), "draft");
        assert_eq!(c.take(), "draft");
        c.up();
        assert_eq!(c.text(), "draft");
    }

    #[test]
    fn triggers_and_completion() {
        let mut c = Composer::new(Vec::new());
        c.insert_str("look at @src/ma");
        assert_eq!(c.trigger(), Some(('@', 8, "src/ma".into())));
        c.complete(8, "@src/main.rs ");
        assert_eq!(c.text(), "look at @src/main.rs ");
        let mut s = Composer::new(Vec::new());
        s.insert_str("/mod");
        assert_eq!(s.trigger(), Some(('/', 0, "mod".into())));
        let mut mid = Composer::new(Vec::new());
        mid.insert_str("a /mod");
        assert_eq!(mid.trigger(), None, "slash commands start the prompt");
    }
}
