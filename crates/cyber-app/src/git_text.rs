//! Commit and PR text generation (`vcs-integration` → Commit and PR text generation): a
//! conventional commit message from the staged diff and a PR title and body from
//! `base..HEAD`, both written by the small model (`model_roles.small`, else `small_model`,
//! else the default model). A `Co-authored-by` trailer is added only when `git.co_author`
//! is configured; any trailer the model writes itself is removed, so commits are
//! attributed to the user alone by default.
//!
//! What reaches the model follows the bundled reviewer's read policy: a changed file
//! whose read would not be allowed without asking under the Plan ceiling (such as `*.env`,
//! `ask` by default, or a path a rule denies) is named but its diff is withheld, and
//! credentials in the remaining text are redacted. Host `git` runs without the
//! repository-configured helpers that start programs (fsmonitor, textconv, external diff,
//! signature verification), with bounded output and a deadline.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use cyber_core::config::GitSettings;
use cyber_core::memory::redact_credentials;
use cyber_llm::catalog::{ModelRole, role_ref};
use cyber_llm::{Message, collect};
use cyber_server::runtime::ModelResolver;
use cyber_tools::permissions::{self, Decision, Mode, Policy, Request};
use serde_json::Value;
use tokio::io::AsyncReadExt;

/// The most diff text a prompt carries; the rest is summarized by `--stat`.
const DIFF_LIMIT: usize = 48 * 1024;
/// Diff read past the limit, so a credential straddling the cut is still redacted.
const REDACTION_MARGIN: usize = 4 * 1024;
/// The most output of a name list, log or stat.
const LIST_LIMIT: usize = 16 * 1024 * 1024;
const STDERR_LIMIT: usize = 64 * 1024;
const GIT_TIMEOUT: Duration = Duration::from_secs(60);
/// Withheld paths excluded by pathspec; more than this fails closed.
const WITHHELD_LIMIT: usize = 256;
/// Configuration that would make a read-only Git command start other programs, and
/// `diff.relative`, which would print Location-relative paths: the read policy and the
/// `:(top)` exclusions of withheld paths need repository-relative ones.
const SAFE_CONFIG: [&str; 6] = [
    "-c",
    "core.fsmonitor=false",
    "-c",
    "log.showSignature=false",
    "-c",
    "diff.relative=false",
];

const COMMIT_SYSTEM: &str = "Write a Conventional Commits message for the staged changes below: \
a `type(scope): summary` subject of at most 72 characters, a blank line, then a short body \
explaining what changed and why when that is not obvious. Reply with the message only, \
without code fences or trailers.";

const PR_SYSTEM: &str = "Draft a pull request for the commits and diff below. Reply with the \
title on the first line (at most 72 characters), a blank line, then a Markdown body: a short \
summary, the notable changes as a list, and how it was tested when the commits say so. Do not \
add trailers.";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitTextError {
    /// Nothing to describe, or not a Git repository: the request cannot succeed as is.
    Invalid(String),
    /// Git, configuration or the model failed.
    Failed(String),
}

impl std::fmt::Display for GitTextError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(message) | Self::Failed(message) => f.write_str(message),
        }
    }
}

/// A drafted pull request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrText {
    pub base: String,
    pub title: String,
    pub body: String,
}

/// A conventional commit message for the staged diff of `location`'s repository.
pub async fn commit_message(
    resolver: &dyn ModelResolver,
    config: &Value,
    location: &Path,
) -> Result<String, GitTextError> {
    let settings = GitSettings::from_config(config).map_err(GitTextError::Invalid)?;
    let changes = Changes::read(config, location, &["--cached"]).await?;
    if changes.is_empty() {
        return Err(GitTextError::Invalid(
            "nothing is staged; stage changes with git add first".into(),
        ));
    }
    let prompt = format!("Staged changes:\n{}", changes.render().await?);
    let text = complete(resolver, config, COMMIT_SYSTEM, prompt).await?;
    let message = without_trailers(&clean(&text));
    if message.is_empty() {
        return Err(GitTextError::Failed("the model returned no message".into()));
    }
    Ok(match settings.trailer() {
        Some(trailer) => format!("{message}\n\n{trailer}"),
        None => message,
    })
}

/// A PR title and body for `base..HEAD` (default: `origin/HEAD`, else `main`, else
/// `master`).
pub async fn pr_description(
    resolver: &dyn ModelResolver,
    config: &Value,
    location: &Path,
    base: Option<&str>,
) -> Result<PrText, GitTextError> {
    let base = match base.filter(|base| !base.trim().is_empty()) {
        Some(base) => base.trim().to_owned(),
        None => default_base(location).await?,
    };
    if base.starts_with('-') {
        return Err(GitTextError::Invalid(format!("invalid base {base}")));
    }
    let range = format!("{base}..HEAD");
    let log = git(
        location,
        &["log", "--no-color", "--format=%s%n%n%b", &range],
    )
    .await?;
    if log.trim().is_empty() {
        return Err(GitTextError::Invalid(format!("no commits in {range}")));
    }
    let three_dot = format!("{base}...HEAD");
    let selection = [three_dot.as_str()];
    let changes = Changes::read(config, location, &selection).await?;
    let prompt = format!(
        "Commits in {range}:\n{}\nChanges:\n{}",
        redact_credentials(&log),
        changes.render().await?
    );
    let text = without_trailers(&clean(
        &complete(resolver, config, PR_SYSTEM, prompt).await?,
    ));
    let (title, body) = text.split_once('\n').unwrap_or((&text, ""));
    let title = title
        .trim()
        .trim_start_matches('#')
        .trim()
        .trim_start_matches("Title:")
        .trim()
        .to_owned();
    if title.is_empty() {
        return Err(GitTextError::Failed("the model returned no title".into()));
    }
    Ok(PrText {
        base,
        title,
        body: body.trim().to_owned(),
    })
}

/// One hidden completion by the small model, without tools.
async fn complete(
    resolver: &dyn ModelResolver,
    config: &Value,
    system: &str,
    prompt: String,
) -> Result<String, GitTextError> {
    let reference = role_ref(config, ModelRole::Small)
        .ok_or_else(|| GitTextError::Invalid("no small or default model is configured".into()))?;
    let resolved = resolver.resolve(&reference).map_err(GitTextError::Failed)?;
    let mut request = resolved.template.clone();
    request.system = vec![system.into()];
    request.messages = vec![Message::user_text(prompt)];
    request.tools = Vec::new();
    request.reasoning = None;
    request.max_output_tokens = Some(1024);
    let stream = resolved
        .adapter
        .stream(request)
        .await
        .map_err(|error| GitTextError::Failed(error.to_string()))?;
    let out = collect(stream, |_| {})
        .await
        .map_err(|error| GitTextError::Failed(error.to_string()))?;
    Ok(out.text)
}

/// `origin/HEAD` when the remote names one, else a local `main` or `master`.
async fn default_base(location: &Path) -> Result<String, GitTextError> {
    if let Ok(remote) = git(location, &["rev-parse", "--abbrev-ref", "origin/HEAD"]).await
        && !remote.trim().is_empty()
    {
        return Ok(remote.trim().to_owned());
    }
    for branch in ["main", "master"] {
        if git(location, &["rev-parse", "--verify", "--quiet", branch])
            .await
            .is_ok()
        {
            return Ok(branch.into());
        }
    }
    Err(GitTextError::Invalid(
        "no base branch found; pass the base explicitly".into(),
    ))
}

/// The changes one diff selection (`--cached`, or `base...HEAD`) covers, split by the
/// read policy into shown and withheld paths.
struct Changes<'a> {
    location: &'a Path,
    selection: &'a [&'a str],
    withheld: Vec<String>,
    shown: usize,
}

impl<'a> Changes<'a> {
    async fn read(
        config: &Value,
        location: &'a Path,
        selection: &'a [&'a str],
    ) -> Result<Self, GitTextError> {
        let reader = Reader::new(config, location).await?;
        let mut args = vec!["diff", "--name-only", "-z", "--no-renames"];
        args.extend_from_slice(selection);
        let names = git(location, &args).await?;
        let (mut shown, mut withheld) = (0, Vec::new());
        for path in names.split('\0').filter(|path| !path.is_empty()) {
            if reader.readable(path) {
                shown += 1;
            } else {
                withheld.push(path.to_owned());
            }
        }
        if withheld.len() > WITHHELD_LIMIT {
            return Err(GitTextError::Failed(format!(
                "read policy withholds more than {WITHHELD_LIMIT} changed files"
            )));
        }
        Ok(Self {
            location,
            selection,
            withheld,
            shown,
        })
    }

    fn is_empty(&self) -> bool {
        self.shown == 0 && self.withheld.is_empty()
    }

    /// The `--stat` summary and the bounded, redacted diff of the shown paths, then the
    /// withheld paths by name.
    async fn render(&self) -> Result<String, GitTextError> {
        let stat = git(self.location, &self.args(&["--stat"])).await?;
        let diff = git_bounded(
            self.location,
            &self.args(&["--no-ext-diff", "--no-textconv"]),
            DIFF_LIMIT + REDACTION_MARGIN,
        )
        .await?;
        let mut text = format!(
            "{}\n{}",
            redact_credentials(&stat),
            bounded(&redact_credentials(&String::from_utf8_lossy(&diff.stdout)))
        );
        if !self.withheld.is_empty() {
            text.push_str("\nWithheld by read policy (not shown):\n");
            for path in &self.withheld {
                text.push_str(&format!("- {path}\n"));
            }
        }
        Ok(text)
    }

    /// `git diff <extra> <selection>`, excluding the withheld paths.
    fn args(&self, extra: &[&'a str]) -> Vec<String> {
        let mut args: Vec<String> = ["diff", "--no-color", "--no-renames"]
            .iter()
            .chain(extra)
            .chain(self.selection)
            .map(|arg| (*arg).to_owned())
            .collect();
        if !self.withheld.is_empty() {
            args.extend(["--".to_owned(), ":/".to_owned()]);
            args.extend(
                self.withheld
                    .iter()
                    .map(|path| format!(":(top,literal,exclude){path}")),
            );
        }
        args
    }
}

/// The reviewer's read policy for text sent to the model: a repository path is shown
/// only when reading it (and, outside the Location, its directory) is allowed without
/// asking under the Plan ceiling.
struct Reader {
    policy: Policy,
    /// The repository root, as Git reports it.
    root: PathBuf,
    /// The Location relative to the root, ending in `/` (empty at the root).
    prefix: String,
}

impl Reader {
    async fn new(config: &Value, location: &Path) -> Result<Self, GitTextError> {
        let paths = git(location, &["rev-parse", "--show-toplevel", "--show-prefix"]).await?;
        let mut lines = paths.lines();
        let root = PathBuf::from(lines.next().unwrap_or_default());
        let prefix = lines.next().unwrap_or_default().to_owned();
        let mut rules = permissions::defaults(&[], true);
        rules.extend(permissions::parse_rules(
            &config["permissions"],
            &BTreeMap::new(),
        ));
        let policy = Policy {
            rules,
            saved: Vec::new(),
            mode: Mode::Plan,
            parent_modes: Vec::new(),
            location: location.to_path_buf(),
            home: cyber_core::paths::home_dir(&cyber_core::env::ProcessEnv).unwrap_or_default(),
            plan_file: PathBuf::new(),
            ownership: None,
        };
        Ok(Self {
            policy,
            root,
            prefix,
        })
    }

    fn readable(&self, path: &str) -> bool {
        if let Some(relative) = path.strip_prefix(&self.prefix) {
            return self.allows("read", relative.to_owned());
        }
        let absolute = self.root.join(path);
        let directory = absolute.parent().unwrap_or(&self.root);
        self.allows(
            "external_directory",
            format!("{}/*", permissions::slash(directory)),
        ) && self.allows("read", permissions::slash(&absolute))
    }

    fn allows(&self, action: &str, resource: String) -> bool {
        let request = Request {
            tool: Some("read".into()),
            action: action.into(),
            resources: vec![resource],
            read_only: true,
            ..Request::default()
        };
        self.policy.decide(&request) == Decision::Allow
    }
}

/// At most `limit` bytes of a Git command's stdout; more is an error.
async fn git(location: &Path, args: &[impl AsRef<str>]) -> Result<String, GitTextError> {
    let output = git_bounded(location, args, LIST_LIMIT).await?;
    if output.truncated {
        return Err(GitTextError::Failed(format!(
            "git output exceeds {} MiB",
            LIST_LIMIT / (1024 * 1024)
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

struct GitOutput {
    stdout: Vec<u8>,
    /// The command wrote more than the limit and was stopped.
    truncated: bool,
}

/// Run host `git` in `location` with [`SAFE_CONFIG`] and [`GIT_TIMEOUT`], keeping the
/// first `limit` bytes of stdout; a longer output stops the command.
async fn git_bounded(
    location: &Path,
    args: &[impl AsRef<str>],
    limit: usize,
) -> Result<GitOutput, GitTextError> {
    let mut child = tokio::process::Command::new("git")
        .arg("-C")
        .arg(location)
        .args(SAFE_CONFIG)
        .args(args.iter().map(AsRef::as_ref))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| GitTextError::Failed(format!("cannot run git: {error}")))?;
    let (Some(stdout), Some(stderr)) = (child.stdout.take(), child.stderr.take()) else {
        return Err(GitTextError::Failed("cannot capture git output".into()));
    };
    let errors = tokio::spawn(drain(stderr));
    let run = async {
        let mut captured = Vec::new();
        stdout
            .take(limit as u64 + 1)
            .read_to_end(&mut captured)
            .await?;
        if captured.len() > limit {
            captured.truncate(limit);
            child.start_kill()?;
            child.wait().await?;
            return Ok((captured, None));
        }
        Ok::<_, std::io::Error>((captured, Some(child.wait().await?)))
    };
    let (stdout, status) = tokio::time::timeout(GIT_TIMEOUT, run)
        .await
        .map_err(|_| GitTextError::Failed("git timed out".into()))?
        .map_err(|error| GitTextError::Failed(format!("cannot run git: {error}")))?;
    let stderr = errors.await.unwrap_or_default();
    match status {
        None => Ok(GitOutput {
            stdout,
            truncated: true,
        }),
        Some(status) if status.success() => Ok(GitOutput {
            stdout,
            truncated: false,
        }),
        Some(_) => Err(git_failure(&stderr)),
    }
}

/// The first `STDERR_LIMIT` bytes of a stream, reading the rest to its end.
async fn drain(mut stream: impl tokio::io::AsyncRead + Unpin) -> Vec<u8> {
    let mut head = Vec::new();
    let _ = (&mut stream)
        .take(STDERR_LIMIT as u64)
        .read_to_end(&mut head)
        .await;
    let _ = tokio::io::copy(&mut stream, &mut tokio::io::sink()).await;
    head
}

fn git_failure(stderr: &[u8]) -> GitTextError {
    let stderr = String::from_utf8_lossy(stderr);
    let message = stderr.lines().next().unwrap_or("git failed").to_owned();
    if message.contains("not a git repository") {
        GitTextError::Invalid("not a Git repository".into())
    } else {
        GitTextError::Failed(message)
    }
}

fn bounded(diff: &str) -> String {
    if diff.len() <= DIFF_LIMIT {
        return diff.to_owned();
    }
    let mut end = DIFF_LIMIT;
    while !diff.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n[diff truncated at 48 KiB]", &diff[..end])
}

/// Reasoning blocks, code fences and surrounding whitespace removed.
fn clean(raw: &str) -> String {
    let mut text = raw.to_owned();
    while let (Some(start), Some(end)) = (text.find("<think>"), text.find("</think>")) {
        if end < start {
            break;
        }
        text.replace_range(start..end + "</think>".len(), "");
    }
    let lines: Vec<&str> = text
        .lines()
        .filter(|line| !line.trim_start().starts_with("```"))
        .map(str::trim_end)
        .collect();
    lines.join("\n").trim().to_owned()
}

/// The text without `Co-authored-by` (or `Signed-off-by`) trailer lines a model added.
fn without_trailers(text: &str) -> String {
    let kept: Vec<&str> = text
        .lines()
        .filter(|line| {
            let line = line.trim_start().to_ascii_lowercase();
            !line.starts_with("co-authored-by:") && !line.starts_with("signed-off-by:")
        })
        .collect();
    kept.join("\n").trim().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_trailers_and_fences_are_removed() {
        let raw = "<think>hmm</think>```\nfeat(app): greet\n\nAdds a greeting.\n\nCo-authored-by: Model <m@x.dev>\n```";
        assert_eq!(
            without_trailers(&clean(raw)),
            "feat(app): greet\n\nAdds a greeting."
        );
    }
}
