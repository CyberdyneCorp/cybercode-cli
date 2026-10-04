//! `webfetch` (`builtin-tools` → webfetch tool).

use std::time::Duration;

use cyber_llm::catalog::ModelRole;
use cyber_llm::{LlmRequest, Message};
use cyber_server::runtime::{RetrySafety, ToolDef};
use futures::StreamExt;
use futures::future::BoxFuture;
use serde_json::json;

use super::{Tool, ToolError, def, failed, number, text};
use crate::host::Ctx;
use crate::permissions::Request;

const MAX_BODY: usize = 5 * 1024 * 1024;
const BROWSER_UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0 Safari/537.36";

pub(crate) struct WebFetch;

impl Tool for WebFetch {
    fn def(&self) -> ToolDef {
        def(
            "webfetch",
            "Fetch an http(s) URL. format: markdown (default), text or html. With prompt, returns a summary answering it.",
            json!({"type": "object", "required": ["url"], "properties": {
                "url": {"type": "string"}, "format": {"type": "string", "enum": ["markdown", "text", "html"]},
                "timeout_s": {"type": "integer"}, "prompt": {"type": "string"}}}),
            RetrySafety::ReadOnly,
            true,
        )
    }

    fn run<'a>(&'a self, ctx: &'a Ctx<'a>) -> BoxFuture<'a, Result<String, ToolError>> {
        Box::pin(async move {
            let input = &ctx.inv.input;
            let url = reqwest::Url::parse(text(input, "url"))
                .map_err(|e| failed(format!("Invalid URL: {e}")))?;
            if !matches!(url.scheme(), "http" | "https") {
                return Err(failed("Only http and https URLs are supported"));
            }
            let always = format!(
                "{}://{}/*",
                url.scheme(),
                url.host_str().unwrap_or_default()
            );
            let req = Request {
                action: "webfetch".into(),
                resources: vec![url.to_string()],
                read_only: true,
                ..Request::default()
            };
            ctx.authorize(req, vec![always], json!({"url": url.as_str()}))
                .await?;
            let timeout =
                Duration::from_secs(number(input, "timeout_s").unwrap_or(30).clamp(1, 120));
            let (content_type, body) = tokio::select! {
                _ = ctx.cancel.cancelled() => return Err(ToolError::Aborted),
                r = fetch(url.clone(), timeout) => r.map_err(failed)?,
            };
            let content = render(&content_type, &body, text(input, "format"))?;
            match input
                .get("prompt")
                .and_then(|p| p.as_str())
                .filter(|p| !p.trim().is_empty())
            {
                Some(prompt) => tokio::select! {
                    _ = ctx.cancel.cancelled() => Err(ToolError::Aborted),
                    r = summarize(ctx, url.as_str(), prompt, &content) => r,
                },
                None => Ok(content),
            }
        })
    }
}

async fn fetch(url: reqwest::Url, timeout: Duration) -> Result<(String, Vec<u8>), String> {
    let client = reqwest::Client::builder()
        .timeout(timeout)
        .build()
        .map_err(|e| e.to_string())?;
    let mut response = get(&client, url.clone(), BROWSER_UA).await?;
    // A Cloudflare challenge answers browser-like agents; an honest agent string often passes.
    if response.status() == reqwest::StatusCode::FORBIDDEN
        && response
            .headers()
            .get("cf-mitigated")
            .is_some_and(|v| v == "challenge")
    {
        response = get(&client, url, "cyber").await?;
    }
    if !response.status().is_success() {
        return Err(format!("Request failed with status {}", response.status()));
    }
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    let body = read_limited(response, MAX_BODY)
        .await
        .map_err(|_| "Response too large (exceeds 5 MiB)".to_string())?;
    Ok((content_type, body))
}

async fn get(
    client: &reqwest::Client,
    url: reqwest::Url,
    agent: &str,
) -> Result<reqwest::Response, String> {
    client
        .get(url)
        .header(reqwest::header::USER_AGENT, agent)
        .header(
            reqwest::header::ACCEPT,
            "text/markdown;q=1.0, text/html;q=0.9, text/plain;q=0.8, */*;q=0.1",
        )
        .send()
        .await
        .map_err(|e| {
            if e.is_timeout() {
                "Request timed out".to_string()
            } else {
                format!("Request failed: {e}")
            }
        })
}

/// Read a body, failing once it exceeds `limit` bytes (declared or streamed).
pub(crate) async fn read_limited(
    response: reqwest::Response,
    limit: usize,
) -> Result<Vec<u8>, String> {
    if response
        .content_length()
        .is_some_and(|n| n as usize > limit)
    {
        return Err("too large".into());
    }
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        body.extend_from_slice(&chunk.map_err(|e| e.to_string())?);
        if body.len() > limit {
            return Err("too large".into());
        }
    }
    Ok(body)
}

fn render(content_type: &str, body: &[u8], format: &str) -> Result<String, ToolError> {
    if content_type.starts_with("image/") {
        return Err(failed(format!(
            "{content_type} images are returned as media from a later milestone; this build fetches text only"
        )));
    }
    let html = content_type.contains("html");
    let raw = String::from_utf8_lossy(body);
    Ok(match (html, format) {
        (false, _) | (true, "html") => raw.into_owned(),
        (true, "text") => html2text::from_read(body, 100).unwrap_or_else(|_| raw.into_owned()),
        (true, _) => htmd::convert(&raw).unwrap_or_else(|_| raw.into_owned()),
    })
}

/// Answer `prompt` from the page with the `small` model role.
async fn summarize(
    ctx: &Ctx<'_>,
    url: &str,
    prompt: &str,
    content: &str,
) -> Result<String, ToolError> {
    let models = ctx
        .host
        .opts
        .models
        .as_ref()
        .ok_or_else(|| failed("No model is available to summarize the page; omit prompt"))?;
    let reference = models
        .role(ModelRole::Small)
        .ok_or_else(|| failed("No small model is configured; omit prompt"))?;
    let resolved = models.resolve(&reference).map_err(failed)?;
    let clipped: String = content.chars().take(100_000).collect();
    let request = LlmRequest {
        system: vec!["Answer the request using only the provided web page content. Be concise and quote exact text where it matters.".into()],
        messages: vec![Message::user_text(format!("URL: {url}\n\n<content>\n{clipped}\n</content>\n\nRequest: {prompt}"))],
        ..resolved.template.clone()
    };
    let stream = resolved
        .adapter
        .stream(request)
        .await
        .map_err(|e| failed(format!("Summary failed: {e}")))?;
    let out = cyber_llm::collect(stream, |_| {})
        .await
        .map_err(|e| failed(format!("Summary failed: {e}")))?;
    Ok(out.text)
}
