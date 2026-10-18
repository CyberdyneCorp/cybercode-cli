//! What the built-in host tells the runtime about instruction files when the Location's
//! config cannot be loaded (`system-context` → Instruction file discovery).

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use cyber_core::paths::DatabaseLocation;
use cyber_server::runtime::{Runtime, ToolHost};
use cyber_store::{Store, StoreOptions};
use cyber_tools::{BuiltinHost, HostOptions};
use serde_json::{Value, json};

fn host(root: &Path, config: Result<Value, String>) -> Arc<BuiltinHost> {
    let store = Store::open(StoreOptions::new(
        DatabaseLocation::Memory,
        Runtime::registry(),
    ))
    .unwrap();
    BuiltinHost::new(HostOptions {
        store: Arc::new(store),
        tool_output_dir: root.join("tool-output"),
        allowed_dirs: Vec::new(),
        home: root.join("home"),
        shell: "bash".into(),
        config: Arc::new(move |_| config.clone().map(|value| (value, BTreeMap::new()))),
        global_config_dir: root.join("home/.config/cyber"),
        env: Arc::new(std::collections::HashMap::<String, String>::new()),
        models: None,
        temp_dir: root.join("tmp"),
        sandbox_policy: None,
        sandbox_helper: None,
        isolation: Default::default(),
    })
}

/// Regression: a config load error read as `Null`, whose `compat.claude` defaults to
/// `true`, so a malformed project config turned `CLAUDE.md` on over a global opt-out.
#[test]
fn a_config_that_fails_to_load_turns_claude_md_off_and_refuses_project_files() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let directory = root.display().to_string();
    let file = root.join("CYBER.md");
    let broken = host(&root, Err("ConfigParseError: cyber.json:1:3".into()));
    assert_eq!(broken.claude_compat(&directory), Some(false));
    let gate = broken.instruction_read_gate(&directory).unwrap();
    assert!(!gate.allows(&file, &file));
    let valid = host(&root, Ok(json!({})));
    assert_eq!(valid.claude_compat(&directory), Some(true));
    let gate = valid.instruction_read_gate(&directory).unwrap();
    assert!(gate.allows(&file, &file));
    let env = root.join(".env");
    assert!(!gate.allows(&env, &env));
}
