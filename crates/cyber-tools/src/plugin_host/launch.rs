//! Starting a plugin host process inside the sandbox with only its declared capabilities
//! (`plugins-marketplace` → Plugin host process, Capability declaration and enforcement).
//!
//! The host always runs sandboxed, whatever the Session's sandbox policy: it may write
//! only its own scratch directory, plus the Location with `fs.write`. Without `network`
//! every connection is refused: direct ones by the OS sandbox, proxied ones by the
//! allowlist proxy, which records the attempt so the core can warn naming the plugin.
//! `network: { hosts }` allows only those hosts through the proxy; `network: true` is
//! as wide as the resolved sandbox network, never wider ([`super::network`]). A machine without sandbox enforcement does not start plugin hosts.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex, PoisonError};

use cyber_core::config::Resolved;
use cyber_core::plugins::{Capabilities, HostEntry, Runtime};
use cyber_sandbox::proxy::{Decide, Proxy};
use cyber_sandbox::{Launch, Policy, SandboxConfig};

use crate::tools::process::Process;

/// Hosts a plugin tried to reach without being allowed to.
pub(crate) type Denials = Arc<Mutex<Vec<String>>>;

/// What one launch needs besides the plugin.
pub(crate) struct LaunchContext<'a> {
    pub resolved: &'a Resolved,
    pub location: &'a Path,
    pub home: &'a Path,
    pub temp_dir: &'a Path,
    pub helper: Option<&'a Path>,
}

/// The plugin being started.
pub(crate) struct PluginLaunch<'a> {
    pub name: &'a str,
    pub root: &'a Path,
    pub host: &'a HostEntry,
    pub capabilities: &'a Capabilities,
}

/// A started host and the native resources it owns until it stops.
pub(crate) struct Started {
    pub process: Process,
    /// Removed when the host's resources are dropped.
    pub _scratch: Scratch,
    pub proxy: Option<Proxy>,
}

impl LaunchContext<'_> {
    pub(crate) async fn start(
        &self,
        plugin: &PluginLaunch<'_>,
        denials: &Denials,
    ) -> Result<Started, String> {
        let (program, args) = command(plugin)?;
        let mut config = SandboxConfig::resolve(
            &self.resolved.value,
            &self.resolved.sources,
            None,
            self.home,
        );
        if let Some(error) = &config.error {
            return Err(format!("plugin {} cannot start: {error}", plugin.name));
        }
        config.policy = Policy::WorkspaceWrite;
        let scratch = Scratch::new(&self.temp_dir.join("plugins"), plugin.name)
            .map_err(|_| format!("plugin {}: scratch creation failed", plugin.name))?;
        let proxy = self.network(plugin, &mut config, &scratch, denials).await?;
        let launch = confinement(
            config,
            plugin,
            (self.location, self.home, &scratch.path),
            proxy.as_ref().map(|proxy| proxy.endpoint.clone()),
            self.helper,
        );
        let wrapped = cyber_sandbox::wrap(&launch, &program, &args)
            .map_err(|error| format!("plugin {} cannot start: {error}", plugin.name))?;
        let environment =
            super::environment::build(plugin.name, plugin.root, &scratch.path, &wrapped.env);
        let process =
            Process::spawn_with_stdin(&wrapped.program, &wrapped.args, self.helper, |child| {
                child
                    .env_clear()
                    .envs(&environment)
                    .current_dir(self.location)
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped())
                    .kill_on_drop(true);
            })
            .await
            .map_err(|error| format!("plugin {} failed to start: {error}", plugin.name))?;
        Ok(Started {
            process,
            _scratch: scratch,
            proxy,
        })
    }

    /// Narrow the resolved network to the capability; a proxy records refused hosts.
    async fn network(
        &self,
        plugin: &PluginLaunch<'_>,
        config: &mut SandboxConfig,
        scratch: &Scratch,
        denials: &Denials,
    ) -> Result<Option<Proxy>, String> {
        let Some(reach) = super::network::narrow(config, &plugin.capabilities.network) else {
            return Ok(None);
        };
        let (name, denials) = (plugin.name.to_owned(), Arc::clone(denials));
        let decide: Decide = Arc::new(move |host| {
            let ok = reach.allows(&host);
            if !ok {
                cyber_core::log::warn(
                    "plugins",
                    &format!("plugin {name} was denied network access to {host}"),
                    serde_json::json!({"plugin": name, "host": host}),
                );
                denials
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push(host);
            }
            Box::pin(async move { ok })
        });
        #[cfg(unix)]
        let proxy = if cyber_sandbox::proxy_uses_unix_socket() {
            Proxy::start_unix(decide, &scratch.path.join("proxy.sock")).await
        } else {
            Proxy::start(decide).await
        };
        #[cfg(not(unix))]
        let proxy = {
            let _ = scratch;
            Proxy::start(decide).await
        };
        proxy
            .map(Some)
            .map_err(|_| format!("plugin {}: network proxy creation failed", plugin.name))
    }
}

/// The sandbox of one host: it writes only its scratch directory (plus the Location
/// with `fs.write`) and reads its own code and the Location. Reads are open elsewhere,
/// but the Windows AppContainer grants only listed paths, so the plugin root (and a
/// read-only Location) are listed as readable, after the credential files are chosen so
/// they never re-expose one.
fn confinement(
    mut config: SandboxConfig,
    plugin: &PluginLaunch<'_>,
    (location, home, scratch): (&Path, &Path, &Path),
    proxy: Option<cyber_sandbox::proxy::Endpoint>,
    helper: Option<&Path>,
) -> Launch {
    let mut writable = vec![scratch.to_path_buf()];
    if plugin.capabilities.fs_write {
        writable.push(location.to_path_buf());
    }
    let unreadable = config.unreadable(home);
    config.readable_paths.push(plugin.root.to_path_buf());
    if !plugin.capabilities.fs_write {
        config.readable_paths.push(location.to_path_buf());
    }
    Launch {
        read_only: writable
            .iter()
            .flat_map(|root| cyber_sandbox::protected_in(root))
            .collect(),
        unreadable,
        deny_read: config.deny_read_for(location, home),
        writable,
        proxy,
        helper: helper.map(Path::to_path_buf),
        config,
    }
}

/// The program and arguments that run `main` with its runtime.
fn command(plugin: &PluginLaunch<'_>) -> Result<(String, Vec<String>), String> {
    let relative = cyber_core::plugins::manifest_path(&plugin.host.main)
        .ok_or_else(|| format!("plugin {}: main is outside the plugin", plugin.name))?;
    let main = plugin.root.join(relative);
    let regular = std::fs::symlink_metadata(&main).is_ok_and(|m| m.is_file());
    if !regular {
        return Err(format!(
            "plugin {}: main {} is not a regular file",
            plugin.name, plugin.host.main
        ));
    }
    let main_arg = main.display().to_string();
    let (candidates, args): (&[&str], Vec<String>) = match plugin.host.runtime {
        Runtime::Native => return Ok((main_arg, Vec::new())),
        Runtime::Node => (&["node"], vec![main_arg]),
        Runtime::Bun => (&["bun"], vec!["run".into(), main_arg]),
        Runtime::Deno => (&["deno"], vec!["run".into(), "-A".into(), main_arg]),
        Runtime::Python => (&["python3", "python"], vec!["-u".into(), main_arg]),
    };
    let program = candidates
        .iter()
        .find_map(|name| find_program(name))
        .ok_or_else(|| {
            format!(
                "plugin {}: runtime {} is not installed",
                plugin.name,
                plugin.host.runtime.as_str()
            )
        })?;
    Ok((program.display().to_string(), args))
}

fn find_program(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let names: Vec<String> = if cfg!(windows) {
        vec![format!("{name}.exe"), format!("{name}.cmd")]
    } else {
        vec![name.to_owned()]
    };
    std::env::split_paths(&path)
        .flat_map(|dir| names.iter().map(move |name| dir.join(name)))
        .find(|candidate| candidate.is_file())
}

/// A host's private temporary directory, removed when the host stops.
pub(crate) struct Scratch {
    pub path: PathBuf,
}

impl Scratch {
    fn new(parent: &Path, name: &str) -> std::io::Result<Self> {
        std::fs::create_dir_all(parent)?;
        let path = parent.join(format!("{name}-{}", ulid::Ulid::new()));
        let builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        let mut builder = builder;
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(&path)?;
        Ok(Self { path })
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn launch_of(
        root: &Path,
        main: &str,
        runtime: Runtime,
    ) -> Result<(String, Vec<String>), String> {
        let host = HostEntry {
            main: main.into(),
            runtime,
        };
        let capabilities = Capabilities::default();
        command(&PluginLaunch {
            name: "kit",
            root,
            host: &host,
            capabilities: &capabilities,
        })
    }

    /// Regression: the Windows AppContainer plan of a host held only its scratch
    /// directory (and the Location with `fs.write`), so the host could not read its own
    /// `main` and crash-looped.
    #[test]
    fn the_container_plan_of_a_host_reads_its_plugin_root_and_the_location() {
        let dir = tempfile::tempdir().unwrap();
        let (root, location, home, scratch) = (
            dir.path().join("cache/plugins/kit/1.0.0-ab"),
            dir.path().join("repo"),
            dir.path().join("home"),
            dir.path().join("tmp/plugins/kit-1"),
        );
        let host = HostEntry {
            main: "host.js".into(),
            runtime: Runtime::Node,
        };
        for fs_write in [false, true] {
            let capabilities = Capabilities {
                fs_write,
                ..Capabilities::default()
            };
            let plugin = PluginLaunch {
                name: "kit",
                root: &root,
                host: &host,
                capabilities: &capabilities,
            };
            let config =
                SandboxConfig::resolve(&serde_json::json!({}), &BTreeMap::new(), None, &home);
            let launch = confinement(config, &plugin, (&location, &home, &scratch), None, None);
            let plan = cyber_sandbox::ContainerPlan::from_launch(&launch).unwrap();
            assert!(plan.readable.contains(&root), "{plan:?}");
            assert_eq!(plan.roots.contains(&location), fs_write, "{plan:?}");
            assert_eq!(plan.readable.contains(&location), !fs_write, "{plan:?}");
            assert!(plan.roots.contains(&scratch), "{plan:?}");
            assert!(plan.unreadable.contains(&home.join(".ssh")), "{plan:?}");
        }
    }

    #[test]
    fn each_runtime_starts_main_and_main_must_be_a_regular_file_inside_the_root() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("host"), "").unwrap();
        let main = root.path().join("host").display().to_string();
        assert_eq!(
            launch_of(root.path(), "host", Runtime::Native).unwrap(),
            (main.clone(), Vec::new())
        );
        for (runtime, args) in [
            (Runtime::Bun, vec!["run".to_owned(), main.clone()]),
            (Runtime::Deno, vec!["run".into(), "-A".into(), main.clone()]),
            (Runtime::Python, vec!["-u".into(), main.clone()]),
            (Runtime::Node, vec![main.clone()]),
        ] {
            match launch_of(root.path(), "host", runtime) {
                Ok((program, got)) => {
                    assert!(Path::new(&program).is_absolute(), "{program}");
                    assert_eq!(got, args);
                }
                Err(error) => assert_eq!(
                    error,
                    format!("plugin kit: runtime {} is not installed", runtime.as_str())
                ),
            }
        }
        let missing = launch_of(root.path(), "absent.js", Runtime::Node).unwrap_err();
        assert_eq!(missing, "plugin kit: main absent.js is not a regular file");
        let outside = launch_of(root.path(), "../host", Runtime::Native).unwrap_err();
        assert_eq!(outside, "plugin kit: main is outside the plugin");
        std::fs::create_dir(root.path().join("dir")).unwrap();
        assert!(launch_of(root.path(), "dir", Runtime::Native).is_err());
    }
}
