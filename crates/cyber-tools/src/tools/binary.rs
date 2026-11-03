//! Bounded live byte operations. Owner drafts and private version images are
//! deliberately absent from this tool's input namespace.
use super::{Tool, ToolError, def, failed};
use crate::{host::Ctx, permissions::Request};
use cyber_core::binary::{Document, Offset, SearchRequest};
use cyber_server::runtime::{RetrySafety, ToolDef};
use futures::future::BoxFuture;
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::{Arc, OnceLock};
#[cfg(any(target_os = "macos", target_os = "linux"))]
mod patch;

pub(crate) struct Binary;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    operation: Operation,
    path: String,
    offset: Option<Offset>,
    limit: Option<usize>,
    revision: Option<String>,
    hex: Option<String>,
    expected_base_sha256: Option<String>,
    splices: Option<Vec<cyber_core::binary::draft::Splice>>,
}
#[derive(Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Operation {
    Inspect,
    Read,
    Search,
    Patch,
}

impl Tool for Binary {
    fn def(&self) -> ToolDef {
        def(
            "binary",
            "Inspect regular-file bytes, read bounded hex/ASCII pages, search literal hex, or apply an exact reviewed patch. Patch requires the inspected whole-file SHA-256 and expected bytes for every splice. Offsets are exact decimal strings. Inspect computes SHA-256 through 512 MiB; larger sources remain live view only. Patch never automatically retries a native replacement.",
            json!({"type":"object","additionalProperties":false,"required":["operation","path"],"properties":{
            "operation":{"type":"string","enum":["inspect","read","search","patch"]},"path":{"type":"string"},
            "offset":{"type":"string","pattern":"^(0|[1-9][0-9]{0,19})$"},"limit":{"type":"integer","minimum":1,"maximum":4096},
            "revision":{"type":"string"},"hex":{"type":"string","maxLength":8192},
            "expected_base_sha256":{"type":"string","pattern":"^[0-9a-f]{64}$"},
            "splices":{"type":"array","minItems":1,"maxItems":128,"items":{"type":"object","additionalProperties":false,"required":["offset","delete_length","expected_hex","insert_hex"],"properties":{
                "offset":{"type":"string","pattern":"^(0|[1-9][0-9]{0,19})$"},"delete_length":{"type":"string","pattern":"^(0|[1-9][0-9]{0,19})$"},"expected_hex":{"type":"string","maxLength":131072},"insert_hex":{"type":"string","maxLength":131072}}}}}}),
            RetrySafety::Never,
            false,
        )
    }
    fn run<'a>(&'a self, ctx: &'a Ctx<'a>) -> BoxFuture<'a, Result<String, ToolError>> {
        Box::pin(run(ctx))
    }
}
fn permit(ctx: &Ctx<'_>) -> Result<tokio::sync::OwnedSemaphorePermit, ToolError> {
    if let Some(runtime) = ctx.host.runtime() {
        return runtime
            .binary_read_permit()
            .map_err(|_| failed("Binary IO capacity reached"));
    }
    static WORKERS: OnceLock<Arc<tokio::sync::Semaphore>> = OnceLock::new();
    WORKERS
        .get_or_init(|| Arc::new(tokio::sync::Semaphore::new(16)))
        .clone()
        .try_acquire_owned()
        .map_err(|_| failed("Binary IO capacity reached"))
}
async fn run(ctx: &Ctx<'_>) -> Result<String, ToolError> {
    if ctx.inv.registration.is_some() {
        return Err(failed("Binary operations require the built-in capability"));
    }
    let input: Input = serde_json::from_value(ctx.inv.input.clone())
        .map_err(|_| failed("Invalid binary operation or byte arguments"))?;
    if input.operation != Operation::Patch
        && (input.expected_base_sha256.is_some() || input.splices.is_some())
    {
        return Err(failed("Patch arguments require the patch operation"));
    }
    let authority = ctx
        .host
        .runtime()
        .map(|runtime| {
            runtime
                .capture_child_admission(&ctx.inv.session_id)
                .map(|authority| (runtime, authority))
        })
        .transpose()
        .map_err(|_| failed("Binary invocation admission changed"))?;
    if let Some((runtime, _)) = &authority {
        runtime
            .verify_binary_invocation(ctx.inv)
            .await
            .map_err(error)?;
    }
    let permit = permit(ctx)?;
    let (directory, path) = (ctx.location.clone(), input.path.clone());
    let (document, permit) = tokio::task::spawn_blocking(move || {
        let root = cyber_core::config::project_root(&directory)
            .canonicalize()
            .map_err(|_| failed("Binary project root unavailable"))?;
        let canonical_directory = directory
            .canonicalize()
            .map_err(|_| failed("Binary Location unavailable"))?;
        let requested = if std::path::Path::new(&path).is_absolute() {
            let absolute = std::path::PathBuf::from(&path);
            absolute
                .strip_prefix(&directory)
                .map_or_else(|_| absolute.clone(), |tail| canonical_directory.join(tail))
        } else {
            canonical_directory.join(&path)
        };
        let relative = requested
            .strip_prefix(&root)
            .map_err(|_| failed("Binary source is outside the project"))?;
        let relative = relative
            .to_str()
            .ok_or_else(|| failed("Binary source requires a UTF-8 path"))?;
        let document = Document::open(&root, relative).map_err(error)?;
        Ok::<_, ToolError>((document, permit))
    })
    .await
    .map_err(|_| failed("Binary source worker interrupted"))??;
    let path = document.path();
    ctx.check_external(std::slice::from_ref(&path)).await?;
    if crate::sandboxing::hidden(ctx).denies(&path) {
        return Err(failed("Binary source is private"));
    }
    let resource = ctx.resource(&path);
    ctx.authorize(
        Request {
            action: "read".into(),
            resources: vec![resource.clone()],
            read_only: true,
            ..Default::default()
        },
        vec![resource],
        Value::Null,
    )
    .await?;
    ctx.verify_binary_read(&path).await?;
    let result = if input.operation == Operation::Patch {
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        {
            patch::run(ctx, document, input, permit).await?
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        {
            let _ = (document, input, permit);
            return Err(failed(
                "Native binary patching is unsupported on this platform",
            ));
        }
    } else {
        let cancel = ctx.cancel.clone();
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            execute(document, input, || cancel.is_cancelled())
        })
        .await
        .map_err(|_| failed("Binary source worker interrupted"))??
    };
    if let Some((runtime, authority)) = authority {
        runtime
            .verify_binary_invocation(ctx.inv)
            .await
            .map_err(error)?;
        authority
            .verify(&runtime, &ctx.inv.session_id)
            .map_err(|_| failed("Binary invocation admission changed"))?;
    }
    ctx.verify_binary_read(&path).await?;
    serde_json::to_string(&result).map_err(|_| failed("Binary result encoding failed"))
}
fn execute(
    mut document: Document,
    input: Input,
    cancelled: impl Fn() -> bool,
) -> Result<Value, ToolError> {
    document.validate().map_err(error)?;
    let metadata = document.metadata();
    if input
        .revision
        .as_ref()
        .is_some_and(|revision| revision != &metadata.revision)
    {
        return Err(failed("Binary live revision changed; inspect again"));
    }
    if cancelled() {
        return Err(ToolError::Aborted);
    }
    let data = match input.operation {
        Operation::Inspect => {
            let hash = if metadata.within_edit_size_limit {
                Some(document.fingerprint(&cancelled).map_err(error)?)
            } else {
                None
            };
            json!({"metadata":metadata,"base_sha256":hash})
        }
        Operation::Read => {
            let limit = input.limit.unwrap_or(256);
            if limit == 0 || limit > 4096 {
                return Err(failed("Binary read limit must be 1–4096 bytes"));
            }
            let page = document
                .page(&metadata.revision, input.offset.unwrap_or(Offset(0)), limit)
                .map_err(error)?;
            let hex = page
                .bytes
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            let ascii = page
                .bytes
                .iter()
                .map(|byte| {
                    if (32..=126).contains(byte) {
                        char::from(*byte)
                    } else {
                        '.'
                    }
                })
                .collect::<String>();
            json!({"metadata":metadata,"offset":page.offset,"length":Offset(page.bytes.len() as u64),"hex":hex,"ascii":ascii,"page_sha256":page.sha256,"eof":page.eof})
        }
        Operation::Search => {
            let hex = input
                .hex
                .ok_or_else(|| failed("Binary search requires literal hex"))?;
            let result = document
                .search(
                    &SearchRequest {
                        revision: metadata.revision.clone(),
                        hex,
                        offset: input.offset.unwrap_or(Offset(0)),
                    },
                    &cancelled,
                )
                .map_err(error)?;
            json!({"metadata":metadata,"result":result})
        }
        Operation::Patch => return Err(failed("Binary patch requires retained native ownership")),
    };
    if cancelled() {
        return Err(ToolError::Aborted);
    }
    document.validate().map_err(error)?;
    Ok(data)
}

fn error(error: cyber_core::binary::Error) -> ToolError {
    if matches!(error, cyber_core::binary::Error::Cancelled) {
        ToolError::Aborted
    } else {
        failed(error.to_string())
    }
}
