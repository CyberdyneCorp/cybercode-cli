//! Invocation-owned staging. Only the retained native worker installs bytes.
use super::{Input, error, failed};
use crate::{
    host::Ctx,
    permissions::{Decision, Mode, Request},
    tools::ToolError,
};
use cyber_core::binary::{
    Document, Error,
    draft::{self, ImmutableBase, PatchBatch},
    editor::{DraftScope, Editor, Owner, SavePreparation},
    receipts::{Operation, SaveReview},
};
use cyber_server::runtime::FileDecision;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

enum Staged {
    Existing(Operation),
    Review(Box<Editor>, Box<SaveReview>),
}

pub(super) async fn run(
    ctx: &Ctx<'_>,
    mut document: Document,
    input: Input,
    permit: tokio::sync::OwnedSemaphorePermit,
) -> Result<Value, ToolError> {
    if Mode::parse(&ctx.inv.mode) == Mode::Plan || ctx.inv.registration.is_some() {
        return Err(failed(
            "Binary patch requires a writable built-in invocation",
        ));
    }
    if input.offset.is_some()
        || input.limit.is_some()
        || input.hex.is_some()
        || input.revision.is_some()
    {
        return Err(failed(
            "Binary patch accepts only expected_base_sha256 and splices",
        ));
    }
    let expected = input
        .expected_base_sha256
        .ok_or_else(|| failed("Binary patch requires expected_base_sha256"))?;
    if expected.len() != 64
        || !expected
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(failed("Binary patch requires a lowercase SHA-256"));
    }
    let mut batch = draft::canonical(&PatchBatch {
        revision: String::new(),
        splices: input
            .splices
            .ok_or_else(|| failed("Binary patch requires exact splices"))?,
    })
    .map_err(error)?;
    let path = document.path();
    let resource = document.resource();
    crate::sandboxing::binary_edit_boundary(ctx, &path).map_err(failed)?;
    let request = Request {
        tool: Some("binary".into()),
        action: "edit".into(),
        resources: vec![ctx.resource(&path)],
        file_edit: true,
        mutates: vec![path.clone()],
        ..Default::default()
    };
    ctx.refuse_denied(request.clone()).await?;
    // Refuse Mode, inherited and protected-path ceilings before copying a base;
    // Ask is deferred until the exact byte review has been prepared.
    decision(ctx.policy.decide(&request))?;
    let runtime = ctx
        .host
        .runtime()
        .ok_or_else(|| failed("Binary patch requires the retained runtime"))?;
    runtime
        .verify_binary_invocation(ctx.inv)
        .await
        .map_err(error)?;
    let storage = runtime.binary_storage().map_err(error)?;
    let key = format!("{:x}", Sha256::digest(ctx.inv.operation_key.as_bytes()));
    let document_id = format!("bin_{key}");
    let operation_id = format!("save_{key}");
    let payload = serde_json::to_vec(&(&ctx.inv.operation_key, &expected, &batch, &path))
        .map_err(|_| failed("Binary payload encoding failed"))?;
    let owner = Owner {
        session_id: ctx.inv.session_id.clone(),
        directory: ctx
            .location
            .canonicalize()
            .map_err(|_| failed("Binary Location unavailable"))?,
        root: document.root().to_owned(),
        scope: DraftScope::Model {
            operation: format!("{}:{:x}", ctx.inv.operation_key, Sha256::digest(&payload)),
        },
    };
    let cancel = ctx.cancel.clone();
    let id = document_id.clone();
    let save = operation_id.clone();
    let (staged, permit) = tokio::task::spawn_blocking(move || {
        let mut editor = Editor::model(&storage, &id, owner, &mut document, || {
            cancel.is_cancelled()
        })
        .map_err(error)?;
        if let Some(receipt) = editor.lookup_operation(&save).map_err(error)? {
            return Ok((Staged::Existing(receipt), permit));
        }
        if editor.draft().base().sha256() != expected {
            return Err(failed("Binary base SHA-256 changed; inspect again"));
        }
        if editor.lookup_operation("patch").map_err(error)?.is_none() {
            batch.revision = editor.draft().revision();
            editor.patch("patch", &batch).map_err(error)?;
        }
        let review = editor
            .review(0, 128, || cancel.is_cancelled())
            .map_err(error)?;
        Ok::<_, ToolError>((Staged::Review(Box::new(editor), Box::new(review)), permit))
    })
    .await
    .map_err(|_| failed("Binary staging worker interrupted"))??;
    let Staged::Review(mut editor, review) = staged else {
        let Staged::Existing(operation) = staged else {
            unreachable!()
        };
        return Ok(json!({"document_id":document_id,"operation":operation}));
    };
    ctx.verify_binary_read(&path).await?;
    ctx.authorize(
        request.clone(),
        request.resources.clone(),
        json!({"byte_review":review,"binary_operation":"patch"}),
    )
    .await?;
    let grants = (
        decision(ctx.policy.decide(&Request {
            tool: Some("binary".into()),
            action: "read".into(),
            resources: request.resources.clone(),
            read_only: true,
            ..Default::default()
        }))?,
        decision(ctx.policy.decide(&request))?,
    );
    let host = ctx
        .host
        .weak
        .upgrade()
        .ok_or_else(|| failed("Binary host unavailable"))?;
    let inv = ctx.inv.clone();
    let cancel = ctx.cancel.clone();
    let executor = tokio::runtime::Handle::current();
    let save = operation_id.clone();
    ctx.host.note_call_metadata(
        &ctx.inv.session_id,
        &ctx.inv.call_id,
        json!({"binary_document_id":document_id,"binary_operation_id":operation_id}),
    );
    let receipt = runtime
        .binary_tool_work(
            inv.clone(),
            document_id.clone(),
            resource,
            grants,
            move |work| {
                let _permit = permit;
                let prepared = editor.prepare_save(&save, &review.fingerprint, || {
                    work.is_cancelled() || cancel.is_cancelled()
                })?;
                let operation = match prepared {
                    SavePreparation::Existing(operation) => operation,
                    SavePreparation::Ready(prepared) => editor.commit_save_checked(
                        *prepared,
                        || work.is_cancelled() || cancel.is_cancelled(),
                        || {
                            work.verify()?;
                            executor.block_on(async {
                                let policy = host.policy(&inv).await.map_err(|_| Error::Changed)?;
                                let fresh = Ctx {
                                    skill_paths: Default::default(),
                                    read_receipt: Default::default(),
                                    loaded_skill: Default::default(),
                                    provenance: Default::default(),
                                    compiler_feedback: Default::default(),
                                    hook_decision: None,
                                    denial: Default::default(),
                                    host: &host,
                                    inv: &inv,
                                    policy,
                                    location: std::path::PathBuf::from(&inv.directory),
                                    cancel: cancel.clone(),
                                };
                                crate::sandboxing::binary_edit_boundary(&fresh, &path)
                                    .map_err(|_| Error::Changed)?;
                                fresh
                                    .verify_binary_read(&path)
                                    .await
                                    .map_err(|_| Error::Changed)?;
                                for (action, grant) in [("read", grants.0), ("edit", grants.1)] {
                                    let mut check = request.clone();
                                    check.action = action.into();
                                    check.read_only = action == "read";
                                    check.file_edit = action == "edit";
                                    let current = fresh.policy.decide(&check);
                                    if !matches!(current, Decision::Allow)
                                        && !(current == Decision::Ask && grant == FileDecision::Ask)
                                    {
                                        return Err(Error::Changed);
                                    }
                                }
                                if cancel.is_cancelled() {
                                    return Err(Error::Cancelled);
                                }
                                Ok(())
                            })
                        },
                    )?,
                };
                work.resume_notifications(&mut editor);
                Ok::<_, Error>(operation)
            },
        )
        .await
        .map_err(|error| {
            failed(format!(
                "Binary operation {operation_id} needs status review before any retry: {error}"
            ))
        })?
        .map_err(error)?;
    ctx.host.note_call_metadata(&ctx.inv.session_id, &ctx.inv.call_id,
        json!({"binary_document_id":document_id,"binary_operation_id":operation_id,"binary_operation":receipt}));
    Ok(json!({"document_id":document_id,"operation":receipt}))
}

fn decision(value: Decision) -> Result<FileDecision, ToolError> {
    match value {
        Decision::Allow => Ok(FileDecision::Allow),
        Decision::Ask => Ok(FileDecision::Ask),
        Decision::Deny(_) => Err(failed("Binary file authority denied")),
    }
}
