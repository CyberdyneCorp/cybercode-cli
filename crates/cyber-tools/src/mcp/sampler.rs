//! Retained sampling owners outlive a disposed RPC until local transport and billing settle.
use super::sampling::{SamplingHandler, SamplingRequest};
use crate::host::{BuiltinHost, Ctx};
use crate::permissions::Request;
use cyber_llm::catalog::{ModelRole, compute_cost};
use cyber_llm::{FinishReason, LlmEvent, LlmRequest, Usage};
use cyber_sandbox::proxy::{Endpoint, Proxy};
use cyber_server::runtime::{AuxiliaryUsage, Invocation, ResolvedModel};
use futures::{StreamExt, future::BoxFuture};
use serde_json::{Value, json};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

pub(super) struct Sampler {
    host: Arc<BuiltinHost>,
    inv: Invocation,
    server: String,
    timeout: Duration,
    cancel: CancellationToken,
    tasks: Mutex<Vec<JoinHandle<()>>>,
}
impl Sampler {
    pub(super) fn new(
        host: Arc<BuiltinHost>,
        inv: Invocation,
        server: String,
        timeout: Duration,
    ) -> Self {
        Self {
            host,
            inv,
            server,
            timeout,
            cancel: CancellationToken::new(),
            tasks: Mutex::new(Vec::new()),
        }
    }
    pub(super) async fn finish(&self) -> Result<(), &'static str> {
        self.cancel.cancel();
        let cleanup_failed = self.inv.asker.cancel_requests().await.is_err();
        let tasks = std::mem::take(
            &mut *self
                .tasks
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        let mut failed = cleanup_failed;
        for task in tasks {
            failed |= task.await.is_err();
        }
        if failed {
            Err("Sampling owner settlement failed")
        } else {
            Ok(())
        }
    }
}
impl Drop for Sampler {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}
impl SamplingHandler for Sampler {
    fn sample<'a>(&'a self, params: &'a Value) -> BoxFuture<'a, (Value, bool)> {
        Box::pin(async move {
            let parsed = match SamplingRequest::parse(params) {
                Ok(parsed) => parsed,
                Err(error) => return (json!({"error":{"code":-32602,"message":error}}), false),
            };
            let host = self.host.clone();
            let inv = self.inv.clone();
            let server = self.server.clone();
            let cancel = self.cancel.child_token();
            let timeout = self.timeout;
            let (tx, rx) = tokio::sync::oneshot::channel();
            let task = tokio::spawn(async move {
                let activity = AtomicBool::new(false);
                let response =
                    execute(&host, &inv, &server, parsed, cancel, timeout, &activity).await;
                let response = match response {
                    Ok(value) => json!({"result":value}),
                    Err(error) => json!({"error":{"code":-32603,"message":error}}),
                };
                let _ = tx.send((response, activity.load(Ordering::Acquire)));
            });
            self.tasks
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(task);
            rx.await.unwrap_or_else(|_| {
                (
                    json!({"error":{"code":-32603,"message":"Sampling owner failed"}}),
                    false,
                )
            })
        })
    }
}

fn authorized(host: &BuiltinHost, inv: &Invocation) -> Result<(), &'static str> {
    host.authorize_mcp_sampling(inv)
        .map_err(|_| "Sampling registration is no longer authorized")?;
    let config = host
        .hook_config
        .get()
        .ok_or("Sampling resolver is unavailable")?;
    let resolved = (config.resolve)(std::path::Path::new(&inv.directory))
        .map_err(|_| "Sampling configuration is unavailable")?;
    if !cyber_core::config::McpSettings::from_config(&resolved.value)
        .map_err(|_| "Sampling configuration is invalid")?
        .sampling_enabled
    {
        return Err("Sampling is disabled");
    }
    Ok(())
}

async fn execute(
    host: &BuiltinHost,
    inv: &Invocation,
    server: &str,
    parsed: SamplingRequest,
    cancel: CancellationToken,
    timeout: Duration,
    activity: &AtomicBool,
) -> Result<Value, &'static str> {
    authorized(host, inv)?;
    let policy = host
        .policy(inv)
        .await
        .map_err(|_| "Sampling permission policy is unavailable")?;
    let ctx = Ctx {
        host,
        inv,
        policy,
        location: inv.directory.clone().into(),
        cancel: cancel.clone(),
        hook_decision: None,
    };
    let permission = Request {
        action: "mcp_sampling".into(),
        resources: vec![server.into()],
        ..Default::default()
    };
    let before = inv.asker.requests_opened();
    let granted = Box::pin(ctx.authorize(
        permission,
        vec![server.into()],
        json!({"server":server,"purpose":"mcp_sampling"}),
    ))
    .await;
    activity.store(inv.asker.requests_opened() != before, Ordering::Release);
    granted.map_err(|_| "Sampling permission was refused")?;
    authorized(host, inv)?;
    if cancel.is_cancelled() {
        return Err("Sampling was cancelled");
    }
    inv.asker
        .check_budget()
        .await
        .map_err(|_| "Sampling budget refused")?;
    let runtime = host.runtime().ok_or("Sampling runtime is unavailable")?;
    let models = runtime.model_resolver();
    let reference = models
        .role(ModelRole::Small)
        .ok_or("No small model is configured")?;
    let mut model = models
        .resolve(&reference)
        .map_err(|_| "Configured small model is unavailable")?;
    let request = parsed.request(
        &model.template,
        model.template.max_output_tokens.unwrap_or(4096),
    )?;
    let proxy = transport(&mut model).await?;
    let started = Instant::now();
    let mut observed = Observed::default();
    let result = tokio::select! {
        biased;
        _ = cancel.cancelled() => Err("Sampling cancelled; remote completion and final usage are unverified"),
        _ = tokio::time::sleep(timeout) => Err("Sampling timed out; remote completion and final usage are unverified"),
        result = async {
            authorized(host, inv)?;
            activity.store(true, Ordering::Release);
            infer(&model, request, &mut observed).await
        } => result,
    };
    let settled = match proxy {
        Some(proxy) => matches!(
            tokio::time::timeout(Duration::from_secs(2), proxy.shutdown()).await,
            Ok(Ok(()))
        ),
        None => observed.ended,
    };
    bill(inv, server, &model, started, &observed).await?;
    if !settled {
        return Err("Sampling transport completion is unverified");
    }
    response(host, inv, &model, parsed, &cancel, result?).await
}

async fn response(
    host: &BuiltinHost,
    inv: &Invocation,
    model: &ResolvedModel,
    parsed: SamplingRequest,
    cancel: &CancellationToken,
    result: (String, &'static str),
) -> Result<Value, &'static str> {
    let (mut text, mut reason) = result;
    if let Some(end) = parsed
        .stop_sequences
        .iter()
        .filter_map(|stop| text.find(stop))
        .min()
    {
        text.truncate(end);
        reason = "stopSequence";
    }
    inv.asker
        .check_budget()
        .await
        .map_err(|_| "Sampling budget exceeded")?;
    authorized(host, inv)?;
    if cancel.is_cancelled() {
        return Err("Sampling was cancelled");
    }
    Ok(
        json!({"role":"assistant","content":{"type":"text","text":text},"model":model.model,"stopReason":reason}),
    )
}

async fn bill(
    inv: &Invocation,
    server: &str,
    model: &ResolvedModel,
    started: Instant,
    observed: &Observed,
) -> Result<(), &'static str> {
    if observed.attempted {
        inv.asker
            .record_model_usage(AuxiliaryUsage {
                provider: model.provider.clone(),
                model: model.model.clone(),
                purpose: format!("mcp_sampling:{server}"),
                call_id: Some(inv.call_id.clone()),
                duration_ms: started.elapsed().as_millis().min(u64::MAX as u128) as u64,
                usage: observed.usage,
                cost: observed
                    .metered
                    .then(|| compute_cost(model.cost.as_ref(), &observed.usage))
                    .flatten(),
            })
            .await
            .map_err(|_| "Sampling usage settlement failed")?;
    }
    Ok(())
}

async fn transport(model: &mut ResolvedModel) -> Result<Option<Proxy>, &'static str> {
    let proxy = Proxy::start(Arc::new(|_| Box::pin(async { true })))
        .await
        .map_err(|_| "Sampling transport preparation failed")?;
    let Endpoint::Tcp(port) = proxy.endpoint else {
        return Err("Sampling requires a TCP proxy");
    };
    let client = cyber_llm::adapters::Endpoint::client_builder()
        .no_proxy()
        .proxy(
            reqwest::Proxy::all(format!("http://127.0.0.1:{port}"))
                .map_err(|_| "Sampling proxy configuration failed")?,
        )
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .pool_max_idle_per_host(0)
        .build()
        .map_err(|_| "Sampling client preparation failed")?;
    if let Some(adapter) = model.adapter.with_http_client(client, super::LIMIT) {
        model.adapter = Arc::from(adapter);
        Ok(Some(proxy))
    } else {
        proxy
            .shutdown()
            .await
            .map_err(|_| "Unused sampling proxy settlement failed")?;
        Ok(None)
    }
}
#[derive(Default)]
struct Observed {
    attempted: bool,
    ended: bool,
    metered: bool,
    usage: Usage,
}
async fn infer(
    model: &ResolvedModel,
    request: LlmRequest,
    observed: &mut Observed,
) -> Result<(String, &'static str), &'static str> {
    observed.attempted = true;
    let mut stream = model
        .adapter
        .stream(request)
        .await
        .map_err(|_| "Sampling provider request failed")?;
    let mut text = String::new();
    let mut bytes = 0usize;
    let mut finish = None;
    while let Some(event) = stream.next().await {
        match event.map_err(|_| "Sampling provider stream failed")? {
            LlmEvent::TextDelta { text: delta } => {
                bytes = bytes.saturating_add(delta.len());
                text.push_str(&delta);
            }
            LlmEvent::ReasoningDelta { text } => bytes = bytes.saturating_add(text.len()),
            LlmEvent::Usage(usage) => {
                observed.usage.add(&usage);
                observed.metered = true;
            }
            LlmEvent::Finish { reason } => finish = Some(reason),
            LlmEvent::ToolCallDone(_) | LlmEvent::ToolCallDelta { .. } => {
                return Err("Sampling provider attempted a tool call");
            }
            LlmEvent::ReasoningSignature { .. } => {}
        }
        if bytes > super::LIMIT / 2 {
            return Err("Sampling response exceeds its byte allowance");
        }
    }
    observed.ended = true;
    let reason = match finish {
        Some(FinishReason::Stop) => "endTurn",
        Some(FinishReason::Length) => "maxTokens",
        _ => return Err("Sampling provider did not complete a basic response"),
    };
    Ok((text, reason))
}
