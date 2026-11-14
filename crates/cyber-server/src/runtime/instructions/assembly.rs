//! Render complete labels/notices within a byte budget; never infer delivery from text.
use super::{Bounded, INSTRUCTIONS_FROM, InstructionFile};
use std::collections::BTreeSet;
use std::path::Path;

pub(super) const OPEN: &str = "\n\n<system-reminder>\n";
pub(super) const CLOSE: &str = "\n</system-reminder>";

#[derive(Debug)]
pub(super) struct Block {
    pub file: InstructionFile,
    pub body: Bounded,
}

pub(super) struct Rendered {
    pub text: String,
    pub body: Bounded,
}

/// The exact files whose complete labels occur in this particular delivery.
#[derive(Debug)]
pub(crate) struct Reminder {
    pub text: String,
    pub files: Vec<InstructionFile>,
}

pub(super) fn block(
    path: &Path,
    body: &Bounded,
    max_file: usize,
    budget: usize,
) -> Option<Rendered> {
    let label = format!("{INSTRUCTIONS_FROM}{}\n", path.display());
    let mut body = body.clone();
    loop {
        let notice = notice(path, body.omitted, max_file, body.limit == max_file);
        let room = budget.checked_sub(label.len().saturating_add(notice.len()))?;
        if body.text.len() <= room {
            let text = if body.omitted == 0 {
                body.text.trim_end()
            } else {
                &body.text
            };
            return Some(Rendered {
                text: format!("{label}{text}{notice}"),
                body,
            });
        }
        let mut end = room;
        while !body.text.is_char_boundary(end) {
            end -= 1;
        }
        body.omitted = body.omitted.saturating_add(body.text.len() - end);
        body.text.truncate(end);
        body.limit = body.limit.min(room);
    }
}

fn notice(path: &Path, omitted: usize, max: usize, per_file: bool) -> String {
    if omitted == 0 {
        return String::new();
    }
    if per_file {
        format!(
            "\n[Truncated: {} exceeds instruction_limits.max_file_bytes ({max} bytes); {omitted} bytes omitted]",
            path.display()
        )
    } else {
        format!(
            "\n[Truncated: instruction files exceed their total budget; {omitted} bytes of {} omitted]",
            path.display()
        )
    }
}

pub(super) fn blocks<'a>(
    blocks: impl Iterator<Item = &'a Block>,
    max_file: usize,
    budget: usize,
) -> (String, Vec<InstructionFile>) {
    let mut text = String::new();
    let mut files = Vec::new();
    for source in blocks {
        let separator = if text.is_empty() { "" } else { "\n\n" };
        let remaining = budget.saturating_sub(text.len().saturating_add(separator.len()));
        if let Some(rendered) = block(&source.file.path, &source.body, max_file, remaining) {
            text.push_str(separator);
            text.push_str(&rendered.text);
            let mut file = source.file.clone();
            file.truncated |= rendered.body.omitted > 0;
            files.push(file);
        }
    }
    (text, files)
}

/// Include only whole path labels. Unlisted paths confer no delivery authority.
pub(super) fn pointers<'a>(
    reason: &str,
    files: impl Iterator<Item = &'a InstructionFile>,
    budget: usize,
) -> Option<Reminder> {
    let mut text = format!("{reason}; read them with the read tool:");
    let mut included = Vec::new();
    for file in files {
        let line = format!("\n- {}", file.path.display());
        if text.len().saturating_add(line.len()) <= budget {
            text.push_str(&line);
            included.push(file.clone());
        }
    }
    (!included.is_empty()).then_some(Reminder {
        text,
        files: included,
    })
}

pub(super) fn finish(text: String, files: Vec<InstructionFile>) -> Option<Reminder> {
    if files.is_empty() {
        return None;
    }
    let mut seen = BTreeSet::new();
    let files = files
        .into_iter()
        .filter(|file| seen.insert(file.identity()))
        .collect();
    Some(Reminder {
        text: format!("{OPEN}{text}{CLOSE}"),
        files,
    })
}

#[cfg(test)]
mod tests {
    use super::super::Instructions;
    use super::*;

    #[test]
    fn labels_notices_and_utf8_body_fit_every_small_budget() {
        let path = Path::new("/repo/日本語/CYBER.md");
        let body = Bounded {
            text: "λ".repeat(800),
            omitted: 500,
            limit: 1600,
            stamp: None,
        };
        for budget in 0..1024 {
            if let Some(rendered) = block(path, &body, 1600, budget) {
                assert!(rendered.text.len() <= budget);
                assert!(
                    rendered
                        .text
                        .starts_with(&format!("{INSTRUCTIONS_FROM}{}\n", path.display()))
                );
                assert!(rendered.text.ends_with("omitted]"));
                assert!(rendered.body.omitted >= body.omitted);
            }
        }
    }

    #[test]
    fn bounded_pointer_prefix_claims_only_complete_labels_and_retries_the_rest() {
        let mut instructions = Instructions::new(256, 256);
        for i in 0..20 {
            let path =
                std::path::PathBuf::from(format!("/repo/rules/{i:04}-{}.md", "é".repeat(20)));
            instructions.omitted.push(InstructionFile {
                canonical: path.clone(),
                path,
                truncated: false,
                rule_binding: None,
                rule_scope: None,
                stamp: None,
            });
        }
        let mut seen = BTreeSet::new();
        for _ in 0..21 {
            let Some(delivery) = instructions.reminder_delivery(&seen) else {
                break;
            };
            assert!(delivery.text.len() <= 256);
            assert!(delivery.text.starts_with(OPEN) && delivery.text.ends_with(CLOSE));
            assert!(delivery.files.len() < 20);
            for file in delivery.files {
                assert!(
                    delivery
                        .text
                        .contains(&format!("\n- {}", file.path.display()))
                );
                assert!(seen.insert(file.canonical.display().to_string()));
            }
        }
        assert_eq!(seen.len(), 20);
    }
}
