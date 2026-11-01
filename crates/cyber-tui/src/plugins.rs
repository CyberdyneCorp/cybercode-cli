//! `/plugins` and `/reload-plugins` (`plugins-marketplace` → Plugin CLI, Pure mode): the
//! Location's plugins from `GET /plugins` with scope, state and trust. Hooks, skills,
//! commands and agents are read at each use, so a reload refreshes this list and the
//! command catalog; MCP servers a plugin adds start with the next Location.

use serde_json::Value;

use crate::app::{App, PickerKind};
use crate::model::Choice;

/// One row per listed plugin and the number that loaded.
pub(crate) fn choices(list: &Value) -> (Vec<Choice>, usize) {
    let text =
        |entry: &Value, key: &str| crate::workflows::printable(entry[key].as_str().unwrap_or(""));
    let mut active = 0;
    let items = list["plugins"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|entry| {
            let state = text(entry, "state").replace('_', " ");
            active += usize::from(entry["state"] == "active");
            let version = entry["version"].as_str().map(crate::workflows::printable);
            let mut detail = vec![text(entry, "scope"), state, "unsigned".into()];
            if let Some(reason) = entry["reason"].as_str() {
                detail.push(crate::workflows::printable(reason));
            }
            if let Some(host) = entry["host"].as_object() {
                let state = host.get("state").and_then(Value::as_str).unwrap_or("");
                detail.push(format!("host {}", state.replace('_', " ")));
                if let Some(error) = host.get("error").and_then(Value::as_str) {
                    detail.push(crate::workflows::printable(error));
                }
            }
            Choice {
                key: String::new(),
                label: match version {
                    Some(version) => format!("{}@{version}", text(entry, "name")),
                    None => text(entry, "name"),
                },
                detail: detail.join(" · "),
            }
        })
        .collect();
    (items, active)
}

/// `(plugin, themes folder)` of every loaded plugin, from its manifest's `themes` paths or
/// `themes/` (`plugins-marketplace` → Component discovery defaults).
pub(crate) fn theme_dirs(list: &Value) -> Vec<(String, std::path::PathBuf)> {
    list["plugins"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|entry| entry["state"] == "active")
        .filter_map(|entry| {
            let name = entry["name"].as_str()?.to_owned();
            let root = std::path::PathBuf::from(entry["root"].as_str()?);
            Some(
                cyber_core::plugins::theme_dirs(&root)
                    .into_iter()
                    .map(move |dir| (name.clone(), dir)),
            )
        })
        .flatten()
        .collect()
}

/// `kv.json` key: project root → plugin names already offered for review there.
const OFFERED: &str = "plugins_offered";

/// The once-per-plugin startup prompt (`plugins-marketplace` → Install scopes): a plugin a
/// project or local scope lists that is not installed or not trusted here.
pub(crate) fn offer(
    store: &crate::store::LocalStore,
    directory: &str,
    list: &Value,
) -> Option<String> {
    let project = cyber_core::config::project_root(std::path::Path::new(directory));
    let key = project.display().to_string();
    let mut offered = store
        .get(OFFERED)
        .filter(Value::is_object)
        .unwrap_or_else(|| serde_json::json!({}));
    let seen: Vec<String> = offered[&key]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|name| name.as_str().map(str::to_owned))
        .collect();
    let pending: Vec<String> = list["plugins"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|entry| matches!(entry["scope"].as_str(), Some("project" | "local")))
        .filter(|entry| matches!(entry["state"].as_str(), Some("untrusted" | "not_installed")))
        .filter_map(|entry| entry["name"].as_str().map(crate::workflows::printable))
        .filter(|name| !seen.contains(name))
        .collect();
    let first = pending.first()?.clone();
    offered[&key] = serde_json::json!(
        seen.into_iter()
            .chain(pending.iter().cloned())
            .collect::<Vec<_>>()
    );
    store.set(OFFERED, offered);
    Some(match pending.len() {
        1 => format!(
            "This project uses plugin {first}: review and trust it with `cyber plugins enable {first}`"
        ),
        n => format!(
            "This project uses {n} plugins that need review: start with `cyber plugins enable {first}`"
        ),
    })
}

/// Open the list, or after a reload say what loaded.
pub(crate) fn show(
    app: &mut App,
    directory: &str,
    reload: bool,
    result: Result<(Vec<Choice>, usize, bool), String>,
) {
    if directory != app.session.directory {
        return;
    }
    let (items, active, pure) = match result {
        Ok(loaded) => loaded,
        Err(error) => return app.toast(format!("plugins: {error}")),
    };
    if reload {
        let note = if pure {
            "Plugins reloaded: external extensions disabled (--pure)".to_owned()
        } else {
            format!(
                "Plugins reloaded: {active} of {} active; new MCP servers start with the next Location",
                items.len()
            )
        };
        return app.toast(note);
    }
    if items.is_empty() {
        return app.toast(if pure {
            "No plugins: external extensions disabled (--pure)"
        } else {
            "No plugins installed · cyber plugins install <git URL or path>"
        });
    }
    let title = if pure {
        "Plugins · disabled by --pure".to_owned()
    } else {
        format!("Plugins · {active} active · scope · state · trust · cyber plugins enable <name>")
    };
    app.open_picker(PickerKind::Plugins, &title, items);
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// `/plugins` against a real App over HTTP lists an installed plugin, and
    /// `/reload-plugins` refreshes the list and the command catalog.
    #[tokio::test]
    async fn plugins_command_lists_installed_plugins_from_the_real_api() {
        use cyber_core::paths::{DatabaseLocation, Paths};
        use cyber_core::plugins::{Locations, Manager, PluginScope, parse_spec};
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().canonicalize().unwrap();
        let paths = Paths {
            data: directory.join("data"),
            config: directory.join("config"),
            state: directory.join("state"),
            cache: directory.join("cache"),
            tmp: directory.join("tmp"),
        };
        paths.ensure().unwrap();
        let kit = directory.join("kit");
        std::fs::create_dir_all(kit.join("commands")).unwrap();
        std::fs::write(
            kit.join("cyber-plugin.json"),
            json!({"name":"review-kit","version":"1.0.0"}).to_string(),
        )
        .unwrap();
        std::fs::write(kit.join("commands/review.md"), "Review with the kit.\n").unwrap();
        let manager = Manager::new(Locations::new(&paths, &directory), "0.1.0");
        let source = parse_spec(kit.to_str().unwrap(), None, None, &directory).unwrap();
        let pending = manager
            .prepare_install(source, PluginScope::User, false)
            .unwrap();
        manager.commit(pending).unwrap();

        let host = cyber_app::App::build(cyber_app::AppOptions {
            paths: paths.clone(),
            home: directory.join("home"),
            database: DatabaseLocation::Memory,
            default_directory: directory.clone(),
            sandbox_policy: None,
            snapshots: false,
            interactive: true,
            password: Some("tui-plugins-password".into()),
            mcp_credentials: None,
            remote_skill_fetcher: None,
            workflow_cores: None,
            isolation: None,
        })
        .await
        .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let client = cyber_client::Client::http(
            &format!("http://{}", listener.local_addr().unwrap()),
            Some("tui-plugins-password".into()),
        )
        .at(&directory.display().to_string());
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let server = tokio::spawn(cyber_server::http::serve_tcp(
            cyber_server::http::router(host.state.clone()),
            listener,
            async {
                let _ = stopped.await;
            },
        ));
        let mut session = crate::tests::session(false);
        session.directory = directory.display().to_string();
        let mut app = App::new(session.clone(), Vec::new(), "cyber");
        let typed = |app: &mut App, text: &str| -> Vec<crate::app::Action> {
            use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
            text.chars()
                .chain(['\n'])
                .flat_map(|c| {
                    let code = if c == '\n' {
                        KeyCode::Enter
                    } else {
                        KeyCode::Char(c)
                    };
                    app.on_key(KeyEvent::new(code, KeyModifiers::NONE))
                })
                .collect()
        };
        let actions = typed(&mut app, "/plugins");
        assert_eq!(actions, [crate::app::Action::LoadPlugins { reload: false }]);
        let message = crate::perform::perform(&client, &session, actions[0].clone()).await;
        crate::runner::apply(&mut app, message, &mut Default::default());
        let crate::app::Overlay::Picker(picker) = &app.overlay else {
            panic!("expected the plugin picker, got {:?}", app.overlay);
        };
        assert_eq!(picker.kind, PickerKind::Plugins);
        assert!(picker.title.contains("1 active"), "{}", picker.title);
        assert_eq!(picker.items[0].label, "review-kit@1.0.0");
        assert_eq!(picker.items[0].detail, "user · active · unsigned");

        app.overlay = crate::app::Overlay::None;
        let actions = typed(&mut app, "/reload-plugins");
        assert_eq!(
            actions,
            [
                crate::app::Action::LoadPlugins { reload: true },
                crate::app::Action::LoadCommands
            ]
        );
        for action in actions {
            let message = crate::perform::perform(&client, &session, action).await;
            crate::runner::apply(&mut app, message, &mut Default::default());
        }
        let toast = app.toast.as_ref().map(|(text, _)| text.clone());
        assert_eq!(
            toast.as_deref(),
            Some("Plugins reloaded: 1 of 1 active; new MCP servers start with the next Location")
        );
        assert!(
            app.commands
                .iter()
                .any(|command| command.key == "review-kit:review"),
            "{:?}",
            app.commands
        );
        let _ = stop.send(());
        let _ = server.await;
        host.runtime.shutdown().await;
    }

    /// Through the runner's startup against the production server: a project plugin that
    /// is not installed here is offered for review once, and never again.
    #[tokio::test]
    async fn project_plugin_needing_review_is_offered_once_at_startup() {
        let server = crate::screens::Server::start().await;
        let root = server.root();
        std::fs::create_dir_all(root.join(".cyber")).unwrap();
        std::fs::write(
            root.join(".cyber/plugins.json"),
            json!({"plugins":{"review-kit":{"source":{"type":"path","path":"kit"}}}}).to_string(),
        )
        .unwrap();
        let opts = crate::TuiOptions {
            client: server.client.clone(),
            directory: root.display().to_string(),
            state_dir: root.join("state"),
            config_dir: root.join("config"),
            start: crate::Start::New,
            fork: false,
            model: None,
            agent: None,
            mode: None,
            prompt: None,
            hex: None,
        };
        let store = crate::store::LocalStore::new(&opts.state_dir);
        let env = |_: &str| None;
        let (app, _) = crate::runner::start(&server.client, &store, &opts, &env)
            .await
            .unwrap();
        let (toast, _) = app.toast.clone().expect("the project plugin is offered");
        assert_eq!(
            toast,
            "This project uses plugin review-kit: review and trust it with `cyber plugins enable review-kit`"
        );
        let (app, _) = crate::runner::start(&server.client, &store, &opts, &env)
            .await
            .unwrap();
        assert!(app.toast.is_none(), "offered once: {:?}", app.toast);
    }

    /// A loaded plugin's `themes/*.json` join the catalog at startup; one whose name a
    /// built-in theme uses is `<plugin>:<name>`.
    #[tokio::test]
    async fn plugin_themes_load_at_startup_namespaced_on_collision() {
        let server = crate::screens::Server::start().await;
        let root = server.root();
        let kit = root.join("kit");
        std::fs::create_dir_all(kit.join("themes")).unwrap();
        std::fs::write(
            kit.join("cyber-plugin.json"),
            json!({"name":"theme-kit","version":"1.0.0"}).to_string(),
        )
        .unwrap();
        let theme = json!({"theme":{"text":"#ffffff","background":"#000000"}}).to_string();
        std::fs::write(kit.join("themes/corp.json"), &theme).unwrap();
        std::fs::write(kit.join("themes/cyber.json"), &theme).unwrap();
        let paths = cyber_core::paths::Paths {
            data: root.join("data"),
            config: root.join("config"),
            state: root.join("state"),
            cache: root.join("cache"),
            tmp: root.join("tmp"),
        };
        let manager = cyber_core::plugins::Manager::new(
            cyber_core::plugins::Locations::new(&paths, &root),
            "0.1.0",
        );
        let source = cyber_core::plugins::Source::Path {
            path: kit.display().to_string(),
        };
        let pending = manager
            .prepare_install(source, cyber_core::plugins::PluginScope::User, false)
            .unwrap();
        manager.commit(pending).unwrap();
        let opts = crate::TuiOptions {
            client: server.client.clone(),
            directory: root.display().to_string(),
            state_dir: root.join("tui-state"),
            config_dir: root.join("config"),
            start: crate::Start::New,
            fork: false,
            model: None,
            agent: None,
            mode: None,
            prompt: None,
            hex: None,
        };
        let store = crate::store::LocalStore::new(&opts.state_dir);
        let (app, _) = crate::runner::start(&server.client, &store, &opts, &|_| None)
            .await
            .unwrap();
        let names: Vec<&str> = app.themes.names().collect();
        assert!(names.contains(&"corp"), "{names:?}");
        assert!(names.contains(&"theme-kit:cyber"), "{names:?}");
        assert_eq!(names.iter().filter(|name| **name == "cyber").count(), 1);
    }

    /// `CYBER_PURE=1` is read by every configuration load of a process, so the App runs
    /// in a child test process that alone has it.
    #[test]
    fn pure_plugins_command_shows_disabled_by_pure() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["plugins::tests::pure_child", "--exact", "--include-ignored"])
            .env("CYBER_PURE", "1")
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success() && stdout.contains("1 passed"),
            "{stdout}\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[tokio::test]
    #[ignore = "run by pure_plugins_command_shows_disabled_by_pure with CYBER_PURE=1"]
    async fn pure_child() {
        assert_eq!(std::env::var("CYBER_PURE").as_deref(), Ok("1"));
        let server = crate::screens::Server::start().await;
        let root = server.root();
        let kit = root.join("kit");
        std::fs::create_dir_all(&kit).unwrap();
        std::fs::write(
            kit.join("cyber-plugin.json"),
            json!({"name":"review-kit","version":"1.0.0"}).to_string(),
        )
        .unwrap();
        let paths = cyber_core::paths::Paths {
            data: root.join("data"),
            config: root.join("config"),
            state: root.join("state"),
            cache: root.join("cache"),
            tmp: root.join("tmp"),
        };
        let manager = cyber_core::plugins::Manager::new(
            cyber_core::plugins::Locations::new(&paths, &root),
            "0.1.0",
        );
        let source = cyber_core::plugins::Source::Path {
            path: kit.display().to_string(),
        };
        let pending = manager
            .prepare_install(source, cyber_core::plugins::PluginScope::User, false)
            .unwrap();
        manager.commit(pending).unwrap();
        let mut session = crate::tests::session(false);
        session.directory = root.display().to_string();
        let mut app = App::new(session.clone(), Vec::new(), "cyber");
        let action = crate::app::Action::LoadPlugins { reload: false };
        let message = crate::perform::perform(&server.client, &session, action).await;
        crate::runner::apply(&mut app, message, &mut Default::default());
        let crate::app::Overlay::Picker(picker) = &app.overlay else {
            panic!("expected the plugin picker, got {:?}", app.overlay);
        };
        assert_eq!(picker.title, "Plugins · disabled by --pure");
        assert_eq!(
            picker.items[0].detail,
            "user · pure · unsigned · disabled by --pure"
        );
    }

    #[test]
    fn rows_show_scope_state_and_unsigned_marker() {
        let (items, active) = choices(&json!({"plugins":[
            {"name":"review-kit","version":"1.0.0","scope":"user","state":"active","unsigned":true},
            {"name":"guard","scope":"project","state":"not_installed",
             "reason":"not installed; run `cyber plugins enable guard`","unsigned":true}
        ],"pure":false}));
        assert_eq!(active, 1);
        assert_eq!(items[0].label, "review-kit@1.0.0");
        assert_eq!(items[0].detail, "user · active · unsigned");
        assert_eq!(items[1].label, "guard");
        let (hosted, _) = choices(&json!({"plugins":[
            {"name":"jira-kit","version":"1.0.0","scope":"user","state":"active","unsigned":true,
             "host":{"state":"disabled","error":"plugin jira-kit disabled after repeated crashes"}}
        ]}));
        assert_eq!(
            hosted[0].detail,
            "user · active · unsigned · host disabled · plugin jira-kit disabled after repeated crashes"
        );
        assert!(
            items[1]
                .detail
                .starts_with("project · not installed · unsigned · not installed; run")
        );
    }
}
