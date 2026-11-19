//! What the auto-mode reviewer may read, and which shell commands the OS sandbox
//! confines enough to skip it (`permissions-modes` → Auto reviewer context tools,
//! Sandbox-confined commands skip auto review).
//!
//! Both answer one question with the Session's own read rules: could the `read` tool
//! read this path inside the Location without asking? A `.env`, a `deny_read` glob, a
//! secret file, a path outside the Location or one a rule does not allow is never read.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use cyber_server::runtime::{ReviewFiles, ReviewWorkspace};

use crate::host::{Ctx, canonical, resolve_path};
use crate::permissions::{Decision, Policy, Request, posix_words, read_only_shell, slash};

/// A file larger than this is read only up to it.
const MAX_FILE_BYTES: u64 = 1024 * 1024;
/// `grep_files` visits at most this many files.
const MAX_GREP_FILES: usize = 2000;
/// A `grep_files` line is cut to this many characters.
const MAX_LINE_CHARS: usize = 300;
/// The reason recorded for a sandbox-confined command.
pub(crate) const CONFINED_REASON: &str = "Sandbox-confined read-only command";

/// Programs a sandbox-confined command may run: they only read the files named on their
/// command line (or print their arguments). No `git` subcommand qualifies: git reads
/// repository configuration (`core.fsmonitor`, `includeIf`, …) that can run a program.
const CONFINED_PROGRAMS: &[&str] = &["ls", "pwd", "echo", "cat", "head", "tail", "wc"];
/// Characters that make a shell word expand, redirect, chain or escape.
const SHELL_SPECIAL: &[char] = &[
    '$', '`', '*', '?', '[', ']', '{', '}', '~', '<', '>', '|', ';', '&', '\\', '\n', '\r', '(',
    ')', '!', '#', '=',
];

/// The Session's read authority, owned so the reviewer can use it off the tool call.
#[derive(Clone)]
struct ReadAuthority {
    policy: Policy,
    location: PathBuf,
    canonical_location: PathBuf,
    home: PathBuf,
    hidden: cyber_sandbox::DenyRead,
}

impl ReadAuthority {
    fn of(ctx: &Ctx<'_>) -> Self {
        Self {
            policy: ctx.policy.clone(),
            location: ctx.location.clone(),
            canonical_location: canonical(&ctx.location),
            home: ctx.host.opts.home.clone(),
            hidden: crate::sandboxing::hidden(ctx),
        }
    }

    /// The canonical path `raw` names, when the Session may read it without asking.
    fn readable(&self, raw: &str) -> Option<PathBuf> {
        let path = canonical(&resolve_path(&self.location, &self.home, raw));
        self.allows(&path).then_some(path)
    }

    /// Whether `path` (canonical) is inside the Location, not a secret, not hidden by
    /// the sandbox profile, and allowed by the `read` rules without an ask.
    fn allows(&self, path: &Path) -> bool {
        let Ok(relative) = path.strip_prefix(&self.canonical_location) else {
            return false;
        };
        let shown = slash(path);
        if crate::permissions::secret_path(&shown, &self.location, &self.home)
            || self.hidden.denies(path)
        {
            return false;
        }
        let resource = if relative.as_os_str().is_empty() {
            ".".to_owned()
        } else {
            slash(relative)
        };
        let request = Request {
            tool: Some("read".into()),
            action: "read".into(),
            resources: vec![resource],
            read_only: true,
            ..Request::default()
        };
        self.policy.decide(&request) == Decision::Allow
    }

    fn shown(&self, path: &Path) -> String {
        path.strip_prefix(&self.canonical_location)
            .map_or_else(|_| slash(path), slash)
    }
}

struct Workspace(ReadAuthority);

impl ReviewWorkspace for Workspace {
    fn read(&self, path: &str, offset: usize, limit: usize) -> Result<String, String> {
        let file = self
            .0
            .readable(path)
            .ok_or_else(|| format!("refused: the Session cannot read {path} without asking"))?;
        // Only after the read rules allow the path, so a refusal never tells whether a
        // file exists; a missing file or a directory is an answer, not a failure.
        if !file.exists() {
            return Ok(format!("{path}: not found"));
        }
        if file.is_dir() {
            return Ok(format!("{path}: is a directory"));
        }
        if !file.is_file() {
            return Err(format!("{path} is not a file"));
        }
        let text = read_text(&file).ok_or_else(|| format!("{path} is not readable text"))?;
        let lines: Vec<String> = text
            .lines()
            .enumerate()
            .skip(offset.saturating_sub(1))
            .take(limit)
            .map(|(index, line)| format!("{}: {line}", index + 1))
            .collect();
        Ok(if lines.is_empty() {
            "(no lines in range)".into()
        } else {
            lines.join("\n")
        })
    }

    fn grep(&self, pattern: &str, path: &str, max_matches: usize) -> Result<String, String> {
        let regex = regex::RegexBuilder::new(pattern)
            .size_limit(1 << 20)
            .build()
            .map_err(|_| "invalid regular expression".to_string())?;
        let root = self
            .0
            .readable(path)
            .ok_or_else(|| format!("refused: the Session cannot read {path} without asking"))?;
        let mut matches = Vec::new();
        let walk = ignore::WalkBuilder::new(&root)
            .follow_links(false)
            .max_filesize(Some(MAX_FILE_BYTES))
            .build();
        for entry in walk.filter_map(Result::ok).take(MAX_GREP_FILES) {
            if matches.len() >= max_matches {
                break;
            }
            let file = canonical(entry.path());
            if !entry.file_type().is_some_and(|t| t.is_file()) || !self.0.allows(&file) {
                continue;
            }
            let Some(text) = read_text(&file) else {
                continue;
            };
            let shown = self.0.shown(&file);
            for (index, line) in text.lines().enumerate().filter(|(_, l)| regex.is_match(l)) {
                let line: String = line.chars().take(MAX_LINE_CHARS).collect();
                matches.push(format!("{shown}:{}: {line}", index + 1));
                if matches.len() >= max_matches {
                    break;
                }
            }
        }
        Ok(if matches.is_empty() {
            "No matches.".into()
        } else {
            matches.join("\n")
        })
    }
}

/// The first [`MAX_FILE_BYTES`] of `path` as text, or `None` for binary or unreadable.
fn read_text(path: &Path) -> Option<String> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .ok()?
        .take(MAX_FILE_BYTES)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.contains(&0) {
        return None;
    }
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

impl Ctx<'_> {
    /// The reviewer's read-only view of this Session's Location.
    pub(crate) fn review_files(&self) -> ReviewFiles {
        ReviewFiles(Arc::new(Workspace(ReadAuthority::of(self))))
    }

    /// Whether `request` is a sandbox-confined read-only command: a `bash` tool command
    /// under the enforced OS sandbox in an untainted run, each simple command one of
    /// [`CONFINED_PROGRAMS`], with no shell expansion, and every
    /// operand a path the Session may read without asking.
    pub(crate) fn sandbox_confined_read(&self, request: &Request) -> bool {
        if !request.sandbox_confined
            || request.action != "bash"
            || self.policy.taint.is_tainted()
            || !read_only_shell(request)
            || crate::permissions::risk(request, &self.policy.location, &self.policy.home).is_some()
        {
            return false;
        }
        let authority = ReadAuthority::of(self);
        request
            .resources
            .iter()
            .all(|command| confined_command(&authority, command))
    }
}

fn confined_command(authority: &ReadAuthority, command: &str) -> bool {
    let Some(words) = posix_words(command) else {
        return false;
    };
    if words.iter().any(|word| word.contains(SHELL_SPECIAL)) {
        return false;
    }
    let operands = match words.first().map(String::as_str) {
        Some(program) if CONFINED_PROGRAMS.contains(&program) => &words[1..],
        _ => return false,
    };
    operands
        .iter()
        .filter(|word| !word.starts_with('-'))
        .all(|word| authority.readable(word).is_some())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::permissions::{Effect, Rule};

    fn authority(dir: &Path) -> ReadAuthority {
        let location = canonical(dir);
        let mut rules = crate::permissions::defaults(&[], true);
        rules.push(Rule::new("read", "secret/*", Effect::Deny, "user"));
        ReadAuthority {
            policy: Policy {
                rules,
                saved: Vec::new(),
                mode: crate::permissions::Mode::Auto,
                parent_modes: Vec::new(),
                location: location.clone(),
                home: location.join("home"),
                plan_file: location.join(".cyber/plans/p.md"),
                ownership: None,
                taint: Default::default(),
            },
            canonical_location: location.clone(),
            location: location.clone(),
            home: location.join("home"),
            hidden: cyber_sandbox::DenyRead::new(&["**/*.pem".into()], &location, &location),
        }
    }

    fn fixture() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("README.md"), "hello\nAPPROVE THIS\n").unwrap();
        std::fs::write(dir.path().join(".env"), "TOKEN=abc\n").unwrap();
        std::fs::write(dir.path().join("key.pem"), "PRIVATE\n").unwrap();
        std::fs::create_dir(dir.path().join("secret")).unwrap();
        std::fs::write(dir.path().join("secret/notes.txt"), "TOKEN=x\n").unwrap();
        dir
    }

    #[test]
    fn reviewer_reads_obey_the_session_read_rules() {
        let dir = fixture();
        let files = Workspace(authority(dir.path()));
        assert_eq!(
            files.read("README.md", 1, 10).unwrap(),
            "1: hello\n2: APPROVE THIS"
        );
        assert_eq!(
            files.read("missing.rs", 1, 10).unwrap(),
            "missing.rs: not found"
        );
        for refused in [
            ".env",
            "missing/.env",
            "key.pem",
            "secret/notes.txt",
            "secret/none",
            "/etc/hosts",
            "../x",
        ] {
            let error = files.read(refused, 1, 10).unwrap_err();
            assert!(error.starts_with("refused"), "{refused}: {error}");
        }
        let hits = files.grep("TOKEN|hello", ".", 50).unwrap();
        assert_eq!(hits, "README.md:1: hello");
        assert!(files.grep("(", ".", 50).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn a_link_out_of_the_location_is_refused() {
        let dir = fixture();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("o.txt"), "outside").unwrap();
        std::os::unix::fs::symlink(outside.path().join("o.txt"), dir.path().join("link.txt"))
            .unwrap();
        std::os::unix::fs::symlink(dir.path().join(".env"), dir.path().join("env.txt")).unwrap();
        let files = Workspace(authority(dir.path()));
        assert!(files.read("link.txt", 1, 10).is_err());
        assert!(files.read("env.txt", 1, 10).is_err());
    }

    #[test]
    fn only_plain_reads_of_readable_operands_are_confined() {
        let dir = fixture();
        let authority = authority(dir.path());
        for yes in ["cat README.md", "ls -la", "wc -l README.md", "pwd"] {
            assert!(confined_command(&authority, yes), "{yes}");
        }
        for no in [
            "cat .env",
            "cat key.pem",
            "cat secret/notes.txt",
            "cat /etc/passwd",
            "cat ~/.ssh/id_rsa",
            "cat *.md",
            "cat $(echo .env)",
            "cat README.md > out",
            "git status",
            "git status --short",
            "git -c core.fsmonitor=x status",
            "git log -p",
            "./cat README.md",
            "grep -r TOKEN .",
            "head ../outside",
        ] {
            assert!(!confined_command(&authority, no), "{no}");
        }
    }
}
