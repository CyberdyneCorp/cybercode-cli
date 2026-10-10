//! Read-only LSP navigation through the Location's owned workers.
mod output;

use super::{Tool, ToolError, def, failed};
use crate::permissions::Request;
use crate::{
    host::Ctx,
    lsp::{ServerHandle, read_origin},
};
use cyber_core::config::LspSettings;
use cyber_server::runtime::{RetrySafety, ToolDef};
use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};

pub(crate) struct Lsp;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Operation {
    Definition,
    References,
    Hover,
    DocumentSymbols,
    WorkspaceSymbols,
    Implementation,
    RenamePreview,
    Diagnostics,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    operation: Operation,
    path: Option<String>,
    line: Option<u64>,
    character: Option<u64>,
    query: Option<String>,
    new_name: Option<String>,
}

impl Input {
    fn parse(value: &Value) -> Result<Self, ToolError> {
        let input: Self = serde_json::from_value(value.clone())
            .map_err(|_| failed("Invalid LSP operation or arguments"))?;
        if !matches!(
            input.operation,
            Operation::WorkspaceSymbols | Operation::Diagnostics
        ) && input.path.as_deref().is_none_or(str::is_empty)
        {
            return Err(failed("This LSP operation requires path"));
        }
        if input.positioned() {
            for number in [input.line, input.character] {
                if number.is_none_or(|value| value == 0 || value > i32::MAX as u64 + 1) {
                    return Err(failed(
                        "LSP line and character must be positive 1-based positions",
                    ));
                }
            }
        }
        if input.operation == Operation::RenamePreview
            && input.new_name.as_deref().is_none_or(str::is_empty)
        {
            return Err(failed("rename_preview requires new_name"));
        }
        Ok(input)
    }
    fn positioned(&self) -> bool {
        matches!(
            self.operation,
            Operation::Definition
                | Operation::References
                | Operation::Hover
                | Operation::Implementation
                | Operation::RenamePreview
        )
    }
    fn request(&self, file: Option<&Path>) -> Result<(&'static str, Value), ToolError> {
        if self.operation == Operation::WorkspaceSymbols {
            return Ok((
                "workspace/symbol",
                json!({"query":self.query.as_deref().unwrap_or("")}),
            ));
        }
        let uri = reqwest::Url::from_file_path(
            file.ok_or_else(|| failed("This LSP operation requires path"))?,
        )
        .map_err(|_| failed("Invalid LSP path"))?;
        let mut params = json!({"textDocument":{"uri":uri.as_str()}});
        if self.positioned() {
            params["position"] =
                json!({"line":self.line.unwrap()-1,"character":self.character.unwrap()-1});
        }
        let method = match self.operation {
            Operation::Definition => "textDocument/definition",
            Operation::References => {
                params["context"] = json!({"includeDeclaration":true});
                "textDocument/references"
            }
            Operation::Hover => "textDocument/hover",
            Operation::DocumentSymbols => "textDocument/documentSymbol",
            Operation::Implementation => "textDocument/implementation",
            Operation::RenamePreview => {
                params["newName"] = json!(self.new_name);
                "textDocument/rename"
            }
            _ => return Err(failed("Unsupported LSP request")),
        };
        Ok((method, params))
    }
}

impl Tool for Lsp {
    fn def(&self) -> ToolDef {
        def(
            "lsp",
            "Read language-server definitions, references, hover, symbols, implementations or cached diagnostics. Positions are 1-based. rename_preview returns proposed edits without applying them. Results are limited to 50 locations inside the current Location.",
            json!({"type":"object","required":["operation"],"additionalProperties":false,"properties":{
                "operation":{"type":"string","enum":["definition","references","hover","document_symbols","workspace_symbols","implementation","rename_preview","diagnostics"]},
                "path":{"type":"string"},"line":{"type":"integer","minimum":1},"character":{"type":"integer","minimum":1},"query":{"type":"string"},"new_name":{"type":"string"}
            }}),
            RetrySafety::ReadOnly,
            true,
        )
    }
    fn run<'a>(&'a self, ctx: &'a Ctx<'a>) -> BoxFuture<'a, Result<String, ToolError>> {
        Box::pin(async move {
            let input = Input::parse(&ctx.inv.input)?;
            let path = input.path.as_deref().map(|path| ctx.resolve(path));
            let resource = path
                .as_ref()
                .map_or_else(|| ".".into(), |path| ctx.resource(path));
            ctx.authorize(
                Request {
                    action: "lsp".into(),
                    resources: vec![resource.clone()],
                    read_only: true,
                    ..Request::default()
                },
                vec![resource],
                json!({"operation":input.operation}),
            )
            .await?;
            tokio::select! { biased; _=ctx.cancel.cancelled()=>Err(ToolError::Aborted), output=query(ctx,input,path)=>output }
        })
    }
}

async fn source(path: &Path) -> Result<String, ToolError> {
    crate::lsp::document_source(path.into()).await.ok_or_else(|| failed("LSP source must be a canonical regular UTF-8 file within the 1 MiB document budget"))
}

async fn query(ctx: &Ctx<'_>, input: Input, path: Option<PathBuf>) -> Result<String, ToolError> {
    let settings = LspSettings::from_config(&ctx.host.config_for(&ctx.location)).map_err(failed)?;
    if !settings.enabled {
        return Err(failed("LSP is disabled for this Location"));
    }
    let locations = ctx
        .host
        .lsp
        .get()
        .ok_or_else(|| failed("LSP services are unavailable"))?;
    let _activity = locations
        .acquire(&ctx.location)
        .map_err(|_| failed("LSP Location is unavailable"))?;
    let file = path
        .map(|path| {
            path.canonicalize()
                .map_err(|_| failed("LSP path is unavailable"))
        })
        .transpose()?;
    if file
        .as_ref()
        .is_some_and(|path| !path.starts_with(&ctx.location) || !path.is_file())
    {
        return Err(failed(
            "LSP path must be a file inside the selected Location",
        ));
    }
    let origin = read_origin(&ctx.location, file.as_deref().unwrap_or(&ctx.location))
        .map_err(|_| failed("LSP creation observation failed"))?;
    let checkouts = origin.document.clone();
    let text = match &file {
        Some(file) => Some(source(file).await?),
        None => None,
    };
    let pool = locations
        .query_pool(&ctx.location, origin)
        .await
        .map_err(|_| failed("LSP services are unavailable"))?;
    verify_source(ctx, file.as_deref(), text.as_deref(), &checkouts).await?;
    let handles = pool.query_handles(file.as_deref());
    if handles.is_empty() {
        return Err(failed(
            "No available language server matches this Location or path",
        ));
    }
    let responses = futures::future::join_all(handles.iter().map(|handle| async {
        let result = query_server(
            handle,
            &input,
            file.as_deref(),
            text.as_deref(),
            checkouts.clone(),
        )
        .await;
        (handle.status(), result)
    }))
    .await;
    verify_source(ctx, file.as_deref(), text.as_deref(), &checkouts).await?;
    output::render(&ctx.location, &input, responses).await
}

async fn verify_source(
    ctx: &Ctx<'_>,
    file: Option<&Path>,
    text: Option<&str>,
    checkouts: &[cyber_core::worktrees::Managed],
) -> Result<(), ToolError> {
    if let Some(file) = file
        && (source(file).await?.as_str() != text.unwrap()
            || read_origin(&ctx.location, file)
                .map_err(|_| failed("LSP source creation changed"))?
                .document
                != checkouts)
    {
        return Err(failed("LSP source changed while the request was running"));
    }
    Ok(())
}

async fn query_server(
    handle: &ServerHandle,
    input: &Input,
    file: Option<&Path>,
    text: Option<&str>,
    checkouts: Vec<cyber_core::worktrees::Managed>,
) -> Result<Value, crate::lsp::LspError> {
    if input.operation == Operation::Diagnostics {
        if let (Some(file), Some(text)) = (file, text) {
            handle.open_observed(file, text.into(), checkouts).await?;
        }
        let paths = match file {
            Some(file) => vec![file.into()],
            None => handle.diagnostic_paths().await?,
        };
        let mut snapshots = vec![];
        for path in paths {
            if let Some(snapshot) = handle.diagnostics(&path).await? {
                snapshots.push(serde_json::to_value(snapshot).map_err(|_| {
                    crate::lsp::LspError::Protocol("diagnostic serialization failed")
                })?);
            }
        }
        return Ok(Value::Array(snapshots));
    }
    let (method, params) = input
        .request(file)
        .map_err(|_| crate::lsp::LspError::Protocol("invalid navigation request"))?;
    match (file, text) {
        (Some(file), Some(text)) => {
            handle
                .navigate_observed(file.into(), text.into(), checkouts, method, params)
                .await
        }
        _ => {
            handle
                .request(method, params, Duration::from_secs(30))
                .await
        }
    }
}
