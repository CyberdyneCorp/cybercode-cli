//! The first-run import offer (`compat-import` → Source detection): the first interactive
//! run in a project holding another coding tool's configuration offers
//! `cyber import auto --dry-run`, once per project. Detection reads names and metadata
//! only, never a source file's contents.

use std::collections::BTreeSet;
use std::path::Path;

use cyber_core::import::{SourceLayer, SourceRoots, SourceTool, discover_sources};
use serde_json::{Value, json};

use crate::store::LocalStore;

/// `kv.json` key: project root → `true` once the offer was shown there.
const OFFERED: &str = "import_offered";

/// The offer for the project containing `directory`, recorded as shown; `None` when it
/// was shown before or the project holds no other tool's configuration.
pub(crate) fn offer(store: &LocalStore, directory: &Path, home: Option<&str>) -> Option<String> {
    let project = cyber_core::config::project_root(directory);
    let key = project.display().to_string();
    let mut offered = store
        .get(OFFERED)
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}));
    if offered.get(&key).is_some() {
        return None;
    }
    let roots = SourceRoots {
        project_root: project,
        directory: directory.to_path_buf(),
        home: home?.into(),
        codex_home: None,
    };
    let inventory = discover_sources(&roots).ok()?;
    let tools: BTreeSet<_> = inventory
        .files
        .iter()
        .filter(|f| f.layer == SourceLayer::Project)
        .map(|f| match f.tool {
            SourceTool::Claude => "Claude Code",
            SourceTool::Codex => "Codex",
            SourceTool::OpenCode => "OpenCode",
        })
        .collect();
    if tools.is_empty() {
        return None;
    }
    offered[key] = json!(true);
    store.set(OFFERED, offered);
    let names: Vec<_> = tools.into_iter().collect();
    Some(format!(
        "Found {} configuration in this project. Preview an import with `cyber import auto --dry-run`",
        names.join(" and ")
    ))
}

#[cfg(test)]
mod tests {
    use crate::screens::Server;

    async fn started(server: &Server, home: &std::path::Path) -> crate::app::App {
        started_with(server, home, false).await
    }

    async fn started_with(server: &Server, home: &std::path::Path, ci: bool) -> crate::app::App {
        let root = server.root();
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
        let home = home.display().to_string();
        let env = move |name: &str| match name {
            "HOME" => Some(home.clone()),
            "CI" if ci => Some("true".into()),
            _ => None,
        };
        crate::runner::start(&server.client, &store, &opts, &env)
            .await
            .unwrap()
            .0
    }

    /// Through the runner's own startup against the production server: the offer shows on
    /// the first run in a project with `.claude/`, never again there, and never in a
    /// project without other tools' files.
    #[tokio::test]
    async fn first_run_offers_import_auto_dry_run_once() {
        let server = Server::start().await;
        let root = server.root();
        let home = root.join("home");
        std::fs::create_dir_all(&home).unwrap();
        // Configuration only in the user's home is not this project's.
        std::fs::create_dir_all(home.join(".claude")).unwrap();
        std::fs::write(home.join(".claude/settings.json"), "{}").unwrap();
        let app = started(&server, &home).await;
        assert!(app.toast.is_none(), "{:?}", app.toast);

        std::fs::create_dir_all(root.join(".claude")).unwrap();
        std::fs::write(
            root.join(".claude/settings.json"),
            "{\"model\":\"private\"}",
        )
        .unwrap();
        let app = started(&server, &home).await;
        let (toast, _) = app.toast.clone().expect("the first run offers the import");
        assert_eq!(
            toast,
            "Found Claude Code configuration in this project. Preview an import with `cyber import auto --dry-run`"
        );
        let app = started(&server, &home).await;
        assert!(
            app.toast.is_none(),
            "offered once per project: {:?}",
            app.toast
        );
        assert!(!root.join("cyber.jsonc").exists());
    }

    /// `installation-upgrade` → Container and CI usage: with `CI=true` the offer is not
    /// shown and not recorded, so a later interactive run still gets it.
    #[tokio::test]
    async fn ci_suppresses_the_import_offer() {
        let server = Server::start().await;
        let root = server.root();
        let home = root.join("home");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(root.join(".claude")).unwrap();
        std::fs::write(root.join(".claude/settings.json"), "{}").unwrap();
        let app = started_with(&server, &home, true).await;
        assert!(app.toast.is_none(), "{:?}", app.toast);
        let app = started(&server, &home).await;
        assert!(app.toast.is_some(), "offered outside CI");
    }
}
