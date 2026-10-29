//! Explicit plugin bootstrap inputs. Ordinary-tool environment overlays cannot
//! inject loaders, runtime options, credentials or startup configuration here.
use std::collections::BTreeMap;
use std::path::Path;

#[cfg(unix)]
const CHILD_PATH: &str = "/usr/local/bin:/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin";

pub(super) fn build(
    name: &str,
    root: &Path,
    scratch: &Path,
    wrapped: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    let mut environment = BTreeMap::from([
        ("LANG".into(), "C".into()),
        ("LC_ALL".into(), "C".into()),
        ("TERM".into(), "dumb".into()),
        ("HOME".into(), text(scratch)),
        ("TMPDIR".into(), text(scratch)),
        ("TMP".into(), text(scratch)),
        ("TEMP".into(), text(scratch)),
        ("XDG_CONFIG_HOME".into(), text(&scratch.join("config"))),
        ("XDG_CACHE_HOME".into(), text(&scratch.join("cache"))),
        ("XDG_DATA_HOME".into(), text(&scratch.join("data"))),
        ("XDG_STATE_HOME".into(), text(&scratch.join("state"))),
        ("DENO_DIR".into(), text(&scratch.join("deno-cache"))),
        ("DENO_NO_UPDATE_CHECK".into(), "1".into()),
        (
            "BUN_RUNTIME_TRANSPILER_CACHE_PATH".into(),
            text(&scratch.join("bun-cache")),
        ),
        ("PYTHONNOUSERSITE".into(), "1".into()),
        ("PYTHONUTF8".into(), "1".into()),
        ("PYTHONIOENCODING".into(), "utf-8".into()),
        ("CYBER_PLUGIN_NAME".into(), name.into()),
        ("CYBER_PLUGIN_ROOT".into(), text(root)),
    ]);
    platform(&mut environment, scratch);
    // Only the owner's sandbox wrapper supplies proxy endpoints. Inherited and
    // configured proxy variables never enter this environment.
    environment.extend(
        wrapped
            .iter()
            .filter(|(name, _)| proxy_variable(name))
            .map(|(name, value)| (name.clone(), value.clone())),
    );
    cyber_sandbox::owner_environment(environment)
        .into_iter()
        .collect()
}

fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}
fn proxy_variable(name: &str) -> bool {
    matches!(
        name.to_ascii_uppercase().as_str(),
        "HTTP_PROXY" | "HTTPS_PROXY" | "ALL_PROXY" | "NO_PROXY"
    )
}
#[cfg(unix)]
fn platform(environment: &mut BTreeMap<String, String>, _scratch: &Path) {
    environment.insert("PATH".into(), CHILD_PATH.into());
}
#[cfg(windows)]
fn platform(environment: &mut BTreeMap<String, String>, scratch: &Path) {
    // The system locator comes only from the trusted owner's startup environment,
    // never sandbox.env.set. Missing/relative locators cannot create a relative PATH.
    let root = std::env::var_os("SystemRoot")
        .map(std::path::PathBuf::from)
        .filter(|root| root.is_absolute());
    if let Some(root) = root {
        environment.insert("SystemRoot".into(), text(&root));
        environment.insert("WINDIR".into(), text(&root));
        environment.insert("COMSPEC".into(), text(&root.join("System32/cmd.exe")));
        environment.insert(
            "PATH".into(),
            format!("{};{}", root.join("System32").display(), root.display()),
        );
    } else {
        environment.insert("PATH".into(), String::new());
    }
    environment.insert("USERPROFILE".into(), text(scratch));
    environment.insert("APPDATA".into(), text(&scratch.join("config")));
    environment.insert("LOCALAPPDATA".into(), text(&scratch.join("cache")));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_bootstrap_and_proxy_inputs_are_explicit() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("package");
        let scratch = temp.path().join("scratch");
        let wrapped = BTreeMap::from([
            ("HTTP_PROXY".into(), "http://127.0.0.1:3128".into()),
            ("NO_PROXY".into(), "".into()),
            ("NODE_OPTIONS".into(), "--require=untrusted".into()),
            ("UNREVIEWED_BOOTSTRAP".into(), "fixture-only".into()),
        ]);
        let environment = build("fixture", &root, &scratch, &wrapped);
        assert_eq!(environment["CYBER_PLUGIN_ROOT"], text(&root));
        assert_eq!(environment["CYBER_PLUGIN_NAME"], "fixture");
        for key in ["HOME", "TMPDIR", "TMP", "TEMP"] {
            assert_eq!(environment[key], text(&scratch));
        }
        for key in [
            "XDG_CONFIG_HOME",
            "XDG_CACHE_HOME",
            "XDG_DATA_HOME",
            "XDG_STATE_HOME",
            "DENO_DIR",
            "BUN_RUNTIME_TRANSPILER_CACHE_PATH",
        ] {
            assert!(Path::new(&environment[key]).starts_with(&scratch), "{key}");
        }
        assert_eq!(environment["HTTP_PROXY"], wrapped["HTTP_PROXY"]);
        assert_eq!(environment["NO_PROXY"], "");
        assert!(!environment.contains_key("NODE_OPTIONS"));
        assert!(!environment.contains_key("UNREVIEWED_BOOTSTRAP"));
        #[cfg(unix)]
        assert_eq!(environment["PATH"], CHILD_PATH);
        assert_eq!(environment["PYTHONNOUSERSITE"], "1");
        assert_eq!(environment["PYTHONUTF8"], "1");
    }

    #[test]
    fn owner_api_values_are_removed_even_from_wrapper_fields() {
        let fixture = format!("plugin-environment-owner-{}", std::process::id());
        cyber_sandbox::reserve_owner_credential(&fixture);
        let wrapped = BTreeMap::from([("HTTPS_PROXY".into(), fixture)]);
        let temp = tempfile::tempdir().unwrap();
        let environment = build("fixture", temp.path(), temp.path(), &wrapped);
        assert!(!environment.contains_key("HTTPS_PROXY"));
    }
}
