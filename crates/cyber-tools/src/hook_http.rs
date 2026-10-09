//! HTTP hooks with durable receipts, explicit network policy and owned transport cleanup.
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use cyber_core::config::{HookKind, Resolved};
use cyber_core::hooks::{HookDecision, HookDefinition, HookEvent};
use cyber_core::trust::{HookInvocationTrust, TrustStore};
use cyber_sandbox::proxy::{Decide, Endpoint, Proxy};
use cyber_sandbox::{NetworkMode, SandboxConfig};
use cyber_server::runtime::{HookExecutionResult, Runtime};
use futures::StreamExt;
use reqwest::header::{CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue};
use tokio_util::sync::CancellationToken;

use crate::BuiltinHost;
use crate::hook_commands::{
    HookCommandError, HookCommandReport, HookOutcome, interpret_hook_command,
};

const LIMIT: usize = 1024 * 1024;

pub struct HookHttpRunner<'a> {
    pub resolved: &'a Resolved,
    pub trust: &'a TrustStore,
    pub invocation_trust: Option<&'a HookInvocationTrust>,
    pub home: &'a Path,
}

struct Capture {
    report: HookCommandReport,
    io: Option<cyber_server::runtime::HookExecutionIo>,
}

impl Capture {
    fn settlement(&self) -> HookExecutionResult {
        HookExecutionResult {
            outcome: self.report.outcome,
            decision: self.report.decision.clone(),
            acknowledged: self.report.acknowledged,
            must_stop: self.report.must_stop,
            io: self.io.clone(),
        }
    }
}

impl HookHttpRunner<'_> {
    fn authorize(
        &self,
        pointer: &str,
        event: &HookEvent,
    ) -> Result<(HookDefinition, bool), String> {
        crate::hook_authority::authorize(
            self.resolved,
            self.trust,
            self.invocation_trust,
            pointer,
            event,
            HookKind::Http,
        )
    }

    pub async fn run_test(
        &self,
        host: &BuiltinHost,
        pointer: &str,
        event: &HookEvent,
        cancel: CancellationToken,
    ) -> Result<HookCommandReport, String> {
        if !event.is_synthetic() {
            return Err("HTTP hook test requires synthetic identity".into());
        }
        let (definition, _) = self.authorize(pointer, event)?;
        if cancel.is_cancelled() {
            return Err("HTTP hook cancelled before admission".into());
        }
        let deadline =
            tokio::time::Instant::now() + Duration::from_secs(definition.handler.timeout.into());
        let Some(owner) = cyber_server::runtime::try_start_synthetic_hook_execution(
            host.opts.store.clone(),
            event,
            &definition,
            log_io(self.resolved),
        )
        .map_err(|error| error.to_string())?
        else {
            return Ok(skipped(false));
        };
        let lease = tokio::time::timeout_at(
            deadline,
            host.claim_hook_test_location(event, &owner.record().id, cancel.clone()),
        )
        .await
        .map_err(|_| "HTTP hook checkout admission timed out".to_string())??;
        let capture = self.execute(pointer, event, deadline, cancel, None).await;
        let retained = if capture.report.acknowledged {
            Some(lease.settle_retained()?)
        } else {
            drop(lease);
            None
        };
        owner
            .finish(capture.settlement())
            .map_err(|error| error.to_string())?;
        drop(retained);
        Ok(capture.report)
    }

    pub async fn run_recorded(
        &self,
        runtime: &Runtime,
        pointer: &str,
        event: &HookEvent,
        cancel: CancellationToken,
    ) -> Result<HookCommandReport, String> {
        let (definition, _) = self.authorize(pointer, event)?;
        if cancel.is_cancelled() {
            return Err("HTTP hook cancelled before admission".into());
        }
        let Some(mut owner) = runtime
            .try_start_hook_execution(event, &definition, log_io(self.resolved))
            .await
            .map_err(|error| error.to_string())?
        else {
            return Ok(skipped(false));
        };
        if let Some(message) = &definition.handler.status_message {
            runtime.hook_notice(
                &event.identity().session_id,
                &owner.record().hook_id,
                message,
            );
        }
        let stop = owner.cancellation();
        let deadline =
            tokio::time::Instant::now() + Duration::from_secs(definition.handler.timeout.into());
        let mut capture = {
            let execution = self.execute(
                pointer,
                event,
                deadline,
                stop.clone(),
                Some((&mut owner, runtime)),
            );
            tokio::pin!(execution);
            tokio::select! {
                biased;
                _ = cancel.cancelled() => { stop.cancel(); execution.await }
                capture = &mut execution => capture,
            }
        };
        capture.report.must_stop |= owner.verify(runtime).is_err();
        let record = owner
            .finish(capture.settlement())
            .map_err(|error| error.to_string())?;
        if record.acknowledged == Some(true)
            && let Some(message) = &definition.handler.system_message
        {
            runtime.hook_notice(&record.session_id, &record.hook_id, message);
        }
        Ok(capture.report)
    }

    async fn execute(
        &self,
        pointer: &str,
        event: &HookEvent,
        deadline: tokio::time::Instant,
        cancel: CancellationToken,
        owner: Option<(&mut cyber_server::runtime::HookExecution, &Runtime)>,
    ) -> Capture {
        let (definition, sandbox_all) = match self.authorize(pointer, event) {
            Ok(value) => value,
            Err(error) => {
                return Capture {
                    report: failure(event, None, true, HookOutcome::Error, error, true),
                    io: None,
                };
            }
        };
        let prepared = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Capture { report: skipped(true), io: None },
            _ = tokio::time::sleep_until(deadline) => return Capture { report: failure(event, Some(&definition), definition.handler.fail_closed, HookOutcome::Timeout, "HTTP hook preparation timed out".into(), true), io: None },
            result = self.prepare(&definition, sandbox_all) => result,
        };
        let prepared = match prepared {
            Ok(prepared) => prepared,
            Err(error) => {
                return Capture {
                    report: failure(
                        event,
                        Some(&definition),
                        definition.handler.fail_closed,
                        HookOutcome::Error,
                        error,
                        true,
                    ),
                    io: None,
                };
            }
        };
        let authorization = self.authorize(pointer, event).and_then(|_| {
            if let Some((owner, runtime)) = owner {
                owner
                    .mark_launch(runtime)
                    .map_err(|error| error.to_string())?;
            }
            Ok(())
        });
        let mut capture = if let Err(error) = authorization {
            Capture {
                report: failure(
                    event,
                    Some(&definition),
                    true,
                    HookOutcome::Error,
                    error,
                    true,
                ),
                io: None,
            }
        } else {
            tokio::select! {
                biased;
                _ = cancel.cancelled() => Capture { report: skipped(true), io: None },
                _ = tokio::time::sleep_until(deadline) => Capture { report: failure(event, Some(&definition), definition.handler.fail_closed, HookOutcome::Timeout, "HTTP hook timed out; remote processing is unverified".into(), true), io: None },
                capture = self.post(&prepared, &definition, event) => capture,
            }
        };
        // Closing/joining the owned proxy settles every local request socket, even
        // when reqwest retains a background connection after its future is dropped.
        if !matches!(
            tokio::time::timeout(Duration::from_secs(2), prepared.proxy.shutdown()).await,
            Ok(Ok(()))
        ) {
            capture.report = failure(
                event,
                Some(&definition),
                true,
                HookOutcome::Error,
                "HTTP hook transport shutdown was not acknowledged".into(),
                false,
            );
            capture.io = None;
        }
        capture
    }

    async fn prepare(
        &self,
        definition: &HookDefinition,
        sandbox_all: bool,
    ) -> Result<Prepared, String> {
        let url = reqwest::Url::parse(
            definition
                .handler
                .url
                .as_deref()
                .ok_or("missing HTTP hook URL")?,
        )
        .map_err(|_| "invalid HTTP hook URL".to_string())?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err("HTTP hook requires http(s) URL".into());
        }
        let headers = headers(&definition.handler.headers)?;
        let sandbox = SandboxConfig::resolve(
            &self.resolved.value,
            &self.resolved.sources,
            None,
            self.home,
        );
        let restricted = definition.scope.requires_sandbox(sandbox_all);
        let network = if restricted {
            sandbox.network
        } else {
            NetworkMode::On
        };
        if network == NetworkMode::Off {
            return Err("HTTP hook network is disabled".into());
        }
        let host = url.host_str().ok_or("HTTP hook URL has no host")?;
        let host = host
            .strip_prefix('[')
            .and_then(|host| host.strip_suffix(']'))
            .unwrap_or(host);
        if network == NetworkMode::Proxy
            && !cyber_sandbox::matches_domain(host, &sandbox.allowed_domains)
        {
            return Err("HTTP hook destination is outside the network allowlist".into());
        }
        let decide: Decide = Arc::new(move |host| {
            let allowed = network == NetworkMode::On
                || cyber_sandbox::matches_domain(&host, &sandbox.allowed_domains);
            Box::pin(async move { allowed })
        });
        let proxy = Proxy::start(decide)
            .await
            .map_err(|_| "could not start HTTP hook proxy".to_string())?;
        let Endpoint::Tcp(port) = proxy.endpoint else {
            return Err("HTTP hook requires a TCP proxy".into());
        };
        let client = reqwest::Client::builder()
            .no_proxy()
            .proxy(
                reqwest::Proxy::all(format!("http://127.0.0.1:{port}"))
                    .map_err(|_| "invalid HTTP hook proxy".to_string())?,
            )
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .pool_max_idle_per_host(0)
            .build()
            .map_err(|_| "could not create HTTP hook client".to_string())?;
        Ok(Prepared {
            client,
            proxy,
            url,
            headers,
        })
    }

    async fn post(
        &self,
        prepared: &Prepared,
        definition: &HookDefinition,
        event: &HookEvent,
    ) -> Capture {
        let response = prepared
            .client
            .post(prepared.url.clone())
            .headers(prepared.headers.clone())
            .json(event)
            .send()
            .await;
        let body = match response {
            Ok(response) if response.status().is_success() => read_body(response).await,
            Ok(response) => Err(format!(
                "HTTP hook returned status {}",
                response.status().as_u16()
            )),
            Err(error) => Err(format!(
                "HTTP hook transport failed: {}",
                error.without_url()
            )),
        };
        match body {
            Ok(bytes) => {
                let parsed = serde_json::from_slice(&bytes)
                    .map_err(|error| format!("invalid HTTP hook JSON: {error}"))
                    .and_then(|value| HookDecision::parse(event.event(), &value));
                let report = match parsed {
                    Ok(parsed) => {
                        let blocked = matches!(
                            parsed.decision.decision,
                            Some(
                                cyber_core::hooks::HookAction::Deny
                                    | cyber_core::hooks::HookAction::Block
                            )
                        );
                        HookCommandReport {
                            outcome: if blocked {
                                HookOutcome::Blocked
                            } else {
                                HookOutcome::Ok
                            },
                            decision: parsed.decision,
                            ignored_fields: parsed.ignored_fields,
                            diagnostic: None,
                            acknowledged: true,
                            must_stop: false,
                        }
                    }
                    Err(error) => failure(
                        event,
                        Some(definition),
                        definition.handler.fail_closed,
                        HookOutcome::Error,
                        error,
                        true,
                    ),
                };
                let io = log_io(self.resolved).then(|| logged_io(event, &bytes));
                Capture { report, io }
            }
            Err(error) => Capture {
                report: failure(
                    event,
                    Some(definition),
                    definition.handler.fail_closed,
                    HookOutcome::Error,
                    error,
                    true,
                ),
                io: None,
            },
        }
    }
}

struct Prepared {
    client: reqwest::Client,
    proxy: Proxy,
    url: reqwest::Url,
    headers: HeaderMap,
}

fn log_io(resolved: &Resolved) -> bool {
    resolved.value["telemetry"]["log_hook_io"]
        .as_bool()
        .unwrap_or(false)
}

fn headers(configured: &std::collections::BTreeMap<String, String>) -> Result<HeaderMap, String> {
    let mut headers = HeaderMap::new();
    for (name, value) in configured {
        let name = HeaderName::from_bytes(name.as_bytes())
            .map_err(|_| "invalid HTTP hook header name".to_string())?;
        if matches!(
            name.as_str(),
            "host"
                | "content-length"
                | "transfer-encoding"
                | "connection"
                | "proxy-authorization"
                | "proxy-connection"
        ) {
            return Err(format!("HTTP hook cannot override transport header {name}"));
        }
        headers.insert(
            name,
            HeaderValue::from_str(value)
                .map_err(|_| "invalid HTTP hook header value".to_string())?,
        );
    }
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    Ok(headers)
}

async fn read_body(response: reqwest::Response) -> Result<Vec<u8>, String> {
    if response
        .content_length()
        .is_some_and(|length| length > LIMIT as u64)
    {
        return Err("HTTP hook response exceeds 1 MiB".into());
    }
    let mut bytes = Vec::new();
    let mut chunks = response.bytes_stream();
    while let Some(chunk) = chunks.next().await {
        let chunk =
            chunk.map_err(|error| format!("HTTP hook response failed: {}", error.without_url()))?;
        if chunk.len() > LIMIT - bytes.len() {
            return Err("HTTP hook response exceeds 1 MiB".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn failure(
    event: &HookEvent,
    definition: Option<&HookDefinition>,
    fail_closed: bool,
    outcome: HookOutcome,
    message: String,
    acknowledged: bool,
) -> HookCommandReport {
    let id = definition
        .map(|definition| {
            definition
                .handler
                .id
                .as_deref()
                .unwrap_or(&definition.digest)
        })
        .unwrap_or("HTTP");
    let mut report = interpret_hook_command(
        event,
        id,
        fail_closed,
        Err(HookCommandError {
            message,
            acknowledged,
        }),
    );
    report.outcome = outcome;
    report
}

fn skipped(must_stop: bool) -> HookCommandReport {
    HookCommandReport {
        outcome: HookOutcome::Skipped,
        decision: Default::default(),
        ignored_fields: Vec::new(),
        diagnostic: None,
        acknowledged: true,
        must_stop,
    }
}

fn logged_io(event: &HookEvent, bytes: &[u8]) -> cyber_server::runtime::HookExecutionIo {
    let mut truncated = false;
    let mut bound = |mut value: String| {
        if value.len() > LIMIT {
            truncated = true;
            let mut end = LIMIT;
            while !value.is_char_boundary(end) {
                end -= 1;
            }
            value.truncate(end);
        }
        value
    };
    cyber_server::runtime::HookExecutionIo {
        stdin: bound(format!("{}", event.as_json())),
        stdout: bound(String::from_utf8_lossy(bytes).into_owned()),
        stderr: String::new(),
        truncated,
    }
}
