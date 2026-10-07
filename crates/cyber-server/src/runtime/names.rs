//! Indexed, parent-scoped child identities; historical background names remain reserved.
use super::{Runtime, RuntimeError};

impl Runtime {
    pub fn subagent_names(&self, parent: &str) -> Result<Vec<String>, RuntimeError> {
        let parent = parent.to_owned();
        self.inner
            .store
            .read(move |db| {
                let mut query = db.prepare(
                "SELECT subagent_name FROM session WHERE parent_id=?1 AND subagent_name IS NOT NULL
                 UNION SELECT name FROM job WHERE session_id=?1 ORDER BY 1",
            )?;
                Ok(query
                    .query_map([parent], |row| row.get(0))?
                    .collect::<rusqlite::Result<Vec<String>>>()?)
            })
            .map_err(Into::into)
    }
}
