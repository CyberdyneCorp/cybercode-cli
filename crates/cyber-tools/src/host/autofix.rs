//! Host-owned restrictions for auto-fix attempts: file tools only, no hooks or clients.
use super::BuiltinHost;

pub fn allowed_tool(name: &str) -> bool {
    matches!(
        name,
        "read" | "write" | "edit" | "apply_patch" | "glob" | "grep" | "ls"
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
            let resource = format!("{}*", crate::permissions::slash(&location.join(path)));
            out.push(Rule::new("edit", &resource, Effect::Deny, "autofix"));
        }
        out
    }
}
