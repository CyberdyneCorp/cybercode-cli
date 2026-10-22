//! Mode admission, sandbox status and the rule dry run of a Location
//! (`permissions-modes` → bypass mode, Rule dry run; `sandbox` → Container detection;
//! `tui` → Permission mode indicator and cycling).

use std::path::{Path, PathBuf};

use cyber_sandbox::{Isolation, SandboxConfig};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::BuiltinHost;
use crate::permissions::{Explained, Request};

/// What may enable `bypass` besides an enforced sandbox.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct IsolationOptions {
    /// `--dangerously-bypass-permissions` was passed to this server.
    pub dangerously_bypass: bool,
    /// The server runs inside a detected container or VM.
    pub container: bool,
}

impl IsolationOptions {
    /// This process: the flag as given, the container probe of the machine.
    pub fn probe(dangerously_bypass: bool) -> Self {
        Self {
            dangerously_bypass,
            container: cyber_sandbox::in_container(),
        }
    }
}

/// `cyber permissions test` and `GET /api/v1/permissions/test`: one tool and resource, or
/// a shell command split as `bash` splits it.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct PermissionTest {
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub tool: Option<String>,
    #[serde(default)]
    pub resource: Option<String>,
    #[serde(default)]
    pub command: Option<String>,
}

/// The dry run's answer.
#[derive(Debug, Clone, Serialize)]
pub struct PermissionTestReport {
    pub agent: String,
    pub mode: String,
    pub results: Vec<Explained>,
}

const FILE_EDIT_TOOLS: &[&str] = &["edit", "write", "apply_patch", "notebook_edit"];
const READ_TOOLS: &[&str] = &["read", "glob", "grep", "list"];

impl BuiltinHost {
    /// The effective sandbox of a Location for `agent`.
    pub fn sandbox_config(&self, location: &Path, agent: Option<&str>) -> SandboxConfig {
        let (config, sources) = (self.opts.config)(location).unwrap_or_default();
        SandboxConfig::resolve_for(
            &config,
            &sources,
            self.opts.sandbox_policy.as_deref(),
            agent,
            &self.opts.home,
        )
    }

    /// What isolates commands run in `location`.
    pub fn isolation(&self, location: &Path) -> Isolation {
        self.isolation_for(location, None)
    }

    /// What isolates the commands of `agent` (its own `sandbox` profile) in `location`.
    fn isolation_for(&self, location: &Path, agent: Option<&str>) -> Isolation {
        Isolation::of(
            &self.sandbox_config(location, agent),
            cyber_sandbox::available(),
            self.opts.isolation.container,
            &self.opts.home,
        )
    }

    /// Whether only the OS sandbox makes `bypass` eligible here: no container and no
    /// `--dangerously-bypass-permissions`. A command that leaves the sandbox (an excluded
    /// command) then crosses the boundary `bypass` relies on, so it still asks.
    pub(crate) fn sandbox_isolates_alone(&self) -> bool {
        !(self.opts.isolation.container || self.opts.isolation.dangerously_bypass)
    }

    /// Whether a Session of `agent` in `location` may run in `mode`: organization
    /// `policy.modes.disable`, then the `bypass` eligibility gate over the agent's own
    /// sandbox.
    pub fn mode_admission(&self, location: &Path, agent: &str, mode: &str) -> Result<(), String> {
        let (config, sources) = (self.opts.config)(location).unwrap_or_default();
        let disabled = sources.iter().any(|(pointer, source)| {
            pointer.starts_with("/policy/modes/disable") && source.starts_with("managed:")
        }) && config["policy"]["modes"]["disable"]
            .as_array()
            .is_some_and(|modes| modes.iter().any(|m| m.as_str() == Some(mode)));
        if disabled {
            return Err(format!("{mode} mode is disabled by your organization"));
        }
        if mode == "bypass" {
            self.isolation_for(location, Some(agent))
                .admits_bypass(self.opts.isolation.dangerously_bypass)
                .map_err(str::to_string)?;
        }
        Ok(())
    }

    /// `{ policy, profile, enforcement, isolation, notice?, error? }` for `/health` and the
    /// session header.
    pub fn sandbox_status(&self, location: &Path) -> Value {
        let sandbox = self.sandbox_config(location, None);
        let enforcement = if sandbox.policy == cyber_sandbox::Policy::FullAccess {
            "off"
        } else if cyber_sandbox::available() {
            "available"
        } else {
            "sandbox unavailable"
        };
        let mut status = json!({
            "policy": sandbox.policy.as_str(),
            "profile": sandbox.profile,
            "enforcement": enforcement,
            "isolation": self.isolation(location).as_str(),
        });
        if let Some(notice) = &sandbox.notice {
            status["notice"] = json!(notice);
        }
        if let Some(error) = &sandbox.error {
            status["error"] = json!(error);
        }
        status
    }

    /// Evaluate a request as a Session in `location` would, without a Session.
    pub async fn permission_test(
        &self,
        location: &Path,
        test: &PermissionTest,
    ) -> Result<PermissionTestReport, String> {
        let agent = test.agent.clone().unwrap_or_else(|| "build".into());
        let mode = test.mode.clone().unwrap_or_else(|| "default".into());
        if crate::permissions::Mode::checked_parse(&mode).is_none() {
            return Err(format!("unknown mode {mode}"));
        }
        let inv = probe_invocation(location, &agent, &mode);
        let policy = self.policy_for(&inv, Some(&agent)).await?;
        let req = self.test_request(location, test)?;
        Ok(PermissionTestReport {
            agent,
            mode,
            results: policy.explain(&req),
        })
    }

    /// The request a Session would evaluate: a tool authorizes under its action (`write`
    /// as `edit`, `powershell`, `monitor` and `pty_*` as `bash` commands, `send_file` as
    /// `read`).
    pub(super) fn test_request(&self, location: &Path, test: &PermissionTest) -> Result<Request, String> {
        let tool = test.tool.as_deref().unwrap_or("bash");
        let action = crate::tools::action_of(tool);
        let command = test
            .command
            .clone()
            .or_else(|| (action == "bash").then(|| test.resource.clone()).flatten());
        let mut req = match command {
            Some(command) => self.command_request(location, &command),
            None => self.tool_request(location, test, action)?,
        };
        req.tool = Some(tool.to_string());
        Ok(req)
    }

    fn command_request(&self, location: &Path, command: &str) -> Request {
        let analysis = crate::bash_analysis::analyze(command, location);
        let mut facts =
            crate::tools::bash::Facts::of(command, location, &analysis, location, &self.opts.home);
        facts.ask_floor =
            crate::tools::bash::excluded_by(&self.sandbox_config(location, None), command);
        facts.boundary = facts.ask_floor && self.sandbox_isolates_alone();
        facts.request(&analysis)
    }

    fn tool_request(
        &self,
        location: &Path,
        test: &PermissionTest,
        action: &str,
    ) -> Result<Request, String> {
        let (Some(tool), Some(resource)) = (&test.tool, &test.resource) else {
            return Err("give --tool and --resource, or a shell command".into());
        };
        let mut req = Request {
            action: action.to_string(),
            resources: vec![resource.clone()],
            read_only: READ_TOOLS.contains(&action),
            ..Request::default()
        };
        if FILE_EDIT_TOOLS.contains(&tool.as_str()) {
            let path: PathBuf = super::resolve_path(location, &self.opts.home, resource);
            req.resources = vec![path.strip_prefix(location).map_or_else(
                |_| crate::permissions::slash(&path),
                crate::permissions::slash,
            )];
            req.mutates = vec![path];
            req.file_edit = true;
        }
        Ok(req)
    }
}

fn probe_invocation(location: &Path, agent: &str, mode: &str) -> cyber_server::runtime::Invocation {
    cyber_server::runtime::Invocation {
        registration: None,
        session_id: String::new(),
        directory: location.display().to_string(),
        agent: agent.into(),
        mode: mode.into(),
        message_id: String::new(),
        call_id: String::new(),
        name: String::new(),
        input: Value::Null,
        attempt: 1,
        operation_key: String::new(),
        asker: cyber_server::runtime::Asker::detached(),
        rules: Value::Null,
    }
}
