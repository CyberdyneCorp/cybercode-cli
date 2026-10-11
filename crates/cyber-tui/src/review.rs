//! `/review`: local target validation for the bundled review skill and the
//! findings view for its reviewer Job.

use std::collections::BTreeMap;

use cyber_core::review::{Findings, Target, escape};
use cyber_server::runtime::{Delegation, DelegationStatus, Runtime};
use serde_json::Value;

/// What an ended Job or a delegation record means for the review view.
#[derive(Debug, PartialEq)]
pub enum Ended {
    /// Validated findings are ready to show.
    Findings,
    /// The awaited review failed or returned invalid findings.
    Failed(String),
    /// The awaited review's Job is not known yet: look up its delegation.
    Lookup {
        session: String,
        delegation: String,
    },
    Unrelated,
}

/// A submitted bundled review. The client chooses the command's message ID,
/// so the reviewer fork's delegation ID is known before anything is posted.
#[derive(Debug)]
struct Awaited {
    session: String,
    delegation: String,
    /// The reviewer Job, once the delegation record names it.
    job: Option<String>,
}

#[derive(Debug, Default)]
pub struct View {
    /// `Some(false)` once the command catalogue shows that a user, project or
    /// configured `review` overrides the bundled skill; unknown means bundled.
    bundled: Option<bool>,
    /// The submitted bundled review whose reviewer Job has not ended.
    awaited: Option<Awaited>,
    /// Jobs that ended while the reviewer Job was still unknown, by ID.
    ended: BTreeMap<String, Value>,
    pub lines: Vec<String>,
    pub scroll: u16,
}

impl View {
    pub fn set_bundled(&mut self, bundled: bool) {
        self.bundled = Some(bundled);
    }

    /// Validates a bundled review target before anything is posted and returns
    /// the message ID to post it with; an overriding command receives its
    /// arguments unchanged and is not awaited.
    pub fn submit(&mut self, session: &str, arguments: &str) -> Result<Option<String>, String> {
        if self.bundled == Some(false) {
            return Ok(None);
        }
        Target::parse(arguments).map_err(|error| escape(&error.message))?;
        let message = cyber_core::ids::new_id("msg");
        self.awaited = Some(Awaited {
            session: session.into(),
            delegation: Runtime::skill_command_delegation_id(session, &message),
            job: None,
        });
        self.ended.clear();
        Ok(Some(message))
    }

    /// The command was not admitted, so no reviewer Job will end.
    pub fn refused(&mut self) {
        self.awaited = None;
        self.ended.clear();
    }

    /// Only the awaited review's Job is reported; any other Job is unrelated.
    pub fn job_ended(&mut self, job: &Value) -> Ended {
        let Some(awaited) = &self.awaited else {
            return Ended::Unrelated;
        };
        let id = job["id"].as_str().unwrap_or_default();
        match &awaited.job {
            Some(review) if review == id => self.settle(job),
            None if !id.is_empty() => {
                self.ended.insert(id.into(), job.clone());
                Ended::Lookup {
                    session: awaited.session.clone(),
                    delegation: awaited.delegation.clone(),
                }
            }
            _ => Ended::Unrelated,
        }
    }

    /// An admitted parent input. The server admits `msg_skill_handback_<id>`
    /// when the awaited fork failed or was cancelled before its Job existed,
    /// so no Job ends: look up the delegation to report its error.
    pub fn admitted(&self, message_id: &str) -> Ended {
        match &self.awaited {
            Some(awaited)
                if awaited.job.is_none()
                    && message_id.strip_prefix("msg_skill_handback_")
                        == Some(awaited.delegation.as_str()) =>
            {
                Ended::Lookup {
                    session: awaited.session.clone(),
                    delegation: awaited.delegation.clone(),
                }
            }
            _ => Ended::Unrelated,
        }
    }

    /// The awaited delegation's record: it names the reviewer Job, or reports
    /// that the fork failed before a Job existed.
    pub fn delegation(&mut self, record: &Value) -> Ended {
        let Ok(record) = serde_json::from_value::<Delegation>(record.clone()) else {
            return Ended::Unrelated;
        };
        let Some(awaited) = self
            .awaited
            .as_mut()
            .filter(|awaited| awaited.delegation == record.id)
        else {
            return Ended::Unrelated;
        };
        match (record.job_id, record.status) {
            (Some(job), _) => {
                awaited.job = Some(job.clone());
                match self.ended.remove(&job) {
                    Some(ended) => self.settle(&ended),
                    None => Ended::Unrelated,
                }
            }
            (None, DelegationStatus::Failed | DelegationStatus::Cancelled) => {
                self.refused();
                let error = record
                    .error
                    .as_deref()
                    .unwrap_or("The reviewer did not start");
                Ended::Failed(escape(error))
            }
            _ => Ended::Unrelated,
        }
    }

    /// The reviewer Job ended: show its findings or its error.
    fn settle(&mut self, job: &Value) -> Ended {
        self.refused();
        let Some(result) = job.pointer("/result/result") else {
            let error = match job["status"].as_str() {
                Some("completed") => "The reviewer returned no structured findings",
                _ => job["error"].as_str().unwrap_or("The reviewer failed"),
            };
            return Ended::Failed(escape(error));
        };
        match Findings::from_result(result) {
            Ok(findings) => {
                self.show(&findings);
                Ended::Findings
            }
            Err(error) => Ended::Failed(escape(&error.to_string())),
        }
    }

    fn show(&mut self, findings: &Findings) {
        let counts = findings.counts();
        self.lines = vec![
            format!(
                "critical {} · high {} · medium {} · low {}",
                counts.critical, counts.high, counts.medium, counts.low
            ),
            String::new(),
        ];
        self.lines
            .extend(findings.render_text().lines().map(str::to_string));
        self.scroll = 0;
    }
}
