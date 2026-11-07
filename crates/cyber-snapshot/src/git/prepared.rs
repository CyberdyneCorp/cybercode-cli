//! Immutable Git objects handed to the synchronous native journal worker.
use super::{Shadow, history, images};
use cyber_server::runtime::SnapshotRestoreRequest;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Image {
    pub source: ImageSource,
    pub mode: String,
    pub object: String,
    pub bytes: Vec<u8>,
}

impl Image {
    pub(crate) fn verify(&self) -> Result<(), String> {
        match self.source {
            ImageSource::Git => images::verify(&self.object, &self.bytes),
            ImageSource::Copy if self.object == crate::copy::object_digest(&self.bytes) => Ok(()),
            ImageSource::Copy => Err("Copy image does not match its object identity".into()),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ImageSource {
    Git,
    Copy,
}

impl ImageSource {
    pub(crate) fn for_id(id: &str) -> Result<Self, String> {
        if let Some(digest) = id.strip_prefix("copy:") {
            if digest.len() == 64
                && digest
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            {
                return Ok(Self::Copy);
            }
        } else if matches!(id.len(), 40 | 64)
            && id
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Ok(Self::Git);
        }
        Err("Invalid snapshot image source identity".into())
    }
}

#[derive(Clone)]
pub(crate) struct PreparedTree {
    pub id: String,
    /// Paths relative to the exact Location, rather than the Git worktree root.
    pub images: BTreeMap<String, Image>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CaptureSettings {
    pub ignore: Vec<String>,
    pub max_untracked: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub(crate) struct IgnoreInputs {
    pub config: crate::copy::config_inputs::ConfigInputs,
    pub info: PathBuf,
    pub global: Option<PathBuf>,
    pub case_insensitive: bool,
    pub private_roots: Vec<PathBuf>,
    pub settings: CaptureSettings,
}

pub(crate) struct PreparedRestore {
    pub directory: PathBuf,
    pub worktree: PathBuf,
    pub prefix: String,
    pub target: PreparedTree,
    pub recorded: PreparedTree,
    pub expected: PreparedTree,
    pub exclusions: Vec<String>,
    /// The expected snapshot's exact worktree-relative exclusions, not globs.
    /// Historical exclusions constrain mutation, but not the current-state check.
    pub expected_excluded: Vec<String>,
    pub historical_excluded: Vec<String>,
    pub allowed: Option<BTreeSet<String>>,
    pub ignore: IgnoreInputs,
    pub expected_policy: Option<String>,
}

impl Shadow {
    pub(crate) async fn ignore_inputs(&self) -> Result<IgnoreInputs, String> {
        self.ignore_inputs_for(&self.capture_settings).await
    }

    pub(super) async fn ignore_inputs_for(
        &self,
        settings: &CaptureSettings,
    ) -> Result<IgnoreInputs, String> {
        let (mut config, origins) = self.config_inputs().await?;
        let default_global = std::env::var_os("XDG_CONFIG_HOME")
            .filter(|value| !value.is_empty())
            .map(|path| PathBuf::from(path).join("git/ignore"))
            .unwrap_or_else(|| PathBuf::from("~/.config/git/ignore"));
        let raw = match origins.selected("core.excludesfile")? {
            Some(value) => value.ok_or("Git ignore path lacks a value")?,
            None => default_global
                .to_str()
                .ok_or("Invalid global ignore path")?,
        };
        let global = self
            .expand_config_paths(&[raw])
            .await?
            .pop()
            .ok_or("Missing Git ignore expansion")?;
        let global = (!global.as_os_str().is_empty()).then(|| self.worktree.join(global));
        let raw = origins
            .selected("core.ignorecase")?
            .map_or("false", |value| value.unwrap_or("true"));
        let ignore_case = self.config_boolean("core.ignorecase", raw).await?;
        self.verify_config_inputs(&config, &origins).await?;
        config.merge(crate::copy::generated_rules::observe(
            &self.git_dir,
            &self.common_dir,
            settings,
        )?)?;
        Ok(IgnoreInputs {
            config,
            info: self.git_dir.join("info/exclude"),
            global,
            case_insensitive: ignore_case,
            private_roots: self.private_roots.clone(),
            settings: settings.clone(),
        })
    }

    /// No source reads, Git index writes or merge subprocesses occur here.
    pub(crate) async fn prepare_restore(
        &self,
        request: &SnapshotRestoreRequest,
        max_file: u64,
        max_total: u64,
    ) -> Result<PreparedRestore, String> {
        let ignore = self.ignore_inputs().await?;
        let expected_policy = self.snapshot_policy(&request.expected).await?;
        let prefix = if self.pathspec == "." {
            ""
        } else {
            &self.pathspec
        };
        let directory = self.worktree.join(prefix);
        if directory != std::path::Path::new(&request.directory) {
            return Err("Git image handoff belongs to another Location".into());
        }
        let ids = [&request.target, &request.recorded, &request.expected];
        if ImageSource::for_id(&request.expected)? != ImageSource::Git {
            return Err("Mixed native restore requires a fresh Git expected checkpoint".into());
        }
        let (excluded, exclusions) = self.git_image_exclusions(ids).await?;
        let expected_excluded = self
            .recorded_exclusions(&request.expected)
            .await?
            .unwrap_or_default();
        let allowed = request
            .paths
            .as_ref()
            .map(|paths| {
                paths
                    .iter()
                    .map(|path| relative(prefix, path))
                    .collect::<Result<_, _>>()
            })
            .transpose()?;
        let mut remaining = max_total;
        let mut trees = Vec::new();
        for (index, id) in ids.into_iter().enumerate() {
            let mut entries = BTreeMap::new();
            if ImageSource::for_id(id)? == ImageSource::Copy {
                trees.push(PreparedTree {
                    id: id.clone(),
                    images: entries,
                });
                continue;
            }
            for (path, (mode, object)) in self.native_image_catalog(id).await?.iter() {
                let omitted = if index == 2 {
                    history::excludes(&expected_excluded, path)
                } else {
                    history::excludes(&excluded, path)
                };
                if omitted {
                    continue;
                }
                if !matches!(mode.as_str(), "100644" | "100755" | "120000") {
                    return Err("Unsupported Git journal image kind".into());
                }
                if entries.len() >= 4096 {
                    return Err("Git journal image count exceeds limit".into());
                }
                let limit = usize::try_from(max_file.min(remaining).min(64 * 1024 * 1024))
                    .map_err(|_| "Git journal image limit overflow")?;
                let bytes = self
                    .run_bounded(&["cat-file", "blob", object], limit)
                    .await?;
                images::verify(object, &bytes)?;
                remaining = remaining
                    .checked_sub(bytes.len() as u64)
                    .ok_or("Git journal image handoff exceeds limit")?;
                entries.insert(
                    relative(prefix, path)?,
                    Image {
                        source: ImageSource::Git,
                        mode: mode.clone(),
                        object: object.clone(),
                        bytes,
                    },
                );
            }
            trees.push(PreparedTree {
                id: id.clone(),
                images: entries,
            });
        }
        let mut trees = trees.into_iter();
        Ok(PreparedRestore {
            directory,
            worktree: self.worktree.clone(),
            prefix: prefix.to_owned(),
            target: trees.next().ok_or("Missing target images")?,
            recorded: trees.next().ok_or("Missing recorded images")?,
            expected: trees.next().ok_or("Missing expected images")?,
            exclusions,
            expected_excluded,
            historical_excluded: excluded.into_iter().collect(),
            allowed,
            ignore,
            expected_policy,
        })
    }

    async fn git_image_exclusions(
        &self,
        ids: [&String; 3],
    ) -> Result<(BTreeSet<String>, Vec<String>), String> {
        let mut excluded = BTreeSet::new();
        let mut git_ids = vec![];
        for id in ids {
            if ImageSource::for_id(id)? == ImageSource::Copy {
                continue;
            }
            git_ids.push(id.clone());
            excluded.extend(self.recorded_exclusions(id).await?.unwrap_or_default());
        }
        Ok((excluded, self.historical_exclusions(&git_ids).await?))
    }
}

fn relative(prefix: &str, path: &str) -> Result<String, String> {
    images::validate_path(path)?;
    let path = if prefix.is_empty() {
        path
    } else {
        path.strip_prefix(prefix)
            .and_then(|path| path.strip_prefix('/'))
            .ok_or("Git image path is outside the Location")?
    };
    images::validate_path(path)?;
    Ok(path.to_owned())
}
