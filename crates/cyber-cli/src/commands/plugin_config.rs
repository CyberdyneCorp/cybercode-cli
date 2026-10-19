//! Plugin settings from the command line (`plugins-marketplace` → User configuration):
//! `install` and `enable` ask for required `userConfig` values that are not set yet, and
//! `cyber plugins configure` sets any of them. Ordinary values go to
//! `plugins.<name>.config` in the user's global `cyber.jsonc`; values marked `secret` are
//! read with masked input on a terminal and stored only in the plugin secret store (the
//! OS keyring on macOS and Windows).

use std::io::{BufRead, IsTerminal, Write};

use cyber_core::plugins::{ConfigField, Manifest, PluginScope, config_fields, secret_owner};
use cyber_tools::plugin_host::PluginSecrets;
use serde_json::{Value, json};

use crate::context::Context;
use crate::error::CliError;

/// Where answers come from: the terminal (secrets masked) or piped stdin lines.
pub struct Answers {
    lines: Box<dyn BufRead>,
    terminal: bool,
}

impl Answers {
    pub fn stdin() -> Self {
        Self {
            terminal: std::io::stdin().is_terminal(),
            lines: Box::new(std::io::stdin().lock()),
        }
    }

    /// One answer; `None` at end of input.
    fn read(&mut self, prompt: &str, secret: bool) -> Result<Option<String>, CliError> {
        let mut err = std::io::stderr();
        write!(err, "{prompt}")?;
        err.flush()?;
        if secret && self.terminal {
            let answer = masked()?;
            writeln!(err)?;
            return Ok(answer);
        }
        let mut line = String::new();
        if self.lines.read_line(&mut line)? == 0 {
            return Ok(None);
        }
        Ok(Some(line.trim_end_matches(['\r', '\n']).to_owned()))
    }
}

/// Read a line from the terminal without echoing it; `None` on Ctrl+C or Ctrl+D.
fn masked() -> Result<Option<String>, CliError> {
    use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers, read};
    crossterm::terminal::enable_raw_mode()?;
    let mut value = String::new();
    let outcome = loop {
        let event = match read() {
            Ok(event) => event,
            Err(error) => break Err(error),
        };
        let Event::Key(key) = event else { continue };
        if key.kind == KeyEventKind::Release {
            continue;
        }
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Enter => break Ok(Some(std::mem::take(&mut value))),
            KeyCode::Char('c' | 'd') if control => break Ok(None),
            KeyCode::Backspace => {
                value.pop();
            }
            KeyCode::Char(c) => value.push(c),
            _ => {}
        }
    };
    crossterm::terminal::disable_raw_mode()?;
    Ok(outcome?)
}

/// Ask for every required setting of `name` that is not set yet. Input that ends early
/// leaves the rest unset and says how to finish.
pub fn ask_missing(
    ctx: &Context,
    (name, scope): (&str, PluginScope),
    manifest: &Manifest,
    answers: &mut Answers,
) -> Result<(), CliError> {
    let fields: Vec<ConfigField> = config_fields(manifest.user_config.as_ref())
        .into_iter()
        .filter(|field| field.required)
        .collect();
    if fields.is_empty() {
        return Ok(());
    }
    let secrets = cyber_tools::plugin_host::open_secrets(&ctx.paths);
    let owner = secret_owner(name, scope, &ctx.location);
    let current = ctx.config()?.value;
    let mut missing = Vec::new();
    for field in fields {
        let set = if field.secret {
            secrets
                .get(&owner, &field.name)
                .map_err(CliError::runtime)?
                .is_some()
        } else {
            current
                .pointer(&format!("/plugins/{name}/config/{}", field.name))
                .is_some()
        };
        if !set {
            missing.push(field);
        }
    }
    let store = Store {
        secrets: secrets.as_ref(),
        owner: &owner,
    };
    configure(ctx, name, manifest, &missing, answers, &store)
}

/// The secret store and the account owner a configuration writes to.
struct Store<'a> {
    secrets: &'a dyn PluginSecrets,
    owner: &'a str,
}

/// `cyber plugins configure <name> [--set key=value]`: set ordinary values from `--set`,
/// or ask for every setting the manifest declares.
pub fn run(ctx: &Context, name: &str, set: &[String]) -> Result<(), CliError> {
    let (scope, manifest) = installed_manifest(ctx, name)?;
    let fields = config_fields(manifest.user_config.as_ref());
    if fields.is_empty() {
        return Err(CliError::usage(format!(
            "plugin {name} declares no userConfig"
        )));
    }
    let secrets = cyber_tools::plugin_host::open_secrets(&ctx.paths);
    if set.is_empty() {
        let owner = secret_owner(name, scope, &ctx.location);
        let store = Store {
            secrets: secrets.as_ref(),
            owner: &owner,
        };
        let mut answers = Answers::stdin();
        return configure(ctx, name, &manifest, &fields, &mut answers, &store);
    }
    for assignment in set {
        let (key, text) = assignment
            .split_once('=')
            .ok_or_else(|| CliError::usage(format!("--set {assignment:?}: expected key=value")))?;
        let field = fields
            .iter()
            .find(|field| field.name == key)
            .ok_or_else(|| CliError::usage(format!("plugin {name} has no setting {key}")))?;
        if field.secret {
            return Err(CliError::usage(format!(
                "{key} is secret; run `cyber plugins configure {name}` to enter it without echo"
            )));
        }
        store_value(ctx, name, key, typed(&manifest, key, text))?;
    }
    println!("Configured plugin {name}");
    Ok(())
}

fn configure(
    ctx: &Context,
    name: &str,
    manifest: &Manifest,
    fields: &[ConfigField],
    answers: &mut Answers,
    store: &Store<'_>,
) -> Result<(), CliError> {
    for (done, field) in fields.iter().enumerate() {
        let about = field
            .description
            .as_deref()
            .map(|text| format!(" ({text})"))
            .unwrap_or_default();
        let prompt = format!("{name} {}{about}: ", field.name);
        let Some(answer) = answers.read(&prompt, field.secret)? else {
            let rest: Vec<&str> = fields[done..].iter().map(|f| f.name.as_str()).collect();
            eprintln!(
                "plugin {name} still needs {}; run `cyber plugins configure {name}`",
                rest.join(", ")
            );
            return Ok(());
        };
        if answer.is_empty() {
            continue;
        }
        if field.secret {
            store
                .secrets
                .put(store.owner, &field.name, &answer)
                .map_err(CliError::runtime)?;
        } else {
            store_value(
                ctx,
                name,
                &field.name,
                typed(manifest, &field.name, &answer),
            )?;
        }
    }
    Ok(())
}

/// A typed setting: numbers, booleans and JSON for non-string properties.
fn typed(manifest: &Manifest, key: &str, text: &str) -> Value {
    let kind = manifest
        .user_config
        .as_ref()
        .and_then(|schema| schema.pointer(&format!("/properties/{key}/type")))
        .and_then(Value::as_str)
        .unwrap_or("string");
    if kind == "string" {
        return Value::String(text.to_owned());
    }
    serde_json::from_str(text).unwrap_or_else(|_| Value::String(text.to_owned()))
}

/// Write `plugins.<name>.config.<key>` into the user's global configuration.
fn store_value(ctx: &Context, name: &str, key: &str, value: Value) -> Result<(), CliError> {
    cyber_core::config::set_global_key(&ctx.paths.config, "plugins", |current| {
        let mut plugins = current
            .cloned()
            .filter(Value::is_object)
            .unwrap_or(json!({}));
        let plugin = &mut plugins[name];
        if !plugin.is_object() {
            *plugin = json!({});
        }
        if !plugin["config"].is_object() {
            plugin["config"] = json!({});
        }
        plugin["config"][key] = value;
        plugins
    })
    .map(|_| ())
    .map_err(CliError::runtime)
}

/// The most specific scope listing plugin `name` with an installed root, and the
/// manifest read from that root.
pub fn installed_manifest(ctx: &Context, name: &str) -> Result<(PluginScope, Manifest), CliError> {
    let resolved = ctx.config()?;
    let (scope, root) = resolved
        .plugins
        .iter()
        .filter(|plugin| plugin.name == name)
        .find_map(|plugin| {
            let root = plugin.root.clone().filter(|root| root.is_dir())?;
            Some((plugin.scope, root))
        })
        .ok_or_else(|| CliError::usage(format!("plugin {name} is not installed")))?;
    let manifest = Manifest::read(&root).map_err(|error| CliError::runtime(error.to_string()))?;
    Ok((scope, manifest))
}

/// Delete every stored secret of `manifest` that `name` owns in `scope` (when it is
/// removed from that scope); the secrets of a same-named plugin elsewhere stay.
pub fn forget_secrets(
    ctx: &Context,
    (name, scope): (&str, PluginScope),
    manifest: &Manifest,
) -> Result<(), CliError> {
    let secrets = cyber_tools::plugin_host::open_secrets(&ctx.paths);
    let owner = secret_owner(name, scope, &ctx.location);
    for field in config_fields(manifest.user_config.as_ref())
        .into_iter()
        .filter(|field| field.secret)
    {
        secrets
            .delete(&owner, &field.name)
            .map_err(CliError::runtime)?;
    }
    Ok(())
}
