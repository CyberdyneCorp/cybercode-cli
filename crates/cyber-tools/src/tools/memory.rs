//! Managed memory scopes through the normal permission and hook boundaries.
use cyber_core::memory::{MemoryDocument, MemorySettings, MemoryStorageError, MemoryStore};
use cyber_server::runtime::{MemoryAdmission, MemoryWrite, RetrySafety, Runtime, ToolDef};
use futures::future::BoxFuture;
use serde_json::{Value, json};
use std::path::Path;

use super::{Tool, ToolError, def, failed, text};
use crate::host::{BuiltinHost, Ctx};
use crate::permissions::{Mode, Request};

pub(crate) struct Memory;

pub(crate) fn definition(writable: bool) -> ToolDef {
    let operations = if writable {
        vec!["list", "read", "write", "update", "delete"]
    } else {
        vec!["list", "read"]
    };
    def(
        "memory",
        "List or read durable project/global notes. Check existing names and descriptions before saving; write replaces a matching name. content is a complete Markdown document with YAML name, description and type fields. Feedback/project notes need Why and How to apply. Never save secrets or facts derivable from the repository.",
        json!({"type":"object","required":["operation"],"additionalProperties":false,"properties":{
            "operation":{"type":"string","enum":operations},
            "scope":{"type":"string","enum":["project","global"]},
            "name":{"type":"string"},"content":{"type":"string"}
        }}),
        if writable {
            RetrySafety::Never
        } else {
            RetrySafety::ReadOnly
        },
        false,
    )
}

pub(crate) fn settings(host: &BuiltinHost, location: &Path) -> Result<MemorySettings, ToolError> {
    let (config, _) = (host.opts.config)(location).map_err(failed)?;
    MemorySettings::from_config(&config, host.opts.env.as_ref()).map_err(|e| failed(e.to_string()))
}

impl Tool for Memory {
    fn def(&self) -> ToolDef {
        definition(true)
    }

    fn run<'a>(&'a self, ctx: &'a Ctx<'a>) -> BoxFuture<'a, Result<String, ToolError>> {
        Box::pin(async move {
            let operation = text(&ctx.inv.input, "operation");
            let writable = matches!(operation, "write" | "update" | "delete");
            check_settings(ctx, writable)?;
            let scope = ctx
                .inv
                .input
                .get("scope")
                .and_then(Value::as_str)
                .unwrap_or("project");
            validate_input(&ctx.inv.input, operation)?;
            authorize_scope(ctx, scope, writable, operation).await?;
            check_settings(ctx, writable)?;
            let project = project_id(ctx, scope).await?;
            if scope != "global" && project == "global" {
                authorize_scope(ctx, "global", writable, operation).await?;
            }
            check_settings(ctx, writable)?;
            if ctx.cancel.is_cancelled() {
                return Err(ToolError::Aborted);
            }
            let data = ctx
                .host
                .opts
                .tool_output_dir
                .parent()
                .ok_or_else(|| failed("Missing memory data directory"))?
                .to_owned();
            let location = ctx.location.clone();
            let input = ctx.inv.input.clone();
            let cancel = ctx.cancel.clone();
            let config = ctx.host.opts.config.clone();
            let env = ctx.host.opts.env.clone();
            let mode = Mode::parse(&ctx.inv.mode);
            let publication = Publication {
                runtime: ctx.host.runtime(),
                directory: location.clone(),
                identity: format!("tool:{}:{}", ctx.inv.session_id, ctx.inv.operation_key),
            };
            // Join the actual owner even after cancellation; never orphan a started commit.
            tokio::task::spawn_blocking(move || {
                if cancel.is_cancelled() {
                    return Err(ToolError::Aborted);
                }
                let (resolved, _) = config(&location).map_err(failed)?;
                let current = MemorySettings::from_config(&resolved, env.as_ref())
                    .map_err(|e| failed(e.to_string()))?;
                check_enabled(current, writable, mode)?;
                if cancel.is_cancelled() {
                    return Err(ToolError::Aborted);
                }
                operate(&data, &project, &input, &cancel, publication)
            })
            .await
            .map_err(|_| failed("Memory owner did not acknowledge completion"))?
        })
    }
}

async fn authorize_scope(
    ctx: &Ctx<'_>,
    scope: &str,
    writable: bool,
    operation: &str,
) -> Result<(), ToolError> {
    ctx.authorize(
        Request {
            action: "memory".into(),
            resources: vec![scope.into()],
            read_only: !writable,
            ..Request::default()
        },
        vec![scope.into()],
        json!({"operation":operation,"scope":scope}),
    )
    .await
}

async fn project_id(ctx: &Ctx<'_>, scope: &str) -> Result<String, ToolError> {
    if ctx.cancel.is_cancelled() {
        return Err(ToolError::Aborted);
    }
    if scope == "global" {
        return Ok("global".into());
    }
    let location = ctx.location.clone();
    let cancel = ctx.cancel.clone();
    tokio::task::spawn_blocking(move || {
        if cancel.is_cancelled() {
            return Err(ToolError::Aborted);
        }
        Ok(cyber_core::project::identify(&location).id)
    })
    .await
    .map_err(|_| failed("Memory identity owner did not acknowledge completion"))?
}

fn check_settings(ctx: &Ctx<'_>, mutation: bool) -> Result<(), ToolError> {
    check_enabled(
        settings(ctx.host, &ctx.location)?,
        mutation,
        Mode::parse(&ctx.inv.mode),
    )
}

fn check_enabled(settings: MemorySettings, mutation: bool, mode: Mode) -> Result<(), ToolError> {
    if !settings.enabled {
        return Err(failed("Memory is disabled"));
    }
    if mutation && (!settings.generate || mode == Mode::Plan) {
        return Err(failed("Memory is read-only"));
    }
    Ok(())
}

fn validate_input(input: &Value, operation: &str) -> Result<(), ToolError> {
    if matches!(operation, "read" | "update" | "delete") {
        cyber_core::memory::validate_name(text(input, "name"))
            .map_err(|e| failed(e.to_string()))?;
    }
    if matches!(operation, "write" | "update") {
        let content = text(input, "content");
        if content.len() > 1_048_576 {
            return Err(failed("Memory file exceeds 1 MiB"));
        }
        let doc = MemoryDocument::for_write(content).map_err(|e| failed(e.to_string()))?;
        if let Some(name) = input.get("name").and_then(Value::as_str)
            && name != doc.metadata.name
        {
            return Err(failed("Memory name does not match frontmatter"));
        }
    }
    Ok(())
}

struct Publication {
    runtime: Option<Runtime>,
    directory: std::path::PathBuf,
    identity: String,
}

fn operate(
    data: &Path,
    project: &str,
    input: &Value,
    cancel: &tokio_util::sync::CancellationToken,
    publication: Publication,
) -> Result<String, ToolError> {
    let operation = text(input, "operation");
    let mutation = matches!(operation, "write" | "update" | "delete");
    if mutation && !cfg!(unix) {
        return Err(failed(
            "Memory mutations require platform privacy and durability support",
        ));
    }
    let existing = MemoryStore::existing(data, project).map_err(|e| failed(e.to_string()))?;
    if existing.is_none() && operation != "write" {
        return if operation == "list" {
            Ok("{\"memories\":[],\"invalid\":[]}".into())
        } else {
            Err(failed("Memory not found"))
        };
    }
    if cancel.is_cancelled() {
        return Err(ToolError::Aborted);
    }
    let store = match existing {
        Some(store) => store,
        None => MemoryStore::open(data, project).map_err(|e| failed(e.to_string()))?,
    };
    let mut owner = store.claim().map_err(|e| failed(e.to_string()))?;
    if cancel.is_cancelled() {
        return Err(ToolError::Aborted);
    }
    let value = match operation {
        "list" => {
            let catalog = owner.list().map_err(|e| failed(e.to_string()))?;
            json!({"memories":catalog.memories,"invalid":catalog.invalid.into_iter().map(|entry|
                json!({"filename":entry.filename,"diagnostic":entry.diagnostic})).collect::<Vec<_>>()})
        }
        "read" => {
            let note = owner
                .read(text(input, "name"))
                .map_err(|e| failed(e.to_string()))?;
            json!({"metadata":note.metadata,"body":note.body})
        }
        "write" | "update" | "delete" => {
            json!(mutate(&mut owner, project, input, &publication, cancel)?)
        }
        _ => return Err(failed("Unknown memory operation")),
    };
    serde_json::to_string(&value).map_err(|_| failed("Memory output encoding failed"))
}

fn mutate(
    owner: &mut cyber_core::memory::MemoryScope<'_>,
    project: &str,
    input: &Value,
    publication: &Publication,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<cyber_core::memory::MemoryMutation, ToolError> {
    let operation = text(input, "operation");
    let content = text(input, "content");
    let name = if operation == "delete" {
        text(input, "name").to_owned()
    } else {
        MemoryDocument::for_write(content)
            .map_err(|error| failed(error.to_string()))?
            .metadata
            .name
    };
    let fingerprint = input.to_string();
    let write = MemoryWrite {
        directory: &publication.directory,
        project_id: project,
        name: &name,
        deleted: operation == "delete",
        identity: Some(&publication.identity),
        content: &fingerprint,
        http_hash: None,
    };
    if let Some(runtime) = &publication.runtime
        && let Some(change) = runtime.memory_write_receipt(&write).map_err(|_| {
            failed("Memory mutation requires reviewed recovery or a fresh request identity")
        })?
    {
        return Ok(change.receipt);
    }
    owner.list().map_err(|error| failed(error.to_string()))?;
    if matches!(operation, "update" | "delete") {
        owner
            .read(&name)
            .map_err(|error| failed(error.to_string()))?;
    }
    if cancel.is_cancelled() {
        return Err(ToolError::Aborted);
    }
    let admitted = publication
        .runtime
        .as_ref()
        .map(|runtime| runtime.admit_memory_write(write))
        .transpose()
        .map_err(|_| {
            failed("Memory mutation requires reviewed recovery or a fresh request identity")
        })?;
    let mut durable_owner = match admitted {
        Some(MemoryAdmission::Replay(change)) => return Ok(change.receipt),
        Some(MemoryAdmission::Owned(owner)) => Some(owner),
        None => None,
    };
    let prepared = if operation == "delete" {
        owner.prepare_delete(&name)
    } else {
        owner.prepare_write(content)
    }
    .map_err(|error| failed(error.to_string()))?;
    if let Some(owner) = &mut durable_owner {
        owner
            .bind_journal(
                prepared
                    .journal_identity()
                    .map_err(|error| failed(error.to_string()))?,
            )
            .map_err(|_| failed("Memory journal binding requires reviewed recovery"))?;
    }
    prepared
        .commit_with_acknowledgement(|receipt| {
            if let Some(owner) = durable_owner {
                (*owner).finish(receipt.clone()).map_err(|error| {
                    cyber_core::log::error(
                        "memory",
                        &error.to_string(),
                        json!({"mutation_id":receipt.id}),
                    );
                    MemoryStorageError::Unsafe("memory acknowledgement requires reviewed recovery")
                })?;
            }
            Ok(())
        })
        .map_err(|error| failed(error.to_string()))
}
