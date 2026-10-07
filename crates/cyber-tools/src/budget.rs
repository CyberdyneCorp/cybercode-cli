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
        let lines = text.lines().count();
        if lines <= self.max_lines && text.len() <= self.max_bytes {
            return Ok(text);
        }
        let path = self.store(&text)?;
        let kept = if keep_tail {
            tail(&text, self.max_lines, self.max_bytes)
        } else {
            head(&text, self.max_lines, self.max_bytes)
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
}
