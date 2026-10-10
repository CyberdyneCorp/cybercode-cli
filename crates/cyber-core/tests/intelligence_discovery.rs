//! Real local discovery and confined roots; no executable candidate is launched.
use cyber_core::{
    config::LspSettings,
    intelligence::{ExecutableSearch, builtin_servers, detect_servers, server_root},
};
use serde_json::json;
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

fn executable(path: &Path) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, "Not launched by discovery").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
}
fn search(root: &Path) -> ExecutableSearch {
    ExecutableSearch::new(root, &root.join("cache"), &HashMap::<String, String>::new())
}

#[test]
fn all_canonical_servers_have_launch_root_extension_and_install_definitions() {
    let definitions = builtin_servers();
    let mut ids: Vec<_> = definitions.iter().map(|d| d.id.as_str()).collect();
    ids.sort();
    assert_eq!(
        ids,
        [
            "bash-language-server",
            "clangd",
            "gopls",
            "jdtls",
            "lua-language-server",
            "pyright",
            "rust-analyzer",
            "solidity",
            "svelte",
            "typescript",
            "verible",
            "vue",
            "yaml-language-server",
            "zls"
        ]
    );
    for definition in definitions {
        assert!(!definition.command.is_empty());
        assert!(!definition.extensions.is_empty());
        assert!(!definition.root_markers.is_empty());
        assert!(
            LspSettings::from_config(&json!({"lsp":{definition.id:{"disabled":true}}})).is_ok()
        );
    }
}

#[test]
fn search_prefers_path_then_cache_and_never_runs_or_creates_candidates() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let first = root.join("path one");
    let second = root.join("path two");
    let cache = root.join("cache/bin");
    for directory in [&first, &second, &cache] {
        executable(&directory.join("server"));
    }
    let mut search = search(root);
    search.directories = vec![first.clone(), second.clone()];
    assert_eq!(
        search.find("server"),
        Some(first.join("server").canonicalize().unwrap())
    );
    std::fs::remove_file(first.join("server")).unwrap();
    assert_eq!(
        search.find("server"),
        Some(second.join("server").canonicalize().unwrap())
    );
    std::fs::remove_file(second.join("server")).unwrap();
    assert_eq!(
        search.find("server"),
        Some(cache.join("server").canonicalize().unwrap())
    );
    assert!(search.find("missing-server").is_none());
    assert!(search.find("").is_none());
    assert_eq!(
        std::fs::read_to_string(cache.join("server")).unwrap(),
        "Not launched by discovery"
    );
    assert!(!cache.join("missing-server").exists());
    assert!(search.find(cache.to_str().unwrap()).is_none());
}

#[test]
fn relative_explicit_commands_and_platform_suffixes_resolve_to_absolute_paths() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    executable(&root.join("tools/server"));
    let search = search(root);
    assert_eq!(
        search.find("tools/server"),
        Some(root.join("tools/server").canonicalize().unwrap())
    );
    #[cfg(windows)]
    {
        executable(&root.join("cache/bin/windows-server.EXE"));
        assert_eq!(
            search.find("windows-server"),
            Some(
                root.join("cache/bin/windows-server.EXE")
                    .canonicalize()
                    .unwrap()
            )
        );
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            root.join("tools/server"),
            std::fs::Permissions::from_mode(0o600),
        )
        .unwrap();
        assert!(search.find("tools/server").is_none());
    }
}

#[test]
fn trusted_overrides_replace_launch_metadata_and_disabled_servers_stay_inactive() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    executable(&root.join("cache/bin/rust-analyzer"));
    executable(&root.join("tools/custom"));
    let settings = LspSettings::from_config(&json!({"lsp":{
        "rust-analyzer":{"disabled":true},
        "pyright":{"command":["tools/custom","arg"],"extensions":[".custom"],"root_markers":["custom.project"],"env":{"CUSTOM":"yes"},"initialization_options":{"mode":"strict"}},
        "nimlsp":{"extensions":[".nim"],"command":["missing-server"]}
    }})).unwrap();
    let discovered = detect_servers(&settings, &search(root));
    let rust = discovered
        .iter()
        .find(|s| s.definition.id == "rust-analyzer")
        .unwrap();
    assert!(rust.installed);
    assert!(!rust.enabled);
    let custom = discovered
        .iter()
        .find(|s| s.definition.id == "pyright")
        .unwrap();
    assert!(custom.enabled && custom.installed);
    assert_eq!(custom.definition.command, ["tools/custom", "arg"]);
    assert_eq!(custom.definition.extensions, [".custom"]);
    assert_eq!(custom.definition.root_markers, ["custom.project"]);
    assert_eq!(custom.definition.env["CUSTOM"], "yes");
    assert_eq!(
        custom.definition.initialization_options.as_ref().unwrap()["mode"],
        "strict"
    );
    let missing = discovered
        .iter()
        .find(|s| s.definition.id == "nimlsp")
        .unwrap();
    assert!(!missing.enabled && !missing.installed);
    let disabled = detect_servers(
        &LspSettings::from_config(&json!({"lsp":false})).unwrap(),
        &search(root),
    );
    assert!(disabled.iter().all(|s| !s.enabled));
    assert!(disabled.iter().any(|s| s.installed));
}

#[test]
fn nearest_root_is_confined_to_location_with_safe_fallback() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("location");
    let nested = root.join("nested");
    let file = nested.join("src/main.rs");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, "fn main() {}").unwrap();
    std::fs::write(temp.path().join("Cargo.toml"), "outside").unwrap();
    assert_eq!(
        server_root(&root, &file, &["Cargo.toml".into()]).unwrap(),
        root.canonicalize().unwrap()
    );
    std::fs::write(root.join("Cargo.toml"), "outer").unwrap();
    std::fs::write(nested.join("Cargo.toml"), "nearest").unwrap();
    assert_eq!(
        server_root(
            &root,
            Path::new("nested/src/main.rs"),
            &["Cargo.toml".into()]
        )
        .unwrap(),
        nested.canonicalize().unwrap()
    );
    assert!(server_root(&root, &temp.path().join("Cargo.toml"), &[]).is_err());
    assert!(server_root(&root, &file, &["../Cargo.toml".into()]).is_err());
    assert!(server_root(&root, &file, &[String::new()]).is_err());
    assert!(server_root(&file, &file, &["Cargo.toml".into()]).is_err());
    assert!(server_root(&root, &file, &[PathBuf::from("/").display().to_string()]).is_err());
    #[cfg(unix)]
    {
        let alias = root.join("outside.rs");
        std::os::unix::fs::symlink(temp.path().join("Cargo.toml"), &alias).unwrap();
        assert!(server_root(&root, &alias, &[]).is_err());
    }
}
