//! `file` prompt parts read at admission (`server-api` → Prompt attachment parts).
//!
//! The path goes through the `read` tool's own gates, as a model's `read` call would:
//! the agent profile's tools and a loaded skill's tool limits, `PreToolUse` hooks (a deny blocks it, a rewrite replaces the path), then
//! `external_directory` for a path outside the Location and the added directories, the
//! `read` rule under the checkout's trust, and the hidden-path sandbox. An ask reaches the
//! attached client; under `plan` or `dont-ask` it is a denial. The size is checked from
//! the metadata before any byte is read, and the read itself stops past the limit.
use std::path::Path;

use cyber_server::runtime::{Asker, FileReadError, Invocation, TurnContext};
use serde_json::json;
use tokio::io::AsyncReadExt as _;
use tokio_util::sync::CancellationToken;

use super::{BuiltinHost, DenialHooks};
use crate::tools::{ToolError, fs};

impl BuiltinHost {
    pub(crate) async fn read_file_part(
        &self,
        turn: &TurnContext,
        path: &str,
        max_bytes: u64,
        asker: Asker,
        cancel: CancellationToken,
    ) -> Result<Vec<u8>, FileReadError> {
        let mut invocation = Invocation {
            registration: None,
            session_id: turn.session_id.clone(),
            directory: turn.directory.clone(),
            agent: turn.agent.clone(),
            mode: turn.mode.clone(),
            message_id: asker.message_id.clone(),
            call_id: asker.call_id.clone(),
            name: "read".into(),
            input: json!({ "path": path }),
            attempt: 1,
            operation_key: cyber_core::ids::new_id("op"),
            asker,
            rules: turn.rules.clone(),
        };
        let schema = self
            .tools
            .iter()
            .find(|tool| tool.def().spec.name == "read")
            .map(|tool| tool.def().spec.input_schema)
            .ok_or_else(|| FileReadError::Denied("the read tool is not available".into()))?;
        self.check_read_call(&invocation).await?;
        let hook_decision = self
            .pre_tool_hooks(&mut invocation, &schema, cancel.clone())
            .await
            .map_err(denied)?;
        let mut ctx = self
            .call_ctx(&invocation, hook_decision, cancel, DenialHooks::default())
            .await
            .map_err(FileReadError::Denied)?;
        ctx.location = super::canonical(Path::new(&invocation.directory));
        let path = fs::readable(&ctx).await.map_err(denied)?;
        read_capped(&path, max_bytes).await
    }
}

impl BuiltinHost {
    /// The gates a `read` call passes before its hooks: the auto-mode override, the
    /// loaded skill's tool limits and the agent profile's tools (the solve judge's too).
    async fn check_read_call(&self, invocation: &Invocation) -> Result<(), FileReadError> {
        invocation
            .asker
            .validate_auto_override(&invocation.name, &invocation.input)
            .map_err(|error| FileReadError::Denied(error.to_string()))?;
        self.check_skill_tool(invocation)
            .await
            .map_err(FileReadError::Denied)?;
        self.check_agent_tool(invocation)
            .map_err(FileReadError::Denied)
    }
}

fn denied(error: ToolError) -> FileReadError {
    match error {
        ToolError::Failed(reason) => FileReadError::Denied(reason),
        ToolError::Aborted => FileReadError::Denied("the request was cancelled".into()),
    }
}

/// A regular file of at most `max_bytes`, judged by its metadata first; a file that grows
/// past the limit while it is read is refused too.
async fn read_capped(path: &Path, max_bytes: u64) -> Result<Vec<u8>, FileReadError> {
    let invalid = |error: std::io::Error| {
        FileReadError::Invalid(format!("Could not read {}: {error}", path.display()))
    };
    let meta = tokio::fs::metadata(path).await.map_err(invalid)?;
    if !meta.is_file() {
        return Err(FileReadError::Invalid(format!(
            "{} is not a regular file",
            path.display()
        )));
    }
    if meta.len() > max_bytes {
        return Err(FileReadError::TooLarge { size: meta.len() });
    }
    let file = tokio::fs::File::open(path).await.map_err(invalid)?;
    let mut bytes = Vec::with_capacity(meta.len() as usize);
    file.take(max_bytes + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(invalid)?;
    if bytes.len() as u64 > max_bytes {
        return Err(FileReadError::TooLarge {
            size: bytes.len() as u64,
        });
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn files_over_the_limit_are_refused_before_they_are_read() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("shot.png");
        std::fs::write(&path, vec![7u8; 64]).unwrap();
        assert_eq!(read_capped(&path, 64).await.unwrap().len(), 64);
        assert_eq!(
            read_capped(&path, 63).await,
            Err(FileReadError::TooLarge { size: 64 })
        );
        assert!(matches!(
            read_capped(temp.path(), 64).await,
            Err(FileReadError::Invalid(_))
        ));
        assert!(matches!(
            read_capped(&temp.path().join("missing.png"), 64).await,
            Err(FileReadError::Invalid(_))
        ));
    }
}
