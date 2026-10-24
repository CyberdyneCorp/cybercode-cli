//! The output style through the real tool host and runtime (`system-context` → Output
//! style selection): the host resolves the Location's configuration for each Turn.

mod support;

use std::collections::HashMap;
use std::sync::Arc;

use cyber_core::config::{self, LoadRequest};
use cyber_core::paths::Paths;
use cyber_core::trust::TrustStore;
use cyber_server::runtime::{Admission, Delivery};
use cyber_server::runtime::{ContextObservation, CreateSession, ToolHost};
use support::flow::{Flow, text};

/// Resolve every model through the real tool host's configuration loader.
fn attach_config(flow: &Flow) {
    let root = flow.f.dir.path().to_path_buf();
    let env = HashMap::from([(
        "CYBER_HOME".into(),
        root.join("cyber").display().to_string(),
    )]);
    let paths = Paths::resolve(&env, &root);
    paths.ensure().unwrap();
    let trust = TrustStore::new(paths.trust_file());
    flow.f
        .host
        .attach_hook_config(
            Arc::new(move |location| {
                config::load(&LoadRequest {
                    location,
                    paths: &paths,
                    env: &env,
                    home: &root,
                    profile: None,
                    overrides: &[],
                    flags: serde_json::json!({}),
                })
                .map_err(|error| error.to_string())
            }),
            trust,
        )
        .unwrap();
}

async fn styled_session(flow: &Flow, style: &str) -> String {
    let request = CreateSession {
        directory: flow.f.repo.display().to_string(),
        model: "test/main".into(),
        mode: Some("default".into()),
        output_style: Some(style.into()),
        ..Default::default()
    };
    flow.runtime.create_session(request).await.unwrap().id
}

/// Regression: a configuration that failed to resolve was observed as "no style", so a
/// Turn reconciled while `cyber.jsonc` was broken would start an epoch without the style
/// and the fix another. It is unavailable now, which keeps the epoch and its prefix.
/// (Prompt admission also refuses while the configuration is broken.)
#[tokio::test]
async fn an_unresolvable_configuration_keeps_the_epochs_style() {
    let flow = Flow::new(vec![text("one"), text("three")], false);
    attach_config(&flow);
    let id = styled_session(&flow, "concise").await;
    let id_dir = flow.f.repo.display().to_string();
    let epoch = async || flow.runtime.state(&id).await.unwrap().epoch.unwrap();

    flow.prompt(&id, "one").await;
    flow.settle(&id).await;
    let first = epoch().await;
    assert!(first.snapshot["core/output-style"].starts_with("Output style: concise"));

    let document = flow.f.repo.join("cyber.jsonc");
    std::fs::write(&document, "{ \"output_style\": ").unwrap();
    let observed = flow.f.host.output_style(&id_dir, Some("concise"));
    assert!(
        matches!(&observed, ContextObservation::Unavailable(reason) if reason.contains("cyber.jsonc")),
        "{observed:?}"
    );
    assert!(
        flow.runtime
            .admit(&id, Admission::text("two", Delivery::Queue))
            .await
            .is_err()
    );
    assert_eq!(epoch().await.number, first.number);

    std::fs::remove_file(&document).unwrap();
    flow.prompt(&id, "three").await;
    flow.settle(&id).await;
    let last = epoch().await;
    assert_eq!(last.number, first.number);
    assert_eq!(last.snapshot, first.snapshot);
    let requests = flow.main.requests();
    assert_eq!(requests.len(), 2);
    assert!(requests.iter().all(|r| r.system == requests[0].system));
    assert!(
        requests[0]
            .system
            .concat()
            .contains("Output style: concise")
    );
}

/// Regression: a style switch restarted the epoch through a fresh snapshot, which failed
/// with `context_initialization_blocked` while another source was unavailable. The new
/// epoch keeps that source's previous value and takes the new style.
#[tokio::test]
async fn a_style_switch_while_another_source_is_unavailable_takes_effect() {
    let flow = Flow::new(vec![text("one"), text("two")], false);
    attach_config(&flow);
    std::fs::write(flow.f.repo.join("AGENTS.md"), "Use pnpm.").unwrap();
    let id = styled_session(&flow, "concise").await;
    flow.prompt(&id, "one").await;
    flow.settle(&id).await;

    let agents = flow.f.repo.join("AGENTS.md");
    std::fs::remove_file(&agents).unwrap();
    std::fs::create_dir(&agents).unwrap(); // reading a directory fails with an I/O error
    flow.runtime
        .switch_output_style(&id, Some("learning"))
        .await
        .unwrap();
    flow.prompt(&id, "two").await;
    flow.settle(&id).await;

    let epoch = flow.runtime.state(&id).await.unwrap().epoch.unwrap();
    assert_eq!(epoch.number, 2);
    assert!(epoch.snapshot["core/output-style"].starts_with("Output style: learning"));
    assert!(epoch.snapshot["core/instructions"].contains("Use pnpm."));
    let requests = flow.main.requests();
    assert_eq!(requests.len(), 2, "the second Turn ran");
    let system = requests[1].system.concat();
    assert!(system.contains("Output style: learning") && system.contains("Use pnpm."));
}

/// Anthropic family for `test/claude`; everything else as the inner resolver has it.
#[cfg(unix)]
struct Families(Arc<dyn cyber_server::runtime::ModelResolver>);

#[cfg(unix)]
impl cyber_server::runtime::ModelResolver for Families {
    fn resolve(&self, model_ref: &str) -> Result<cyber_server::runtime::ResolvedModel, String> {
        let mut resolved = self.0.resolve(model_ref)?;
        if model_ref == "test/claude" {
            resolved.family = cyber_core::prompts::ModelFamily::Anthropic;
        }
        Ok(resolved)
    }

    fn role(&self, role: cyber_llm::catalog::ModelRole) -> Option<String> {
        self.0.role(role)
    }
}

/// Regression: an epoch restarted for a model-family switch failed while the selected
/// style's file was unreadable. It keeps the previous style text instead.
#[cfg(unix)]
#[tokio::test]
async fn an_unreadable_style_file_survives_a_family_switch() {
    use std::os::unix::fs::PermissionsExt;
    let flow = Flow::with_resolver_factory(
        support::Fixture::new(),
        vec![text("one")],
        false,
        Arc::new(cyber_server::runtime::NoSnapshots),
        vec![("test/claude", vec![text("two")])],
        |models| Arc::new(Families(models)),
    );
    attach_config(&flow);
    let styles = flow.f.dir.path().join("home/.config/cyber/output-styles");
    std::fs::create_dir_all(&styles).unwrap();
    let file = styles.join("terse.md");
    std::fs::write(&file, "Answer in one line.").unwrap();
    let id = styled_session(&flow, "terse").await;
    flow.prompt(&id, "one").await;
    flow.settle(&id).await;
    let first = flow.runtime.state(&id).await.unwrap().epoch.unwrap();
    assert!(first.snapshot["core/output-style"].contains("Answer in one line."));

    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o000)).unwrap();
    if std::fs::read(&file).is_ok() {
        return; // Running as root: permissions do not apply.
    }
    let observed = flow
        .f
        .host
        .output_style(&flow.f.repo.display().to_string(), Some("terse"));
    assert!(
        matches!(observed, ContextObservation::Unavailable(_)),
        "{observed:?}"
    );
    flow.runtime.switch_model(&id, "test/claude").await.unwrap();
    flow.prompt(&id, "two").await;
    flow.settle(&id).await;

    let epoch = flow.runtime.state(&id).await.unwrap().epoch.unwrap();
    assert_eq!(epoch.number, first.number + 1);
    assert_eq!(epoch.family.as_deref(), Some("anthropic"));
    assert_eq!(
        epoch.snapshot["core/output-style"],
        first.snapshot["core/output-style"]
    );
    let requests = flow.requests("test/claude");
    assert_eq!(requests.len(), 1, "the Turn after the switch ran");
    assert!(requests[0].system.concat().contains("Answer in one line."));
}
