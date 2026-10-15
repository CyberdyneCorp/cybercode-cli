//! Remote skill sources in discovery (`skills-commands` → Remote skill sources).
//!
//! An entry from the user's own config (global, environment or command line) is active as
//! soon as it is cached. An entry from any other layer, such as project config, stays
//! inactive until this checkout approves the digest of its URL and cached content, so a
//! changed remote set needs a new approval.
use super::BuiltinHost;
use crate::remote_skills::{Network, RemoteSkillFetcher, RemoteSkills};
use cyber_core::skills::{SkillDiagnostic, SkillOrigin};
use cyber_sandbox::{NetworkMode, SandboxConfig};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Config layers the user controls directly; anything else needs a checkout approval.
fn user_layer(label: Option<&String>) -> bool {
    label.is_some_and(|l| ["global:", "env:", "cli:"].iter().any(|p| l.starts_with(p)))
}

impl BuiltinHost {
    /// Serve `https://` skill entries from `cache_root` (`<cache>/skills`), refreshed in the
    /// background through `fetcher`.
    pub fn attach_remote_skills(
        &self,
        cache_root: PathBuf,
        fetcher: Arc<dyn RemoteSkillFetcher>,
    ) -> Result<(), String> {
        self.remote_skills
            .set(RemoteSkills::new(cache_root, fetcher))
            .map_err(|_| "Remote skill sources already attached".into())
    }

    /// The cached skill directories one remote entry contributes, and its notes.
    pub(super) fn remote_skill_dirs(
        &self,
        location: &Path,
        config: &Value,
        sources: &BTreeMap<String, String>,
        url: &str,
        layer: Option<&String>,
    ) -> (Vec<PathBuf>, Vec<SkillDiagnostic>) {
        let note = |skill: Option<String>, message: String| SkillDiagnostic {
            path: url.to_string(),
            origin: SkillOrigin::Config,
            replaces: skill,
            message: format!("remote skill source: {message}"),
        };
        let Some(remote) = self.remote_skills.get() else {
            let message = "remote skill sources are unavailable in this host".to_string();
            return (Vec::new(), vec![note(None, message)]);
        };
        let user = user_layer(layer);
        // The user's own sources use the network like an unsandboxed integration; any
        // other source is held to the sandbox network mode and allowlist.
        let network = if user {
            Network {
                mode: NetworkMode::On,
                allowed_domains: Vec::new(),
            }
        } else {
            let sandbox = SandboxConfig::resolve(config, sources, None, &self.opts.home);
            Network {
                mode: sandbox.network,
                allowed_domains: sandbox.allowed_domains,
            }
        };
        let resolved = remote.resolve(url, network);
        let mut notes: Vec<SkillDiagnostic> = resolved
            .notes
            .into_iter()
            .map(|(skill, message)| note(skill, message))
            .collect();
        let Some(approval) = resolved.approval.filter(|_| !resolved.dirs.is_empty()) else {
            return (Vec::new(), notes);
        };
        if !user && !self.remote_source_trusted(location, &approval) {
            let message = format!(
                "configured outside your user config and not trusted for this checkout; \
                 review it and run `cyber skills trust {url}` to approve {approval}"
            );
            // Notes about skills that will not load would attach to unrelated entries.
            notes.retain(|n| n.replaces.is_none());
            notes.push(note(None, message));
            return (Vec::new(), notes);
        }
        (resolved.dirs, notes)
    }

    fn remote_source_trusted(&self, location: &Path, approval: &str) -> bool {
        let Some(config) = self.hook_config.get() else {
            return false;
        };
        let root = cyber_core::config::project_root(location);
        config
            .trust
            .is_remote_skill_approved(&root, approval)
            .unwrap_or(false)
    }

    /// Approve the content currently cached for a remote skill source that is configured
    /// outside the user's own config. Returns the approved digest.
    pub fn trust_remote_skill_source(&self, location: &Path, url: &str) -> Result<String, String> {
        let (config, sources) = (self.opts.config)(location)?;
        let settings = &config["skills"];
        let (entries, pointer) = if settings.is_array() {
            (settings, "/skills")
        } else {
            (&settings["paths"], "/skills/paths")
        };
        let index = entries
            .as_array()
            .into_iter()
            .flatten()
            .position(|entry| entry.as_str() == Some(url))
            .ok_or_else(|| format!("{url} is not a configured skill source"))?;
        if user_layer(sources.get(&format!("{pointer}/{index}"))) {
            return Err(format!(
                "{url} comes from your user config and needs no approval"
            ));
        }
        let remote = self
            .remote_skills
            .get()
            .ok_or("Remote skill sources are unavailable")?;
        let approval = remote.cached_approval_digest(url)?;
        let trust = self
            .hook_config
            .get()
            .ok_or("Trust storage is unavailable")?;
        let root = cyber_core::config::project_root(location);
        trust
            .trust
            .approve_remote_skill(&root, &approval)
            .map_err(|error| error.to_string())?;
        Ok(approval)
    }
}
