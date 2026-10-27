//! Host-owned restrictions for auto-fix attempts: file tools only, no hooks or clients.
use super::BuiltinHost;

pub fn allowed_tool(name: &str) -> bool {
    matches!(
        name,
        "read" | "write" | "edit" | "apply_patch" | "glob" | "grep" | "list"
    )
}
impl BuiltinHost {
    pub fn home_for_autofix(&self) -> std::path::PathBuf {
        self.opts.home.clone()
    }
    /// Database errors fail closed. The marker is installed before Session creation.
    pub fn is_autofix_session(&self, id: &str) -> bool {
        let id = id.to_owned();
        self.opts
            .store
            .read(move |db| {
                Ok(db.query_row(
                    "SELECT EXISTS(SELECT 1 FROM autofix_session WHERE session_id=?1)",
                    [id],
                    |r| r.get::<_, bool>(0),
                )?)
            })
            .unwrap_or(true)
    }
    pub(crate) fn autofix_rules(
        &self,
        location: &std::path::Path,
    ) -> Vec<crate::permissions::Rule> {
        use crate::permissions::{Effect, Rule};
        let mut out = vec![Rule::new(
            "external_directory",
            "*",
            Effect::Deny,
            "autofix",
        )];
        for path in [
            ".git",
            ".cyber",
            ".github",
            "cyber.json",
            "cyber.jsonc",
            "AGENTS.md",
        ] {
            for resource in [
                path.to_owned(),
                crate::permissions::slash(&location.join(path)),
            ] {
                out.push(Rule::new("edit", &resource, Effect::Deny, "autofix"));
                out.push(Rule::new(
                    "edit",
                    &format!("{resource}/*"),
                    Effect::Deny,
                    "autofix",
                ));
            }
        }
        out
    }
    /// Native file tools execute in the host, so enforce canonical boundaries as well
    /// as permission resources (which are relative inside the Location).
    pub(crate) fn autofix_paths(
        &self,
        session: &str,
        location: &std::path::Path,
        paths: &[std::path::PathBuf],
        writing: bool,
    ) -> Result<(), String> {
        let session = session.to_owned();
        let (expected, verification): (String, String) = self
            .opts
            .store
            .read(move |db| {
                Ok(db.query_row(
                    "SELECT a.directory,json_extract(s.data,'$.config.verification_paths') FROM autofix_session a JOIN autofix_subscription s ON s.id=a.subscription_id WHERE a.session_id=?1",
                    [session],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )?)
            })
            .map_err(|_| "auto-fix checkout authority is unavailable")?;
        let verification: Vec<String> = serde_json::from_str(&verification)
            .map_err(|_| "auto-fix verification authority is unavailable")?;
        let root =
            std::fs::canonicalize(&expected).map_err(|_| "auto-fix checkout is unavailable")?;
        if super::canonical(location) != root {
            return Err("auto-fix invocation changed its checkout".into());
        }
        for path in paths {
            let canonical = super::canonical(path);
            if !canonical.starts_with(&root) {
                return Err("auto-fix cannot access paths outside its checkout".into());
            }
            let lexical = path
                .strip_prefix(location)
                .map_err(|_| "auto-fix path is outside its checkout")?;
            let actual = canonical
                .strip_prefix(&root)
                .map_err(|_| "auto-fix path is outside its checkout")?;
            if protected(lexical, writing)
                || protected(actual, writing)
                || (writing
                    && (verification_path(lexical, &verification)
                        || verification_path(actual, &verification)))
            {
                return Err(
                    "auto-fix cannot access git metadata or edit protected configuration".into(),
                );
            }
        }
        Ok(())
    }
}

fn protected(path: &std::path::Path, writing: bool) -> bool {
    path.components()
        .filter_map(|component| component.as_os_str().to_str())
        .any(|name| {
            // Include Windows path aliases even when tests run on a case-sensitive host.
            let name = name
                .split(':')
                .next()
                .unwrap_or(name)
                .trim_end_matches([' ', '.']);
            name.eq_ignore_ascii_case(".git")
                || (writing
                    && [
                        ".cyber",
                        ".github",
                        "cyber.json",
                        "cyber.jsonc",
                        "AGENTS.md",
                    ]
                    .iter()
                    .any(|p| name.eq_ignore_ascii_case(p)))
        })
}

fn verification_path(path: &std::path::Path, verification: &[String]) -> bool {
    let path = crate::permissions::slash(path).to_ascii_lowercase();
    verification.iter().any(|protected| {
        let protected = protected.to_ascii_lowercase();
        path == protected || path.starts_with(&format!("{protected}/"))
    })
}
