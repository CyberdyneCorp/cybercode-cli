//! Model-facing output budget and managed overflow files
//! (`tool-registry` → Output size budget, Managed tool output files).

use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Budget {
    pub max_lines: usize,
    pub max_bytes: usize,
    pub dir: PathBuf,
}

impl Budget {
    /// Return `text` when it fits; otherwise keep the head (or the tail) within both limits,
    /// write the full text to an exclusively created managed file, and say where it is.
    /// Failure to write the file fails the call rather than returning lossy output.
    pub fn apply(&self, text: String, keep_tail: bool) -> Result<String, String> {
        self.apply_with_token_limit(text, keep_tail, None)
    }

    pub(crate) fn apply_with_token_limit(
        &self,
        text: String,
        keep_tail: bool,
        token_limit: Option<usize>,
    ) -> Result<String, String> {
        let char_limit = token_limit.map(|limit| limit.saturating_mul(4));
        let lines = text.lines().count();
        if lines <= self.max_lines
            && text.len() <= self.max_bytes
            && char_limit.is_none_or(|limit| text.chars().count() <= limit)
        {
            return Ok(text);
        }
        let path = self.store(&text)?;
        let kept = if keep_tail {
            tail(&text, self.max_lines, self.max_bytes)
        } else {
            head(&text, self.max_lines, self.max_bytes)
        };
        let kept = match char_limit {
            Some(limit) if keep_tail => {
                let skip = kept.chars().count().saturating_sub(limit);
                kept.chars().skip(skip).collect::<String>()
            }
            Some(limit) => kept.chars().take(limit).collect(),
            None => kept,
        };
        let omitted_lines = lines.saturating_sub(kept.lines().count());
        let omitted_bytes = text.len() - kept.len();
        Ok(format!(
            "{kept}\n[output truncated: {omitted_lines} lines / {omitted_bytes} bytes omitted; full output at {}]",
            path.display()
        ))
    }

    pub(crate) fn store(&self, text: &str) -> Result<PathBuf, String> {
        std::fs::create_dir_all(&self.dir)
            .map_err(|e| format!("could not store the full output: {e}"))?;
        let path = self.dir.join(format!(
            "tool_{}",
            cyber_core::ids::new_id("out").trim_start_matches("out_")
        ));
        write_new(&path, text.as_bytes())
            .map_err(|e| format!("could not store the full output: {e}"))?;
        Ok(path)
    }
}

fn write_new(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

fn head(text: &str, max_lines: usize, max_bytes: usize) -> String {
    let mut out = String::new();
    for line in text.lines().take(max_lines) {
        if out.len() + line.len() + 1 > max_bytes {
            break;
        }
        out.push_str(line);
        out.push('\n');
    }
    out.truncate(out.trim_end_matches('\n').len());
    out
}

fn tail(text: &str, max_lines: usize, max_bytes: usize) -> String {
    let mut kept: Vec<&str> = Vec::new();
    let mut size = 0;
    for line in text.lines().rev().take(max_lines) {
        if size + line.len() + 1 > max_bytes {
            break;
        }
        size += line.len() + 1;
        kept.push(line);
    }
    kept.reverse();
    kept.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oversized_output_keeps_head_and_links_the_full_file() {
        let dir = tempfile::tempdir().unwrap();
        let budget = Budget {
            max_lines: 3,
            max_bytes: 1000,
            dir: dir.path().into(),
        };
        let text = (1..=10)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let out = budget.apply(text.clone(), false).unwrap();
        assert!(out.starts_with("line 1\nline 2\nline 3\n[output truncated: 7 lines"));
        let path = out
            .rsplit("full output at ")
            .next()
            .unwrap()
            .trim_end_matches(']');
        assert_eq!(std::fs::read_to_string(path).unwrap(), text);
        let tail = budget.apply(text, true).unwrap();
        assert!(tail.starts_with("line 8\nline 9\nline 10\n"));
    }

    #[test]
    fn small_output_is_untouched() {
        let budget = Budget {
            max_lines: 10,
            max_bytes: 100,
            dir: "/nonexistent".into(),
        };
        assert_eq!(budget.apply("ok".into(), false).unwrap(), "ok");
    }
    #[test]
    fn token_cap_counts_unicode_preserves_full_text_and_respects_global_limits() {
        let dir = tempfile::tempdir().unwrap();
        let budget = Budget {
            max_lines: 100,
            max_bytes: 1000,
            dir: dir.path().into(),
        };
        let text = "🦀".repeat(20);
        let output = budget
            .apply_with_token_limit(text.clone(), false, Some(2))
            .unwrap();
        assert!(output.starts_with(&format!("{}\n[output truncated:", "🦀".repeat(8))));
        let path = output
            .rsplit("full output at ")
            .next()
            .unwrap()
            .trim_end_matches(']');
        assert_eq!(std::fs::read_to_string(path).unwrap(), text);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
        assert_eq!(
            budget
                .apply_with_token_limit("12345678".into(), false, Some(2))
                .unwrap(),
            "12345678"
        );
        let global = Budget {
            max_lines: 1,
            ..budget
        };
        let output = global
            .apply_with_token_limit("first\nsecond".into(), false, Some(100))
            .unwrap();
        assert!(output.starts_with("first\n[output truncated:"));
        let broken = Budget {
            dir: dir.path().join("file"),
            ..global
        };
        std::fs::write(&broken.dir, "not a directory").unwrap();
        assert!(broken.apply_with_token_limit(text, false, Some(2)).is_err());
    }
}
