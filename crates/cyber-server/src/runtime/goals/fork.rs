//! `/goal fork` (`goals` → Parallel goals across sessions): a new root Session pursues a
//! Goal in the background while the source Session continues. With
//! `goals.fork_isolation = "worktree"` (the default) and a git checkout, the new Session
//! runs in its own managed worktree; otherwise it shares the source's Location.

use cyber_core::goals::ForkIsolation;
use serde::Serialize;
use tokio_util::sync::CancellationToken;

use super::records::Goal;
use super::service::GoalRequest;
use crate::runtime::{CreateSession, GoalWorktree, Runtime, RuntimeError};

/// The Session a fork started and its active Goal.
#[derive(Debug, Clone, Serialize, schemars::JsonSchema)]
pub struct GoalFork {
    pub session_id: String,
    pub directory: String,
    /// The worktree branch; absent when the fork shares the source's checkout.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    pub goal: Goal,
}

impl Runtime {
    /// Start a background root Session with `request` as its active Goal.
    pub async fn fork_goal(
        &self,
        session_id: &str,
        request: GoalRequest,
    ) -> Result<GoalFork, RuntimeError> {
        cyber_core::goals::check_condition(&request.condition)
            .map_err(RuntimeError::GoalCondition)?;
        let source = self
            .inner
            .handle(session_id)
            .await?
            .state
            .lock()
            .await
            .info
            .clone();
        let (settings, _) = self.goal_settings(&source.directory)?;
        let new_session = CreateSession {
            id: Some(cyber_core::ids::new_id("ses")),
            directory: source.directory.clone(),
            model: source.model.clone(),
            agent: Some(source.agent.clone()),
            mode: Some(source.mode.clone()),
            title: Some(format!("Goal: {}", title(&request.condition))),
            ..CreateSession::default()
        };
        let isolated = settings.fork_isolation == ForkIsolation::Worktree
            && cyber_core::project::identify(std::path::Path::new(&source.directory)).id
                != "global";
        let (info, branch) = if isolated {
            let worktree = GoalWorktree::new(&cyber_core::ids::new_id("gol"));
            let info = self
                .inner
                .tools
                .create_goal_fork_session(&source, new_session, &worktree, CancellationToken::new())
                .await
                .map_err(RuntimeError::Invalid)?;
            (info, Some(worktree.branch))
        } else {
            (self.create_session(new_session).await?, None)
        };
        let goal = self.set_goal(&info.id, request).await?;
        Ok(GoalFork {
            session_id: info.id,
            directory: info.directory,
            branch,
            goal,
        })
    }
}

/// The first line of a condition, at most 60 characters, for the Session title.
fn title(condition: &str) -> String {
    let first = condition.lines().next().unwrap_or_default().trim();
    if first.chars().count() <= 60 {
        return first.to_owned();
    }
    let mut cut: String = first.chars().take(59).collect();
    cut.push('…');
    cut
}
