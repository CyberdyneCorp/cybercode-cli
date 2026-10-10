use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InstallMethod {
    Package { manager: String, package: String },
    Release { repository: String },
    Custom,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ServerDefinition {
    pub id: String,
    pub extensions: Vec<String>,
    pub root_markers: Vec<String>,
    pub command: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub initialization_options: Option<serde_json::Value>,
    pub install: InstallMethod,
}

fn definition(
    id: &str,
    extensions: &[&str],
    markers: &[&str],
    command: &[&str],
    install: InstallMethod,
) -> ServerDefinition {
    ServerDefinition {
        id: id.into(),
        extensions: extensions.iter().map(|s| (*s).into()).collect(),
        root_markers: markers.iter().map(|s| (*s).into()).collect(),
        command: command.iter().map(|s| (*s).into()).collect(),
        env: BTreeMap::new(),
        initialization_options: None,
        install,
    }
}
fn npm(package: &str) -> InstallMethod {
    InstallMethod::Package {
        manager: "npm".into(),
        package: package.into(),
    }
}
fn release(repository: &str) -> InstallMethod {
    InstallMethod::Release {
        repository: repository.into(),
    }
}

/// Declarative launch/install definitions. This function never downloads or runs anything.
pub fn builtin_servers() -> Vec<ServerDefinition> {
    let js = &["package.json", "tsconfig.json", "jsconfig.json"];
    vec![
        definition(
            "rust-analyzer",
            &[".rs"],
            &["Cargo.toml"],
            &["rust-analyzer"],
            release("rust-lang/rust-analyzer"),
        ),
        definition(
            "typescript",
            &[".ts", ".tsx", ".js", ".jsx", ".mjs", ".cjs", ".mts", ".cts"],
            js,
            &["typescript-language-server", "--stdio"],
            npm("typescript-language-server"),
        ),
        definition(
            "pyright",
            &[".py", ".pyi"],
            &[
                "pyproject.toml",
                "pyrightconfig.json",
                "setup.py",
                "requirements.txt",
            ],
            &["pyright-langserver", "--stdio"],
            npm("pyright"),
        ),
        definition(
            "gopls",
            &[".go"],
            &["go.work", "go.mod"],
            &["gopls"],
            InstallMethod::Package {
                manager: "go".into(),
                package: "golang.org/x/tools/gopls".into(),
            },
        ),
        definition(
            "clangd",
            &[".c", ".h", ".cc", ".cpp", ".cxx", ".hpp", ".hh", ".hxx"],
            &[
                "compile_commands.json",
                "compile_flags.txt",
                "CMakeLists.txt",
            ],
            &["clangd"],
            release("clangd/clangd"),
        ),
        definition(
            "jdtls",
            &[".java"],
            &[
                "pom.xml",
                "build.gradle",
                "build.gradle.kts",
                "settings.gradle",
            ],
            &["jdtls"],
            release("eclipse-jdtls/eclipse.jdt.ls"),
        ),
        definition(
            "lua-language-server",
            &[".lua"],
            &[".luarc.json", ".luarc.jsonc"],
            &["lua-language-server"],
            release("LuaLS/lua-language-server"),
        ),
        definition(
            "zls",
            &[".zig", ".zon"],
            &["build.zig", "build.zig.zon"],
            &["zls"],
            release("zigtools/zls"),
        ),
        definition(
            "bash-language-server",
            &[".sh", ".bash", ".zsh", ".ksh"],
            &[".git"],
            &["bash-language-server", "start"],
            npm("bash-language-server"),
        ),
        definition(
            "yaml-language-server",
            &[".yaml", ".yml"],
            &[".git"],
            &["yaml-language-server", "--stdio"],
            npm("yaml-language-server"),
        ),
        definition(
            "svelte",
            &[".svelte"],
            &["svelte.config.js", "svelte.config.ts", "package.json"],
            &["svelteserver", "--stdio"],
            npm("svelte-language-server"),
        ),
        definition(
            "vue",
            &[".vue"],
            js,
            &["vue-language-server", "--stdio"],
            npm("@vue/language-server"),
        ),
        definition(
            "solidity",
            &[".sol"],
            &[
                "foundry.toml",
                "hardhat.config.js",
                "hardhat.config.ts",
                "truffle-config.js",
            ],
            &["nomicfoundation-solidity-language-server", "--stdio"],
            npm("@nomicfoundation/solidity-language-server"),
        ),
        definition(
            "verible",
            &[".sv", ".svh", ".v", ".vh"],
            &["verible.filelist", ".git"],
            &["verible-verilog-ls"],
            release("chipsalliance/verible"),
        ),
    ]
}
