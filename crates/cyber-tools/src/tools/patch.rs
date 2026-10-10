//! `apply_patch` (`builtin-tools` → apply_patch tool).

use std::path::PathBuf;

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

#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct Chunk {
    pub context: Option<String>,
    pub old: Vec<String>,
    pub new: Vec<String>,
}

/// Parse the `*** Begin Patch` … `*** End Patch` envelope.
pub(crate) fn parse(patch: &str) -> Result<Vec<Op>, String> {
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
        if let Some(ctx) = &chunk.context
            && let Some(at) = find(&lines, std::slice::from_ref(ctx), cursor)
        {
            cursor = at + 1;
        }
        let at = if chunk.old.is_empty() {
            lines.len()
        } else {
            find(&lines, &chunk.old, cursor)
                .ok_or_else(|| format!("Hunk {} does not match {path}", n + 1))?
        };
        lines.splice(at..at + chunk.old.len(), chunk.new.iter().cloned());
        cursor = at + chunk.new.len();
    }
    let mut out = lines.join("\n");
    out.push('\n');
    Ok(out)
}

/// Exact match first, then ignoring trailing whitespace, then ignoring surrounding whitespace.
pub(crate) fn find(lines: &[String], needle: &[String], from: usize) -> Option<usize> {
    let passes: [fn(&str) -> &str; 3] = [|s| s, str::trim_end, str::trim];
    passes.iter().find_map(|norm| {
        (from..=lines.len().saturating_sub(needle.len())).find(|&i| {
            needle
                .iter()
                .enumerate()
                .all(|(j, n)| lines.get(i + j).is_some_and(|l| norm(l) == norm(n)))
        })
    })
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
    match move_to {
        Some(dest) => {
            let dest = ctx.resolve(&dest);
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
        Box::pin(async move {
            let ops = parse(text(&ctx.inv.input, "patch")).map_err(failed)?;
            let changes = plan(ctx, ops).await?;
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
            ctx.authorize(req, resources, json!({ "diff": diff }))
                .await?;
            write_all(ctx, &changes).await
        })
    }
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
}
