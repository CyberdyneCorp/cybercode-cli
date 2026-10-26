//! Narrow owner-side GitHub adapter for PR auto-fix. Credentials never enter a Session.
//!
//! REST contracts: https://docs.github.com/en/rest/checks/runs,
//! /commits/statuses, /pulls/comments, /pulls/reviews and /collaborators/collaborators.
//! All pagination is completed before returning observations; missing/truncated evidence
//! is an error, never a successful CI result. Only github.com is supported in production.
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use cyber_core::autofix::{Subscription, Trigger, TriggerKind, valid_sha};
use reqwest::{Client, Method, StatusCode};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;
use tokio::sync::Mutex;

const PAGE_SIZE: usize = 100;
const MAX_PAGES: usize = 10;
const MAX_BODY: usize = 2 * 1024 * 1024;
const POLL_MS: i64 = 60_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub repository_id: u64,
    pub repository: String,
    pub head: String,
    pub branch: String,
    pub closed: bool,
    pub same_repo: bool,
    pub can_write: bool,
    pub viewer: String,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CiOutcome {
    Pending,
    Success,
    Failure,
}
#[derive(Debug, Clone)]
pub struct Poll {
    pub snapshot: Snapshot,
    pub triggers: Vec<Trigger>,
    pub ci: CiOutcome,
    pub ci_head: String,
    pub poll_after_ms: i64,
}
/// Messages intentionally exclude URLs, response bodies and authentication material.
#[derive(Debug, Clone)]
pub struct Error {
    message: String,
    pub retry_after_ms: i64,
    /// Safe to retry an observation. Mutations still require receipt reconciliation.
    pub retryable: bool,
}
impl Error {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            retry_after_ms: POLL_MS,
            retryable: false,
        }
    }
    fn network(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            retry_after_ms: POLL_MS,
            retryable: true,
        }
    }
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for Error {}

#[derive(Clone)]
struct Page {
    value: Value,
    next: bool,
}
#[derive(Clone)]
struct Cached {
    etag: String,
    page: Page,
}
/// No Debug implementation: this object contains an owner credential.
#[derive(Clone)]
pub struct GitHub {
    client: Client,
    token: Arc<String>,
    base: String,
    cache: Arc<Mutex<BTreeMap<String, Cached>>>,
    gate: Arc<Mutex<()>>,
    poll_ms: Arc<std::sync::atomic::AtomicI64>,
}
impl GitHub {
    #[cfg(test)]
    pub(super) fn fixture(base: String) -> Result<Self, Error> {
        Self::new("fixture-token".into(), base)
    }
    /// Uses the owner's github.com login, independently of any provider login.
    pub async fn from_gh() -> Result<Self, Error> {
        let mut child = tokio::process::Command::new("gh")
            .args(["auth", "token", "--hostname", "github.com"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|_| {
                Error::new(
                    "GitHub CLI unavailable; sign in with gh auth login --hostname github.com",
                )
            })?;
        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| Error::new("GitHub credential pipe unavailable"))?;
        let result = tokio::time::timeout(Duration::from_secs(10), async {
            let mut bytes = vec![];
            (&mut stdout)
                .take(16_385)
                .read_to_end(&mut bytes)
                .await
                .map_err(|_| Error::new("GitHub credential unavailable"))?;
            if bytes.len() > 16_384 {
                return Err(Error::new("GitHub credential exceeds limit"));
            }
            let status = child
                .wait()
                .await
                .map_err(|_| Error::new("GitHub credential helper failed"))?;
            if !status.success() {
                return Err(Error::new(
                    "GitHub login unavailable; run gh auth login --hostname github.com",
                ));
            }
            let token = String::from_utf8(bytes)
                .map_err(|_| Error::new("GitHub credential is invalid"))?
                .trim()
                .to_owned();
            Self::new(token, "https://api.github.com".into())
        })
        .await
        .map_err(|_| Error::new("GitHub credential helper timed out"))?;
        let _ = child.kill().await;
        result
    }
    fn new(token: String, base: String) -> Result<Self, Error> {
        if token.is_empty() || token.contains(['\n', '\r', '\0']) {
            return Err(Error::new("GitHub credential is invalid"));
        }
        let client = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(20))
            .connect_timeout(Duration::from_secs(10))
            .user_agent("cyber-code-pr-autofix")
            .build()
            .map_err(|_| Error::new("GitHub HTTP client unavailable"))?;
        Ok(Self {
            client,
            token: Arc::new(token),
            base,
            cache: Arc::new(Mutex::new(BTreeMap::new())),
            gate: Arc::new(Mutex::new(())),
            poll_ms: Arc::new(std::sync::atomic::AtomicI64::new(POLL_MS)),
        })
    }
    async fn request(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Page, Error> {
        // Paths are generated here from validated repository names, numeric IDs and SHAs.
        if !path.starts_with('/') || path.contains(['\r', '\n', '#']) {
            return Err(Error::new("invalid GitHub request path"));
        }
        let _serial = self.gate.lock().await;
        let cached = if method == Method::GET {
            self.cache.lock().await.get(path).cloned()
        } else {
            None
        };
        let mut request = self
            .client
            .request(method.clone(), format!("{}{path}", self.base))
            .bearer_auth(self.token.as_str())
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2026-03-10");
        if let Some(c) = &cached {
            request = request.header("If-None-Match", &c.etag)
        }
        if let Some(body) = body {
            request = request.json(&body)
        }
        let response = request.send().await.map_err(|_| {
            Error::network(
                "GitHub request failed or timed out; external mutation outcome may be unknown",
            )
        })?;
        let poll = header_seconds(response.headers(), "x-poll-interval")
            .saturating_mul(1000)
            .clamp(POLL_MS, 86_400_000);
        self.poll_ms
            .fetch_max(poll, std::sync::atomic::Ordering::Relaxed);
        if response.status() == StatusCode::NOT_MODIFIED {
            return cached
                .map(|c| c.page)
                .ok_or_else(|| Error::new("GitHub returned an uncached conditional response"));
        }
        if !response.status().is_success() {
            return Err(http_error(response.status(), response.headers()));
        }
        let (page, etag) = body_page(response).await?;
        if method == Method::GET {
            self.cache_page(path, etag, &page).await;
        }

        Ok(page)
    }
    async fn cache_page(&self, path: &str, etag: Option<String>, page: &Page) {
        let Some(etag) = etag else {
            return;
        };
        let mut cache = self.cache.lock().await;
        if cache.len() >= 32
            && !cache.contains_key(path)
            && let Some(first) = cache.keys().next().cloned()
        {
            cache.remove(&first);
        }
        cache.insert(
            path.into(),
            Cached {
                etag,
                page: page.clone(),
            },
        );
    }
    async fn get(&self, path: &str) -> Result<Value, Error> {
        Ok(self.request(Method::GET, path, None).await?.value)
    }
    async fn pages(&self, path: &str, field: Option<&str>) -> Result<Vec<Value>, Error> {
        let separator = if path.contains('?') { '&' } else { '?' };
        let mut rows = vec![];
        for page in 1..=MAX_PAGES {
            let response = self
                .request(
                    Method::GET,
                    &format!("{path}{separator}per_page={PAGE_SIZE}&page={page}"),
                    None,
                )
                .await?;
            let data = match field {
                Some(field) => response.value.get(field),
                None => Some(&response.value),
            };
            let values = data
                .and_then(Value::as_array)
                .ok_or_else(|| Error::new("GitHub list response is incomplete"))?;
            if values.len() > PAGE_SIZE {
                return Err(Error::new("GitHub page exceeds requested size"));
            }
            rows.extend(values.iter().cloned());
            if !response.next {
                if response
                    .value
                    .get("total_count")
                    .and_then(Value::as_u64)
                    .is_some_and(|n| n > rows.len() as u64)
                {
                    return Err(Error::new("GitHub list response is truncated"));
                }
                return Ok(rows);
            }
        }
        Err(Error::new(
            "GitHub pagination limit reached; observation incomplete",
        ))
    }
    pub async fn inspect(&self, repository: &str, pr: u64) -> Result<Snapshot, Error> {
        validate_repository(repository)?;
        if pr == 0 {
            return Err(Error::new("invalid PR number"));
        }
        let repo = self.get(&format!("/repos/{repository}")).await?;
        let pull = self.get(&format!("/repos/{repository}/pulls/{pr}")).await?;
        let viewer = self.get("/user").await?;
        let repository_id = number(&repo, "id")?;
        let canonical = text(&repo, "full_name")?.to_owned();
        validate_repository(&canonical)?;
        if !canonical.eq_ignore_ascii_case(repository) {
            return Err(Error::new("GitHub repository identity changed"));
        }
        let head = text(&pull["head"], "sha")?.to_owned();
        if !valid_sha(&head) {
            return Err(Error::new("GitHub returned invalid PR head"));
        }
        let branch = text(&pull["head"], "ref")?.to_owned();
        if branch.is_empty() || branch.len() > 1024 || branch.contains(['\0', '\r', '\n']) {
            return Err(Error::new("GitHub returned invalid branch name"));
        }
        let same_repo = pull["head"]["repo"]["id"].as_u64() == Some(repository_id)
            && pull["base"]["repo"]["id"].as_u64() == Some(repository_id);
        let state = text(&pull, "state")?;
        if !matches!(state, "open" | "closed") {
            return Err(Error::new("GitHub returned unknown PR state"));
        }
        let viewer = text(&viewer, "login")?.to_owned();
        validate_login(&viewer)?;
        Ok(Snapshot {
            repository_id,
            repository: canonical,
            head,
            branch,
            closed: state == "closed" || pull["merged"].as_bool() == Some(true),
            same_repo,
            can_write: repo["permissions"]["push"].as_bool() == Some(true),
            viewer,
        })
    }
    async fn reviews(&self, repository: &str, pr: u64) -> Result<Vec<Review>, Error> {
        validate_repository(repository)?;
        let mut result = vec![];
        for (path, kind) in [
            (
                format!("/repos/{repository}/pulls/{pr}/comments?sort=updated&direction=asc"),
                "comment",
            ),
            (format!("/repos/{repository}/pulls/{pr}/reviews"), "review"),
        ] {
            for value in self.pages(&path, None).await? {
                let Some(body) = value["body"].as_str().filter(|s| !s.trim().is_empty()) else {
                    continue;
                };
                if kind == "review"
                    && !matches!(
                        value["state"].as_str(),
                        Some("COMMENTED" | "CHANGES_REQUESTED")
                    )
                {
                    continue;
                }
                let id = number(&value, "id")?;
                let login = text(&value["user"], "login")?.to_owned();
                validate_login(&login)?;
                let revision = value["updated_at"]
                    .as_str()
                    .or(value["submitted_at"].as_str())
                    .ok_or_else(|| Error::new("GitHub review revision is missing"))?;
                let digest = format!("{:x}", Sha256::digest(format!("{revision}\0{body}")));
                let key = format!("{kind}:{id}:{digest}");
                result.push(Review {
                    key,
                    login,
                    is_bot: value["user"]["type"].as_str() != Some("User"),
                    text: bounded(
                        &format!(
                            "Review feedback by {} ({kind} {id}):\n{body}",
                            value["user"]["login"].as_str().unwrap_or("unknown")
                        ),
                        16_384,
                    ),
                });
            }
        }
        Ok(result)
    }
    /// Seed durable `seen` keys at enable time so historical reviews do not start fixes.
    pub async fn baseline_reviews(&self, repository: &str, pr: u64) -> Result<Vec<String>, Error> {
        Ok(self
            .reviews(repository, pr)
            .await?
            .into_iter()
            .map(|r| r.key)
            .collect())
    }
    async fn writer(&self, repository: &str, login: &str) -> Result<bool, Error> {
        validate_login(login)?;
        let value = self
            .get(&format!(
                "/repos/{repository}/collaborators/{login}/permission"
            ))
            .await?;
        Ok(matches!(
            value["permission"].as_str(),
            Some("write" | "admin" | "maintain")
        ) || value["user"]["permissions"]["push"].as_bool() == Some(true))
    }
    pub async fn poll(&self, subscription: &Subscription) -> Result<Poll, Error> {
        let repository = &subscription.config.repository;
        let pr = subscription.config.pr;
        let snapshot = self.inspect(repository, pr).await?;
        if snapshot.repository_id != subscription.repository_id
            || snapshot.branch != subscription.branch
        {
            return Err(Error::new(
                "subscribed PR repository or branch identity changed",
            ));
        }
        let ci_head = subscription
            .active()
            .and_then(|a| a.commit.clone())
            .unwrap_or_else(|| snapshot.head.clone());
        let checks = self.checks(repository, &ci_head).await?;
        let mut triggers = if ci_head == snapshot.head {
            checks.triggers.clone()
        } else {
            vec![]
        };
        if !snapshot.closed && snapshot.same_repo && snapshot.can_write {
            for review in self.reviews(repository, pr).await? {
                if review.is_bot
                    || review.login.eq_ignore_ascii_case(&snapshot.viewer)
                    || subscription.seen.contains(&review.key)
                {
                    continue;
                }
                if self.writer(repository, &review.login).await? {
                    triggers.push(Trigger {
                        key: review.key,
                        head: snapshot.head.clone(),
                        kind: TriggerKind::Review,
                        text: review.text,
                    })
                }
            }
        }
        if triggers.len() > 100 {
            return Err(Error::new(
                "GitHub trigger batch exceeds limit; observation incomplete",
            ));
        }
        let ci = checks.outcome(&subscription.config.required_ci);
        Ok(Poll {
            snapshot,
            triggers,
            ci,
            ci_head,
            poll_after_ms: self.poll_ms.load(std::sync::atomic::Ordering::Relaxed),
        })
    }
    async fn checks(&self, repository: &str, head: &str) -> Result<Checks, Error> {
        if !valid_sha(head) {
            return Err(Error::new("invalid CI commit SHA"));
        }
        let runs = self
            .pages(
                &format!("/repos/{repository}/commits/{head}/check-runs?filter=latest"),
                Some("check_runs"),
            )
            .await?;
        let statuses = self
            .pages(
                &format!("/repos/{repository}/commits/{head}/statuses"),
                None,
            )
            .await?;
        let mut checks = parse_checks(runs.clone(), statuses, head)?;
        if checks.triggers.len() > 100 {
            return Err(Error::new("GitHub failing-check batch exceeds limit"));
        }
        for run in runs {
            let Some(count) = run["output"]["annotations_count"]
                .as_u64()
                .filter(|n| *n > 0)
            else {
                continue;
            };
            let id = number(&run, "id")?;
            let prefix = format!("check:{id}:");
            let Some(trigger) = checks
                .triggers
                .iter_mut()
                .find(|t| t.key.starts_with(&prefix))
            else {
                continue;
            };
            let annotations = self
                .pages(
                    &format!("/repos/{repository}/check-runs/{id}/annotations"),
                    None,
                )
                .await?;
            if annotations.len() < count as usize {
                return Err(Error::new("GitHub check annotations are incomplete"));
            }
            let feedback = annotation_text(&annotations);
            trigger.text = bounded(
                &format!("{}\n{}", bounded(&trigger.text, 8192), feedback),
                16384,
            );
        }
        Ok(checks)
    }
    /// The caller persists a notice intent BEFORE this call. On error it must keep the
    /// outcome unknown and reconcile with `find_notice`; do not call this method again.
    pub async fn publish_notice(&self, subscription: &Subscription) -> Result<String, Error> {
        if let Some(id) = self.find_notice(subscription).await? {
            return Ok(id);
        }
        let snapshot = self
            .inspect(&subscription.config.repository, subscription.config.pr)
            .await?;
        if !snapshot.can_write || snapshot.repository_id != subscription.repository_id {
            return Err(Error::new("GitHub notice authorization changed"));
        }
        let marker = notice_marker(subscription);
        let reason = subscription
            .reason
            .as_deref()
            .unwrap_or("consecutive attempt limit reached");
        let body = format!(
            "Cyber auto-fix stopped after {} consecutive failed attempts. {}\n\n{}",
            subscription.failures,
            bounded(reason, 500),
            marker
        );
        let value = self
            .request(
                Method::POST,
                &format!(
                    "/repos/{}/issues/{}/comments",
                    subscription.config.repository, subscription.config.pr
                ),
                Some(json!({"body":body})),
            )
            .await?
            .value;
        Ok(number(&value, "id")?.to_string())
    }
    pub async fn find_notice(&self, subscription: &Subscription) -> Result<Option<String>, Error> {
        validate_repository(&subscription.config.repository)?;
        let viewer = self.get("/user").await?;
        let viewer = number(&viewer, "id")?;
        let marker = notice_marker(subscription);
        let path = format!(
            "/repos/{}/issues/{}/comments",
            subscription.config.repository, subscription.config.pr
        );
        for value in self.pages(&path, None).await? {
            if value["user"]["id"].as_u64() == Some(viewer)
                && value["body"]
                    .as_str()
                    .is_some_and(|body| body.contains(&marker))
            {
                return Ok(Some(number(&value, "id")?.to_string()));
            }
        }
        Ok(None)
    }
}
fn http_error(status: StatusCode, headers: &reqwest::header::HeaderMap) -> Error {
    let mut error = Error::new(format!(
        "GitHub API refused request (HTTP {})",
        status.as_u16()
    ));
    let rate_limited = status == StatusCode::TOO_MANY_REQUESTS
        || (status == StatusCode::FORBIDDEN
            && (header_seconds(headers, "retry-after") > 0
                || headers
                    .get("x-ratelimit-remaining")
                    .and_then(|v| v.to_str().ok())
                    == Some("0")));
    error.retryable = rate_limited || status.is_server_error();
    if rate_limited {
        let retry = header_seconds(headers, "retry-after");
        let reset = header_seconds(headers, "x-ratelimit-reset")
            .saturating_sub(chrono::Utc::now().timestamp());
        error.retry_after_ms = retry
            .max(reset)
            .max(60)
            .saturating_mul(1000)
            .min(86_400_000);
    }
    error
}
async fn body_page(mut response: reqwest::Response) -> Result<(Page, Option<String>), Error> {
    if response
        .content_length()
        .is_some_and(|size| size > MAX_BODY as u64)
    {
        return Err(Error::new("GitHub response exceeds size limit"));
    }
    let next = response
        .headers()
        .get("link")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|s| s.split(',').any(|part| part.contains("rel=\"next\"")));
    let etag = response
        .headers()
        .get("etag")
        .and_then(|v| v.to_str().ok())
        .filter(|s| s.len() <= 1024)
        .map(str::to_owned);
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| Error::network("GitHub response was interrupted"))?
    {
        if bytes.len().saturating_add(chunk.len()) > MAX_BODY {
            return Err(Error::new("GitHub response exceeds size limit"));
        }
        bytes.extend_from_slice(&chunk);
    }
    let value =
        serde_json::from_slice(&bytes).map_err(|_| Error::new("GitHub returned invalid JSON"))?;
    Ok((Page { value, next }, etag))
}
fn header_seconds(headers: &reqwest::header::HeaderMap, key: &str) -> i64 {
    headers
        .get(key)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
        .max(0)
}
fn bounded(s: &str, limit: usize) -> String {
    let mut end = s.len().min(limit);
    while !s.is_char_boundary(end) {
        end -= 1
    }
    s[..end].into()
}
fn text<'a>(v: &'a Value, key: &str) -> Result<&'a str, Error> {
    v.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| Error::new("GitHub response is missing required identity data"))
}
fn number(v: &Value, key: &str) -> Result<u64, Error> {
    v.get(key)
        .and_then(Value::as_u64)
        .filter(|n| *n > 0 && *n <= i64::MAX as u64)
        .ok_or_else(|| Error::new("GitHub response is missing required numeric identity"))
}
fn validate_repository(repository: &str) -> Result<(), Error> {
    let parts = repository.split('/').collect::<Vec<_>>();
    if parts.len() != 2
        || parts.iter().any(|s| {
            s.is_empty()
                || *s == "."
                || *s == ".."
                || s.len() > 100
                || !s
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        })
    {
        Err(Error::new("invalid GitHub repository"))
    } else {
        Ok(())
    }
}
fn validate_login(login: &str) -> Result<(), Error> {
    if login.is_empty()
        || login.len() > 100
        || !login
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-[]".contains(&b))
    {
        Err(Error::new("invalid GitHub user identity"))
    } else {
        Ok(())
    }
}
fn notice_marker(subscription: &Subscription) -> String {
    format!(
        "<!-- cyber-autofix-exhausted:{:x} -->",
        Sha256::digest(&subscription.id)
    )
}
struct Review {
    key: String,
    login: String,
    is_bot: bool,
    text: String,
}
#[derive(Default)]
struct Checks {
    states: BTreeMap<String, CiOutcome>,
    triggers: Vec<Trigger>,
}
impl Checks {
    fn outcome(&self, required: &[String]) -> CiOutcome {
        if self.states.values().any(|s| *s == CiOutcome::Failure) {
            return CiOutcome::Failure;
        }
        if required.is_empty()
            || self.states.is_empty()
            || self.states.values().any(|s| *s == CiOutcome::Pending)
            || required
                .iter()
                .any(|name| self.states.get(name) != Some(&CiOutcome::Success))
        {
            CiOutcome::Pending
        } else {
            CiOutcome::Success
        }
    }
}
fn parse_checks(runs: Vec<Value>, statuses: Vec<Value>, head: &str) -> Result<Checks, Error> {
    let mut result = Checks::default();
    for run in runs {
        if run["head_sha"].as_str() != Some(head) {
            return Err(Error::new(
                "GitHub returned check evidence for a different commit",
            ));
        }
        let id = number(&run, "id")?;
        let provider = number(&run["app"], "id")?;
        let name = text(&run, "name")?;
        let canonical = format!("check:{provider}:{name}");
        let state = if run["status"].as_str() != Some("completed") {
            CiOutcome::Pending
        } else {
            match run["conclusion"].as_str() {
                Some("success" | "neutral" | "skipped") => CiOutcome::Success,
                Some("failure" | "timed_out" | "cancelled" | "action_required" | "stale") => {
                    CiOutcome::Failure
                }
                _ => CiOutcome::Pending,
            }
        };
        // Duplicate check names from the same provider are conservative: a pending or
        // failing sibling wins instead of whichever JSON row was last.
        result
            .states
            .entry(canonical.clone())
            .and_modify(|old| *old = combine(*old, state))
            .or_insert(state);
        if state == CiOutcome::Failure {
            let completed = text(&run, "completed_at")?;
            let revision = format!(
                "{:x}",
                Sha256::digest(format!("{completed}\0{}", run["conclusion"]))
            );
            let text = bounded(
                &format!(
                    "Failed GitHub check {canonical} (run {id}) on {head}.\n{}\n{}",
                    run["output"]["title"].as_str().unwrap_or(""),
                    run["output"]["summary"].as_str().unwrap_or("")
                ),
                16_384,
            );
            result.triggers.push(Trigger {
                key: format!("check:{id}:{revision}"),
                head: head.into(),
                kind: TriggerKind::CheckFailure,
                text,
            });
        }
    }
    // List statuses is reverse chronological. The latest state per context is authoritative;
    // its creator remains part of the canonical provider identity for required CI.
    let mut contexts = BTreeSet::new();
    for status in statuses {
        let context = text(&status, "context")?;
        if !contexts.insert(context.to_owned()) {
            continue;
        }
        let id = number(&status, "id")?;
        let provider = number(&status["creator"], "id")?;
        let canonical = format!("status:{provider}:{context}");
        let state = match status["state"].as_str() {
            Some("success") => CiOutcome::Success,
            Some("failure" | "error") => CiOutcome::Failure,
            _ => CiOutcome::Pending,
        };
        result.states.insert(canonical.clone(), state);
        if state == CiOutcome::Failure {
            result.triggers.push(Trigger {
                key: format!("status:{id}"),
                head: head.into(),
                kind: TriggerKind::CheckFailure,
                text: bounded(
                    &format!(
                        "Failed GitHub status {canonical} on {head}.\n{}",
                        status["description"].as_str().unwrap_or("")
                    ),
                    16_384,
                ),
            });
        }
    }
    Ok(result)
}
fn annotation_text(annotations: &[Value]) -> String {
    let mut output = String::new();
    for annotation in annotations
        .iter()
        .filter(|a| a["annotation_level"].as_str() == Some("failure"))
    {
        let path = annotation["path"].as_str().unwrap_or("unknown file");
        let message = annotation["message"]
            .as_str()
            .unwrap_or("no failure message");
        let line = annotation["start_line"].as_u64().unwrap_or(0);
        output.push_str(&bounded(
            &format!("\nFailure annotation {path}:{line}: {message}"),
            4096,
        ));
        if output.len() >= 8192 {
            return bounded(&output, 8192);
        }
    }
    output
}
fn combine(a: CiOutcome, b: CiOutcome) -> CiOutcome {
    if a == CiOutcome::Failure || b == CiOutcome::Failure {
        CiOutcome::Failure
    } else if a == CiOutcome::Pending || b == CiOutcome::Pending {
        CiOutcome::Pending
    } else {
        CiOutcome::Success
    }
}

#[cfg(test)]
#[path = "github_tests.rs"]
mod tests;
