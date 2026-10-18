//! `vcs-integration` → PR checkout and linked sessions: a `bash` call that creates a pull
//! request (`gh pr create`) or pushes (`git push`) links its Session to the PR of the
//! branch. The PR comes from the command's own output, else, when the command succeeded,
//! from `gh pr view` in the command's working directory. That lookup is a `bash` command
//! the Session's rules, Mode and `PreToolUse` hooks must allow outright (it never asks),
//! and `gh` is found on the trusted `PATH` and run in the command sandbox, as for review;
//! under `sandbox.network: proxy` it reaches only allowlisted or already approved domains.
//! Commands from the model's `bash` and `powershell` tools and the user's `!` shell are
//! observed.
//! Linking is best effort and never fails the call.

use std::path::Path;
use std::time::Duration;

use serde_json::Value;

use crate::host::Ctx;
use crate::permissions::{Decision, Request};
use cyber_server::runtime::PrLink;

/// The lookup after a push, as the `bash` command its permission is checked for.
const LOOKUP: &str = "gh pr view --json number,url";
/// How long the `gh pr view` lookup after a push may take.
const LOOKUP_TIMEOUT: Duration = Duration::from_secs(15);
const LOOKUP_LIMIT: usize = 64 * 1024;

/// Link the Session after `command` ran in `workdir` and printed `output` (the rendered
/// tool output, ending with an exit note when the command failed).
pub(crate) async fn observe(ctx: &Ctx<'_>, command: &str, workdir: &Path, output: &str) {
    if !(invokes(command, &["gh", "pr", "create"]) || invokes(command, &["git", "push"])) {
        return;
    }
    let link = match pr_url(output) {
        Some(link) => Some(link),
        None if succeeded(output) => current_pr(ctx, workdir).await,
        None => None,
    };
    let (Some(link), Some(runtime)) = (link, ctx.host.runtime()) else {
        return;
    };
    if let Err(error) = runtime.link_pr(&ctx.inv.session_id, link).await {
        cyber_core::log::debug(
            "vcs",
            "PR link was not recorded",
            serde_json::json!({"session_id":ctx.inv.session_id,"error":error.to_string()}),
        );
    }
}

/// Whether the rendered output carries no failure note from the `bash` tool.
fn succeeded(output: &str) -> bool {
    let last = output.lines().last().unwrap_or_default();
    !(last.starts_with("Exit code: ")
        || last == "Terminated by a signal"
        || last.starts_with("Command timed out after "))
}

/// Whether one simple command of `command` starts with `words`.
fn invokes(command: &str, words: &[&str]) -> bool {
    command
        .split([';', '&', '|', '\n', '(', ')'])
        .any(|segment| {
            let tokens: Vec<&str> = segment.split_whitespace().collect();
            tokens.len() >= words.len() && tokens[..words.len()] == *words
        })
}

/// The first `https://…/pull/<number>` in `text`.
fn pr_url(text: &str) -> Option<PrLink> {
    text.split_whitespace().find_map(|token| {
        let url = token.trim_matches(|c: char| matches!(c, '"' | '\'' | '<' | '>' | ',' | '.'));
        let (_, rest) = url.strip_prefix("https://")?.split_once("/pull/")?;
        let number: u64 = rest.split(['/', '#', '?']).next()?.parse().ok()?;
        (number > 0).then(|| PrLink {
            number,
            url: url.to_owned(),
        })
    })
}

/// The open PR of the branch checked out in `workdir`, by `gh pr view --json number,url`.
async fn current_pr(ctx: &Ctx<'_>, workdir: &Path) -> Option<PrLink> {
    if !lookup_allowed(ctx).await {
        return None;
    }
    let args = ["pr", "view", "--json", "number,url"].map(String::from);
    // Nobody asked for the lookup: a domain the proxy would ask about is refused, so it
    // neither prompts nor turns this call's approvals into a Session network grant.
    let lookup = crate::host::review::run_gh(
        ctx,
        &args,
        workdir,
        LOOKUP_LIMIT,
        crate::host::review::NetworkAsks::Refuse,
    );
    let output = tokio::time::timeout(LOOKUP_TIMEOUT, lookup)
        .await
        .ok()?
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let value: Value = serde_json::from_slice(&output.stdout).ok()?;
    let number = value["number"].as_u64().filter(|number| *number > 0)?;
    let url = value["url"].as_str()?.to_owned();
    Some(PrLink { number, url })
}

/// Whether the Session may run the lookup without asking: its `bash` rules and Mode
/// allow it outright, no classifier review is required, and `PreToolUse` hooks admit it
/// unchanged. Anything else skips the lookup.
async fn lookup_allowed(ctx: &Ctx<'_>) -> bool {
    let req = Request {
        tool: Some("bash".into()),
        action: "bash".into(),
        resources: vec![LOOKUP.into()],
        ..Request::default()
    };
    ctx.policy.decide(&req) == Decision::Allow
        && !ctx.policy.forces_classifier(&req)
        && ctx
            .host
            .hooks_admit_host_bash(ctx.inv, LOOKUP, ctx.cancel.clone())
            .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_and_pushes_are_recognized_in_compound_commands() {
        assert!(invokes("gh pr create --fill", &["gh", "pr", "create"]));
        assert!(invokes(
            "cd app && git push -u origin feature",
            &["git", "push"]
        ));
        assert!(!invokes("echo git push", &["git", "push"]));
        assert!(!invokes("git pushx", &["git", "push"]));
    }

    #[test]
    fn the_pr_url_is_read_from_command_output() {
        let link = pr_url("Creating pull request\nhttps://github.com/acme/app/pull/42\n").unwrap();
        assert_eq!(link.number, 42);
        assert_eq!(link.url, "https://github.com/acme/app/pull/42");
        assert!(pr_url("visit https://github.com/acme/app/pull/new/feature").is_none());
    }

    #[test]
    fn a_failed_command_is_recognized_by_its_exit_note() {
        assert!(succeeded("Everything up-to-date"));
        assert!(!succeeded("fatal: no remote\nExit code: 128"));
        assert!(!succeeded("partial\nCommand timed out after 10 ms"));
        assert!(!succeeded("Terminated by a signal"));
    }
}
