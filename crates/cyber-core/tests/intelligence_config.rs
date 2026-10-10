//! Typed LSP/formatter admission without executable side effects.
use cyber_core::config::{FormatterSettings, LspSettings};
use serde_json::json;

#[test]
fn default_and_disabled_sections_keep_installation_opt_in() {
    let lsp = LspSettings::from_config(&json!({})).unwrap();
    assert!(lsp.enabled);
    assert!(!lsp.auto_install);
    assert_eq!(lsp.diagnostics_wait_ms, 5000);
    assert!(lsp.servers.is_empty());
    assert!(FormatterSettings::from_config(&json!({})).unwrap().enabled);
    assert!(
        !LspSettings::from_config(&json!({"lsp":false}))
            .unwrap()
            .enabled
    );
    assert!(
        !FormatterSettings::from_config(&json!({"formatters":false}))
            .unwrap()
            .enabled
    );
}

#[test]
fn built_in_overrides_custom_servers_and_formatter_arguments_are_retained() {
    let config = json!({"lsp": {
        "auto_install": true, "diagnostics_wait_ms": 0,
        "rust-analyzer": {"disabled": true},
        "nimlsp": {"command":["nimlangserver", ""],"extensions":[".nim"],
            "root_markers":["project.nimble"],"env":{"LANG":"en_US.UTF-8"},
            "initialization_options":{"nested":[true, null, {"value":1}]}}
    }, "formatters": {"taplo":{"command":["taplo","fmt","$FILE"],"extensions":[".toml"],"env":{"STYLE":"compact"}}}});
    let lsp = LspSettings::from_config(&config).unwrap();
    assert!(lsp.auto_install);
    assert_eq!(lsp.diagnostics_wait_ms, 0);
    assert!(lsp.servers["rust-analyzer"].disabled);
    let custom = &lsp.servers["nimlsp"];
    assert_eq!(custom.command.as_ref().unwrap(), &["nimlangserver", ""]);
    assert_eq!(custom.root_markers.as_ref().unwrap(), &["project.nimble"]);
    assert_eq!(custom.env["LANG"], "en_US.UTF-8");
    assert_eq!(
        custom.initialization_options.as_ref().unwrap()["nested"][2]["value"],
        1
    );
    let fmt = FormatterSettings::from_config(&config).unwrap();
    assert_eq!(
        fmt.formatters["taplo"].command.as_ref().unwrap(),
        &["taplo", "fmt", "$FILE"]
    );
    assert!(!fmt.formatters["taplo"].disabled);
}

#[test]
fn custom_server_requires_extensions_but_builtin_override_can_omit_them() {
    for entry in [
        json!({}),
        json!({"command":["nimlangserver"]}),
        json!({"extensions":[]}),
        json!({"disabled":true}),
    ] {
        let error = LspSettings::from_config(&json!({"lsp":{"nimlsp":entry}})).unwrap_err();
        assert!(error.contains("custom servers require extensions"));
    }
    for id in [
        "rust-analyzer",
        "typescript",
        "pyright",
        "gopls",
        "clangd",
        "jdtls",
        "lua-language-server",
        "zls",
        "bash-language-server",
        "yaml-language-server",
        "svelte",
        "vue",
        "solidity",
        "verible",
    ] {
        assert!(
            LspSettings::from_config(&json!({"lsp":{id:{"disabled":true}}})).is_ok(),
            "{id}"
        );
    }
}

#[test]
fn malformed_sections_reserved_controls_and_entries_are_refused() {
    for value in [
        json!(true),
        json!(null),
        json!([]),
        json!("false"),
        json!(1),
    ] {
        assert!(LspSettings::from_config(&json!({"lsp":value})).is_err());
        assert!(FormatterSettings::from_config(&json!({"formatters":value})).is_err());
    }
    for value in [json!(null), json!("true"), json!(1), json!({})] {
        assert!(LspSettings::from_config(&json!({"lsp":{"auto_install":value}})).is_err());
    }
    for value in [
        json!(-1),
        json!(1.5),
        json!("5000"),
        json!(true),
        json!(null),
    ] {
        assert!(LspSettings::from_config(&json!({"lsp":{"diagnostics_wait_ms":value}})).is_err());
    }
    for entry in [
        json!(false),
        json!({"command":"tool"}),
        json!({"command":[]}),
        json!({"command":[""]}),
        json!({"command":["tool","\u{0000}"]}),
        json!({"command":null}),
        json!({"extensions":[1]}),
        json!({"extensions":[" "]}),
        json!({"env":{"TOKEN":true}}),
        json!({"env":{"A=B":"v"}}),
        json!({"disabled":"true"}),
        json!({"unknown":true}),
    ] {
        assert!(
            LspSettings::from_config(&json!({"lsp":{"rust-analyzer":entry}})).is_err(),
            "{entry}"
        );
        assert!(
            FormatterSettings::from_config(&json!({"formatters":{"rustfmt":entry}})).is_err(),
            "{entry}"
        );
    }
}
