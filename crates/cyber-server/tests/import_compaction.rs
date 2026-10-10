use cyber_core::import::opencode_settings_config;
use cyber_server::runtime::CompactionConfig;
use serde_json::json;
use std::collections::HashMap;

#[test]
fn imported_compaction_settings_reach_runtime_and_keep_environment_override() {
    let source = json!({"compaction":{"auto":true,"buffer":4096,"preserve_recent_tokens":12000}});
    let imported = opencode_settings_config(&source).unwrap();
    for (env, expected) in [
        (HashMap::<String, String>::new(), true),
        (
            HashMap::from([("CYBER_DISABLE_AUTOCOMPACT".into(), "1".into())]),
            false,
        ),
    ] {
        let settings = CompactionConfig::from_config(&imported.config, &env);
        assert_eq!(
            (settings.auto, settings.buffer, settings.keep_tokens),
            (expected, 4096, 12000)
        );
        assert_eq!(settings.keep_turns, None);
    }
    let imported =
        opencode_settings_config(&json!({"compaction":{"auto":false,"keep":{"tokens":0}}}))
            .unwrap();
    let settings =
        CompactionConfig::from_config(&imported.config, &HashMap::<String, String>::new());
    assert!(!settings.auto);
    assert_eq!(settings.keep_tokens, 0);
}
