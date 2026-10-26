use std::sync::{Arc, Mutex, PoisonError};

use cyber_llm::{Adapter, ErrorKind, EventStream, LlmError, LlmRequest};
use cyber_store::reservations::{self as ledger, Estimate, Request, TokenAmounts};
use futures::future::BoxFuture;

use super::{accounting::Attempt, error, refusal};
use crate::runtime::{Handle, Inner, RuntimeError, host::ResolvedModel};

pub(in crate::runtime) struct GatedAdapter<'a> {
    inner: &'a Inner,
    handle: &'a Handle,
    resolved: &'a ResolvedModel,
    purpose: &'static str,
    call_id: Option<String>,
    origin: Option<crate::runtime::usage_origin::StepOrigin>,
    pub(super) ownership: Option<Arc<std::sync::atomic::AtomicBool>>,
    pub(super) failure: Arc<Mutex<Option<RuntimeError>>>,
    pub(super) attempts: Arc<Mutex<Vec<Arc<Attempt>>>>,
}

impl<'a> GatedAdapter<'a> {
    pub fn new(
        inner: &'a Inner,
        handle: &'a Handle,
        resolved: &'a ResolvedModel,
        purpose: &'static str,
    ) -> Self {
        Self {
            inner,
            handle,
            resolved,
            purpose,
            call_id: None,
            origin: None,
            ownership: None,
            failure: Arc::new(Mutex::new(None)),
            attempts: Arc::default(),
        }
    }
    pub fn for_call(mut self, id: String) -> Self {
        self.call_id = Some(id);
        self
    }
    pub(super) fn for_origin(mut self, origin: crate::runtime::usage_origin::StepOrigin) -> Self {
        self.origin = Some(origin);
        self
    }
    pub fn take_error(&self) -> Option<RuntimeError> {
        self.failure
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
    }
    pub fn request_ids(&self) -> Vec<String> {
        self.attempts
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .map(|a| a.request_id.clone())
            .collect()
    }
    pub async fn flush(&self) -> Result<(), RuntimeError> {
        let attempts = self
            .attempts
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        for attempt in attempts {
            attempt.flush().await?;
        }
        Ok(())
    }

    async fn begin(
        &self,
        request: LlmRequest,
    ) -> Result<(LlmRequest, Option<Arc<Attempt>>), RuntimeError> {
        self.inner.activate_reserved_budget(self.handle).await?;
        let (info, origin) = {
            let state = self.handle.state.lock().await;
            let origin = self.origin.clone().unwrap_or_else(|| {
                if self.purpose == "turn" {
                    state.step_origin.clone()
                } else {
                    Default::default()
                }
            });
            (state.info.clone(), origin)
        };
        let handle = self.inner.handle(&info.id).await?;
        let daily = self
            .inner
            .tools
            .daily_budget(&info.directory)
            .map_err(RuntimeError::Invalid)?;
        // Preparation is pure: unsupported bounds are ignored for an entirely soft
        // dispatch, and USD-price refusal takes precedence for reserved scopes.
        let prepared = self
            .resolved
            .adapter
            .prepare_reserved(request.clone(), self.resolved.context_limit);
        let source = info.id.clone();
        let provider = self.resolved.provider.clone();
        let model = self.resolved.model.clone();
        let purpose = self.purpose.to_string();
        let call_id = self.call_id.clone();
        let price = self.resolved.cost.clone();
        let fallback = request.clone();
        let ownership = self.ownership.clone();
        let now = chrono::Utc::now().timestamp_millis();
        // Serialize reservation admission with subtree deletion fencing. No async
        // work occurs while this lock is held.
        let drains = self
            .inner
            .drains
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if drains.get(&info.id).is_some_and(|entry| entry.deleting) {
            return Err(RuntimeError::Busy(info.id));
        }
        let admitted=self.inner.store.transaction(move|tx| {
            let (scopes,deadline_ms)=super::scopes::collect(tx,&source,daily.as_ref(),now)?;
            if scopes.is_empty() {return Ok((fallback,None));}
            if let Some(ownership)=&ownership {ownership.store(true,std::sync::atomic::Ordering::Release);}
            if price.is_none() && let Some(scope)=scopes.iter().find(|s|s.max_cost_microusd.is_some()) {
                return Err(refusal("unpriced","USD reservation requires known model pricing",Some(&scope.id)));
            }
            if deadline_ms.is_some_and(|at|std::time::Instant::now().checked_add(std::time::Duration::from_millis(at.saturating_sub(now).max(0) as u64)).is_none()) {
                return Err(refusal("bound_unavailable","wall deadline exceeds the native timer range",None));
            }
            let prepared=prepared.map_err(|e|refusal("bound_unavailable",&e.0,None))?;
            let cost=price.as_ref().map(|p|prepared.bound.cost_microusd(p)).transpose().map_err(|e|refusal("bound_unavailable",&e.0,None))?;
            let classes=prepared.bound.classes;
            let turns=if purpose=="turn" {
                let claimed=if let Some(call)=&call_id {tx.query_row("SELECT EXISTS(SELECT 1 FROM budget_reservation WHERE session_id=?1 AND call_id=?2 AND purpose='turn' AND ((state IN ('reserved','uncertain') AND turns_upper>0) OR actual_turns>0))",rusqlite::params![source,call],|row|row.get::<_,bool>(0))?}else{false};
                u64::from(!claimed)
            }else{0};
            let reservation=Request{
                request_id:cyber_core::ids::new_id("breq"),session_id:source,provider,model,purpose:purpose.clone(),call_id,
                bound_source:Some(prepared.bound.source),normalized_sha256:Some(prepared.bound.normalized_sha256),
                deadline_ms,
                estimate:Estimate{tokens:TokenAmounts{input:classes.input,output:classes.output,reasoning:classes.reasoning,cache_read:classes.cache_read,cache_write:classes.cache_write},total_tokens:prepared.bound.total_tokens,turns,cost_microusd:cost},
            };
            let admitted=ledger::reserve(tx,&reservation,&scopes,now).map_err(cyber_store::StoreError::from)?;
            if !admitted.new {return Err(refusal("conflict","duplicate request cannot dispatch",None));}
            let origin = serde_json::to_string(&origin).map_err(|e|refusal("invalid",&e.to_string(),None))?;
            tx.execute("UPDATE budget_reservation SET origin_context=?2 WHERE request_id=?1",rusqlite::params![reservation.request_id,origin])?;
            Ok((prepared.request,Some(reservation)))
        }).map_err(error)?;
        drop(drains);
        let attempt = admitted.1.map(|reservation| {
            Arc::new(Attempt::new(
                self.inner.me.clone(),
                handle,
                reservation,
                self.resolved.cost.clone(),
                Arc::clone(&self.failure),
            ))
        });
        Ok((admitted.0, attempt))
    }
}

impl Adapter for GatedAdapter<'_> {
    fn stream(&self, request: LlmRequest) -> BoxFuture<'_, Result<EventStream, LlmError>> {
        Box::pin(async move {
            // Existing main/compaction gates check each retry. Other soft callers
            // retain their own historical boundaries; reserved admission is always
            // per request regardless of purpose.
            if matches!(self.purpose, "turn" | "compaction") {
                self.inner
                    .check_budget(self.handle)
                    .await
                    .map_err(|e| self.fail(e))?;
            }
            let (request, attempt) = self.begin(request).await.map_err(|e| self.fail(e))?;
            let Some(attempt) = attempt else {
                return self.resolved.adapter.stream(request).await;
            };
            self.attempts
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(Arc::clone(&attempt));
            attempt.mark_dispatched().map_err(|e| self.fail(e))?;
            let mut lease = super::stream::Lease::new(Arc::clone(&attempt));
            let opened = if let Some(deadline) = attempt.deadline {
                match tokio::time::timeout_at(deadline, self.resolved.adapter.stream(request)).await
                {
                    Ok(opened) => opened,
                    Err(_) => {
                        attempt.ended(false);
                        attempt.flush().await.map_err(|e| self.fail(e))?;
                        lease.disarm();
                        return Err(self.fail(attempt.wall_error()));
                    }
                }
            } else {
                self.resolved.adapter.stream(request).await
            };
            match opened {
                Ok(stream) => Ok(super::stream::wrap(stream, attempt, lease)),
                Err(error) => {
                    attempt.ended(false);
                    attempt.flush().await.map_err(|e| self.fail(e))?;
                    lease.disarm();
                    Err(error)
                }
            }
        })
    }
}

impl GatedAdapter<'_> {
    fn fail(&self, error: RuntimeError) -> LlmError {
        let message = error.to_string();
        *self.failure.lock().unwrap_or_else(PoisonError::into_inner) = Some(error);
        LlmError::new(ErrorKind::InvalidRequest, message)
    }
}
