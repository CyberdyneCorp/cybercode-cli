//! Per-invocation context: environment, paths, Location and config request.

use std::path::PathBuf;

use cyber_core::config::{self, LoadRequest, Resolved, TrustReport};
use cyber_core::env::ProcessEnv;
use cyber_core::paths::{self, DatabaseLocation, Paths};
use cyber_core::version::{BuildInfo, build_info};
use serde_json::{Map, Value};

use crate::cli::GlobalArgs;
use crate::error::CliError;

pub struct Context {
    pub env: ProcessEnv,
    pub home: PathBuf,
    pub paths: Paths,
    pub location: PathBuf,
    pub build: BuildInfo,
    profile: Option<String>,
    overrides: Vec<String>,
    flags: Value,
}

impl Context {
    pub fn new(global: &GlobalArgs) -> Result<Self, CliError> {
        let ctx = Self::existing(global)?;
        ctx.paths.ensure()?;
        cyber_tools::plugin_host::hide_computer_artifacts(&ctx.paths, &ctx.home)
            .map_err(CliError::runtime)?;
        // Before any command can start a sandboxed process (`sandbox run`, hooks, MCP
        // servers, LSP), not only when an App is built.
        cyber_tools::plugin_host::hide_credential_files(&ctx.paths, &ctx.home);
        config::ensure_global_config(&ctx.paths)?;
        Ok(ctx)
    }

    /// The context without creating any directory or file (`cyber uninstall`).
    pub fn existing(global: &GlobalArgs) -> Result<Self, CliError> {
        let env = ProcessEnv;
        let home = paths::home_dir(&env).ok_or_else(|| {
            CliError::runtime("cannot determine the home directory").with_hint("set HOME")
        })?;
        let paths = Paths::resolve(&env, &home);
        Ok(Self {
            location: location(global)?,
            env,
            home,
            paths,
            build: build_info(),
            profile: global.profile.clone(),
            overrides: global.config.clone(),
            flags: flags(global),
        })
    }

    fn request(&self) -> LoadRequest<'_> {
        LoadRequest {
            location: &self.location,
            paths: &self.paths,
            env: &self.env,
            home: &self.home,
            profile: self.profile.as_deref(),
            overrides: &self.overrides,
            flags: self.flags.clone(),
        }
    }

    pub fn config(&self) -> Result<Resolved, CliError> {
        Ok(config::load(&self.request())?)
    }

    /// The defaults and global layers only: what global-only keys (`mcp_serve`) resolve
    /// from, without reading the Location's project configuration.
    pub fn global_config(&self) -> Result<Value, CliError> {
        Ok(config::load_global(&self.request())?)
    }

    /// Trusted global owner transport settings, isolated from unrelated peer/project settings.
    pub fn owner_server_config(&self) -> Result<Value, CliError> {
        Ok(config::load_owner_server(&self.request())?)
    }

    pub fn hook_resolver(&self) -> std::sync::Arc<cyber_tools::HookConfigFn> {
        let paths = self.paths.clone();
        let home = self.home.clone();
        let profile = self.profile.clone();
        let overrides = self.overrides.clone();
        let flags = self.flags.clone();
        std::sync::Arc::new(move |location| {
            config::load(&LoadRequest {
                location,
                paths: &paths,
                env: &ProcessEnv,
                home: &home,
                profile: profile.as_deref(),
                overrides: &overrides,
                flags: flags.clone(),
            })
            .map_err(|error| error.to_string())
        })
    }

    /// A tool host over this Location's configuration, without a server or a Session:
    /// what `cyber sandbox` and `cyber permissions test` evaluate with. `sandbox` is the
    /// `--sandbox`/`--policy`/`--profile` selection.
    pub fn local_host(
        &self,
        database: DatabaseLocation,
        sandbox: Option<String>,
    ) -> Result<std::sync::Arc<cyber_tools::BuiltinHost>, CliError> {
        let store = std::sync::Arc::new(cyber_store::Store::open(cyber_store::StoreOptions::new(
            database,
            cyber_server::runtime::Runtime::registry(),
        ))?);
        let resolver = self.hook_resolver();
        let shell = self.config()?.value["shell"]
            .as_str()
            .map(str::to_string)
            .or_else(|| cyber_core::env::EnvSource::get(&self.env, "SHELL"))
            .unwrap_or_else(|| "/bin/sh".into());
        Ok(cyber_tools::BuiltinHost::new(cyber_tools::HostOptions {
            store,
            tool_output_dir: self.paths.data.join("tool-output"),
            allowed_dirs: Vec::new(),
            home: self.home.clone(),
            shell,
            config: std::sync::Arc::new(move |location| {
                resolver(location).map(|resolved| (resolved.value, resolved.sources))
            }),
            global_config_dir: self.paths.config.clone(),
            env: std::sync::Arc::new(ProcessEnv),
            models: None,
            temp_dir: self.paths.tmp.join("sessions"),
            sandbox_policy: sandbox,
            sandbox_helper: cyber_sandbox::find_helper(),
            isolation: cyber_tools::IsolationOptions::probe(false),
        }))
    }

    pub fn withheld_hooks(&self) -> Result<Vec<config::RawHookSection>, CliError> {
        Ok(config::withheld_hook_sections(&self.request())?)
    }

    pub fn trust(&self) -> Result<TrustReport, CliError> {
        Ok(config::trust_report(&self.request())?)
    }

    pub fn database(&self) -> DatabaseLocation {
        paths::database_location(&self.paths, &self.env, self.build.channel)
    }
}

fn location(global: &GlobalArgs) -> Result<PathBuf, CliError> {
    let dir = match &global.cwd {
        Some(dir) => dir.clone(),
        None => std::env::current_dir()?,
    };
    std::fs::canonicalize(&dir)
        .map_err(|_| CliError::usage(format!("--cwd {}: no such directory", dir.display())))
}

fn flags(global: &GlobalArgs) -> Value {
    let mut map = Map::new();
    if let Some(model) = &global.model {
        map.insert("model".into(), Value::String(model.clone()));
    }
    if let Some(mode) = &global.mode {
        map.insert("mode".into(), Value::String(mode.clone()));
    }
    Value::Object(map)
}
