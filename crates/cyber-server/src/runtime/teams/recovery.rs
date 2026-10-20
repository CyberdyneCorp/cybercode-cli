//! Startup reconciliation of teams (`agent-teams` → Shutdown and resume, Idle
//! notifications; design section 12). After a restart:
//!
//! - teammates a restart cut off are recorded idle, and idle notices it left unsent are
//!   admitted to their leads (`recover_team_idle`);
//! - teams still `stopping` finish stopping, since no teammate's Drain survived;
//! - idle watches need no arming: they are the Drain ends of the teammates of every
//!   `active` team, which the gate and the idle notices read from the team's status.

use serde::Serialize;

use crate::runtime::{Runtime, RuntimeError};

/// What startup reconciliation did.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct TeamRecovery {
    /// Teammates still recorded `working` or `waiting_input` that were checked.
    pub members: usize,
    /// Teams that were `stopping` and are now `stopped`.
    pub stopped: Vec<String>,
}

impl Runtime {
    /// Reconcile teams at startup. Must run after `recover_watches`.
    pub async fn recover_teams(&self) -> Result<TeamRecovery, RuntimeError> {
        let members = self.recover_team_idle().await?;
        let stopping = self.inner.store.read(|db| {
            let mut statement =
                db.prepare("SELECT id FROM team WHERE status = 'stopping' ORDER BY created_ms")?;
            let ids = statement
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(ids)
        })?;
        let mut stopped = Vec::new();
        for team_id in stopping {
            match self.finish_team_stop(&team_id).await {
                Ok(true) => stopped.push(team_id),
                Ok(false) => {}
                Err(error) => cyber_core::log::warn(
                    "teams",
                    "team stop recovery failed",
                    serde_json::json!({ "team_id": team_id, "error": error.to_string() }),
                ),
            }
        }
        Ok(TeamRecovery { members, stopped })
    }
}
