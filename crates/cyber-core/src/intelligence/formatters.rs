//! Declarative formatters and local marker discovery; never evaluates configuration code.
use super::ExecutableSearch;
use crate::config::FormatterSettings;
use serde::Serialize;
use std::{
    collections::BTreeMap,
    io::Read,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FormatterDetection {
    Binary,
    Markers { names: Vec<String> },
    Prettier,
}
#[derive(Debug, Clone, Serialize)]
pub struct FormatterDefinition {
    pub id: String,
    pub extensions: Vec<String>,
    pub command: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub detection: FormatterDetection,
}
fn definition(
    id: &str,
    extensions: &[&str],
    command: &[&str],
    markers: &[&str],
) -> FormatterDefinition {
    FormatterDefinition {
        id: id.into(),
        extensions: extensions.iter().map(|s| (*s).into()).collect(),
        command: command.iter().map(|s| (*s).into()).collect(),
        env: BTreeMap::new(),
        detection: if markers.is_empty() {
            FormatterDetection::Binary
        } else {
            FormatterDetection::Markers {
                names: markers.iter().map(|s| (*s).into()).collect(),
            }
        },
    }
}
pub fn builtin_formatters() -> Vec<FormatterDefinition> {
    let js = &[
        ".js", ".jsx", ".mjs", ".cjs", ".ts", ".tsx", ".mts", ".cts", ".json", ".jsonc", ".css",
    ];
    let mut prettier = definition(
        "prettier",
        &[
            ".js", ".jsx", ".mjs", ".cjs", ".ts", ".tsx", ".mts", ".cts", ".json", ".jsonc",
            ".html", ".css", ".scss", ".less", ".vue", ".svelte", ".yaml", ".yml", ".md", ".mdx",
            ".graphql",
        ],
        &["prettier", "--write", "$FILE"],
        &[],
    );
    prettier.detection = FormatterDetection::Prettier;
    vec![
        definition("rustfmt", &[".rs"], &["rustfmt", "$FILE"], &[]),
        prettier,
        definition(
            "biome",
            js,
            &["biome", "format", "--write", "$FILE"],
            &["biome.json", "biome.jsonc"],
        ),
        definition("ruff", &[".py", ".pyi"], &["ruff", "format", "$FILE"], &[]),
        definition("black", &[".py", ".pyi"], &["black", "$FILE"], &[]),
        definition("gofmt", &[".go"], &["gofmt", "-w", "$FILE"], &[]),
        definition(
            "clang-format",
            &[".c", ".cc", ".cpp", ".cxx", ".h", ".hh", ".hpp", ".hxx"],
            &["clang-format", "-i", "$FILE"],
            &[".clang-format", "_clang-format"],
        ),
        definition("shfmt", &[".sh", ".bash"], &["shfmt", "-w", "$FILE"], &[]),
        definition("stylua", &[".lua", ".luau"], &["stylua", "$FILE"], &[]),
        definition("zig", &[".zig", ".zon"], &["zig", "fmt", "$FILE"], &[]),
        definition(
            "forge",
            &[".sol"],
            &["forge", "fmt", "$FILE"],
            &["foundry.toml"],
        ),
        definition(
            "verible-verilog-format",
            &[".v", ".vh", ".sv", ".svh"],
            &["verible-verilog-format", "--inplace", "$FILE"],
            &[],
        ),
    ]
}
#[derive(Debug, Clone, Serialize)]
pub struct DetectedFormatter {
    pub definition: FormatterDefinition,
    pub enabled: bool,
    pub installed: bool,
    pub executable: Option<PathBuf>,
    pub detected_by: String,
}

pub fn detect_formatters(
    settings: &FormatterSettings,
    search: &ExecutableSearch,
) -> Vec<DetectedFormatter> {
    let mut definitions: BTreeMap<_, _> = builtin_formatters()
        .into_iter()
        .map(|s| (s.id.clone(), s))
        .collect();
    for (id, configured) in &settings.formatters {
        let definition = definitions
            .entry(id.clone())
            .or_insert_with(|| definition(id, &[], &[], &[]));
        if let Some(command) = &configured.command {
            definition.command = command.clone();
        }
        if let Some(extensions) = &configured.extensions {
            definition.extensions = extensions.clone();
        }
        definition.env.extend(configured.env.clone());
    }
    definitions
        .into_iter()
        .map(|(id, definition)| {
            let configured = settings.formatters.get(&id);
            let forced = configured.is_some_and(|c| c.command.is_some());
            let disabled = configured.is_some_and(|c| c.disabled);
            let executable = definition
                .command
                .first()
                .and_then(|command| search.find(command));
            let installed = executable.is_some();
            let marker = marker(&definition.detection, &search.location);
            let detected_by = if forced {
                "config".into()
            } else if !installed {
                "binary-not-found".into()
            } else {
                marker
                    .clone()
                    .unwrap_or_else(|| "project-marker-missing".into())
            };
            DetectedFormatter {
                definition,
                enabled: settings.enabled
                    && !disabled
                    && (forced || (installed && marker.is_some())),
                installed,
                executable,
                detected_by,
            }
        })
        .collect()
}
fn marker(detection: &FormatterDetection, location: &Path) -> Option<String> {
    match detection {
        FormatterDetection::Binary => Some("binary".into()),
        FormatterDetection::Markers { names } => names
            .iter()
            .find(|name| regular_marker(&location.join(name)))
            .cloned(),
        FormatterDetection::Prettier => prettier_marker(location),
    }
}
fn regular_marker(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|m| m.is_file())
}
fn prettier_marker(location: &Path) -> Option<String> {
    let names = [
        ".prettierrc",
        ".prettierrc.json",
        ".prettierrc.yml",
        ".prettierrc.yaml",
        ".prettierrc.json5",
        ".prettierrc.toml",
        ".prettierrc.js",
        ".prettierrc.cjs",
        ".prettierrc.mjs",
        ".prettierrc.ts",
        ".prettierrc.cts",
        ".prettierrc.mts",
        "prettier.config.js",
        "prettier.config.cjs",
        "prettier.config.mjs",
        "prettier.config.ts",
        "prettier.config.cts",
        "prettier.config.mts",
    ];
    if let Some(name) = names
        .iter()
        .find(|name| regular_marker(&location.join(name)))
    {
        return Some((*name).into());
    }
    let package = read_package(&location.join("package.json"))?;
    if package
        .get("prettier")
        .is_some_and(|v| v.is_object() || v.is_string())
    {
        return Some("package.json:prettier".into());
    }
    for section in [
        "dependencies",
        "devDependencies",
        "optionalDependencies",
        "peerDependencies",
    ] {
        if package
            .get(section)
            .and_then(|v| v.get("prettier"))
            .and_then(serde_json::Value::as_str)
            .is_some_and(|v| !v.is_empty())
        {
            return Some(format!("package.json:{section}.prettier"));
        }
    }
    None
}
fn read_package(path: &Path) -> Option<serde_json::Value> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let file = options.open(path).ok()?;
    let metadata = file.metadata().ok()?;
    if !metadata.is_file() {
        return None;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes()
            & windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT
            != 0
        {
            return None;
        }
    }
    let mut bytes = Vec::new();
    file.take(1_048_577).read_to_end(&mut bytes).ok()?;
    if bytes.len() > 1_048_576 {
        return None;
    }
    serde_json::from_slice(&bytes).ok()
}
