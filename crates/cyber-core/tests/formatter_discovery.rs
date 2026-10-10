//! Local formatter detection without executing binaries or configuration code.
use cyber_core::{
    config::FormatterSettings,
    intelligence::{ExecutableSearch, builtin_formatters, detect_formatters},
};
use serde_json::json;
use std::{collections::HashMap, path::Path};
fn executable(path: &Path) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, "Discovery must not run this file").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
}
fn search(root: &Path) -> ExecutableSearch {
    ExecutableSearch::new(root, &root.join("cache"), &HashMap::<String, String>::new())
}
fn row(root: &Path, id: &str) -> cyber_core::intelligence::DetectedFormatter {
    detect_formatters(&FormatterSettings::default(), &search(root))
        .into_iter()
        .find(|f| f.definition.id == id)
        .unwrap()
}

#[test]
fn all_required_formatters_have_extensions_and_file_arguments() {
    let definitions = builtin_formatters();
    let mut ids: Vec<_> = definitions.iter().map(|f| f.id.as_str()).collect();
    ids.sort();
    assert_eq!(
        ids,
        [
            "biome",
            "black",
            "clang-format",
            "forge",
            "gofmt",
            "prettier",
            "ruff",
            "rustfmt",
            "shfmt",
            "stylua",
            "verible-verilog-format",
            "zig"
        ]
    );
    for definition in definitions {
        assert!(!definition.extensions.is_empty());
        assert!(definition.command.iter().any(|s| s == "$FILE"));
    }
}
#[test]
fn prettier_requires_config_or_dependency_and_does_not_evaluate_javascript() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    executable(&root.join("cache/bin/prettier"));
    assert!(!row(root, "prettier").enabled);
    std::fs::write(
        root.join("package.json"),
        r#"{"devDependencies":{"prettier":"^3"}}"#,
    )
    .unwrap();
    assert_eq!(
        row(root, "prettier").detected_by,
        "package.json:devDependencies.prettier"
    );
    std::fs::write(root.join("package.json"), r#"{"prettier":{"semi":false}}"#).unwrap();
    assert!(row(root, "prettier").enabled);
    std::fs::remove_file(root.join("package.json")).unwrap();
    std::fs::write(
        root.join("prettier.config.mjs"),
        "throw new Error('must not execute');",
    )
    .unwrap();
    let detected = row(root, "prettier");
    assert!(detected.enabled);
    assert_eq!(detected.detected_by, "prettier.config.mjs");
    assert_eq!(
        std::fs::read_to_string(root.join("cache/bin/prettier")).unwrap(),
        "Discovery must not run this file"
    );
}
#[test]
fn malformed_oversized_and_aliased_package_manifests_do_not_enable_prettier() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    executable(&root.join("cache/bin/prettier"));
    let path = root.join("package.json");
    for content in [
        "not json".to_owned(),
        " ".repeat(1_048_577),
        r#"{"devDependencies":{"prettier":true}}"#.into(),
    ] {
        std::fs::write(&path, content).unwrap();
        assert!(!row(root, "prettier").enabled);
    }
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    assert!(!row(root, "prettier").enabled);
    std::fs::remove_dir(&path).unwrap();
    #[cfg(unix)]
    {
        let outside = root.join("outside.json");
        std::fs::write(&outside, r#"{"prettier":{}}"#).unwrap();
        std::os::unix::fs::symlink(&outside, &path).unwrap();
        assert!(!row(root, "prettier").enabled);
        std::fs::remove_file(&path).unwrap();
        assert!(
            std::process::Command::new("mkfifo")
                .arg(&path)
                .status()
                .unwrap()
                .success()
        );
        assert!(!row(root, "prettier").enabled);
    }
}
#[test]
fn builtin_markers_and_explicit_command_overrides_have_distinct_admission() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    executable(&root.join("cache/bin/biome"));
    assert!(!row(root, "biome").enabled);
    std::fs::write(root.join("biome.jsonc"), "{}").unwrap();
    assert!(row(root, "biome").enabled);
    let config = json!({"formatters":{"prettier":{"command":["missing-custom","$FILE"],"extensions":[".custom"],"env":{"STYLE":"strict"}},"taplo":{"command":["taplo","fmt","$FILE"],"extensions":[".toml"],"disabled":true}}});
    let settings = FormatterSettings::from_config(&config).unwrap();
    let rows = detect_formatters(&settings, &search(root));
    let forced = rows.iter().find(|f| f.definition.id == "prettier").unwrap();
    assert!(forced.enabled);
    assert!(!forced.installed);
    assert_eq!(forced.detected_by, "config");
    assert_eq!(forced.definition.extensions, [".custom"]);
    assert_eq!(forced.definition.env["STYLE"], "strict");
    assert!(
        !rows
            .iter()
            .find(|f| f.definition.id == "taplo")
            .unwrap()
            .enabled
    );
    assert!(
        detect_formatters(
            &FormatterSettings::from_config(&json!({"formatters":false})).unwrap(),
            &search(root)
        )
        .iter()
        .all(|f| !f.enabled)
    );
}
