//! Durable verification evidence, independent of solver and judge assertions.
use cyber_store::{Expected, NewEvent};
use cyber_workflow::solve::Check;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::{BuiltinHost, Ctx};
use crate::tools::{ToolError, failed};
const EVENT: &str = "workflow.solve.evidence.1";

impl BuiltinHost {
    pub(super) async fn verify_solve(&self, ctx: &Ctx<'_>, run: &str) -> Result<Value, ToolError> {
        let input = &ctx.inv.input;
        let base = input["base"]["commit"]
            .as_str()
            .ok_or_else(|| failed("missing pinned base"))?;
        let candidate = input["candidate"]
            .as_str()
            .ok_or_else(|| failed("missing candidate authority"))?;
        let checks: Vec<Check> = serde_json::from_value(input["checks"].clone())
            .map_err(|error| failed(error.to_string()))?;
        let snapshot = super::review::solve_snapshot(ctx, base)
            .await
            .map_err(|error| failed(error.to_string()))?;
        if !snapshot.limitations.is_empty() {
            return Err(failed("source snapshot is incomplete"));
        }
        let source = snapshot
            .source_sha256
            .as_deref()
            .ok_or_else(|| failed("missing full source fingerprint"))?;
        let identity = format!("solve_{run}_{}_{}", input["revision"], candidate);
        let stored = self
            .opts
            .store
            .read_events(&identity, -1, 2)
            .map_err(|error| failed(error.to_string()))?;
        if let Some(record) = stored.events.first() {
            if record.data["source_sha256"] != source {
                return Err(failed(
                    "candidate source changed after submission; start a new solve revision",
                ));
            }
        } else {
            self.opts.store.append(&identity, Expected::Seq(-1), vec![NewEvent::new(EVENT,
                json!({"run_id":run,"candidate":candidate,"revision":input["revision"],
                    "base":base,"cwd":ctx.location,"source_sha256":source,"patch_sha256":snapshot.patch_sha256}))]).map_err(|error| failed(error.to_string()))?;
        }
        let mut evidence = Vec::new();
        for (ordinal, check) in checks.iter().enumerate() {
            let id = format!("{identity}_check_{ordinal}");
            let definition = format!(
                "{:x}",
                Sha256::digest(
                    serde_json::to_vec(check).map_err(|error| failed(error.to_string()))?
                )
            );
            let recorded = self
                .opts
                .store
                .read_events(&id, -1, 3)
                .map_err(|error| failed(error.to_string()))?;
            if let Some(intent) = recorded.events.first() {
                evidence.push(recover_check(
                    &intent.data,
                    recorded.events.get(1).map(|event| &event.data),
                    source,
                    &definition,
                )?);
                continue;
            }
            if evidence
                .iter()
                .any(|check| check["outcome"] == "unknown" || check["settled"] == false)
            {
                evidence.push(
                    json!({"id":id,"argv":check.argv,"outcome":"not_run","settled":false,
                    "limitation":"an earlier native effect is unresolved"}),
                );
                continue;
            }
            let current = super::review::solve_snapshot(ctx, base)
                .await
                .map_err(|error| failed(error.to_string()))?;
            if current.source_sha256.as_deref() != Some(source) {
                evidence.push(
                    json!({"id":id,"argv":check.argv,"outcome":"not_run","settled":true,
                    "limitation":"source changed before verification"}),
                );
                continue;
            }
            let started = now();
            let intent = json!({"id":id,"run_id":run,"candidate":candidate,"revision":input["revision"],
                "source_sha256":source,"definition_sha256":definition,"ordinal":ordinal,"argv":check.argv,
                "cwd":ctx.location,"started_ms":started,"outcome":"unknown","settled":false});
            self.opts
                .store
                .append(
                    &id,
                    Expected::Seq(-1),
                    vec![NewEvent::new(EVENT, intent.clone())],
                )
                .map_err(|error| failed(error.to_string()))?;
            evidence.push(self.settle_solve_check(ctx, check, &id, intent).await?);
        }
        let final_snapshot = super::review::solve_snapshot(ctx, base)
            .await
            .map_err(|error| failed(error.to_string()))?;
        let eligible = final_snapshot.source_sha256.as_deref() == Some(source)
            && evidence.len() == checks.len()
            && evidence.iter().all(|check| {
                check["outcome"] == "passed"
                    && check["settled"] == true
                    && check["source_unchanged"] == true
            });
        Ok(
            json!({"label":input["label"],"candidate_id":candidate,"branch":input["branch"],
            "model":input["model"],"cost":input["cost"],"tokens":input["tokens"],"session_id":ctx.inv.session_id,
            "base":base,"source_sha256":source,"patch_sha256":snapshot.patch_sha256,"patch":snapshot.patch(),
            "worktree":ctx.location,"limitations":[],"checks":evidence,"eligible":eligible,"evidence_id":identity}),
        )
    }
    async fn settle_solve_check(
        &self,
        ctx: &Ctx<'_>,
        check: &Check,
        id: &str,
        mut intent: Value,
    ) -> Result<Value, ToolError> {
        let base = ctx.inv.input["base"]["commit"]
            .as_str()
            .ok_or_else(|| failed("missing base"))?;
        let source = intent["source_sha256"]
            .as_str()
            .ok_or_else(|| failed("missing source"))?
            .to_owned();
        let outcome = if ctx.cancel.is_cancelled() {
            json!({"outcome":"cancelled","settled":true})
        } else {
            match crate::tools::solve_check::run(ctx, check).await {
                Ok(value) => value,
                Err(ToolError::Aborted) => json!({"outcome":"cancelled","settled":false}),
                Err(ToolError::Failed(error)) => {
                    json!({"outcome":"blocked","settled":true,"limitation":error})
                }
            }
        };
        for (key, value) in outcome
            .as_object()
            .ok_or_else(|| failed("invalid native check result"))?
        {
            intent[key] = value.clone();
        }
        intent["ended_ms"] = now().into();
        let after = super::review::solve_snapshot(ctx, base).await;
        let unchanged = after
            .as_ref()
            .is_ok_and(|after| after.source_sha256.as_deref() == Some(source.as_str()));
        intent["source_unchanged"] = unchanged.into();
        if !unchanged {
            if intent["outcome"] == "passed" {
                intent["outcome"] = "failed".into();
            }
            intent["limitation"] =
                "verification changed submitted source or source inspection failed".into();
        }
        self.opts
            .store
            .append(
                id,
                Expected::Seq(0),
                vec![NewEvent::new(EVENT, intent.clone())],
            )
            .map_err(|error| failed(error.to_string()))?;
        Ok(intent)
    }
}

/// An intent is never settlement proof, regardless of fields in its payload. Only a
/// separate matching terminal record may authorize reuse; unresolved effects stay fenced.
fn recover_check(
    intent: &Value,
    settled: Option<&Value>,
    source: &str,
    definition: &str,
) -> Result<Value, ToolError> {
    if intent["source_sha256"] != source || intent["definition_sha256"] != definition {
        return Err(failed(
            "verification identity changed; new revision required",
        ));
    }
    if let Some(settled) = settled {
        let keys = [
            "id",
            "run_id",
            "candidate",
            "revision",
            "source_sha256",
            "definition_sha256",
            "ordinal",
            "argv",
            "cwd",
        ];
        if keys.iter().any(|key| intent[*key] != settled[*key]) {
            return Err(failed(
                "verification settlement identity does not match its intent",
            ));
        }
        return Ok(settled.clone());
    }
    let mut unknown = intent.clone();
    unknown["outcome"] = "unknown".into();
    unknown["settled"] = false.into();
    unknown["limitation"] =
        "an earlier native attempt has no durable settlement; it was not repeated".into();
    Ok(unknown)
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_intent_alone_cannot_authorize_reuse_even_when_it_claims_success() {
        let intent = json!({"id":"check", "source_sha256":"source", "definition_sha256":"definition", "outcome":"passed", "settled":true});
        let recovered = recover_check(&intent, None, "source", "definition").unwrap();
        assert_eq!(recovered["outcome"], "unknown");
        assert_eq!(recovered["settled"], false);
        assert_eq!(recovered["id"], "check");
    }

    #[test]
    fn recovery_requires_matching_source_definition_and_terminal_identity() {
        let intent = json!({"id":"check", "source_sha256":"source", "definition_sha256":"definition", "ordinal":0, "outcome":"unknown"});
        let mut settled = intent.clone();
        settled["outcome"] = "passed".into();
        settled["settled"] = true.into();
        assert_eq!(
            recover_check(&intent, Some(&settled), "source", "definition").unwrap(),
            settled
        );
        assert!(recover_check(&intent, Some(&settled), "other source", "definition").is_err());
        assert!(recover_check(&intent, Some(&settled), "source", "other definition").is_err());
        settled["ordinal"] = 1.into();
        assert!(recover_check(&intent, Some(&settled), "source", "definition").is_err());
    }
}
