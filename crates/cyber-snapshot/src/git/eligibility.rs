//! Capture size classification from frozen user index and resolved config inputs.
use super::{Shadow, config::Inventory, user_index::UserIndex};
use crate::copy::config_inputs::ConfigInputs;
use crate::git::prepared::CaptureSettings;
use std::path::Path;

struct Context {
    inputs: ConfigInputs,
    index: UserIndex,
    user: Shadow,
    config: Inventory,
    rules: crate::copy::git_rules::RuleProof,
    global: Option<std::path::PathBuf>,
}

impl Shadow {
    async fn classification_context(&self) -> Result<Context, String> {
        let index = self.user_index().await?;
        let mut user = self.clone();
        user.git_dir = index.directory.clone();
        let (mut inputs, config) = user.user_config_inputs().await?;
        inputs.merge(index.inputs.clone())?;
        let global = user_global(&user, &config).await?;
        let paths = global
            .iter()
            .cloned()
            .chain(std::iter::once(user.common_dir.join("info/exclude")))
            .collect();
        inputs.merge(ConfigInputs::capture_files(paths)?)?;
        let raw = config
            .selected("core.ignorecase")?
            .map_or("false", |value| value.unwrap_or("true"));
        let case_insensitive = user.config_boolean("core.ignorecase", raw).await?;
        let policy = crate::git::prepared::IgnoreInputs {
            config: inputs.clone(),
            info: user.common_dir.join("info/exclude"),
            global: global.clone(),
            case_insensitive,
            private_roots: self.private_roots.clone(),
            settings: CaptureSettings {
                ignore: vec![],
                max_untracked: u64::MAX,
            },
            size_excluded: vec![],
            classification_rules: None,
        };
        let prefix = if self.pathspec == "." {
            ""
        } else {
            &self.pathspec
        };
        let worktree = self.worktree.clone();
        let prefix = prefix.to_owned();
        let rules = tokio::task::spawn_blocking(move || {
            crate::copy::git_rules::classification_rules(&worktree, &prefix, &policy)
        })
        .await
        .map_err(|error| error.to_string())??;
        user.verify_config_inputs(&inputs, &config).await?;
        Ok(Context {
            inputs,
            index,
            user,
            config,
            rules,
            global,
        })
    }

    pub(super) async fn write_capture_excludes(
        &self,
        ignore: &[String],
        max_untracked: u64,
    ) -> Result<(), String> {
        let classified = self.classify_untracked(max_untracked).await;
        match classified {
            Ok(classified) => {
                classified.rules.verify_collection()?;
                crate::copy::generated_rules::write(
                    &self.git_dir,
                    &self.common_dir,
                    &CaptureSettings {
                        ignore: ignore.to_vec(),
                        max_untracked,
                    },
                    &classified.skipped,
                    Some((&classified.fingerprint()?, &classified.inputs)),
                )?;
                classified.rules.verify_collection()
            }
            Err(_) => {
                let skipped = self.large_untracked(max_untracked).await;
                crate::copy::generated_rules::write(
                    &self.git_dir,
                    &self.common_dir,
                    &CaptureSettings {
                        ignore: ignore.to_vec(),
                        max_untracked,
                    },
                    &skipped,
                    None,
                )
            }
        }
    }

    pub(super) async fn classify_untracked(
        &self,
        max_untracked: u64,
    ) -> Result<Classified, String> {
        let context = self.classification_context().await?;
        // Git refreshes shared-index timestamps while reading. Only disposable
        // copies may be passed to its index reader, including split dependencies.
        let temporary = tempfile::Builder::new()
            .prefix(".eligibility-")
            .tempdir_in(&self.git_dir)
            .map_err(|error| error.to_string())?;
        let directory = temporary.path().to_path_buf();
        let (context, copies) = tokio::task::spawn_blocking(move || {
            let copies = context.materialize(&directory)?;
            Ok::<_, String>((context, copies))
        })
        .await
        .map_err(|error| error.to_string())??;
        let global_argument = format!(
            "core.excludesfile={}",
            temporary.path().join("global-ignore").display()
        );
        let mut command = self.command(&[
            "-c",
            &global_argument,
            "-c",
            "core.untrackedcache=false",
            "ls-files",
            "--others",
            "--exclude-standard",
            "-z",
            "--",
            &self.pathspec,
        ]);
        command
            .env("GIT_DIR", temporary.path())
            .env("GIT_OPTIONAL_LOCKS", "0")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_COUNT", "0")
            .env_remove("GIT_CONFIG_PARAMETERS")
            .env_remove("GIT_CONFIG");
        let bytes =
            Self::bounded_command(command, 8 * 1024 * 1024, std::time::Duration::from_secs(10))
                .await?;
        context
            .user
            .verify_config_inputs(&context.inputs, &context.config)
            .await?;
        let worktree = self.worktree.clone();
        let private_roots = self.private_roots.clone();
        tokio::task::spawn_blocking(move || {
            let skipped = size_candidates(&worktree, &private_roots, &bytes, max_untracked)?;
            context.inputs.verify()?;
            copies.verify()?;
            context.rules.verify_collection()?;
            Ok(Classified {
                skipped,
                inputs: context.inputs,
                rules: context.rules,
            })
        })
        .await
        .map_err(|error| error.to_string())?
    }
}

pub(super) struct Classified {
    pub skipped: Vec<String>,
    pub inputs: ConfigInputs,
    pub rules: crate::copy::git_rules::RuleProof,
}
impl Classified {
    pub(super) fn fingerprint(&self) -> Result<String, String> {
        let bytes = serde_json::to_vec(&(
            "git-size-classification-v1",
            self.inputs.fingerprint()?,
            &self.rules,
        ))
        .map_err(|error| error.to_string())?;
        Ok(blake3::hash(&bytes).to_hex().to_string())
    }
}

async fn user_global(
    user: &Shadow,
    config: &Inventory,
) -> Result<Option<std::path::PathBuf>, String> {
    let default = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|value| !value.is_empty())
        .map(|path| std::path::PathBuf::from(path).join("git/ignore"))
        .unwrap_or_else(|| std::path::PathBuf::from("~/.config/git/ignore"));
    let raw = match config.selected("core.excludesfile")? {
        Some(value) => value.ok_or("Git classification ignore path lacks value")?,
        None => default
            .to_str()
            .ok_or("Non-UTF-8 classification ignore path")?,
    };
    let path = user
        .expand_config_paths(&[raw])
        .await?
        .pop()
        .ok_or("Missing classification ignore path")?;
    Ok((!path.as_os_str().is_empty()).then(|| user.worktree.join(path)))
}

impl Context {
    fn materialize(&self, directory: &Path) -> Result<PrivateCopies, String> {
        for child in ["objects/info", "refs"] {
            std::fs::create_dir_all(directory.join(child)).map_err(|error| error.to_string())?;
        }
        // Includes have already been resolved in the user's real branch/gitdir
        // context. Store values privately rather than placing secrets in argv.
        private_write(directory.join("config"), self.config.frozen_config()?)
            .map_err(|error| error.to_string())?;
        private_write(
            directory.join("global-ignore"),
            self.global
                .as_ref()
                .map(|path| self.inputs.bytes(path))
                .transpose()?
                .unwrap_or_default(),
        )
        .map_err(|error| error.to_string())?;
        std::fs::create_dir(directory.join("info")).map_err(|error| error.to_string())?;
        private_write(
            directory.join("info/exclude"),
            self.inputs
                .bytes(&self.user.common_dir.join("info/exclude"))?,
        )
        .map_err(|error| error.to_string())?;
        let head = self.inputs.bytes(&self.index.directory.join("HEAD"))?;
        private_write(directory.join("HEAD"), head).map_err(|error| error.to_string())?;
        for path in &self.index.files {
            if let Some(bytes) = self.inputs.optional_bytes(path)? {
                let name = path.file_name().ok_or("Missing frozen index filename")?;
                private_write(directory.join(name), bytes).map_err(|error| error.to_string())?;
            }
        }
        let objects = self.user.common_dir.join("objects");
        let path = objects.to_str().ok_or("Non-UTF-8 Git object directory")?;
        if path.chars().any(char::is_control) {
            return Err("Unsupported frozen Git object directory".into());
        }
        let alternate = serde_json::to_string(path).map_err(|error| error.to_string())?;
        private_write(
            directory.join("objects/info/alternates"),
            format!("{alternate}\n"),
        )
        .map_err(|error| error.to_string())?;
        let mut paths = [
            "config",
            "global-ignore",
            "info/exclude",
            "HEAD",
            "objects/info/alternates",
        ]
        .map(|name| directory.join(name))
        .to_vec();
        for source in &self.index.files {
            if self.inputs.optional_bytes(source)?.is_some() {
                paths.push(
                    directory.join(source.file_name().ok_or("Missing copied index filename")?),
                );
            }
        }
        let mut layout = paths
            .iter()
            .map(|path| {
                Ok((
                    path.strip_prefix(directory)
                        .map_err(|error| error.to_string())?
                        .to_path_buf(),
                    false,
                ))
            })
            .collect::<Result<std::collections::BTreeMap<_, _>, String>>()?;
        for child in ["objects", "objects/info", "refs", "info"] {
            layout.insert(child.into(), true);
        }
        let copies = PrivateCopies {
            inputs: ConfigInputs::capture_files(paths)?,
            root: directory.into(),
            layout,
        };
        self.inputs.verify()?;
        copies.verify()?;
        Ok(copies)
    }
}

struct PrivateCopies {
    inputs: ConfigInputs,
    root: std::path::PathBuf,
    layout: std::collections::BTreeMap<std::path::PathBuf, bool>,
}
impl PrivateCopies {
    fn verify(&self) -> Result<(), String> {
        use cap_fs_ext::DirExt;
        self.inputs.verify_private_index_copies()?;
        let root = cap_std::fs::Dir::open_ambient_dir(&self.root, cap_std::ambient_authority())
            .map_err(|error| error.to_string())?;
        let mut pending = vec![(root, std::path::PathBuf::new())];
        let mut found = 0;
        while let Some((directory, prefix)) = pending.pop() {
            for entry in directory.entries().map_err(|error| error.to_string())? {
                let entry = entry.map_err(|error| error.to_string())?;
                let name = entry.file_name();
                let path = prefix.join(&name);
                let kind = entry.file_type().map_err(|error| error.to_string())?;
                if self.layout.get(&path) != Some(&kind.is_dir())
                    || (!kind.is_dir() && !kind.is_file())
                {
                    return Err("Private Git classifier namespace changed".into());
                }
                found += 1;
                if kind.is_dir() {
                    pending.push((
                        directory
                            .open_dir_nofollow(name)
                            .map_err(|error| error.to_string())?,
                        path,
                    ));
                }
            }
        }
        if found != self.layout.len() {
            return Err("Private Git classifier namespace is incomplete".into());
        }
        Ok(())
    }
}

fn private_write(path: impl AsRef<Path>, bytes: impl AsRef<[u8]>) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?
        .write_all(bytes.as_ref())
}

fn size_candidates(
    worktree: &Path,
    private_roots: &[std::path::PathBuf],
    bytes: &[u8],
    limit: u64,
) -> Result<Vec<String>, String> {
    if !bytes.is_empty() && bytes.last() != Some(&0) {
        return Err("Unterminated Git untracked listing".into());
    }
    let mut skipped = std::collections::BTreeSet::new();
    for (count, path) in bytes
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .enumerate()
    {
        if count >= 65536 {
            return Err("Git untracked listing exceeds entry limit".into());
        }
        let path = std::str::from_utf8(path).map_err(|_| "Non-UTF-8 Git untracked path")?;
        super::images::validate_path(path)?;
        if private_roots
            .iter()
            .any(|root| worktree.join(path).starts_with(root))
            || Path::new(path).components().any(|part| {
                matches!(
                    part.as_os_str().to_str(),
                    Some(".cyber" | ".claude" | ".git")
                )
            })
        {
            continue;
        }
        let metadata =
            std::fs::symlink_metadata(worktree.join(path)).map_err(|error| error.to_string())?;
        if metadata.is_file() && metadata.len() > limit {
            skipped.insert(path.to_owned());
        }
        if skipped.len() > 4096 {
            return Err("Git size exclusion count exceeds limit".into());
        }
    }
    Ok(skipped.into_iter().collect())
}
