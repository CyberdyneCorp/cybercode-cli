use std::collections::BTreeSet;

use cyber_server::runtime::{RetrySafety, ToolDef};
use futures::future::BoxFuture;
use serde::Deserialize;
use serde_json::{Value, json};

use super::{Tool, ToolError, def, failed, fs};
use crate::host::Ctx;

mod document;

pub(crate) struct NotebookEdit;

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Mode {
    Replace,
    Insert,
    Delete,
}

impl Mode {
    fn label(self) -> &'static str {
        match self {
            Self::Replace => "replace",
            Self::Insert => "insert",
            Self::Delete => "delete",
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    path: String,
    mode: Mode,
    cell_id: Option<String>,
    cell_index: Option<usize>,
    new_source: Option<String>,
    cell_type: Option<String>,
}

impl Tool for NotebookEdit {
    fn def(&self) -> ToolDef {
        def(
            "notebook_edit",
            "Replace, insert or delete a Jupyter notebook cell. Cell indices are zero-based. Edited code outputs are cleared; other cells and metadata are preserved.",
            json!({"type":"object","required":["path","mode"],"additionalProperties":false,"properties":{
                "path":{"type":"string","minLength":1},
                "mode":{"type":"string","enum":["replace","insert","delete"]},
                "cell_id":{"type":"string","minLength":1},
                "cell_index":{"type":"integer","minimum":0,"description":"Zero-based index; insert places a cell before this index. Omit both selectors to append."},
                "new_source":{"type":"string","description":"Required for replace and insert."},
                "cell_type":{"type":"string","enum":["code","markdown","raw"]}
            }}),
            RetrySafety::Reconcile,
            false,
        )
    }

    fn run<'a>(&'a self, ctx: &'a Ctx<'a>) -> BoxFuture<'a, Result<String, ToolError>> {
        Box::pin(async move {
            let input: Input = serde_json::from_value(ctx.inv.input.clone())
                .map_err(|_| failed("Invalid notebook_edit parameters"))?;
            let path = ctx.resolve(&input.path);
            if !path
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("ipynb"))
            {
                return Err(failed("notebook_edit requires an .ipynb file"));
            }
            ctx.check_external(std::slice::from_ref(&path)).await?;
            let before = tokio::fs::read(&path)
                .await
                .map_err(|_| failed("Could not read notebook"))?;
            let (document, original) = document::Document::parse(&before)?;
            let mut updated = original.clone();
            let index = mutate(&mut updated, &input)?;
            if updated == original {
                fs::authorize_edit(ctx, &path, String::new()).await?;
                let lock = ctx.host.path_lock(&path);
                let _guard = lock.lock().await;
                if tokio::fs::read(&path).await.ok().as_deref() != Some(before.as_slice()) {
                    return Err(failed("Notebook changed after permission approval"));
                }
                return Ok(format!("Notebook unchanged {}", ctx.resource(&path)));
            }
            let mut after = document.render(&updated, input.mode, index)?;
            after.push('\n');
            let diff = fs::unified_diff(
                &ctx.resource(&path),
                std::str::from_utf8(&before)
                    .map_err(|_| failed("Notebook must contain valid UTF-8 JSON"))?,
                &after,
            );
            let feedback =
                fs::guarded_write(ctx, &path, Some(&before), after.as_bytes(), diff).await?;
            Ok(format!(
                "Edited notebook {} ({} cell {index}){feedback}",
                ctx.resource(&path),
                input.mode.label()
            ))
        })
    }
}

fn mutate(notebook: &mut Value, input: &Input) -> Result<usize, ToolError> {
    let modern = validate(notebook)?;
    let cells = notebook["cells"].as_array_mut().unwrap();
    let index = select(cells, input)?;
    match input.mode {
        Mode::Delete => {
            cells.remove(index);
        }
        Mode::Replace => replace(&mut cells[index], input)?,
        Mode::Insert => {
            let mut cell = json!({"cell_type":input.cell_type.as_deref().unwrap_or("code"),"metadata":{},"source":[]});
            if modern {
                cell["id"] = json!(cyber_core::ids::new_id("cell"));
            }
            replace(&mut cell, input)?;
            cells.insert(index, cell);
        }
    }
    Ok(index)
}

fn select(cells: &[Value], input: &Input) -> Result<usize, ToolError> {
    let by_id = input
        .cell_id
        .as_ref()
        .map(|id| {
            cells
                .iter()
                .position(|cell| cell["id"].as_str() == Some(id.as_str()))
                .ok_or_else(|| failed("Notebook cell_id was not found"))
        })
        .transpose()?;
    if let (Some(index), Some(by_id)) = (input.cell_index, by_id)
        && index != by_id
    {
        return Err(failed("cell_id and cell_index refer to different cells"));
    }
    let index = input
        .cell_index
        .or(by_id)
        .or_else(|| matches!(input.mode, Mode::Insert).then_some(cells.len()))
        .ok_or_else(|| failed("replace and delete require cell_id or cell_index"))?;
    let bound = cells.len() + usize::from(matches!(input.mode, Mode::Insert));
    if index >= bound {
        return Err(failed("Notebook cell_index is out of range"));
    }
    Ok(index)
}

fn replace(cell: &mut Value, input: &Input) -> Result<(), ToolError> {
    let source = input
        .new_source
        .as_deref()
        .ok_or_else(|| failed("replace and insert require new_source"))?;
    let kind = input
        .cell_type
        .as_deref()
        .or_else(|| cell["cell_type"].as_str())
        .unwrap_or("code");
    if !matches!(kind, "code" | "markdown" | "raw") {
        return Err(failed("Unsupported target notebook cell_type"));
    }
    let kind = kind.to_owned();
    let string_source = cell["source"].is_string();
    let cell = cell.as_object_mut().unwrap();
    cell.insert("cell_type".into(), json!(kind));
    cell.insert(
        "source".into(),
        if string_source {
            json!(source)
        } else {
            json!(source.split_inclusive('\n').collect::<Vec<_>>())
        },
    );
    if kind == "code" {
        cell.insert("outputs".into(), json!([]));
        cell.insert("execution_count".into(), Value::Null);
        cell.remove("attachments");
    } else {
        cell.remove("outputs");
        cell.remove("execution_count");
    }
    Ok(())
}

fn validate(notebook: &Value) -> Result<bool, ToolError> {
    if notebook["nbformat"].as_u64() != Some(4) || !notebook["metadata"].is_object() {
        return Err(failed("Notebook must use nbformat 4 with object metadata"));
    }
    let modern = notebook["nbformat_minor"]
        .as_u64()
        .ok_or_else(|| failed("Notebook nbformat_minor must be a nonnegative integer"))?
        >= 5;
    let cells = notebook["cells"]
        .as_array()
        .ok_or_else(|| failed("Notebook cells must be an array"))?;
    let mut ids = BTreeSet::new();
    for cell in cells {
        validate_cell(cell)?;
        if modern && cell.get("id").is_none() {
            return Err(failed("nbformat 4.5 and later require cell IDs"));
        }
        if let Some(id) = cell.get("id") {
            let id = id
                .as_str()
                .filter(|id| valid_id(id))
                .ok_or_else(|| failed("Invalid notebook cell ID"))?;
            if !ids.insert(id) {
                return Err(failed("Duplicate notebook cell IDs"));
            }
        }
    }
    Ok(modern)
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

fn validate_cell(cell: &Value) -> Result<(), ToolError> {
    let source = &cell["source"];
    let text = source.is_string()
        || source
            .as_array()
            .is_some_and(|lines| lines.iter().all(Value::is_string));
    if !cell["cell_type"].is_string() || !cell["metadata"].is_object() || !text {
        return Err(failed(
            "Notebook cells require cell_type, object metadata and text source",
        ));
    }
    if cell["cell_type"] == "code"
        && (!cell["outputs"].is_array()
            || !cell
                .get("execution_count")
                .is_some_and(|count| count.is_null() || count.as_i64().is_some()))
    {
        return Err(failed(
            "Code cells require outputs and an integer or null execution_count",
        ));
    }
    Ok(())
}
