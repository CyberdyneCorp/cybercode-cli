//! Cancel a background task owned by the caller Session.
use super::{Tool, ToolError, def, failed, text};
use crate::host::Ctx;
use cyber_server::runtime::RetrySafety;
use futures::future::BoxFuture;
use serde_json::json;

pub(crate) struct TaskStop;
impl Tool for TaskStop {
    fn def(&self) -> cyber_server::runtime::ToolDef {
        def(
            "task_stop",
            "Stop a background task owned by this Session and wait for its child to settle.",
            json!({"type":"object","required":["job_id"],"additionalProperties":false,"properties":{"job_id":{"type":"string"}}}),
            RetrySafety::Idempotent,
            false,
        )
    }
    fn run<'a>(&'a self, ctx: &'a Ctx<'a>) -> BoxFuture<'a, Result<String, ToolError>> {
        Box::pin(async move {
            let runtime = ctx
                .host
                .runtime()
                .ok_or_else(|| failed("Task stop requires the runtime"))?;
            let id = text(&ctx.inv.input, "job_id");
            let job = runtime.job(id).map_err(|e| failed(e.to_string()))?;
            if job.session_id != ctx.inv.session_id {
                return Err(failed("Task does not belong to this Session"));
            }
            let job = runtime
                .cancel_job(id)
                .await
                .map_err(|e| failed(e.to_string()))?;
            Ok(json!({"job_id":job.id,"status":job.status}).to_string())
        })
    }
}
