//! Bind config resolution to native read-only observations, not subprocess authority.
use super::Shadow;
use crate::copy::config_inputs::{ConfigInputs, environment_digest};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

#[derive(PartialEq, Eq)]
pub(super) struct Inventory {
    values: Vec<u8>,
    paths: BTreeSet<PathBuf>,
}

impl Inventory {
    pub(super) fn frozen_config(&self) -> Result<Vec<u8>, String> {
        let mut text = String::new();
        for (_, key, value) in records(&self.values)? {
            if include_key(key) {
                continue;
            }
            let (section, rest) = key.split_once('.').ok_or("Invalid frozen Git config key")?;
            let (subsection, variable) = rest
                .rsplit_once('.')
                .map_or((None, rest), |(subsection, variable)| {
                    (Some(subsection), variable)
                });
            if !config_name(section) || !config_name(variable) {
                return Err("Unsupported frozen Git config key".into());
            }
            text.push('[');
            text.push_str(section);
            if let Some(subsection) = subsection {
                if subsection.chars().any(char::is_control) {
                    return Err("Unsupported frozen Git config subsection".into());
                }
                text.push(' ');
                text.push_str(&quote(subsection)?);
            }
            text.push_str("]\n\t");
            text.push_str(variable);
            if let Some(value) = value {
                text.push_str(" = ");
                text.push_str(&quote(value)?);
            }
            text.push('\n');
            if text.len() > 1024 * 1024 {
                return Err("Frozen Git configuration exceeds limit".into());
            }
        }
        Ok(text.into_bytes())
    }

    pub(super) fn selected(&self, key: &str) -> Result<Option<Option<&str>>, String> {
        Ok(records(&self.values)?
            .into_iter()
            .rev()
            .find(|(_, name, _)| *name == key)
            .map(|(_, _, value)| value))
    }
}

impl Shadow {
    pub(super) async fn config_inputs(&self) -> Result<(ConfigInputs, Inventory), String> {
        self.config_inputs_profile(true).await
    }

    pub(super) async fn user_config_inputs(&self) -> Result<(ConfigInputs, Inventory), String> {
        self.config_inputs_profile(false).await
    }

    async fn config_inputs_profile(
        &self,
        private: bool,
    ) -> Result<(ConfigInputs, Inventory), String> {
        let environment = environment_digest()?;
        let contexts = vec![self.worktree.clone(), self.worktree.join(&self.git_dir)];
        let mut roots = self.config_roots().await?;
        if !private {
            roots.push(self.common_dir.join("config"));
        }
        let original = capture(
            roots.clone(),
            environment.clone(),
            contexts.clone(),
            private,
        )
        .await?;
        let inventory = self.config_inventory().await?;
        let paths = roots
            .into_iter()
            .chain(inventory.paths.iter().cloned())
            .collect();
        let proof = capture(paths, environment, contexts, private).await?;
        if !proof.retains(&original) {
            return Err("Git configuration roots changed during resolution".into());
        }
        Ok((proof, inventory))
    }

    pub(super) async fn verify_config_inputs(
        &self,
        proof: &ConfigInputs,
        inventory: &Inventory,
    ) -> Result<(), String> {
        if &self.config_inventory().await? != inventory {
            return Err("Git configuration origins changed during resolution".into());
        }
        let proof = proof.clone();
        tokio::task::spawn_blocking(move || proof.verify())
            .await
            .map_err(|error| error.to_string())?
    }

    async fn config_roots(&self) -> Result<Vec<PathBuf>, String> {
        let mut paths = BTreeSet::from([
            self.worktree.join(self.git_dir.join("config")),
            self.worktree.join(self.git_dir.join("config.worktree")),
            self.worktree.join(self.git_dir.join("HEAD")),
        ]);
        let no_system = std::env::var("GIT_CONFIG_NOSYSTEM").is_ok_and(|value| {
            matches!(
                value.to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        });
        for name in ["GIT_CONFIG_SYSTEM", "GIT_CONFIG_GLOBAL"] {
            if (name == "GIT_CONFIG_SYSTEM" && no_system)
                || std::env::var_os(name).is_some_and(|value| value.is_empty())
            {
                continue;
            }
            let output = self.run_bounded(&["var", name], 32 * 1024).await?;
            let text = std::str::from_utf8(&output).map_err(|_| "Non-UTF-8 Git config roots")?;
            let text = text
                .strip_suffix('\n')
                .ok_or("Invalid Git configuration roots")?;
            for path in text.split('\n') {
                if path.is_empty() || path.contains('\0') {
                    return Err("Invalid Git configuration root path".into());
                }
                paths.insert(self.worktree.join(path));
            }
        }
        Ok(paths.into_iter().collect())
    }

    async fn config_inventory(&self) -> Result<Inventory, String> {
        let values = self
            .run_bounded(
                &["config", "--null", "--includes", "--show-origin", "--list"],
                1024 * 1024,
            )
            .await?;
        let entries = records(&values)?;
        for (_, key, _) in &entries {
            if !supported_condition(key) {
                return Err(
                    "Conditional Git configuration lacks complete native dependency proof".into(),
                );
            }
        }
        let mut paths = BTreeSet::new();
        for (origin, _, _) in &entries {
            if let Some(path) = file_origin(origin)? {
                paths.insert(self.worktree.join(path));
            }
        }
        let includes: Vec<_> = entries
            .iter()
            .filter(|(_, key, _)| include_key(key))
            .collect();
        let raw = includes
            .iter()
            .map(|(_, _, value)| {
                value
                    .filter(|value| !value.is_empty())
                    .ok_or_else(|| "Invalid Git include target".to_owned())
            })
            .collect::<Result<Vec<_>, _>>()?;
        let expanded = self.expand_config_paths(&raw).await?;
        for ((origin, _, _), path) in includes.into_iter().zip(expanded) {
            let parent = file_origin(origin)?
                .and_then(Path::parent)
                .unwrap_or(&self.worktree);
            paths.insert(self.worktree.join(parent).join(path));
        }
        if paths.len() > 1024 {
            return Err("Git configuration origin count exceeds limit".into());
        }
        Ok(Inventory { values, paths })
    }

    pub(super) async fn expand_config_paths(&self, raw: &[&str]) -> Result<Vec<PathBuf>, String> {
        if raw.is_empty() {
            return Ok(vec![]);
        }
        if raw.len() > 1024 || raw.iter().map(|value| value.len()).sum::<usize>() > 64 * 1024 {
            return Err("Git configuration path expansion exceeds limit".into());
        }
        // Expand only values from the bound inventory. A second config lookup
        // could select a different conditional include between observations.
        let key = format!(
            "cybernativepolicy.path{}",
            ulid::Ulid::new().to_string().to_ascii_lowercase()
        );
        let mut args = Vec::new();
        for value in raw {
            if value.starts_with('~') && !value.starts_with("~/") {
                return Err("Named-user Git home expansion lacks native policy proof".into());
            }
            args.extend(["-c".to_owned(), format!("{key}={value}")]);
        }
        args.extend(
            ["config", "--null", "--show-origin", "--path", "--get-all"].map(str::to_owned),
        );
        args.push(key);
        let output = self
            .run_bounded(
                &args.iter().map(String::as_str).collect::<Vec<_>>(),
                1024 * 1024,
            )
            .await?;
        let values = pairs(&output)?;
        if values.len() != raw.len() || values.iter().any(|(origin, _)| *origin != "command line:")
        {
            return Err("Git path expansion origin differs".into());
        }
        Ok(values
            .into_iter()
            .map(|(_, path)| PathBuf::from(path))
            .collect())
    }

    pub(super) async fn config_boolean(&self, key: &str, raw: &str) -> Result<bool, String> {
        let output = self
            .run_bounded(
                &[
                    "-c",
                    &format!("{key}={raw}"),
                    "config",
                    "--bool",
                    "--get",
                    key,
                ],
                16,
            )
            .await?;
        match output.as_slice() {
            b"true\n" => Ok(true),
            b"false\n" => Ok(false),
            _ => Err("Invalid Git configuration boolean expansion".into()),
        }
    }
}

fn config_name(name: &str) -> bool {
    name.as_bytes().first().is_some_and(u8::is_ascii_alphabetic)
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

fn quote(value: &str) -> Result<String, String> {
    let mut text = String::from("\"");
    for character in value.chars() {
        match character {
            '\\' => text.push_str("\\\\"),
            '"' => text.push_str("\\\""),
            '\n' => text.push_str("\\n"),
            '\t' => text.push_str("\\t"),
            '\u{0008}' => text.push_str("\\b"),
            character if character.is_control() => {
                return Err("Unsupported frozen Git config value".into());
            }
            character => text.push(character),
        }
    }
    text.push('"');
    Ok(text)
}

async fn capture(
    paths: Vec<PathBuf>,
    environment: String,
    contexts: Vec<PathBuf>,
    private: bool,
) -> Result<ConfigInputs, String> {
    tokio::task::spawn_blocking(move || {
        let mut proof = ConfigInputs::capture(paths, environment)?;
        let repository = contexts
            .get(1)
            .ok_or("Missing private Git configuration context")?
            .clone();
        proof.bind_contexts(contexts)?;
        if private {
            proof.bind_repository(repository)?;
        }
        Ok(proof)
    })
    .await
    .map_err(|error| error.to_string())?
}

fn include_key(key: &str) -> bool {
    key == "include.path" || (key.starts_with("includeif.") && key.ends_with(".path"))
}

fn supported_condition(key: &str) -> bool {
    let Some(condition) = key
        .strip_prefix("includeif.")
        .and_then(|value| value.strip_suffix(".path"))
    else {
        return true;
    };
    if let Some(pattern) = condition
        .strip_prefix("gitdir:")
        .or_else(|| condition.strip_prefix("gitdir/i:"))
    {
        return !pattern.starts_with('~') || pattern.starts_with("~/");
    }
    condition.starts_with("onbranch:")
}

fn file_origin(origin: &str) -> Result<Option<&Path>, String> {
    if let Some(path) = origin.strip_prefix("file:").filter(|path| !path.is_empty()) {
        return Ok(Some(Path::new(path)));
    }
    if origin == "command line:" {
        return Ok(None);
    }
    Err("Unsupported Git configuration origin".into())
}

type Record<'a> = (&'a str, &'a str, Option<&'a str>);
fn pairs(bytes: &[u8]) -> Result<Vec<(&str, &str)>, String> {
    if bytes.is_empty() {
        return Ok(vec![]);
    }
    let bytes = bytes
        .strip_suffix(&[0])
        .ok_or("Invalid Git config origin framing")?;
    let parts: Vec<_> = bytes.split(|byte| *byte == 0).collect();
    if parts.len() % 2 != 0 || parts.len() > 32768 {
        return Err("Invalid or over-limit Git configuration origins".into());
    }
    parts
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| {
            let origin = std::str::from_utf8(pair[0]).map_err(|_| "Non-UTF-8 Git config origin")?;
            let entry = std::str::from_utf8(pair[1]).map_err(|_| "Non-UTF-8 Git config value")?;
            if origin.is_empty() {
                return Err("Empty Git configuration origin".into());
            }
            Ok((origin, entry))
        })
        .collect()
}

fn records(bytes: &[u8]) -> Result<Vec<Record<'_>>, String> {
    pairs(bytes)?
        .into_iter()
        .map(|(origin, entry)| {
            let (key, value) = entry
                .split_once('\n')
                .map_or((entry, None), |(key, value)| (key, Some(value)));
            if key.is_empty() {
                return Err("Empty Git configuration key".into());
            }
            Ok((origin, key, value))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn origin_records_keep_literal_paths_and_reject_incomplete_frames() {
        let values =
            b"file:/tmp/a\nconfig\0include.path\n../with space\0command line:\0core.bare\nfalse\0";
        let parsed = records(values).unwrap();
        assert_eq!(
            parsed[0],
            ("file:/tmp/a\nconfig", "include.path", Some("../with space"))
        );
        assert_eq!(parsed[1], ("command line:", "core.bare", Some("false")));
        assert!(records(b"file:/tmp/config\0unpaired\0file:missing\0").is_err());
        assert!(records(b"file:/tmp/config\0key\nvalue").is_err());
        assert!(file_origin("blob:unknown").is_err());
        assert!(include_key("includeif.onbranch:main.path"));
        assert!(!include_key("include.other.path"));
        assert!(!supported_condition(
            "includeif.hasconfig:remote.*.url:*.path"
        ));
        assert!(!supported_condition(
            "includeif.gitdir:~named-user/project/.path"
        ));
        let inventory = Inventory {
            values: b"file:a\0core.ignorecase\nfalse\0command line:\0core.ignorecase\0".to_vec(),
            paths: BTreeSet::new(),
        };
        assert_eq!(inventory.selected("core.ignorecase").unwrap(), Some(None));
    }

    #[test]
    fn frozen_configuration_round_trips_values_without_replaying_includes() {
        let entries = [
            ("include.path", Some("never-read")),
            (
                "custom.quoted.sub.value",
                Some(" quotes \" \\ \n\t\u{0008}"),
            ),
            ("custom.flag", None),
            ("custom.empty", Some("")),
        ];
        let mut values = Vec::new();
        for (key, value) in entries {
            values.extend_from_slice(format!("file:a\0{key}").as_bytes());
            if let Some(value) = value {
                values.push(b'\n');
                values.extend_from_slice(value.as_bytes());
            }
            values.push(0);
        }
        let inventory = Inventory {
            values: values.to_vec(),
            paths: BTreeSet::new(),
        };
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config");
        std::fs::write(&path, inventory.frozen_config().unwrap()).unwrap();
        let output = std::process::Command::new("git")
            .args(["config", "--file"])
            .arg(path)
            .args(["--null", "--show-origin", "--list"])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let actual: Vec<_> = records(&output.stdout)
            .unwrap()
            .into_iter()
            .map(|(_, key, value)| (key, value))
            .collect();
        let expected: Vec<_> = records(&values)
            .unwrap()
            .into_iter()
            .filter(|(_, key, _)| !include_key(key))
            .map(|(_, key, value)| (key, value))
            .collect();
        assert_eq!(actual, expected);
        assert!(quote("unsupported\rvalue").is_err());
    }

    #[tokio::test]
    async fn expansion_uses_supplied_values_instead_of_reselecting_mutable_settings() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let work = root.join("work");
        std::fs::create_dir(&work).unwrap();
        assert!(
            std::process::Command::new("git")
                .args(["init", "--quiet"])
                .arg(&work)
                .status()
                .unwrap()
                .success()
        );
        let shadow = Shadow::locate(&root.join("data"), &work).unwrap();
        shadow.ensure_init().await.unwrap();
        shadow
            .run(&["config", "core.ignorecase", "true"])
            .await
            .unwrap();
        shadow
            .run(&["config", "core.excludesfile", "unrelated-current-setting"])
            .await
            .unwrap();
        let values = shadow
            .expand_config_paths(&["../literal with space", ""])
            .await
            .unwrap();
        assert_eq!(
            values,
            [PathBuf::from("../literal with space"), PathBuf::new()]
        );
        assert!(
            !shadow
                .config_boolean("core.ignorecase", "false")
                .await
                .unwrap()
        );
        assert!(
            shadow
                .expand_config_paths(&["~named-user/ignore"])
                .await
                .is_err()
        );
        shadow
            .run(&[
                "config",
                "includeIf.hasconfig:remote.*.url:example.path",
                "missing-conditional-config",
            ])
            .await
            .unwrap();
        assert!(
            shadow
                .ignore_inputs()
                .await
                .unwrap_err()
                .contains("dependency proof")
        );
        std::fs::write(work.join("file"), "legacy-readable").unwrap();
        let snapshot = shadow.track(&[], 65536).await.unwrap();
        assert!(
            snapshot
                .skipped
                .iter()
                .any(|value| value.contains("native Git policy/raw-state proof unavailable"))
        );
        assert!(
            shadow
                .snapshot_policy(&snapshot.tree)
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(
            shadow
                .blob(&snapshot.tree, "file")
                .await
                .unwrap()
                .unwrap()
                .1,
            b"legacy-readable"
        );
    }

    #[test]
    fn missing_global_config_candidate_is_bound_in_an_isolated_process() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--ignored",
                "--exact",
                "git::config::tests::global_root_child",
            ])
            .env("CYBER_CONFIG_ROOT_FIXTURE", &root)
            .env("GIT_CONFIG_GLOBAL", root.join("global.config"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_COUNT", "0")
            .env_remove("GIT_CONFIG_PARAMETERS")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[tokio::test]
    #[ignore = "subprocess fixture with isolated global configuration candidate"]
    async fn global_root_child() {
        use cyber_server::runtime::{SnapshotRestoreRequest, Snapshots};
        let root = PathBuf::from(std::env::var_os("CYBER_CONFIG_ROOT_FIXTURE").unwrap());
        let global = PathBuf::from(std::env::var_os("GIT_CONFIG_GLOBAL").unwrap());
        assert_eq!(global, root.join("global.config"));
        assert!(!global.exists());
        let work = root.join("work");
        std::fs::create_dir(&work).unwrap();
        assert!(
            std::process::Command::new("git")
                .args(["init", "--quiet"])
                .arg(&work)
                .status()
                .unwrap()
                .success()
        );
        let data = root.join("data");
        let snapshots =
            crate::GitSnapshots::new(data.clone(), std::sync::Arc::new(|_| serde_json::json!({})));
        let directory = work.to_str().unwrap();
        std::fs::write(work.join("file"), "before").unwrap();
        let target = snapshots.track(directory).await.unwrap().unwrap().tree;
        std::fs::write(work.join("file"), "after").unwrap();
        let recorded = snapshots.track(directory).await.unwrap().unwrap().tree;
        let prepared = snapshots
            .shadow(directory)
            .unwrap()
            .prepare_restore(
                &SnapshotRestoreRequest {
                    directory: directory.into(),
                    target,
                    recorded: recorded.clone(),
                    expected: recorded,
                    paths: None,
                },
                1024 * 1024,
                8 * 1024 * 1024,
            )
            .await
            .unwrap();
        let store = crate::copy::Store::open(
            &data,
            directory,
            crate::copy::Options {
                max_file: 1024 * 1024,
                max_total: 8 * 1024 * 1024,
                ignore: vec![],
                private_roots: vec![],
            },
        )
        .unwrap();
        assert_eq!(store.preview_git(&prepared).unwrap().len(), 1);
        std::fs::write(global, "# newly created global config\n").unwrap();
        assert!(store.restore_git_worker(prepared, None).is_err());
        assert_eq!(std::fs::read(work.join("file")).unwrap(), b"after");
        assert!(store.recovery().unwrap().is_empty());
    }
}
