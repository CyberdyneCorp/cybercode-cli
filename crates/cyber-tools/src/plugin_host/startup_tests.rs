//! Actual runtime receipts through the production command/environment builders.
//! This isolates startup behavior; it does not establish native OS confinement.
use super::*;
use serde_json::{Value, json};
use std::io::Write;
use std::process::{Command, Output};
use std::time::{Duration, Instant};

const HOST: &str = r#"
import readline from 'node:readline';
import process from 'node:process';
import {createRequire} from 'node:module';
const require = createRequire(import.meta.url);
const bundled = require('./sibling.cjs');
const alias = __BUNDLED_ALIAS__;
const lines = readline.createInterface({input: process.stdin});
lines.on('line', line => {
  const request = JSON.parse(line);
  let result;
  if (request.method === 'initialize') {
    result = {protocolVersion: '1', tools: [{name: 'bootstrap', description: 'fixture', inputSchema: {type: 'object'}}]};
  } else if (request.method === 'tool/execute') {
    result = {content: JSON.stringify({bundled, alias, packagePreload: process.env.CYBER_RUNTIME_PACKAGE_PRELOAD === 'fixture-only', preload: process.env.CYBER_RUNTIME_PRELOAD === 'fixture-only', dotenv: process.env.CYBER_RUNTIME_DOTENV === 'fixture-only'})};
  } else if (request.method === 'shutdown') {
    process.stdout.write(JSON.stringify({jsonrpc: '2.0', id: request.id, result: null}) + '\n', () => process.exit(0));
    return;
  } else { throw new Error('unexpected fixture method'); }
  process.stdout.write(JSON.stringify({jsonrpc: '2.0', id: request.id, result}) + '\n');
});
"#;

fn receipt(command: &mut Command) -> Output {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = String::new();
    for (id, method) in [(1, "initialize"), (2, "tool/execute"), (3, "shutdown")] {
        input.push_str(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": {"name": "bootstrap", "input": {}}}).to_string());
        input.push('\n');
    }
    let sent = child.stdin.take().unwrap().write_all(input.as_bytes());
    if let Err(error) = sent
        && error.kind() != std::io::ErrorKind::BrokenPipe
    {
        let _ = child.kill();
        let _ = child.wait();
        panic!("fixture input failed: {error}");
    }
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if child.try_wait().unwrap().is_some() {
            return child.wait_with_output().unwrap();
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let output = child.wait_with_output().unwrap();
            panic!("runtime bootstrap did not settle: {output:?}");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

struct StartupImage {
    path: PathBuf,
    _generation: cyber_sandbox::plugin_image::Generation,
    #[cfg(unix)]
    owned: Option<cyber_sandbox::plugin_image::Package>,
}

fn package_configuration(package: &Path, runtime: Runtime, config_name: Option<&str>) {
    let bundled_config = config_name.is_some();
    let host_source = HOST.replace(
        "__BUNDLED_ALIAS__",
        if bundled_config && runtime == Runtime::Deno {
            "(await import('fixture-bundled-alias')).default"
        } else {
            "'no-alias'"
        },
    );
    std::fs::write(package.join("host.mjs"), host_source).unwrap();
    if bundled_config {
        match runtime {
            Runtime::Bun => {
                let preload = package.join("package-preload.cjs");
                std::fs::write(
                    &preload,
                    "process.env.CYBER_RUNTIME_PACKAGE_PRELOAD = 'fixture-only';",
                )
                .unwrap();
                std::fs::write(
                    package.join("bunfig.toml"),
                    format!(
                        "preload = [{}]\n",
                        serde_json::to_string("./package-preload.cjs").unwrap()
                    ),
                )
                .unwrap();
            }
            Runtime::Deno => {
                std::fs::write(
                    package.join(config_name.unwrap()),
                    json!({"imports": {"fixture-bundled-alias": "./sibling.cjs"}}).to_string(),
                )
                .unwrap();
                std::fs::write(
                    package.join("deno.lock"),
                    json!({"version":"4","specifiers":{}}).to_string(),
                )
                .unwrap();
            }
            _ => unreachable!(),
        }
    }
}

#[cfg(unix)]
fn startup_image(
    package: &Path,
    root: &Path,
    runtime: Runtime,
    config_name: Option<&str>,
) -> StartupImage {
    let generation = cyber_sandbox::plugin_image::Generation::new();
    if runtime != Runtime::Deno || config_name.is_none() {
        return StartupImage {
            path: package.to_owned(),
            _generation: generation,
            owned: None,
        };
    }
    let base = root.join("staging");
    std::fs::create_dir(&base).unwrap();
    let digest = cyber_core::plugins::tree_digest(package).unwrap();
    let helper = cyber_sandbox::find_helper().expect("focused workflow builds the sandbox helper");
    let executor = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let owned = executor
        .block_on(cyber_sandbox::plugin_image::prepare(
            &generation.lease(),
            &helper,
            &base,
            package,
            &digest,
        ))
        .unwrap();
    StartupImage {
        path: owned.path().unwrap().to_owned(),
        _generation: generation,
        owned: Some(owned),
    }
}

#[cfg(windows)]
fn startup_image(
    package: &Path,
    root: &Path,
    runtime: Runtime,
    config_name: Option<&str>,
) -> StartupImage {
    let generation = cyber_sandbox::plugin_image::Generation::new();
    if runtime != Runtime::Deno || config_name.is_none() {
        return StartupImage {
            path: package.to_owned(),
            _generation: generation,
        };
    }
    // Windows copy admission remains unavailable; this controlled fixture tests
    // only the CLI's workspace-boundary semantics, without claiming OS admission.
    let namespace = root.join("staging");
    let image = namespace.join("image");
    std::fs::create_dir_all(&image).unwrap();
    for name in ["host.mjs", "sibling.cjs", config_name.unwrap(), "deno.lock"] {
        std::fs::copy(package.join(name), image.join(name)).unwrap();
    }
    std::fs::write(
        namespace.join("package.json"),
        b"{\"private\":true,\"workspaces\":[]}\n",
    )
    .unwrap();
    StartupImage {
        path: image,
        _generation: generation,
    }
}

fn assert_rpc(result: Output, runtime: Runtime, bundled_config: bool) {
    assert!(
        result.status.success(),
        "runtime startup failed ({runtime:?}, bundled={bundled_config}): {result:?}"
    );
    let lines: Vec<Value> = String::from_utf8(result.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(lines.len(), 3);
    assert_eq!(lines[0]["id"], 1);
    assert_eq!(lines[0]["result"]["protocolVersion"], "1");
    assert_eq!(lines[0]["result"]["tools"][0]["name"], "bootstrap");
    assert_eq!(lines[1]["id"], 2);
    let observed: Value =
        serde_json::from_str(lines[1]["result"]["content"].as_str().unwrap()).unwrap();
    let alias = if bundled_config && runtime == Runtime::Deno {
        "bundled-module"
    } else {
        "no-alias"
    };
    let package_preload = bundled_config && runtime == Runtime::Bun;
    assert_eq!(
        observed,
        json!({"bundled": "bundled-module", "alias": alias, "packagePreload": package_preload, "preload": false, "dotenv": false}),
        "project startup code ran before the approved entry"
    );
    assert_eq!(lines[2]["id"], 3);
    assert_eq!(lines[2]["result"], Value::Null);
}

fn verify(runtime: Runtime, config_name: Option<&str>) {
    let bundled_config = config_name.is_some();
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let project = root.join("project");
    let package = project.join("package");
    let scratch = root.join("scratch");
    for directory in [&project, &package, &scratch] {
        std::fs::create_dir(directory).unwrap();
    }
    package_configuration(&package, runtime, config_name);
    std::fs::write(
        package.join("sibling.cjs"),
        "module.exports = 'bundled-module';",
    )
    .unwrap();
    let preload = project.join("preload.cjs");
    std::fs::write(
        &preload,
        "process.env.CYBER_RUNTIME_PRELOAD = 'fixture-only';",
    )
    .unwrap();
    std::fs::write(
        project.join("bunfig.toml"),
        format!(
            "preload = [{}]\n",
            serde_json::to_string(&preload.to_string_lossy()).unwrap()
        ),
    )
    .unwrap();
    std::fs::write(project.join(".env"), "CYBER_RUNTIME_DOTENV=fixture-only\n").unwrap();
    std::fs::write(package.join(".env"), "CYBER_RUNTIME_DOTENV=fixture-only\n").unwrap();
    let config = "{ deliberately invalid project configuration";
    let lock = "deliberately invalid project lock";
    std::fs::write(project.join("deno.json"), config).unwrap();
    std::fs::write(project.join("deno.lock"), lock).unwrap();
    // A prepared image's private workspace boundary must stop config discovery
    // before an unrelated ancestor of the staging namespace can be consulted.
    std::fs::write(root.join("deno.json"), "{ invalid ancestor of staging").unwrap();
    let image = startup_image(&package, &root, runtime, config_name);
    let package = &image.path;
    let host = HostEntry {
        main: "host.mjs".into(),
        runtime,
    };
    let capabilities = Capabilities::default();
    let plugin = PluginLaunch {
        name: "runtime-fixture",
        root: package,
        host: &host,
        capabilities: &capabilities,
    };
    let (program, args) =
        command(&plugin).expect("the focused runtime workflow must install this runtime");
    let environment =
        crate::plugin_host::environment::build(plugin.name, package, &scratch, &Default::default());
    let result = receipt(
        Command::new(program)
            .args(args)
            .env_clear()
            .envs(environment)
            .current_dir(package),
    );
    assert_rpc(result, runtime, bundled_config);
    assert_eq!(
        std::fs::read_to_string(project.join("deno.json")).unwrap(),
        config
    );
    assert_eq!(
        std::fs::read_to_string(project.join("deno.lock")).unwrap(),
        lock
    );
    assert!(!project.join("node_modules").exists());
    assert!(!package.join("node_modules").exists());
    #[cfg(unix)]
    if let Some(owned) = &image.owned {
        assert_eq!(
            cyber_core::plugins::tree_digest(owned.path().unwrap()).unwrap(),
            owned.digest()
        );
        assert_eq!(
            std::fs::read(owned.path().unwrap().parent().unwrap().join("package.json")).unwrap(),
            b"{\"private\":true,\"workspaces\":[]}\n"
        );
    }
}

#[test]
#[ignore = "requires Bun; the focused workflow installs and explicitly executes this receipt"]
fn bun_starts_bundled_rpc_without_project_preloads_or_dotenv() {
    for config in [None, Some("bunfig.toml")] {
        verify(Runtime::Bun, config);
    }
}

#[test]
#[ignore = "requires Deno; the focused workflow installs and explicitly executes this receipt"]
fn deno_starts_bundled_rpc_without_project_configuration_or_lock_changes() {
    for config in [None, Some("deno.json"), Some("deno.jsonc")] {
        verify(Runtime::Deno, config);
    }
}
