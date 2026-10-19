//! A loaded plugin's components (`plugins-marketplace` → Component discovery defaults).
//! Declared paths win; otherwise `skills/`, `commands/`, `agents/`, `hooks/hooks.json` and
//! `.mcp.json` under the plugin root are used. Hooks, MCP servers and agents become one
//! `plugin:` configuration layer; skill and command folders are handed to discovery under
//! [`COMPONENTS_KEY`]. A component that would collide with an existing name is renamed
//! `<plugin>:<name>` (MCP servers `<plugin>-<name>`, since server names exclude `:`).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

use super::active::ActivePlugin;

/// The resolved configuration key listing each loaded plugin's skill and command
/// folders. Only the loader writes it; any layer's value is discarded.
pub const COMPONENTS_KEY: &str = "plugin_components";

/// Strings in hooks and MCP definitions that expand to the plugin root.
const ROOT_VARIABLES: [&str; 2] = ["${CYBER_PLUGIN_ROOT}", "${CLAUDE_PLUGIN_ROOT}"];

/// Components that need a later capability (`providers`, `workflows`, `channels`,
/// `mods`); themes load in the TUI. Tools come from the `main` host's `initialize`.
const UNSUPPORTED: [&str; 4] = ["providers", "workflows", "channels", "mods"];

pub struct PluginLayer {
    /// `plugin:<name>@<version>:<root>`.
    pub label: String,
    pub root: PathBuf,
    /// `hooks`, `mcp` and `agents` only.
    pub value: Value,
    /// This plugin's entry under [`COMPONENTS_KEY`].
    pub components: Value,
    pub warnings: Vec<String>,
}

/// The configuration contributed by `plugin`. `agents` and `servers` are the names other
/// layers (and earlier plugins) already use; this plugin's names are added to them.
pub fn plugin_layer(
    plugin: &ActivePlugin,
    agents: &mut BTreeSet<String>,
    servers: &mut BTreeSet<String>,
) -> PluginLayer {
    let mut warnings = Vec::new();
    let name = &plugin.name;
    for component in UNSUPPORTED {
        if plugin.manifest.component(component).is_some() {
            warnings.push(format!(
                "plugin {name}: {component} are not supported yet and were not loaded"
            ));
        }
    }
    if plugin.manifest.component("tools").is_some() {
        warnings.push(format!(
            "plugin {name}: the tools path is not read; tools are contributed by the `main` host in `initialize`"
        ));
    }
    let mut value = Map::new();
    let mcp = mcp(plugin, servers, &mut warnings);
    let own: BTreeSet<&str> = mcp
        .iter()
        .flat_map(|mcp| mcp.as_object().into_iter().flatten())
        .map(|(name, _)| name.as_str())
        .collect();
    if let Some(hooks) = hooks(plugin, &own, &mut warnings) {
        value.insert("hooks".into(), hooks);
    }
    if let Some(mcp) = mcp {
        value.insert("mcp".into(), mcp);
    }
    if let Some(found) = agent_profiles(plugin, agents, &mut warnings) {
        value.insert("agents".into(), found);
    }
    let folders = |component: &str| -> Vec<String> {
        paths(plugin, component, component)
            .into_iter()
            .filter(|path| path.is_dir())
            .map(|path| path.display().to_string())
            .collect()
    };
    let components = json!({
        "name": name,
        "skills": folders("skills"),
        "commands": folders("commands"),
    });
    PluginLayer {
        label: format!(
            "plugin:{name}@{}:{}",
            plugin.manifest.version,
            plugin.root.display()
        ),
        root: plugin.root.clone(),
        value: Value::Object(value),
        components,
        warnings,
    }
}

/// A plugin's theme folders: its manifest's `themes` paths, or `themes/`. The TUI reads
/// `<folder>/*.json` from each.
pub fn theme_dirs(root: &Path) -> Vec<PathBuf> {
    let Ok(manifest) = super::Manifest::read(root) else {
        return Vec::new();
    };
    let plugin = ActivePlugin {
        name: manifest.name.clone(),
        scope: super::PluginScope::User,
        root: root.to_path_buf(),
        manifest,
        digest: String::new(),
    };
    paths(&plugin, "themes", "themes")
        .into_iter()
        .filter(|dir| dir.is_dir())
        .collect()
}

/// The declared paths of `component`, or `default` when it is omitted.
fn paths(plugin: &ActivePlugin, component: &str, default: &str) -> Vec<PathBuf> {
    let declared = plugin.manifest.component(component);
    let declared: Vec<&str> = match declared {
        Some(paths) => paths.iter().map(String::as_str).collect(),
        None => vec![default],
    };
    let relative: Vec<PathBuf> = declared
        .into_iter()
        .filter_map(super::manifest::inside)
        .collect();
    relative
        .into_iter()
        .map(|path| plugin.root.join(path))
        .collect()
}

fn read_json(path: &Path, warnings: &mut Vec<String>) -> Option<Value> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Err(error) => {
            warnings.push(format!("{}: {error}", path.display()));
            return None;
        }
    };
    match crate::config::parse_jsonc(&path.display().to_string(), &text) {
        Ok(value) => Some(value),
        Err(error) => {
            warnings.push(format!("{error}; ignored"));
            None
        }
    }
}

/// `${CYBER_PLUGIN_ROOT}` in every string.
fn with_root(value: &Value, root: &str) -> Value {
    match value {
        Value::String(text) => Value::String(
            ROOT_VARIABLES
                .iter()
                .fold(text.clone(), |text, variable| text.replace(variable, root)),
        ),
        Value::Array(items) => Value::Array(items.iter().map(|v| with_root(v, root)).collect()),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, value)| (key.clone(), with_root(value, root)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// Event arrays from every hooks file, appended in order. A file is either the event
/// map or `{ "hooks": <event map> }`; anything else, including hook controls such as
/// `sandbox_all`, rejects that file.
/// The plugin root as hook commands and MCP definitions see it, without a `\\?\` prefix.
fn root_text(root: &Path) -> String {
    super::install::plain(root).display().to_string()
}

fn hooks(
    plugin: &ActivePlugin,
    servers: &BTreeSet<&str>,
    warnings: &mut Vec<String>,
) -> Option<Value> {
    let root = root_text(&plugin.root);
    let mut events = Map::new();
    for path in paths(plugin, "hooks", "hooks/hooks.json") {
        let Some(document) = read_json(&path, warnings) else {
            continue;
        };
        let document = document.get("hooks").cloned().unwrap_or(document);
        let file = match plugin_hooks(&document, &plugin.manifest.capabilities, servers) {
            Ok(file) => file,
            Err(error) => {
                warnings.push(format!("{}: {error}; ignored", path.display()));
                continue;
            }
        };
        for (event, groups) in file {
            let groups = with_root(&groups, &root);
            let slot = events.entry(event).or_insert_with(|| json!([]));
            if let (Some(slot), Some(groups)) = (slot.as_array_mut(), groups.as_array()) {
                slot.extend(groups.iter().cloned());
            }
        }
    }
    (!events.is_empty()).then_some(Value::Object(events))
}

/// A plugin's hooks file, refused whole when a handler reaches past the plugin's
/// declared capabilities (`plugins-marketplace` → Capability declaration and
/// enforcement). `servers` are the MCP servers this plugin contributes, as named in the
/// resolved configuration.
fn plugin_hooks(
    document: &Value,
    capabilities: &super::Capabilities,
    servers: &BTreeSet<&str>,
) -> Result<Map<String, Value>, String> {
    let events = document.as_object().ok_or("hooks: expected an object")?;
    if let Some(key) = events.keys().find(|key| !crate::config::is_event_name(key)) {
        return Err(format!(
            "hooks.{key}: plugin hooks support only event arrays"
        ));
    }
    let settings = crate::config::HookSettings::from_config(&json!({ "hooks": document }))?;
    for (event, groups) in &settings.events {
        for (index, group) in groups.iter().enumerate() {
            for handler in &group.hooks {
                confined(handler, capabilities, servers)
                    .map_err(|error| format!("hooks.{event}[{index}]: {error}"))?;
            }
        }
    }
    Ok(events.clone())
}

/// Whether one handler stays within the plugin's capabilities: an `http` hook needs
/// `network` covering its host, a `prompt` hook `session.prompt`, and an `mcp_tool` hook
/// may call only a server this plugin contributes. Command hooks run sandboxed with the
/// plugin's network at launch.
fn confined(
    handler: &crate::config::HookHandler,
    capabilities: &super::Capabilities,
    servers: &BTreeSet<&str>,
) -> Result<(), String> {
    use crate::config::HookKind;
    match handler.kind {
        HookKind::Http => {
            let url = handler.url.as_deref().unwrap_or_default();
            let host = url::Url::parse(url)
                .ok()
                .and_then(|url| url.host_str().map(str::to_owned))
                .ok_or("an http hook needs a literal URL")?;
            let host = host.trim_start_matches('[').trim_end_matches(']');
            if !capabilities.network.allows_host(host) {
                return Err(format!(
                    "an http hook to {host} needs the network capability for that host"
                ));
            }
        }
        HookKind::Prompt if !capabilities.session_prompt => {
            return Err("a prompt hook needs the session.prompt capability".into());
        }
        HookKind::McpTool => {
            let server = handler.server.as_deref().unwrap_or_default();
            if !servers.contains(server) {
                return Err(format!(
                    "an mcp_tool hook may call only this plugin's MCP servers, not \"{server}\""
                ));
            }
        }
        _ => {}
    }
    Ok(())
}

fn mcp(
    plugin: &ActivePlugin,
    taken: &mut BTreeSet<String>,
    warnings: &mut Vec<String>,
) -> Option<Value> {
    let root = root_text(&plugin.root);
    let mut servers = Map::new();
    for path in paths(plugin, "mcp", ".mcp.json") {
        let Some(document) = read_json(&path, warnings) else {
            continue;
        };
        let (converted, notes) =
            crate::config::convert_mcp_file(&with_root(&document, &root), &path);
        warnings.extend(notes);
        for (server, definition) in converted["mcp"].as_object().into_iter().flatten() {
            let Some(name) = unique(server, &plugin.name, '-', taken) else {
                warnings.push(format!(
                    "{}: MCP server \"{server}\" skipped: its name is already used",
                    path.display()
                ));
                continue;
            };
            servers.insert(name, definition.clone());
        }
    }
    (!servers.is_empty()).then_some(Value::Object(servers))
}

/// `name`, or `<plugin><separator><name>` when `name` is taken. `None` when both are.
fn unique(
    name: &str,
    plugin: &str,
    separator: char,
    taken: &mut BTreeSet<String>,
) -> Option<String> {
    let candidates = [name.to_owned(), format!("{plugin}{separator}{name}")];
    let chosen = candidates
        .into_iter()
        .find(|candidate| !taken.contains(candidate))?;
    taken.insert(chosen.clone());
    Some(chosen)
}

fn agent_profiles(
    plugin: &ActivePlugin,
    taken: &mut BTreeSet<String>,
    warnings: &mut Vec<String>,
) -> Option<Value> {
    let mut agents = Map::new();
    for path in agent_files(plugin, warnings) {
        let Ok(text) = std::fs::read_to_string(&path) else {
            warnings.push(format!("{}: cannot be read; ignored", path.display()));
            continue;
        };
        let (agent, profile, notes) = match crate::import::plugin_agent(&path, &text) {
            Ok(converted) => converted,
            Err(error) => {
                warnings.push(format!("{}: {error}; ignored", path.display()));
                continue;
            }
        };
        warnings.extend(
            notes
                .into_iter()
                .map(|note| format!("{}: {note}", path.display())),
        );
        let Some(name) = unique(&agent, &plugin.name, ':', taken) else {
            warnings.push(format!(
                "{}: agent \"{agent}\" skipped: its name is already used",
                path.display()
            ));
            continue;
        };
        agents.insert(name, profile);
    }
    if agents.is_empty() {
        return None;
    }
    // A plugin agent cannot widen its Mode, permissions or sandbox.
    let safe = crate::config::split_sensitive(&json!({ "agents": agents })).safe;
    safe.get("agents").cloned()
}

fn agent_files(plugin: &ActivePlugin, warnings: &mut Vec<String>) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for path in paths(plugin, "agents", "agents") {
        if path.is_file() {
            files.push(path);
            continue;
        }
        if !path.is_dir() {
            continue;
        }
        match super::digest::files(&path) {
            Ok(found) => files.extend(
                found
                    .into_iter()
                    .map(|(_, path)| path)
                    .filter(|path| path.extension().is_some_and(|e| e == "md")),
            ),
            Err(error) => warnings.push(error),
        }
    }
    files
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::{Manifest, PluginScope};

    fn plugin(root: &Path, manifest: &str) -> ActivePlugin {
        ActivePlugin {
            name: "review-kit".into(),
            scope: PluginScope::User,
            root: root.to_path_buf(),
            manifest: Manifest::parse(manifest).unwrap(),
            digest: String::new(),
        }
    }

    /// Regression: plugin hooks accepted every handler kind, so a plugin that declared no
    /// capability could post tool events over `http`, spend model credentials through a
    /// `prompt` hook, or call the user's own MCP servers with `mcp_tool`.
    #[test]
    fn hooks_beyond_the_declared_capabilities_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("hooks")).unwrap();
        std::fs::write(
            root.join(".mcp.json"),
            json!({"mcpServers":{"own":{"command":"own"}}}).to_string(),
        )
        .unwrap();
        let cases = [
            (
                json!({"type":"http","url":"https://collect.example/events"}),
                r#"{}"#,
                false,
            ),
            (
                json!({"type":"http","url":"https://collect.example/events"}),
                r#"{"network":{"hosts":["other.example"]}}"#,
                false,
            ),
            (
                json!({"type":"http","url":"https://api.collect.example/events"}),
                r#"{"network":{"hosts":["*.collect.example"]}}"#,
                true,
            ),
            (
                json!({"type":"http","url":"https://collect.example/"}),
                r#"{"network":true}"#,
                true,
            ),
            (json!({"type":"prompt","prompt":"Judge it"}), r#"{}"#, false),
            (
                json!({"type":"prompt","prompt":"Judge it"}),
                r#"{"session.prompt":true}"#,
                true,
            ),
            (
                json!({"type":"mcp_tool","server":"github","tool":"create_issue"}),
                r#"{}"#,
                false,
            ),
            (
                json!({"type":"mcp_tool","server":"own","tool":"check"}),
                r#"{}"#,
                true,
            ),
            (json!({"type":"command","command":"true"}), r#"{}"#, true),
        ];
        for (handler, capabilities, loaded) in cases {
            std::fs::write(
                root.join("hooks/hooks.json"),
                json!({"PostToolUse":[{"hooks":[handler]}]}).to_string(),
            )
            .unwrap();
            let manifest = format!(
                r#"{{"name":"review-kit","version":"1.0.0","capabilities":{capabilities}}}"#
            );
            let layer = plugin_layer(
                &plugin(root, &manifest),
                &mut BTreeSet::new(),
                &mut BTreeSet::new(),
            );
            assert_eq!(
                layer.value.get("hooks").is_some(),
                loaded,
                "{handler} with {capabilities}: {:?}",
                layer.warnings
            );
            assert_eq!(layer.warnings.is_empty(), loaded, "{:?}", layer.warnings);
        }
    }

    #[test]
    fn default_locations_load_and_colliding_names_are_namespaced() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for folder in ["hooks", "agents", "commands", "skills/lint"] {
            std::fs::create_dir_all(root.join(folder)).unwrap();
        }
        std::fs::write(
            root.join("hooks/hooks.json"),
            json!({"hooks":{"PostToolUse":[{"matcher":"edit","hooks":[
                {"type":"command","command":"${CYBER_PLUGIN_ROOT}/fmt.sh"}]}]}})
            .to_string(),
        )
        .unwrap();
        std::fs::write(
            root.join(".mcp.json"),
            json!({"mcpServers":{"docs":{"command":"docs"}}}).to_string(),
        )
        .unwrap();
        std::fs::write(
            root.join("agents/reviewer.md"),
            "---\ndescription: Reviews\npermissionMode: bypassPermissions\n---\nReview it.\n",
        )
        .unwrap();
        let plugin = plugin(root, r#"{"name":"review-kit","version":"1.0.0"}"#);
        let mut agents = BTreeSet::from(["reviewer".to_owned()]);
        let mut servers = BTreeSet::from(["docs".to_owned()]);
        let layer = plugin_layer(&plugin, &mut agents, &mut servers);
        let command = &layer.value["hooks"]["PostToolUse"][0]["hooks"][0]["command"];
        assert_eq!(command, &json!(format!("{}/fmt.sh", root.display())));
        assert!(
            layer.value["mcp"]["review-kit-docs"].is_object(),
            "{:#}",
            layer.value
        );
        let agent = &layer.value["agents"]["review-kit:reviewer"];
        assert_eq!(agent["system"], "Review it.\n");
        assert!(agent.get("permission_mode").is_none(), "{agent:#}");
        assert_eq!(
            layer.components["commands"][0],
            json!(root.join("commands").display().to_string())
        );
        assert_eq!(
            layer.components["skills"][0],
            json!(root.join("skills").display().to_string())
        );
        assert!(layer.label.starts_with("plugin:review-kit@1.0.0:"));
    }

    #[test]
    fn hook_controls_and_host_components_are_not_loaded() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(
            root.join("h.json"),
            json!({"sandbox_all":false}).to_string(),
        )
        .unwrap();
        let plugin = plugin(
            root,
            r#"{"name":"review-kit","version":"1.0.0","hooks":"h.json","tools":"tools"}"#,
        );
        let layer = plugin_layer(&plugin, &mut BTreeSet::new(), &mut BTreeSet::new());
        assert_eq!(layer.value, json!({}));
        let warnings = layer.warnings.join("\n");
        assert!(
            warnings.contains("plugin hooks support only event arrays"),
            "{warnings}"
        );
        assert!(
            warnings.contains("the tools path is not read"),
            "{warnings}"
        );
    }
}
