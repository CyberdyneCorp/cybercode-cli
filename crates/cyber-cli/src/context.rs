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
        let env = ProcessEnv;
        let home = paths::home_dir(&env).ok_or_else(|| {
            CliError::runtime("cannot determine the home directory").with_hint("set HOME")
        })?;
        let paths = Paths::resolve(&env, &home);
        paths.ensure()?;
        config::ensure_global_config(&paths)?;
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
