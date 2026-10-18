//! Container detection and `bypass` eligibility (`sandbox` → Container detection,
//! `permissions-modes` → bypass mode).

use std::path::Path;

use crate::{NetworkMode, Policy, SandboxConfig};

/// The refusal for `bypass` outside a sandbox, a container or an explicit flag.
pub const BYPASS_REQUIREMENT: &str =
    "bypass mode requires a sandbox, a container, or --dangerously-bypass-permissions";

/// Environment variables that mark a container or a devcontainer.
const CONTAINER_VARS: &[(&str, Option<&str>)] = &[
    ("CYBER_CONTAINER", Some("1")),
    ("REMOTE_CONTAINERS", None),
    ("DEVCONTAINER", None),
    ("CODESPACES", Some("true")),
    ("container", None),
];

/// cgroup paths written by container runtimes.
const CGROUP_MARKERS: &[&str] = &[
    "docker",
    "kubepods",
    "containerd",
    "lxc",
    "podman",
    "libpod",
];

/// What isolates the commands Cyber runs, for `bypass` and the session header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Isolation {
    /// The OS sandbox enforces a `workspace-write` or stricter policy.
    Sandbox,
    /// No OS sandbox, but the process runs inside a detected container or VM.
    Container,
    /// Nothing isolates commands.
    None,
}

impl Isolation {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Sandbox => "sandbox",
            Self::Container => "container",
            Self::None => "none",
        }
    }

    /// The isolation of a policy, given whether OS enforcement and a container exist. A
    /// sandbox that leaves the network open or makes `/`, `home` or an ancestor of it
    /// writable does not isolate.
    pub fn of(config: &SandboxConfig, enforced: bool, container: bool, home: &Path) -> Self {
        if config.policy != Policy::FullAccess
            && enforced
            && config.error.is_none()
            && confines(config, home)
        {
            Self::Sandbox
        } else if container {
            Self::Container
        } else {
            Self::None
        }
    }

    /// `bypass` is allowed only when isolated, or with the explicit flag.
    pub fn admits_bypass(self, explicit: bool) -> Result<(), &'static str> {
        if explicit || self != Self::None {
            Ok(())
        } else {
            Err(BYPASS_REQUIREMENT)
        }
    }
}

/// Whether the sandbox confines commands: no open network and no writable root that
/// contains the home directory (or is relative, so could).
fn confines(config: &SandboxConfig, home: &Path) -> bool {
    let canonical = std::fs::canonicalize(home).ok();
    config.network != NetworkMode::On
        && !config.extra_writable.iter().any(|root| {
            !root.is_absolute()
                || home.starts_with(root)
                || canonical
                    .as_ref()
                    .is_some_and(|home| home.starts_with(root))
        })
}

/// Whether this process runs inside a container or VM.
pub fn in_container() -> bool {
    container_markers(&|name| std::env::var(name).ok(), Path::new("/"))
}

/// The probe behind [`in_container`], over an environment and a filesystem root. Only
/// `CYBER_CONTAINER=1` vouches for a Flatpak sandbox, a toolbox or a distrobox: they
/// share the user's home and expose the host, so they do not isolate it.
pub fn container_markers(env: &dyn Fn(&str) -> Option<String>, root: &Path) -> bool {
    if env("CYBER_CONTAINER").is_some_and(|v| v == "1") {
        return true;
    }
    if shares_the_host(env, root) {
        return false;
    }
    let marked = CONTAINER_VARS.iter().any(|(name, value)| {
        env(name).is_some_and(|v| match value {
            Some(expected) => v.eq_ignore_ascii_case(expected),
            None => !v.is_empty(),
        })
    });
    marked
        || root.join(".dockerenv").exists()
        || root.join("run/.containerenv").exists()
        || cgroup_marked(&root.join("proc/1/cgroup"))
}

/// Runtimes that set container markers but bind the user's home and the host.
fn shares_the_host(env: &dyn Fn(&str) -> Option<String>, root: &Path) -> bool {
    env("container").is_some_and(|v| v.eq_ignore_ascii_case("flatpak"))
        || ["DISTROBOX_ENTER_PATH", "TOOLBOX_PATH"]
            .iter()
            .any(|name| env(name).is_some_and(|v| !v.is_empty()))
        || root.join("run/.toolboxenv").exists()
        || root.join(".flatpak-info").exists()
}

fn cgroup_marked(path: &Path) -> bool {
    std::fs::read_to_string(path).is_ok_and(|text| {
        text.lines()
            .any(|line| CGROUP_MARKERS.iter().any(|marker| line.contains(marker)))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn none(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn markers_files_cgroups_and_variables_detect_a_container() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        assert!(!container_markers(&none, root));
        std::fs::create_dir_all(root.join("proc/1")).unwrap();
        std::fs::write(root.join("proc/1/cgroup"), "0::/init.scope\n").unwrap();
        assert!(!container_markers(&none, root));
        std::fs::write(root.join("proc/1/cgroup"), "0::/kubepods/besteffort/x\n").unwrap();
        assert!(container_markers(&none, root));
        let bare = tempfile::tempdir().unwrap();
        std::fs::write(bare.path().join(".dockerenv"), "").unwrap();
        assert!(container_markers(&none, bare.path()));
        let empty = tempfile::tempdir().unwrap();
        let flag = |name: &str| (name == "CYBER_CONTAINER").then(|| "1".to_string());
        assert!(container_markers(&flag, empty.path()));
        let off = |name: &str| (name == "CYBER_CONTAINER").then(|| "0".to_string());
        assert!(!container_markers(&off, empty.path()));
        let dev = |name: &str| (name == "REMOTE_CONTAINERS").then(|| "true".to_string());
        assert!(container_markers(&dev, empty.path()));
    }

    /// Regression: Flatpak, toolbox and distrobox set container markers but share the
    /// user's home and the host, so they are not containers unless `CYBER_CONTAINER=1`.
    #[test]
    fn flatpak_toolbox_and_distrobox_are_not_containers() {
        let empty = tempfile::tempdir().unwrap();
        let flatpak = |name: &str| (name == "container").then(|| "flatpak".to_string());
        assert!(!container_markers(&flatpak, empty.path()));
        let podman = |name: &str| (name == "container").then(|| "podman".to_string());
        assert!(
            container_markers(&podman, empty.path()),
            "a real podman container"
        );
        let toolbox = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(toolbox.path().join("run")).unwrap();
        std::fs::write(toolbox.path().join("run/.containerenv"), "").unwrap();
        std::fs::write(toolbox.path().join("run/.toolboxenv"), "").unwrap();
        assert!(!container_markers(&podman, toolbox.path()));
        let distrobox = |name: &str| match name {
            "container" => Some("oci".to_string()),
            "DISTROBOX_ENTER_PATH" => Some("/usr/bin/distrobox-enter".to_string()),
            _ => None,
        };
        let containerenv = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(containerenv.path().join("run")).unwrap();
        std::fs::write(containerenv.path().join("run/.containerenv"), "").unwrap();
        assert!(container_markers(&none, containerenv.path()));
        assert!(!container_markers(&distrobox, containerenv.path()));
        let toolbox_path =
            |name: &str| (name == "TOOLBOX_PATH").then(|| "/usr/bin/toolbox".to_string());
        assert!(!container_markers(&toolbox_path, containerenv.path()));
        let vouched = |name: &str| match name {
            "CYBER_CONTAINER" => Some("1".to_string()),
            "container" => Some("flatpak".to_string()),
            _ => None,
        };
        assert!(container_markers(&vouched, empty.path()));
    }

    #[test]
    fn bypass_needs_an_enforced_sandbox_a_container_or_the_flag() {
        let home = Path::new("/home/u");
        let resolve = |policy: &str| {
            SandboxConfig::resolve(&serde_json::json!({}), &BTreeMap::new(), Some(policy), home)
        };
        let full = resolve("full-access");
        let workspace = resolve("workspace-write");
        let of = |config: &SandboxConfig, enforced, container| {
            Isolation::of(config, enforced, container, home)
        };
        assert_eq!(of(&workspace, true, false), Isolation::Sandbox);
        assert_eq!(of(&workspace, false, false), Isolation::None);
        assert_eq!(of(&full, true, false), Isolation::None);
        assert_eq!(of(&full, true, true), Isolation::Container);
        assert_eq!(
            Isolation::None.admits_bypass(false),
            Err(BYPASS_REQUIREMENT)
        );
        assert!(Isolation::None.admits_bypass(true).is_ok());
        assert!(Isolation::Container.admits_bypass(false).is_ok());
    }

    /// Regression: a sandbox with the network on, or a writable root at `/`, the home
    /// directory or above it, does not isolate, so it does not admit `bypass` alone.
    #[test]
    fn an_open_network_or_a_home_writable_root_is_not_isolation() {
        let home = std::env::temp_dir().join("cyber-isolation-home");
        let home = home.as_path();
        let resolve = |sandbox: serde_json::Value| {
            SandboxConfig::resolve(
                &serde_json::json!({ "sandbox": sandbox }),
                &BTreeMap::new(),
                None,
                home,
            )
        };
        let of = |config: &SandboxConfig, container| Isolation::of(config, true, container, home);
        let open = resolve(serde_json::json!({"network": "on"}));
        assert_eq!(of(&open, false), Isolation::None);
        assert_eq!(of(&open, true), Isolation::Container);
        let parent = home.parent().unwrap().display().to_string();
        for root in ["/", "~/", &parent, "~", ".."] {
            let rooted = resolve(serde_json::json!({"writable_roots": [root]}));
            assert_eq!(of(&rooted, false), Isolation::None, "{root}");
        }
        let scoped = resolve(serde_json::json!({"network": "off", "writable_roots": ["~/cache"]}));
        assert_eq!(of(&scoped, false), Isolation::Sandbox);
    }
}
