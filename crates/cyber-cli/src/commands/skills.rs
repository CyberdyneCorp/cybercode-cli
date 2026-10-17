//! `cyber skills list|show <name>` and `cyber commands list` (`skills-commands` → Skill and
//! command inspection): the app's `GET /skills` and `GET /commands` for the current Location.

use clap::Subcommand;
use cyber_app::{App, AppOptions};
use cyber_client::Client;
use cyber_core::paths::DatabaseLocation;
use serde_json::{Value, json};

use crate::cli::GlobalArgs;
use crate::context::Context;
use crate::error::CliError;
use crate::output;

#[derive(Debug, Subcommand)]
pub enum SkillsCmd {
    /// List skills with source, scope, permission effect and diagnostics.
    List {
        /// Evaluate permissions for this agent.
        #[arg(long, default_value = "build")]
        agent: String,
    },
    /// Show one skill, including skipped definitions of that name.
    Show {
        name: String,
        /// Evaluate permissions for this agent.
        #[arg(long, default_value = "build")]
        agent: String,
    },
    /// Approve a remote skill source (`https://` entry) configured outside your user config
    /// for this checkout, bound to its URL and currently cached content.
    Trust { url: String },
}

#[derive(Debug, Subcommand)]
pub enum CommandsCmd {
    /// List slash commands and command discovery diagnostics.
    List,
    /// Approve a project command or skill template that runs shell commands
    /// (`` !`cmd` ``) for this checkout. A changed template needs a new approval.
    Trust { name: String },
}

/// An in-memory app for this Location: nothing is written to the shared database.
pub(crate) struct Inspector {
    client: Client,
    app: App,
}

impl Inspector {
    pub(crate) async fn start(ctx: &Context) -> Result<Self, CliError> {
        let app = App::build(AppOptions {
            paths: ctx.paths.clone(),
            home: ctx.home.clone(),
            database: DatabaseLocation::Memory,
            default_directory: ctx.location.clone(),
            sandbox_policy: None,
            snapshots: false,
            interactive: false,
            password: None,
            mcp_credentials: None,
            remote_skill_fetcher: None,
            workflow_cores: None,
        })
        .await
        .map_err(CliError::runtime)?;
        Ok(Self {
            client: Client::embedded(app.embedded()),
            app,
        })
    }

    pub(crate) async fn skills(&self, agent: &str) -> Result<Value, CliError> {
        let path = format!("/skills?agent={}", encode(agent));
        let body = self.client.get(&path).await.map_err(client_error)?;
        Ok(body["data"].clone())
    }

    pub(crate) async fn commands(&self) -> Result<Value, CliError> {
        let body = self.client.get("/commands").await.map_err(client_error)?;
        Ok(body["data"].clone())
    }

    pub(crate) async fn shutdown(self) {
        self.app.runtime.shutdown().await;
    }
}

fn client_error(error: cyber_client::ClientError) -> CliError {
    CliError::runtime(error.to_string())
}

fn encode(value: &str) -> String {
    value
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

pub fn run(cmd: SkillsCmd, ctx: &Context, global: &GlobalArgs) -> Result<(), CliError> {
    let runtime = super::serve::runtime()?;
    runtime.block_on(async {
        let inspector = Inspector::start(ctx).await?;
        let result = match &cmd {
            SkillsCmd::List { agent } | SkillsCmd::Show { agent, .. } => {
                inspector.skills(agent).await
            }
            SkillsCmd::Trust { url } => inspector
                .app
                .host
                .trust_remote_skill_source(&ctx.location, url)
                .map(Value::String)
                .map_err(CliError::usage),
        };
        inspector.shutdown().await;
        match cmd {
            SkillsCmd::List { .. } => list(global, &result?),
            SkillsCmd::Show { name, .. } => show(global, &result?, &name),
            SkillsCmd::Trust { url } => trusted_source(global, &url, &result?),
        }
    })
}

fn trusted_source(global: &GlobalArgs, url: &str, digest: &Value) -> Result<(), CliError> {
    if output::is_json(global.format) {
        return output::json(&json!({"url": url, "digest": digest}));
    }
    println!("Approved {url} ({}) for this checkout", text(digest));
    Ok(())
}

pub fn run_commands(cmd: CommandsCmd, ctx: &Context, global: &GlobalArgs) -> Result<(), CliError> {
    if let CommandsCmd::Trust { name } = cmd {
        return trust_template(ctx, global, &name);
    }
    let runtime = super::serve::runtime()?;
    runtime.block_on(async {
        let inspector = Inspector::start(ctx).await?;
        let commands = inspector.commands().await;
        let skills = inspector.skills("build").await;
        inspector.shutdown().await;
        commands_list(global, &commands?, &skills?["command_issues"])
    })
}

fn trust_template(ctx: &Context, global: &GlobalArgs, name: &str) -> Result<(), CliError> {
    let runtime = super::serve::runtime()?;
    runtime.block_on(async {
        let inspector = Inspector::start(ctx).await?;
        let approved = inspector.app.host.trust_template(&ctx.location, name);
        inspector.shutdown().await;
        let digest = approved.map_err(CliError::usage)?;
        if output::is_json(global.format) {
            return output::json(&json!({"name": name, "digest": digest}));
        }
        println!("Approved /{name} ({digest}) for this checkout");
        Ok(())
    })
}

fn entries(catalog: &Value) -> &[Value] {
    catalog["skills"].as_array().map_or(&[], Vec::as_slice)
}

fn text(value: &Value) -> &str {
    value.as_str().unwrap_or("")
}

fn summary(entry: &Value) -> String {
    let effect = entry["permission"].as_str().unwrap_or("-");
    format!(
        "{}  [{}] {} {}  {}",
        text(&entry["name"]),
        text(&entry["scope"]),
        text(&entry["status"]),
        effect,
        text(&entry["source"])
    )
}

fn print_diagnostics(entry: &Value) {
    for diagnostic in entry["diagnostics"].as_array().into_iter().flatten() {
        println!("    {}", text(diagnostic));
    }
}

fn list(global: &GlobalArgs, catalog: &Value) -> Result<(), CliError> {
    if output::is_json(global.format) {
        return output::json(catalog);
    }
    for entry in entries(catalog) {
        println!("{}", summary(entry));
        print_diagnostics(entry);
    }
    Ok(())
}

fn show(global: &GlobalArgs, catalog: &Value, name: &str) -> Result<(), CliError> {
    let matches: Vec<&Value> = entries(catalog)
        .iter()
        .filter(|entry| entry["name"] == name)
        .collect();
    if matches.is_empty() {
        let available: Vec<&str> = entries(catalog)
            .iter()
            .filter(|e| e["status"] == "loaded")
            .map(|e| text(&e["name"]))
            .collect();
        return Err(CliError::runtime(format!(
            "skill {name:?} not found. Available: {}",
            available.join(", ")
        )));
    }
    if output::is_json(global.format) {
        return output::json(&json!({"agent": catalog["agent"], "skills": matches}));
    }
    for entry in matches {
        println!("{}", text(&entry["name"]));
        for (label, key) in [
            ("status", "status"),
            ("description", "description"),
            ("source", "source"),
            ("scope", "scope"),
            ("permission", "permission"),
            ("argument hint", "argument_hint"),
            ("user invocable", "user_invocable"),
            ("model invocable", "model_invocable"),
        ] {
            let value = match &entry[key] {
                Value::String(s) => s.clone(),
                Value::Null => "-".into(),
                other => other.to_string(),
            };
            println!("  {label}: {value}");
        }
        if entry["diagnostics"]
            .as_array()
            .is_some_and(|d| !d.is_empty())
        {
            println!("  diagnostics:");
            print_diagnostics(entry);
        }
    }
    Ok(())
}

fn commands_list(global: &GlobalArgs, commands: &Value, issues: &Value) -> Result<(), CliError> {
    if output::is_json(global.format) {
        return output::json(&json!({"commands": commands, "issues": issues}));
    }
    for command in commands.as_array().into_iter().flatten() {
        let hint = command["argument_hint"].as_str().unwrap_or("");
        println!(
            "/{} {hint}  [{}]  {}",
            text(&command["name"]),
            text(&command["source"]),
            text(&command["description"])
        );
    }
    for issue in issues.as_array().into_iter().flatten() {
        println!(
            "issue: {}: {}",
            text(&issue["path"]),
            text(&issue["reason"])
        );
    }
    Ok(())
}
