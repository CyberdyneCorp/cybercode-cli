//! `apply_patch` (`builtin-tools` → apply_patch tool).

use std::path::{Path, PathBuf};

use cyber_server::runtime::{RetrySafety, ToolDef};
use futures::future::BoxFuture;
use serde_json::json;

use super::fs::unified_diff;
use super::{Tool, ToolError, def, failed, text};
use crate::host::Ctx;
use crate::permissions::Request;

pub(crate) struct ApplyPatch;

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Op {
    Add {
        path: String,
        content: String,
    },
    Delete {
        path: String,
    },
    Update {
        path: String,
        move_to: Option<String>,
        chunks: Vec<Chunk>,
    },
}

impl Op {
    /// Resolve the operation's relative paths against `base`.
    fn rebase(&mut self, base: &Path) {
        let join = |path: &mut String| {
            if Path::new(path.as_str()).is_relative() {
                *path = base.join(path.as_str()).display().to_string();
            }
        };
        match self {
            Op::Add { path, .. } | Op::Delete { path } => join(path),
            Op::Update { path, move_to, .. } => {
                join(path);
                move_to.iter_mut().for_each(join);
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct Chunk {
    pub context: Option<String>,
    pub old: Vec<String>,
    pub new: Vec<String>,
}

/// Parse the `*** Begin Patch` … `*** End Patch` envelope, also when it comes wrapped
/// in a shell heredoc (`builtin-tools` → Tolerant patch matching).
pub(crate) fn parse(patch: &str) -> Result<Vec<Op>, String> {
    let patch = super::patch_shell::unwrap_heredoc(patch.trim()).unwrap_or(patch);
    let lines: Vec<&str> = patch.trim().lines().collect();
    if lines.first().map(|l| l.trim()) != Some("*** Begin Patch")
        || lines.last().map(|l| l.trim()) != Some("*** End Patch")
    {
        return Err("The patch must start with *** Begin Patch and end with *** End Patch".into());
    }
    let body = &lines[1..lines.len() - 1];
    let mut ops = Vec::new();
    let mut i = 0;
    while i < body.len() {
        let line = body[i];
        i += 1;
        if let Some(path) = line.strip_prefix("*** Add File: ") {
            let (content, next) = take_added(body, i);
            ops.push(Op::Add {
                path: path.trim().into(),
                content,
            });
            i = next;
        } else if let Some(path) = line.strip_prefix("*** Delete File: ") {
            ops.push(Op::Delete {
                path: path.trim().into(),
            });
        } else if let Some(path) = line.strip_prefix("*** Update File: ") {
            let (op, next) = take_update(path.trim(), body, i)?;
            ops.push(op);
            i = next;
        } else if !line.trim().is_empty() {
            return Err(format!("Unexpected line in patch: {line}"));
        }
    }
    if ops.is_empty() {
        return Err("The patch contains no operations".into());
    }
    Ok(ops)
}

fn take_added(body: &[&str], mut i: usize) -> (String, usize) {
    let mut content = Vec::new();
    while i < body.len() && !body[i].starts_with("*** ") {
        content.push(body[i].strip_prefix('+').unwrap_or(body[i]).to_string());
        i += 1;
    }
    let mut text = content.join("\n");
    text.push('\n');
    (text, i)
}

fn take_update(path: &str, body: &[&str], mut i: usize) -> Result<(Op, usize), String> {
    let mut move_to = None;
    if let Some(dest) = body.get(i).and_then(|l| l.strip_prefix("*** Move to: ")) {
        move_to = Some(dest.trim().to_string());
        i += 1;
    }
    let mut chunks: Vec<Chunk> = Vec::new();
    while i < body.len() && !is_op(body[i]) {
        let line = body[i];
        i += 1;
        if let Some(header) = line.strip_prefix("@@") {
            let header = header.trim();
            chunks.push(Chunk {
                context: (!header.is_empty()).then(|| header.to_string()),
                ..Chunk::default()
            });
            continue;
        }
        if line.starts_with("*** End of File") {
            continue;
        }
        if chunks.is_empty() {
            chunks.push(Chunk::default());
        }
        let chunk = chunks.last_mut().expect("chunk exists");
        push_line(chunk, line).map_err(|e| format!("{e} in {path}"))?;
    }
    if chunks.iter().all(|c| c.old.is_empty() && c.new.is_empty()) && move_to.is_none() {
        return Err(format!("Update of {path} has no changes"));
    }
    Ok((
        Op::Update {
            path: path.into(),
            move_to,
            chunks,
        },
        i,
    ))
}

fn is_op(line: &str) -> bool {
    ["*** Add File: ", "*** Delete File: ", "*** Update File: "]
        .iter()
        .any(|p| line.starts_with(p))
}

fn push_line(chunk: &mut Chunk, line: &str) -> Result<(), String> {
    match line.chars().next() {
        Some('+') => chunk.new.push(line[1..].to_string()),
        Some('-') => chunk.old.push(line[1..].to_string()),
        Some(' ') => {
            chunk.old.push(line[1..].to_string());
            chunk.new.push(line[1..].to_string());
        }
        None => {
            chunk.old.push(String::new());
            chunk.new.push(String::new());
        }
        Some(_) => return Err(format!("Invalid patch line {line:?}")),
    }
    Ok(())
}

/// Apply chunks to `content`, locating each after the previous one.
pub(crate) fn apply_chunks(content: &str, chunks: &[Chunk], path: &str) -> Result<String, String> {
    let mut lines: Vec<String> = content.lines().map(str::to_string).collect();
    let mut cursor = 0;
    for (n, chunk) in chunks.iter().enumerate() {
        let hunk_error = |found: Found| match found {
            Found::Ambiguous(count) => format!(
                "Hunk {} is ambiguous in {path}: it matches {count} places once whitespace and punctuation are relaxed; add context lines",
                n + 1
            ),
            _ => format!("Hunk {} does not match {path}", n + 1),
        };
        if let Some(ctx) = &chunk.context {
            match find(&lines, std::slice::from_ref(ctx), cursor) {
                Found::At(at) => cursor = at + 1,
                Found::Missing => {}
                ambiguous => return Err(hunk_error(ambiguous)),
            }
        }
        let at = if chunk.old.is_empty() {
            lines.len()
        } else {
            match find(&lines, &chunk.old, cursor) {
                Found::At(at) => at,
                other => return Err(hunk_error(other)),
            }
        };
        lines.splice(at..at + chunk.old.len(), chunk.new.iter().cloned());
        cursor = at + chunk.new.len();
    }
    let mut out = lines.join("\n");
    out.push('\n');
    Ok(out)
}

/// Where a hunk's lines are in a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Found {
    At(usize),
    /// The loosest level that matched matched this many places.
    Ambiguous(usize),
    Missing,
}

/// How loosely a line may match, tightest first (`builtin-tools` → Tolerant patch
/// matching): exact, trailing whitespace ignored, surrounding whitespace ignored, then
/// typographic punctuation folded as well.
const LEVELS: [fn(&str) -> std::borrow::Cow<'_, str>; 4] = [
    |s| s.into(),
    |s| s.trim_end().into(),
    |s| s.trim().into(),
    |s| fold_punctuation(s.trim()).into(),
];

/// Search from `from`, level by level. An exact match takes the first place, as the
/// cursor already orders the hunks; a looser level must match exactly one place.
pub(crate) fn find(lines: &[String], needle: &[String], from: usize) -> Found {
    for (level, norm) in LEVELS.iter().enumerate() {
        let wanted: Vec<_> = needle.iter().map(|n| norm(n)).collect();
        let mut places = (from..=lines.len().saturating_sub(needle.len())).filter(|&i| {
            wanted
                .iter()
                .enumerate()
                .all(|(j, n)| lines.get(i + j).is_some_and(|l| norm(l) == *n))
        });
        let Some(first) = places.next() else {
            continue;
        };
        let others = if level == 0 { 0 } else { places.count() };
        return match others {
            0 => Found::At(first),
            more => Found::Ambiguous(more + 1),
        };
    }
    Found::Missing
}

/// Curly quotes, dashes and non-breaking or narrow spaces as their ASCII forms.
fn fold_punctuation(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}' | '\u{2032}' => '\'',
            '\u{201C}' | '\u{201D}' | '\u{201E}' | '\u{201F}' | '\u{2033}' => '"',
            '\u{2010}'..='\u{2015}' | '\u{2212}' => '-',
            '\u{00A0}' | '\u{2007}' | '\u{2009}' | '\u{200A}' | '\u{202F}' | '\u{3000}' => ' ',
            other => other,
        })
        .collect()
}

/// One validated file change.
struct Change {
    kind: char,
    path: PathBuf,
    before: Option<Vec<u8>>,
    after: Option<Vec<u8>>,
}

async fn plan(ctx: &Ctx<'_>, ops: Vec<Op>) -> Result<Vec<Change>, ToolError> {
    let mut changes = Vec::new();
    for op in ops {
        match op {
            Op::Add { path, content } => {
                let path = ctx.resolve(&path);
                crate::sandboxing::refuse_hidden(ctx, &[&path]).await?;
                let before = tokio::fs::read(&path).await.ok();
                changes.push(Change {
                    kind: 'A',
                    path,
                    before,
                    after: Some(content.into_bytes()),
                });
            }
            Op::Delete { path } => {
                let path = ctx.resolve(&path);
                crate::sandboxing::refuse_hidden(ctx, &[&path]).await?;
                let before = tokio::fs::read(&path).await.map_err(|_| {
                    failed(format!("Cannot delete missing file {}", path.display()))
                })?;
                changes.push(Change {
                    kind: 'D',
                    path,
                    before: Some(before),
                    after: None,
                });
            }
            Op::Update {
                path,
                move_to,
                chunks,
            } => update(ctx, &path, move_to, &chunks, &mut changes).await?,
        }
    }
    Ok(changes)
}

async fn update(
    ctx: &Ctx<'_>,
    path: &str,
    move_to: Option<String>,
    chunks: &[Chunk],
    out: &mut Vec<Change>,
) -> Result<(), ToolError> {
    let source = ctx.resolve(path);
    let dest = move_to.map(|dest| ctx.resolve(&dest));
    let checked: Vec<&Path> = std::iter::once(source.as_path())
        .chain(dest.as_deref())
        .collect();
    crate::sandboxing::refuse_hidden(ctx, &checked).await?;
    let before = tokio::fs::read(&source)
        .await
        .map_err(|_| failed(format!("Cannot update missing file {}", source.display())))?;
    let text = String::from_utf8(before.clone())
        .map_err(|_| failed(format!("{path} is not UTF-8 text")))?;
    let crlf = text.contains("\r\n");
    let updated = apply_chunks(&text.replace("\r\n", "\n"), chunks, path).map_err(failed)?;
    let updated = if crlf {
        updated.replace('\n', "\r\n")
    } else {
        updated
    };
    match dest {
        Some(dest) => {
            let existing = tokio::fs::read(&dest).await.ok();
            out.push(Change {
                kind: 'D',
                path: source,
                before: Some(before),
                after: None,
            });
            out.push(Change {
                kind: 'A',
                path: dest,
                before: existing,
                after: Some(updated.into_bytes()),
            });
        }
        None => out.push(Change {
            kind: 'M',
            path: source,
            before: Some(before),
            after: Some(updated.into_bytes()),
        }),
    }
    Ok(())
}

impl Tool for ApplyPatch {
    fn def(&self) -> ToolDef {
        def(
            "apply_patch",
            "Apply a patch in the *** Begin Patch / *** End Patch format with Add File, Update File (optional Move to) and Delete File sections.",
            json!({"type": "object", "required": ["patch"], "properties": {"patch": {"type": "string"}}}),
            RetrySafety::Reconcile,
            false,
        )
    }

    fn run<'a>(&'a self, ctx: &'a Ctx<'a>) -> BoxFuture<'a, Result<String, ToolError>> {
        Box::pin(apply(ctx, text(&ctx.inv.input, "patch"), None))
    }
}

/// Validate, approve as one `apply_patch` edit (also when a shell call carried it) and
/// apply `patch`. Relative paths resolve against
/// `base` when given (a shell call's `cd <dir>`), else against the Location.
pub(crate) async fn apply(
    ctx: &Ctx<'_>,
    patch: &str,
    base: Option<&Path>,
) -> Result<String, ToolError> {
    let mut ops = parse(patch).map_err(failed)?;
    if let Some(base) = base {
        ops.iter_mut().for_each(|op| op.rebase(base));
    }
    let changes = plan(ctx, ops).await?;
    refuse_markers(&changes)?;
    let paths: Vec<PathBuf> = changes.iter().map(|c| c.path.clone()).collect();
    ctx.check_external(&paths).await?;
    let resources: Vec<String> = paths.iter().map(|p| ctx.resource(p)).collect();
    let diff: String = changes
        .iter()
        .map(|c| {
            let text = |b: &Option<Vec<u8>>| {
                b.as_deref()
                    .map(String::from_utf8_lossy)
                    .unwrap_or_default()
                    .into_owned()
            };
            unified_diff(&ctx.resource(&c.path), &text(&c.before), &text(&c.after))
        })
        .collect();
    let req = Request {
        action: "edit".into(),
        resources: resources.clone(),
        mutates: paths,
        file_edit: true,
        ..Request::default()
    };
    ctx.authorize_as("apply_patch", req, resources, json!({ "diff": diff }))
        .await?;
    write_all(ctx, &changes).await
}

/// A patch may not add a redaction marker; one moved or kept with its file is no addition.
fn refuse_markers(changes: &[Change]) -> Result<(), ToolError> {
    let text = |bytes: &Option<Vec<u8>>| {
        bytes
            .as_deref()
            .map(String::from_utf8_lossy)
            .unwrap_or_default()
            .into_owned()
    };
    let before: String = changes
        .iter()
        .map(|c| text(&c.before))
        .collect::<Vec<_>>()
        .join("\n");
    let after: String = changes
        .iter()
        .map(|c| text(&c.after))
        .collect::<Vec<_>>()
        .join("\n");
    crate::host::refuse_marker(&before, &after)
}

async fn write_all(ctx: &Ctx<'_>, changes: &[Change]) -> Result<String, ToolError> {
    let mut applied: Vec<String> = Vec::new();
    let mut pending = Vec::new();
    for change in changes {
        let label = format!("{} {}", change.kind, ctx.resource(&change.path));
        let origin = super::intelligence::capture(ctx, &change.path);
        if let Err(e) = write_one(ctx, change).await {
            let message = format!(
                "Patch partially applied before failing at {}: {e}. Applied: {}",
                ctx.resource(&change.path),
                if applied.is_empty() {
                    "nothing".into()
                } else {
                    applied.join(", ")
                }
            );
            let feedback = patch_feedback(ctx, pending).await;
            return Err(failed(format!("{message}{feedback}")));
        }
        pending.push((change, origin));
        ctx.note_skill_path(&change.path);
        applied.push(label);
    }
    let feedback = patch_feedback(ctx, pending).await;
    Ok(format!("{}{feedback}", applied.join("\n")))
}

async fn patch_feedback(
    ctx: &Ctx<'_>,
    pending: Vec<(&Change, Option<crate::lsp::ReadOrigin>)>,
) -> String {
    let results =
        futures::future::join_all(pending.into_iter().map(|(change, origin)| async move {
            match &change.after {
                Some(bytes) => {
                    super::intelligence::feedback(ctx, &change.path, bytes, origin).await
                }
                None => {
                    super::intelligence::removed(ctx, &change.path, origin).await;
                    String::new()
                }
            }
        }))
        .await;
    results.concat()
}

async fn write_one(ctx: &Ctx<'_>, change: &Change) -> Result<(), String> {
    let lock = ctx.host.path_lock(&change.path);
    let _guard = lock.lock().await;
    if tokio::fs::read(&change.path).await.ok() != change.before {
        return Err("the file changed after permission approval".into());
    }
    match &change.after {
        Some(bytes) => {
            if let Some(parent) = change.path.parent() {
                tokio::fs::create_dir_all(parent)
                    .await
                    .map_err(|e| e.to_string())?;
            }
            tokio::fs::write(&change.path, bytes)
                .await
                .map_err(|e| e.to_string())?;
            ctx.host.mark_read(&ctx.inv.session_id, &change.path);
        }
        None => tokio::fs::remove_file(&change.path)
            .await
            .map_err(|e| e.to_string())?,
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_all_operations() {
        let patch = "*** Begin Patch\n*** Add File: new.txt\n+hello\n*** Update File: a.rs\n*** Move to: b.rs\n@@ fn main\n-    old();\n+    new();\n*** Delete File: gone.txt\n*** End Patch";
        let ops = parse(patch).unwrap();
        assert_eq!(
            ops[0],
            Op::Add {
                path: "new.txt".into(),
                content: "hello\n".into()
            }
        );
        assert!(
            matches!(&ops[1], Op::Update { move_to: Some(m), chunks, .. } if m == "b.rs" && chunks[0].context.as_deref() == Some("fn main"))
        );
        assert_eq!(
            ops[2],
            Op::Delete {
                path: "gone.txt".into()
            }
        );
        assert!(parse("*** Update File: x").is_err());
    }

    #[test]
    fn applies_chunks_with_whitespace_fuzz() {
        let content = "fn main() {\n    old();   \n    keep();\n}\n";
        let chunk = Chunk {
            context: Some("fn main() {".into()),
            old: vec!["    old();".into()],
            new: vec!["    new();".into()],
        };
        assert_eq!(
            apply_chunks(content, &[chunk], "a.rs").unwrap(),
            "fn main() {\n    new();\n    keep();\n}\n"
        );
        let bad = Chunk {
            context: None,
            old: vec!["missing".into()],
            new: vec![],
        };
        assert_eq!(
            apply_chunks(content, &[bad], "a.rs").unwrap_err(),
            "Hunk 1 does not match a.rs"
        );
    }

    fn chunk(old: &[&str], new: &[&str]) -> Chunk {
        Chunk {
            context: None,
            old: old.iter().map(|l| l.to_string()).collect(),
            new: new.iter().map(|l| l.to_string()).collect(),
        }
    }

    /// `builtin-tools` → Tolerant patch matching, "Smart quotes in context".
    #[test]
    fn smart_quotes_in_context_match_ascii_quotes() {
        let content = "let a = \"x\";\nlet b = 'y';\nlet c = 1 - 2;\n";
        let hunk = chunk(
            &[
                "let a = \u{201C}x\u{201D};",
                "let b = \u{2018}y\u{2019};",
                "let c = 1 \u{2013} 2;",
            ],
            &["let a = \"z\";"],
        );
        assert_eq!(
            apply_chunks(content, &[hunk], "a.rs").unwrap(),
            "let a = \"z\";\n"
        );
        let spaced = chunk(&["let\u{00A0}b = 'y';"], &["let b = 'w';"]);
        assert_eq!(
            apply_chunks(content, &[spaced], "a.rs").unwrap(),
            "let a = \"x\";\nlet b = 'w';\nlet c = 1 - 2;\n"
        );
    }

    /// "Ambiguous loose match": no exact match and two with whitespace ignored fails,
    /// naming the hunk.
    #[test]
    fn ambiguous_loose_match_fails_naming_the_hunk() {
        let content = "fn a() {\n    call();  \n}\nfn b() {\n    call();\t\n}\n";
        let first = chunk(&["fn a() {"], &["fn a() {"]);
        let loose = chunk(&["    call();"], &["    other();"]);
        let error = apply_chunks(content, &[first, loose], "lib.rs").unwrap_err();
        assert!(
            error.starts_with("Hunk 2 is ambiguous in lib.rs"),
            "{error}"
        );
        assert!(error.contains("2 places"), "{error}");
    }

    /// Trailing whitespace is ignored before surrounding whitespace, and an exact
    /// match keeps taking the first place after the cursor.
    #[test]
    fn trailing_whitespace_and_exact_matches() {
        let content = "x\n  value  \nvalue\nx\n";
        let trailing = chunk(&["  value"], &["  VALUE"]);
        assert_eq!(
            apply_chunks(content, &[trailing], "f").unwrap(),
            "x\n  VALUE\nvalue\nx\n"
        );
        let exact = chunk(&["x"], &["y"]);
        assert_eq!(
            apply_chunks(content, &[exact], "f").unwrap(),
            "y\n  value  \nvalue\nx\n"
        );
        let lines: Vec<String> = content.lines().map(str::to_string).collect();
        assert_eq!(find(&lines, &["missing".into()], 0), Found::Missing);
    }

    /// A patch wrapped in a heredoc, with surrounding whitespace, parses as the bare one.
    #[test]
    fn heredoc_wrapped_envelopes_parse() {
        let bare = "*** Begin Patch\n*** Delete File: gone.txt\n*** End Patch";
        let expected = parse(bare).unwrap();
        for wrapped in [
            format!("\n  <<'EOF'\n{bare}\nEOF\n"),
            format!("apply_patch <<EOF\n{bare}\nEOF"),
            format!("\t{bare}\n\n"),
        ] {
            assert_eq!(parse(&wrapped).unwrap(), expected, "{wrapped}");
        }
    }
}
