//! Running bash under the OS sandbox (`sandbox`): roots, masked environment, the network
//! proxy and `network` permission requests for domains outside the allowlist.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use cyber_sandbox::proxy::{Decide, Proxy};
use cyber_sandbox::{Launch, NetworkMode, Policy, SandboxConfig, matches_domain, protected_in};
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
    _proxy: Option<Proxy>,
}

pub(crate) async fn prepare(
    ctx: &Ctx<'_>,
    shell: &str,
    command: &str,
) -> Result<Prepared, ToolError> {
    prepare_command(ctx, shell, &["-c".to_string(), command.to_string()]).await
}

pub(crate) async fn prepare_command(
    ctx: &Ctx<'_>,
    program: &str,
    args: &[String],
) -> Result<Prepared, ToolError> {
    prepare_scoped_command(ctx, program, args, true, None, &[]).await
}

/// Setup must not gain ambient temporary-directory access outside its owned roots.
pub(crate) async fn prepare_worktree_command(
    ctx: &Ctx<'_>,
    program: &str,
    args: &[String],
    credentials: &[String],
) -> Result<Prepared, ToolError> {
    prepare_scoped_command(ctx, program, args, false, None, credentials).await
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

/// Git creation may mutate only verified common metadata and the reserved target.
pub(crate) async fn prepare_worktree_git(
    ctx: &Ctx<'_>,
    args: &[String],
    writable: &[PathBuf],
) -> Result<Prepared, ToolError> {
    prepare_scoped_command(ctx, "git", args, false, Some(writable), &[]).await
}

async fn prepare_scoped_command(
    ctx: &Ctx<'_>,
    program: &str,
    args: &[String],
    ambient_temp: bool,
    git_roots: Option<&[PathBuf]>,
    extra_credentials: &[String],
) -> Result<Prepared, ToolError> {
    let (config, sources) = (ctx.host.opts.config)(&ctx.location).unwrap_or_default();
    let home = &ctx.host.opts.home;
    let sandbox = SandboxConfig::resolve(
        &config,
        &sources,
        ctx.host.opts.sandbox_policy.as_deref(),
        home,
    );
    let tmp = session_tmp(ctx)?;
    let (asks, proxy) =
        if sandbox.policy != Policy::FullAccess && sandbox.network == NetworkMode::Proxy {
            let (tx, rx) = mpsc::channel(16);
            let decide = decider(
                &sandbox.allowed_domains,
                ctx.host.network_approvals(&ctx.inv.session_id),
                tx,
            );
            let proxy = start_proxy(decide, &tmp)
                .await
                .map_err(|e| failed(format!("Could not start the sandbox network proxy: {e}")))?;
            (Some(rx), Some(proxy))
        } else {
            (None, None)
        };
    let writable = match git_roots {
        Some(owned) => owned
            .iter()
            .cloned()
            .chain([tmp.clone(), ctx.host.opts.tool_output_dir.clone()])
            .chain(sandbox.extra_writable.iter().cloned())
            .collect(),
        None => roots(ctx, &sandbox, &tmp, ambient_temp),
    };
    let launch = Launch {
        read_only: writable
            .iter()
            .filter(|root| !git_roots.is_some_and(|owned| owned.contains(root)))
            .flat_map(|root| protected_in(root))
            .collect(),
        unreadable: sandbox.unreadable(home),
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
        extra_credentials,
    );
    let mut env: Vec<(String, String)> = base
        .into_iter()
        .filter(|(k, _)| !wrapped.env.contains_key(k) && k != "TMPDIR")
        .collect();
    env.extend(wrapped.env);
    env.push(("TMPDIR".into(), tmp.display().to_string()));
    Ok(Prepared {
        program: wrapped.program,
        args: wrapped.args,
        env,
        asks,
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
    cyber_sandbox::mask_env(vars, &sandbox.env_allow, &credentials)
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

/// Allowlisted and session-approved domains pass at once; others wait for the tool loop.
fn decider(
    allowed: &[String],
    approved: Arc<Mutex<HashSet<String>>>,
    tx: mpsc::Sender<NetworkAsk>,
) -> Decide {
    let allowed = allowed.to_vec();
    Arc::new(move |host: String| {
        let known = matches_domain(&host, &allowed)
            || approved
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .contains(&host);
        let tx = tx.clone();
        Box::pin(async move {
            if known {
                return true;
            }
            let (reply, answer) = oneshot::channel();
            tx.send((host, reply)).await.is_ok() && answer.await.unwrap_or(false)
        })
    })
}

/// Ask for `network` on a domain; an approval holds for the rest of the Session.
pub(crate) async fn answer(ctx: &Ctx<'_>, host: String) -> bool {
    let req = Request {
        action: "network".into(),
        resources: vec![host.clone()],
        read_only: true,
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

/// Location, worktree root, the Session's temp directory, managed tool output and
/// `sandbox.writable_roots`.
fn roots(ctx: &Ctx<'_>, sandbox: &SandboxConfig, tmp: &Path, ambient_temp: bool) -> Vec<PathBuf> {
    let mut roots = vec![
        ctx.location.clone(),
        cyber_core::config::project_root(&ctx.location),
        tmp.to_path_buf(),
        ctx.host.opts.tool_output_dir.clone(),
    ];
    // macOS tools such as mktemp use the per-user temp directory even when TMPDIR is set.
    if cfg!(target_os = "macos") && ambient_temp {
        roots.push(std::env::temp_dir());
    }
    roots.extend(sandbox.extra_writable.iter().cloned());
    let mut unique: Vec<PathBuf> = Vec::new();
    for root in roots {
        if !unique.contains(&root) {
            unique.push(root);
        }
    }
    unique
}

fn session_tmp(ctx: &Ctx<'_>) -> Result<PathBuf, ToolError> {
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
        });
        let inv = Invocation {
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
        let prepared = prepare_scoped_command(
            &ctx,
            "bash",
            &args,
            false,
            Some(&[metadata.clone(), target.clone()]),
            &[],
        )
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
