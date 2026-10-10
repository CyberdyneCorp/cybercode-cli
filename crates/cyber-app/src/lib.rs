//! Assembly of a running Cyber Code server: store, catalog, tools, sandbox, snapshots,
//! runtime and HTTP router, plus the server lifecycle (`server-api` → One server per user,
//! Background service management, Listener defaults, Local password authentication).

pub mod backup;
mod host;
mod registered_status;
mod registration;
pub mod retention;
mod server;
mod services;

use std::path::PathBuf;
use std::sync::Arc;

use cyber_core::config::{self, LoadRequest};
use cyber_core::env::{EnvSource, ProcessEnv};
use cyber_core::paths::{DatabaseLocation, Paths};
use cyber_llm::RetryPolicy;
use cyber_llm::catalog::{SourceOptions, load_source};
use cyber_server::http::{self, AppState, HttpOptions};
use cyber_server::runtime::{
    CatalogResolver, CompactionConfig, ModelResolver, NoSnapshots, Runtime, RuntimeOptions,
    Snapshots, ToolHost,
};
use cyber_store::{Store, StoreOptions};
use cyber_tools::{BuiltinHost, ConfigFn, HookConfigFn, HostOptions};
use serde_json::{Value, json};

pub use registered_status::registered_lsp_status;
pub use registration::{
    Registration, ServerClientInfo, health, read_registration, registration_path, start_service,
    stop_service,
};
pub use server::{
    ServeOptions, password, replace_password, run_server, run_server_until, serve_stdio,
};

/// What an App is built from.
pub struct AppOptions {
    pub paths: Paths,
    pub home: PathBuf,
    pub database: DatabaseLocation,
    /// The server's own working directory: the default Location.
    pub default_directory: PathBuf,
    /// `--sandbox`, the only way to select `full-access`.
    pub sandbox_policy: Option<String>,
    /// Snapshots are off for ephemeral runs.
    pub snapshots: bool,
    /// Whether a client can answer permission requests.
    pub interactive: bool,
    /// Basic-auth password for TCP; `None` disables auth (embedded).
    pub password: Option<String>,
}

pub struct App {
    pub runtime: Runtime,
    pub store: Arc<Store>,
    pub host: Arc<BuiltinHost>,
    pub state: AppState,
    pub paths: Paths,
    /// Resolved config of a Location.
    pub config: Arc<ConfigFn>,
}

pub fn version() -> &'static str {
    cyber_core::version::build_info().version
}

/// The resolved config of a Location as `(value, sources)`; empty on errors.
pub fn hook_config_loader(paths: Paths, home: PathBuf) -> Arc<HookConfigFn> {
    Arc::new(move |location: &std::path::Path| {
        let req = LoadRequest {
            location,
            paths: &paths,
            env: &ProcessEnv,
            home: &home,
            profile: None,
            overrides: &[],
            flags: json!({}),
        };
        config::load(&req).map_err(|error| error.to_string())
    })
}

fn withheld_hook_loader(paths: Paths, home: PathBuf) -> Arc<services::HookReviewFn> {
    Arc::new(move |location| {
        config::withheld_hook_sections(&LoadRequest {
            location,
            paths: &paths,
            env: &ProcessEnv,
            home: &home,
            profile: None,
            overrides: &[],
            flags: json!({}),
        })
        .map_err(|error| error.to_string())
    })
}

pub fn config_loader(paths: Paths, home: PathBuf) -> Arc<ConfigFn> {
    let resolved = hook_config_loader(paths, home);
    Arc::new(move |location| resolved(location).map(|resolved| (resolved.value, resolved.sources)))
}

impl App {
    pub async fn build(opts: AppOptions) -> Result<App, String> {
        let env = ProcessEnv;
        let config = config_loader(opts.paths.clone(), opts.home.clone());
        let global = config(&opts.default_directory)
            .map(|(v, _)| v)
            .unwrap_or(Value::Null);
        let loaded = load_source(&SourceOptions::from_env(&env, &opts.paths.cache))
            .await
            .map_err(|e| e.to_string())?;
        let resolver = Arc::new(CatalogResolver::new(&loaded.data, global.clone(), &env));
        let store = Arc::new(
            Store::open(StoreOptions::new(
                opts.database.clone(),
                Runtime::registry(),
            ))
            .map_err(|e| e.to_string())?,
        );
        let host = BuiltinHost::new(HostOptions {
            store: Arc::clone(&store),
            tool_output_dir: opts.paths.data.join("tool-output"),
            allowed_dirs: Vec::new(),
            home: opts.home.clone(),
            shell: shell(&global, &env),
            config: Arc::clone(&config),
            global_config_dir: opts.paths.config.clone(),
            env: Arc::new(ProcessEnv),
            models: Some(Arc::clone(&resolver) as Arc<dyn ModelResolver>),
            temp_dir: opts.paths.tmp.join("sessions"),
            sandbox_policy: opts.sandbox_policy.clone(),
            sandbox_helper: cyber_sandbox::find_helper(),
        });
        host.attach_hook_config(
            hook_config_loader(opts.paths.clone(), opts.home.clone()),
            cyber_core::trust::TrustStore::new(opts.paths.trust_file()),
        )?;
        let lsp_options = cyber_tools::lsp::LaunchOptions {
            checkout_claim: Some(host.lsp_checkout_claim()),
            paths: opts.paths.clone(),
            home: opts.home.clone(),
            environment: std::env::vars().collect(),
            profile: None,
            overrides: vec![],
            flags: json!({}),
            sandbox_policy: opts.sandbox_policy.clone(),
            helper: cyber_sandbox::find_helper(),
            credential_env_names: resolver.credential_env_names(),
        };
        host.attach_lsp(Arc::new(move |location| {
            let launcher = Arc::new(
                cyber_tools::lsp::LocalLauncher::new(location, lsp_options.clone())
                    .map_err(|error| error.error)?,
            );
            launcher.pool().map_err(|error| error.error)
        }))?;
        let snapshots: Arc<dyn Snapshots> = if opts.snapshots {
            let loader = Arc::clone(&config);
            let snaps = cyber_snapshot::GitSnapshots::new(
                opts.paths.data.clone(),
                Arc::new(move |p| loader(p).map(|(v, _)| v).unwrap_or(Value::Null)),
            );
            snaps.spawn_gc();
            snaps
        } else {
            Arc::new(NoSnapshots)
        };
        let remote_tools = Arc::new(http::remote_tools::RemoteTools::default());
        let tools = host::AppHost {
            builtin: Arc::clone(&host),
            remote: Arc::clone(&remote_tools),
        };
        let runtime = Runtime::new(RuntimeOptions {
            store: Arc::clone(&store),
            resolver: Arc::clone(&resolver) as Arc<dyn ModelResolver>,
            tools: Arc::new(tools) as Arc<dyn ToolHost>,
            global_config_dir: opts.paths.config.clone(),
            shell: shell(&global, &env),
            claude_compat: global
                .pointer("/compat/claude")
                .and_then(Value::as_bool)
                .unwrap_or(true),
            compaction: compaction(&global),
            retry: RetryPolicy::default(),
            max_steps: None,
            today: None,
            interactive: opts.interactive,
            snapshots,
        });
        host.attach(runtime.clone());
        runtime.recover_jobs().await.map_err(|e| e.to_string())?;
        let config_for_app = Arc::clone(&config);
        let services = services::AppServices::new(
            resolver,
            Arc::clone(&host),
            Arc::clone(&remote_tools),
            config,
            opts.paths.clone(),
            withheld_hook_loader(opts.paths.clone(), opts.home.clone()),
        );
        let state = AppState {
            service: None,
            runtime: runtime.clone(),
            remote_tools,
            store: Arc::clone(&store),
            services: Arc::new(services),
            options: Arc::new(HttpOptions {
                version: version().into(),
                password: opts.password,
                cors_origins: strings(global.pointer("/remote/cors_origins")),
                default_directory: opts.default_directory,
                features: [
                    "session",
                    "message",
                    "permission",
                    "question",
                    "model",
                    "agent",
                    "tool",
                    "fs",
                    "command",
                    "event",
                ]
                .map(str::to_string)
                .to_vec(),
            }),
        };
        Ok(App {
            runtime,
            store,
            host,
            state,
            paths: opts.paths,
            config: Arc::clone(&config_for_app),
        })
    }

    pub fn router(&self) -> axum::Router {
        http::router(self.state.clone())
    }

    /// An in-process client with no listener.
    pub fn embedded(&self) -> http::EmbeddedClient {
        http::EmbeddedClient::new(self.router())
    }
}

fn shell(config: &Value, env: &dyn EnvSource) -> String {
    config["shell"]
        .as_str()
        .map(str::to_string)
        .or_else(|| env.get("SHELL"))
        .unwrap_or_else(|| "/bin/sh".into())
}

fn compaction(config: &Value) -> CompactionConfig {
    let c = &config["compaction"];
    let d = CompactionConfig::default();
    CompactionConfig {
        auto: c["auto"].as_bool().unwrap_or(d.auto),
        buffer: c["buffer"].as_u64().unwrap_or(d.buffer),
        keep_tokens: c
            .pointer("/keep/tokens")
            .and_then(Value::as_u64)
            .unwrap_or(d.keep_tokens),
        keep_turns: c
            .pointer("/keep/turns")
            .and_then(Value::as_u64)
            .map(|n| n as usize),
        model: c["model"].as_str().map(str::to_string),
    }
}

fn strings(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect()
}
