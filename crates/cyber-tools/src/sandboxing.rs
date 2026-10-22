//! Running bash under the OS sandbox (`sandbox`): roots, the environment policy, the
//! network proxy and `network` permission requests for domains outside the allowlist,
//! plus the classification of sandbox denials (`sandbox` → Escalation requests, Sandbox
//! events and audit).

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use cyber_sandbox::proxy::{Decide, Proxy};
use cyber_sandbox::{DenyRead, Launch, NetworkMode, Policy, SandboxConfig, protected_in};
use tokio::sync::{mpsc, oneshot};

use crate::host::Ctx;
use crate::permissions::Request;
use crate::tools::{ToolError, failed};

/// A domain the proxy needs a decision for.
pub(crate) type NetworkAsk = (String, oneshot::Sender<bool>);

/// A command ready to spawn, with the proxy kept alive for its lifetime.
pub(crate) struct Prepared {
    pub program: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub asks: Option<mpsc::Receiver<NetworkAsk>>,
    /// What the OS sandbox enforces; `None` when the command runs unsandboxed.
    pub boundary: Option<Boundary>,
    /// The command cannot serve on the host's loopback: below a sandbox network other
    /// than `on`, Linux gives it its own network namespace and macOS denies listening,
    /// and a Windows AppContainer never reaches the host's loopback.
    pub loopback_isolated: bool,
    /// Hosts the proxy refused while the command ran.
    pub refused: Arc<Mutex<Vec<String>>>,
    _proxy: Option<Proxy>,
}

/// The enforced filesystem boundary of one sandboxed command.
pub(crate) struct Boundary {
    writable: Vec<PathBuf>,
    read_only: Vec<PathBuf>,
    unreadable: Vec<PathBuf>,
    deny_read: DenyRead,
}

/// How one launch is confined beyond the effective policy.
#[derive(Default)]
struct Scope<'a> {
    ambient_temp: bool,
    git_roots: Option<&'a [PathBuf]>,
    /// Kept read-only inside `git_roots`.
    git_read_only: &'a [PathBuf],
    extra_credentials: &'a [String],
    /// An approved escalation or an excluded command: no OS enforcement.
    unsandboxed: bool,
}

/// The effective sandbox of the call: config, `--sandbox` and the agent's profile.
pub(crate) fn effective(ctx: &Ctx<'_>) -> SandboxConfig {
    let (config, sources) = (ctx.host.opts.config)(&ctx.location).unwrap_or_default();
    resolve(ctx, &config, &sources)
}

fn resolve(
    ctx: &Ctx<'_>,
    config: &serde_json::Value,
    sources: &std::collections::BTreeMap<String, String>,
) -> SandboxConfig {
    SandboxConfig::resolve_for(
        config,
        sources,
        ctx.host.opts.sandbox_policy.as_deref(),
        Some(&ctx.inv.agent),
        &ctx.host.opts.home,
    )
}

/// The Workflow Run of the Session a tool process was started for
/// (`workflows` → agent() call): set for workflow agents and their subagents only.
pub(crate) const WORKFLOW_RUN_ENV: &str = "CYBER_WORKFLOW_RUN_ID";

/// A shell command under the effective sandbox (`bash -l` with `sandbox.env.login_shell`).
pub(crate) async fn prepare(
    ctx: &Ctx<'_>,
    shell: &str,
    command: &str,
) -> Result<Prepared, ToolError> {
    prepare_shell(ctx, shell, command, false).await
}

/// As [`prepare`], or without OS enforcement for an approved escalation or an excluded
/// command.
pub(crate) async fn prepare_shell(
    ctx: &Ctx<'_>,
    shell: &str,
    command: &str,
    unsandboxed: bool,
) -> Result<Prepared, ToolError> {
    let flag = if effective(ctx).env.login_shell {
        "-lc"
    } else {
        "-c"
    };
    let scope = Scope {
        ambient_temp: true,
        unsandboxed,
        ..Scope::default()
    };
    prepare_scoped_command(ctx, shell, &[flag.to_string(), command.to_string()], scope).await
}

pub(crate) async fn prepare_command(
    ctx: &Ctx<'_>,
    program: &str,
    args: &[String],
) -> Result<Prepared, ToolError> {
    let scope = Scope {
        ambient_temp: true,
        ..Scope::default()
    };
    prepare_scoped_command(ctx, program, args, scope).await
}

/// Automatic formatters use owned temporary roots rather than the ambient OS temp tree.
pub(crate) async fn prepare_formatter_command(
    ctx: &Ctx<'_>,
    program: &str,
    args: &[String],
) -> Result<Prepared, ToolError> {
    prepare_scoped_command(ctx, program, args, Scope::default()).await
}

/// Setup must not gain ambient temporary-directory access outside its owned roots.
pub(crate) async fn prepare_worktree_command(
    ctx: &Ctx<'_>,
    program: &str,
    args: &[String],
    credentials: &[String],
) -> Result<Prepared, ToolError> {
    let scope = Scope {
        extra_credentials: credentials,
        ..Scope::default()
    };
    prepare_scoped_command(ctx, program, args, scope).await
}

pub(crate) fn credential_env_names(config: &serde_json::Value) -> Vec<String> {
    config["providers"]
        .as_object()
        .into_iter()
        .flat_map(|providers| providers.values())
        .flat_map(|provider| provider["env"].as_array().into_iter().flatten())
        .filter_map(serde_json::Value::as_str)
        .map(str::to_owned)
        .collect()
}

/// Git creation may mutate only verified common metadata and the reserved target;
/// `read_only` paths inside them stay read-only.
pub(crate) async fn prepare_worktree_git(
    ctx: &Ctx<'_>,
    args: &[String],
    writable: &[PathBuf],
    read_only: &[PathBuf],
) -> Result<Prepared, ToolError> {
    let scope = Scope {
        git_roots: Some(writable),
        git_read_only: read_only,
        ..Scope::default()
    };
    prepare_scoped_command(ctx, "git", args, scope).await
}

async fn prepare_scoped_command(
    ctx: &Ctx<'_>,
    program: &str,
    args: &[String],
    scope: Scope<'_>,
) -> Result<Prepared, ToolError> {
    let (config, _) = (ctx.host.opts.config)(&ctx.location).unwrap_or_default();
    let home = &ctx.host.opts.home;
    let mut sandbox = effective(ctx);
    if scope.unsandboxed {
        sandbox.policy = Policy::FullAccess;
    }
    let tmp = session_tmp(ctx)?;
    let refused = Arc::new(Mutex::new(Vec::new()));
    let (asks, proxy) =
        if sandbox.policy != Policy::FullAccess && sandbox.network == NetworkMode::Proxy {
            let (tx, rx) = mpsc::channel(16);
            let decide = decider(
                &sandbox,
                ctx.host.network_approvals(&ctx.inv.session_id),
                Arc::clone(&refused),
                tx,
            );
            let proxy = start_proxy(decide, &tmp)
                .await
                .map_err(|e| failed(format!("Could not start the sandbox network proxy: {e}")))?;
            (Some(rx), Some(proxy))
        } else {
            (None, None)
        };
    let writable = match scope.git_roots {
        Some(owned) => owned
            .iter()
            .cloned()
            .chain([tmp.clone(), ctx.host.opts.tool_output_dir.clone()])
            .chain(sandbox.extra_writable.iter().cloned())
            .collect(),
        None => roots(ctx, &sandbox, &tmp, scope.ambient_temp),
    };
    let launch = Launch {
        read_only: writable
            .iter()
            .filter(|root| !scope.git_roots.is_some_and(|owned| owned.contains(root)))
            .flat_map(|root| protected_in(root))
            .chain(scope.git_read_only.iter().cloned())
            .collect(),
        unreadable: sandbox.unreadable(home),
        deny_read: sandbox.deny_read_for(&ctx.location, home),
        writable,
        proxy: proxy.as_ref().map(|p| p.endpoint.clone()),
        helper: ctx.host.opts.sandbox_helper.clone(),
        config: sandbox.clone(),
    };
    let wrapped = cyber_sandbox::wrap(&launch, program, args).map_err(|e| failed(e.to_string()))?;
    let base = command_environment(
        std::env::vars(),
        &sandbox,
        &config,
        ctx.host.opts.models.as_deref(),
        scope.extra_credentials,
    );
    let mut env: Vec<(String, String)> = base
        .into_iter()
        .filter(|(k, _)| !wrapped.env.contains_key(k) && k != "TMPDIR")
        .collect();
    env.extend(wrapped.env);
    env.push(("TMPDIR".into(), tmp.display().to_string()));
    env.retain(|(key, _)| key != WORKFLOW_RUN_ENV);
    if let Some(run) = ctx.host.process_workflow_run(&ctx.inv.session_id) {
        env.push((WORKFLOW_RUN_ENV.into(), run));
    }
    let loopback_isolated = sandbox.policy != Policy::FullAccess
        && (cfg!(windows) || sandbox.network != NetworkMode::On);
    let boundary = (sandbox.policy != Policy::FullAccess).then(|| Boundary {
        writable: launch.writable,
        read_only: launch.read_only,
        unreadable: launch.unreadable,
        deny_read: launch.deny_read,
    });
    Ok(Prepared {
        program: wrapped.program,
        args: wrapped.args,
        env,
        asks,
        boundary,
        loopback_isolated,
        refused,
        _proxy: proxy,
    })
}

fn command_environment(
    vars: impl Iterator<Item = (String, String)>,
    sandbox: &SandboxConfig,
    config: &serde_json::Value,
    models: Option<&dyn cyber_server::runtime::ModelResolver>,
    extra_credentials: &[String],
) -> Vec<(String, String)> {
    if sandbox.policy == Policy::FullAccess {
        return vars.collect();
    }
    let mut credentials = models
        .map(|resolver| resolver.credential_env_names())
        .unwrap_or_default();
    credentials.extend(credential_env_names(config));
    credentials.extend_from_slice(extra_credentials);
    cyber_sandbox::policy_env(vars, sandbox, &credentials)
}

async fn start_proxy(decide: Decide, _tmp: &Path) -> std::io::Result<Proxy> {
    #[cfg(unix)]
    if cyber_sandbox::proxy_uses_unix_socket() {
        let socket = _tmp.join(format!(
            "proxy-{}.sock",
            ulid::Ulid::new().to_string().to_lowercase()
        ));
        return Proxy::start_unix(decide, &socket).await;
    }
    Proxy::start(decide).await
}

/// Whether the effective sandbox has its network off (`sandbox.network: "off"` under an
/// enforcing policy).
pub(crate) fn network_off(ctx: &Ctx<'_>) -> bool {
    let sandbox = effective(ctx);
    sandbox.policy != Policy::FullAccess && sandbox.network == NetworkMode::Off
}

/// Allowlisted and session-approved domains pass at once, denied ones never; others wait
/// for the tool loop. Every refusal is kept for the denial report.
fn decider(
    sandbox: &SandboxConfig,
    approved: Arc<Mutex<HashSet<String>>>,
    refused: Arc<Mutex<Vec<String>>>,
    tx: mpsc::Sender<NetworkAsk>,
) -> Decide {
    let sandbox = sandbox.clone();
    Arc::new(move |host: String| {
        let denied = sandbox.domain_denied(&host);
        let known = !denied
            && (sandbox.domain_allowed(&host)
                || approved
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .contains(&host));
        let (tx, refused) = (tx.clone(), Arc::clone(&refused));
        Box::pin(async move {
            if known {
                return true;
            }
            let allowed = !denied && {
                let (reply, answer) = oneshot::channel();
                tx.send((host.clone(), reply)).await.is_ok() && answer.await.unwrap_or(false)
            };
            if !allowed {
                let mut refused = refused.lock().unwrap_or_else(PoisonError::into_inner);
                if !refused.contains(&host) {
                    refused.push(host);
                }
            }
            allowed
        })
    })
}

/// Ask for `network` on a domain; an approval holds for the rest of the Session. The
/// allowlist is a sandbox boundary, so `bypass` still asks.
pub(crate) async fn answer(ctx: &Ctx<'_>, host: String) -> bool {
    let req = Request {
        action: "network".into(),
        resources: vec![host.clone()],
        read_only: true,
        sandbox_boundary: true,
        ..Request::default()
    };
    let allowed = ctx
        .authorize(
            req,
            vec![host.clone()],
            serde_json::json!({ "domain": host }),
        )
        .await
        .is_ok();
    if allowed {
        ctx.host
            .network_approvals(&ctx.inv.session_id)
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(host);
    }
    allowed
}

/// One sandbox denial: `operation` is `read`, `write` or `network`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Denial {
    pub operation: &'static str,
    pub target: String,
}

/// Messages the OS reports for sandbox refusals, lowercase: Seatbelt answers `EPERM`,
/// bubblewrap's read-only binds `EROFS`.
/// Refusal texts: POSIX errno messages, and the .NET message Windows tools print when an
/// AppContainer ACL refuses access (`Access to the path 'C:\x' is denied.`).
const DENIAL_MARKERS: &[&str] = &[
    "operation not permitted",
    "read-only file system",
    "access to the path",
];

/// The denials in a sandboxed command's output and the proxy's refusals. A path error
/// counts only when the boundary explains it: an unreadable or `deny_read` path, or a
/// write outside the writable roots or onto a protected path.
pub(crate) fn denials(
    output: &str,
    workdir: &Path,
    boundary: &Boundary,
    refused: &[String],
) -> Vec<Denial> {
    let mut out: Vec<Denial> = Vec::new();
    for line in output.lines() {
        let Some(denial) = denied_path(line, workdir).and_then(|path| boundary.classify(&path))
        else {
            continue;
        };
        if !out.contains(&denial) {
            out.push(denial);
        }
    }
    out.extend(refused.iter().map(|host| Denial {
        operation: "network",
        target: host.clone(),
    }));
    out
}

impl Boundary {
    fn classify(&self, path: &Path) -> Option<Denial> {
        let target = path.display().to_string();
        let hidden =
            self.unreadable.iter().any(|p| path.starts_with(p)) || self.deny_read.denies(path);
        if hidden {
            return Some(Denial {
                operation: "read",
                target,
            });
        }
        let canonical = crate::host::canonical(path);
        let inside = |roots: &[PathBuf]| {
            roots.iter().any(|root| {
                path.starts_with(root) || canonical.starts_with(crate::host::canonical(root))
            })
        };
        (!inside(&self.writable) || inside(&self.read_only)).then_some(Denial {
            operation: "write",
            target,
        })
    }
}

/// The path of a refusal line: `tool: path: message`, `cannot touch 'path': message`
/// or zsh's `zsh:1: message: path`.
fn denied_path(line: &str, workdir: &Path) -> Option<PathBuf> {
    let lower = line.to_ascii_lowercase();
    let (at, marker) = DENIAL_MARKERS
        .iter()
        .find_map(|marker| lower.find(marker).map(|at| (at, *marker)))?;
    let raw = quoted(line).or_else(|| {
        let after = line[at + marker.len()..].strip_prefix(": ");
        after.or_else(|| {
            let before = line[..at].trim_end().strip_suffix(':')?;
            let segment = before.rsplit(": ").next()?;
            Some(
                segment
                    .trim()
                    .trim_start_matches("cannot create ")
                    .trim_start_matches("cannot open "),
            )
        })
    })?;
    let raw = raw.trim();
    if raw.is_empty() || (raw.contains(char::is_whitespace) && !raw.contains(['/', '\\'])) {
        return None;
    }
    let path = Path::new(raw);
    Some(crate::bash_analysis::normalize(&if path.is_absolute() {
        path.to_path_buf()
    } else {
        workdir.join(path)
    }))
}

/// The first quoted span of `line`, as GNU tools print paths.
fn quoted(line: &str) -> Option<&str> {
    let quotes = ['\'', '‘', '’', '"', '`'];
    let start = line.find(quotes)?;
    let rest = &line[start + line[start..].chars().next()?.len_utf8()..];
    Some(&rest[..rest.find(quotes)?])
}

/// The model-visible report of `denials`, recorded as `sandbox.denied.1` events.
pub(crate) async fn report(ctx: &Ctx<'_>, denials: &[Denial]) -> String {
    let mut text = String::new();
    for denial in denials {
        let _ = ctx
            .inv
            .asker
            .record_sandbox(
                cyber_server::runtime::SandboxEvent::Denied,
                serde_json::json!({
                    "session_id": ctx.inv.session_id,
                    "call_id": ctx.inv.call_id,
                    "operation": denial.operation,
                    "target": denial.target,
                }),
            )
            .await;
        text.push_str(&format!(
            "\nSandbox denied: {} {}",
            denial.operation, denial.target
        ));
    }
    if !denials.is_empty() {
        text.push_str(
            "\nTo run this one command outside the sandbox, call bash again with sandbox: \"escalate\" and a justification.",
        );
    }
    text
}

/// The profile's `deny_read` globs, which `read`, `glob` and `grep` also hide.
pub(crate) fn hidden(ctx: &Ctx<'_>) -> DenyRead {
    effective(ctx).deny_read_for(&ctx.location, &ctx.host.opts.home)
}

/// The refusal of a `read` of a hidden path, recorded as a sandbox denial.
pub(crate) async fn hidden_read(ctx: &Ctx<'_>, path: &Path) -> String {
    let denial = Denial {
        operation: "read",
        target: path.display().to_string(),
    };
    report(ctx, std::slice::from_ref(&denial))
        .await
        .lines()
        .find(|line| line.starts_with("Sandbox denied:"))
        .unwrap_or("Sandbox denied: read")
        .to_string()
}

/// The in-process editors (`edit`, `write`, `apply_patch`, `notebook_edit`) run outside
/// the OS sandbox, so they refuse a `deny_read` path, before reading it, as `read` does:
/// a diff, a match count or a move would otherwise disclose or copy it.
pub(crate) async fn refuse_hidden(ctx: &Ctx<'_>, paths: &[&Path]) -> Result<(), ToolError> {
    // Without the configuration the `deny_read` globs are unknown: refuse.
    let (config, sources) = (ctx.host.opts.config)(&ctx.location).map_err(|error| {
        failed(format!(
            "Sandbox denied: read (the sandbox configuration could not be read: {error})"
        ))
    })?;
    let hidden = resolve(ctx, &config, &sources).deny_read_for(&ctx.location, &ctx.host.opts.home);
    match paths.iter().find(|path| hidden.denies(path)) {
        Some(path) => Err(failed(hidden_read(ctx, path).await)),
        None => Ok(()),
    }
}

/// Location, worktree root, the Session's temp directory, managed tool output and
/// `sandbox.writable_roots`.
fn roots(ctx: &Ctx<'_>, sandbox: &SandboxConfig, tmp: &Path, ambient_temp: bool) -> Vec<PathBuf> {
    crate::sandbox_cli::root_list(
        &ctx.location,
        tmp,
        &ctx.host.opts.tool_output_dir,
        sandbox,
        ambient_temp,
    )
}

pub(crate) fn session_tmp(ctx: &Ctx<'_>) -> Result<PathBuf, ToolError> {
    let dir = ctx.host.opts.temp_dir.join(&ctx.inv.session_id);
    std::fs::create_dir_all(&dir)
        .map_err(|e| failed(format!("Could not create {}: {e}", dir.display())))?;
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cyber_sandbox::proxy::Endpoint;

    fn credential_fixture() -> cyber_server::runtime::CatalogResolver {
        cyber_server::runtime::CatalogResolver::new(
            &serde_json::json!({
                "catalog-only": {"name": "Catalog", "env": ["CatalogCredential"], "models": {}}
            }),
            serde_json::json!({"providers": {
                "disabled": {"disabled": true, "env": ["DisabledCredential"]},
                "unavailable": {"env": ["UnavailableCredential"]}
            }}),
            &std::collections::HashMap::<String, String>::new(),
        )
    }

    fn filtered_names(
        config: serde_json::Value,
        policy: Option<&str>,
        models: Option<&dyn cyber_server::runtime::ModelResolver>,
    ) -> Vec<String> {
        let sandbox = SandboxConfig::resolve(&config, &Default::default(), policy, Path::new("."));
        let vars = [
            "CatalogCredential",
            "DisabledCredential",
            "UnavailableCredential",
            "LocationCredential",
            "PATH",
            "LocationCredentialExtra",
            "CUSTOM_SECRET",
        ]
        .map(|name| (name.to_owned(), "fixture".to_owned()));
        command_environment(vars.into_iter(), &sandbox, &config, models, &[])
            .into_iter()
            .map(|(name, _)| name)
            .collect()
    }

    #[test]
    fn loaded_and_location_credentials_are_filtered_from_tool_environments() {
        let resolver = credential_fixture();
        let config = serde_json::json!({"providers": {
            "catalog-only": {"env": []},
            "local": {"env": ["LocationCredential"]}
        }});
        assert_eq!(
            filtered_names(config.clone(), None, Some(&resolver)),
            ["PATH", "LocationCredentialExtra"]
        );
        assert_eq!(
            filtered_names(config, None, None),
            [
                "CatalogCredential",
                "DisabledCredential",
                "UnavailableCredential",
                "PATH",
                "LocationCredentialExtra"
            ]
        );
        assert_eq!(
            filtered_names(serde_json::json!({}), None, Some(&resolver)),
            ["LocationCredential", "PATH", "LocationCredentialExtra"]
        );
    }

    #[test]
    fn tool_environment_exceptions_and_full_access_preserve_explicit_choices() {
        let resolver = credential_fixture();
        let config = serde_json::json!({
            "providers": {"local": {"env": ["LocationCredential"]}},
            "sandbox": {"env": {"allow": ["DisabledCredential", "CUSTOM_SECRET"]}}
        });
        assert_eq!(
            filtered_names(config.clone(), None, Some(&resolver)),
            [
                "DisabledCredential",
                "PATH",
                "LocationCredentialExtra",
                "CUSTOM_SECRET"
            ]
        );
        assert_eq!(
            filtered_names(config, Some("full-access"), Some(&resolver)),
            [
                "CatalogCredential",
                "DisabledCredential",
                "UnavailableCredential",
                "LocationCredential",
                "PATH",
                "LocationCredentialExtra",
                "CUSTOM_SECRET"
            ]
        );
    }

    #[test]
    fn captured_source_credentials_respect_env_allow_and_full_access() {
        let extra = vec!["SourceCredential".into()];
        for (config, policy, expected) in [
            (serde_json::json!({}), None, vec!["SourceCredentialExtra"]),
            (
                serde_json::json!({"sandbox": {"env": {"allow": ["SourceCredential"]}}}),
                None,
                vec!["SourceCredential", "SourceCredentialExtra"],
            ),
            (
                serde_json::json!({}),
                Some("full-access"),
                vec!["SourceCredential", "SourceCredentialExtra"],
            ),
        ] {
            let sandbox =
                SandboxConfig::resolve(&config, &Default::default(), policy, Path::new("."));
            let vars = ["SourceCredential", "SourceCredentialExtra"]
                .map(|name| (name.into(), "fixture".into()));
            let names: Vec<_> =
                command_environment(vars.into_iter(), &sandbox, &config, None, &extra)
                    .into_iter()
                    .map(|(name, _)| name)
                    .collect();
            assert_eq!(names, expected);
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn git_provisioning_grants_only_owned_metadata_and_target() {
        use crate::{BuiltinHost, HostOptions};
        use cyber_server::runtime::{Asker, Invocation, Runtime};
        use cyber_store::{Store, StoreOptions};
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let source = root.join("source");
        let metadata = source.join(".git");
        let target = root.join("target");
        let sibling = root.join("sibling");
        for directory in [&metadata, &target, &sibling] {
            std::fs::create_dir_all(directory).unwrap();
        }
        let store = Arc::new(
            Store::open(StoreOptions::new(
                cyber_core::paths::DatabaseLocation::File(root.join("store.db")),
                Runtime::registry(),
            ))
            .unwrap(),
        );
        let host = BuiltinHost::new(HostOptions {
            store,
            tool_output_dir: root.join("output"),
            allowed_dirs: vec![],
            home: root.join("home"),
            shell: "bash".into(),
            config: Arc::new(|_| {
                Ok((
                    serde_json::json!({"sandbox": {"network": "off"}}),
                    Default::default(),
                ))
            }),
            global_config_dir: root.join("config"),
            env: Arc::new(cyber_core::env::ProcessEnv),
            models: None,
            temp_dir: root.join("temp"),
            sandbox_policy: None,
            sandbox_helper: cyber_sandbox::find_helper(),
            isolation: Default::default(),
        });
        let inv = Invocation {
            registration: None,
            session_id: "ses_scope".into(),
            directory: source.display().to_string(),
            agent: "build".into(),
            mode: "bypass".into(),
            message_id: "msg_scope".into(),
            call_id: "call_scope".into(),
            name: "worktree".into(),
            input: serde_json::Value::Null,
            attempt: 1,
            operation_key: "scope".into(),
            asker: Asker::detached(),
            rules: serde_json::Value::Null,
        };
        let ctx = Ctx {
            skill_paths: Default::default(),
            loaded_skill: Default::default(),
            compiler_feedback: Default::default(),
            hook_decision: None,
            denial: Default::default(),
            host: &host,
            inv: &inv,
            policy: host.policy(&inv).await.unwrap(),
            location: source.clone(),
            cancel: Default::default(),
        };
        // Use the same provisioning route with a diagnostic process to test actual enforcement.
        let args = vec!["-c".into(),
            "printf metadata > \"$1/index\"; printf target > \"$2/cyber.json\"; if printf forbidden > \"$3/file\" 2>/dev/null; then exit 8; fi; if printf forbidden > \"$4/file\" 2>/dev/null; then exit 9; fi".into(),
            "scope".into(), metadata.display().to_string(), target.display().to_string(),
            source.display().to_string(), sibling.display().to_string()];
        let owned = [metadata.clone(), target.clone()];
        let scope = Scope {
            git_roots: Some(&owned),
            ..Scope::default()
        };
        let prepared = prepare_scoped_command(&ctx, "bash", &args, scope)
            .await
            .unwrap();
        let result = tokio::process::Command::new(&prepared.program)
            .args(&prepared.args)
            .env_clear()
            .envs(prepared.env.iter().map(|(key, value)| (key, value)))
            .current_dir(&source)
            .output()
            .await
            .unwrap();
        assert!(
            result.status.success(),
            "{:?}: {}",
            result.status,
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(
            std::fs::read_to_string(metadata.join("index")).unwrap(),
            "metadata"
        );
        assert_eq!(
            std::fs::read_to_string(target.join("cyber.json")).unwrap(),
            "target"
        );
        assert!(!source.join("file").exists());
        assert!(!sibling.join("file").exists());
    }

    #[test]
    fn refusal_lines_of_common_shells_and_tools_name_their_path() {
        let workdir = Path::new("/repo");
        for (line, path) in [
            ("sh: /usr/x: Operation not permitted", "/usr/x"),
            ("bash: /usr/x: Read-only file system", "/usr/x"),
            (
                "/bin/sh: 1: cannot create /usr/x: Read-only file system",
                "/usr/x",
            ),
            (
                "touch: cannot touch '/usr/x': Read-only file system",
                "/usr/x",
            ),
            ("zsh:1: operation not permitted: /usr/x", "/usr/x"),
            ("cat: key.pem: Operation not permitted", "/repo/key.pem"),
            (
                "Set-Content: Access to the path '/usr/x' is denied.",
                "/usr/x",
            ),
            (
                "Out-File: Access to the path 'out dir/x.txt' is denied.",
                "/repo/out dir/x.txt",
            ),
        ] {
            assert_eq!(
                denied_path(line, workdir),
                Some(PathBuf::from(path)),
                "{line}"
            );
        }
        assert_eq!(
            denied_path("cat: x: No such file or directory", workdir),
            None
        );
        assert_eq!(
            denied_path("zsh:1: nice(5) failed: operation not permitted", workdir),
            None
        );
    }

    #[tokio::test]
    async fn tool_proxy_listens_on_the_supported_platform_transport() {
        let tmp = tempfile::tempdir().unwrap();
        let decide: Decide = Arc::new(|_| Box::pin(async { false }));
        let proxy = start_proxy(decide, tmp.path()).await.unwrap();
        match &proxy.endpoint {
            Endpoint::Tcp(port) => {
                assert!(!cyber_sandbox::proxy_uses_unix_socket());
                tokio::net::TcpStream::connect(("127.0.0.1", *port))
                    .await
                    .unwrap();
            }
            Endpoint::Unix(path) => {
                assert!(cyber_sandbox::proxy_uses_unix_socket());
                #[cfg(unix)]
                tokio::net::UnixStream::connect(path).await.unwrap();
                #[cfg(not(unix))]
                panic!("unsupported Unix proxy endpoint: {}", path.display());
            }
        }
    }
}
