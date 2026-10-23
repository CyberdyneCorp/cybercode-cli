//! `/review`: local target validation and the findings view. With the bundled Workflows
//! available, the review is a run of the `review` Workflow and its findings come from the
//! run's result; otherwise the bundled review skill's reviewer Job reports them.

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
    /// The awaited review run ended: fetch it for its result.
    FetchRun(String),
    Unrelated,
}

/// How a valid `/review` is submitted.
#[derive(Debug, PartialEq)]
pub enum Submit {
    /// As the `review` command: an override (no ID) or the bundled skill, whose reviewer
    /// delegation follows from the client-chosen message ID.
    Command(Option<String>),
    /// As a run of the `review` Workflow.
    Workflow,
}

/// A `review` Workflow Run being awaited.
#[derive(Debug)]
enum AwaitedRun {
    /// Posted; the run ID is not known yet. Runs that ended meanwhile, by ID.
    Starting(Vec<String>),
    Run(String),
}

/// The lifecycle events that end a run.
const RUN_ENDS: [&str; 3] = [
    "workflow.run.completed",
    "workflow.run.failed",
    "workflow.run.stopped",
];

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
    /// The library has a `review` Workflow, so a bundled review runs it.
    workflow: bool,
    /// The submitted bundled review whose reviewer Job has not ended.
    awaited: Option<Awaited>,
    /// The submitted review run that has not ended.
    run: Option<AwaitedRun>,
    /// Jobs that ended while the reviewer Job was still unknown, by ID.
    ended: BTreeMap<String, Value>,
    pub lines: Vec<String>,
    pub scroll: u16,
}

impl View {
    pub fn set_bundled(&mut self, bundled: bool) {
        self.bundled = Some(bundled);
    }

    pub fn set_workflow(&mut self, workflow: bool) {
        self.workflow = workflow;
    }

    /// Validates a bundled review target before anything is posted and says how to
    /// submit it; an overriding command receives its arguments unchanged and is not
    /// awaited.
    pub fn submit(&mut self, session: &str, arguments: &str) -> Result<Submit, String> {
        if self.bundled == Some(false) {
            return Ok(Submit::Command(None));
        }
        Target::parse(arguments).map_err(|error| escape(&error.message))?;
        self.ended.clear();
        if self.workflow {
            self.awaited = None;
            self.run = Some(AwaitedRun::Starting(Vec::new()));
            return Ok(Submit::Workflow);
        }
        let message = cyber_core::ids::new_id("msg");
        self.awaited = Some(Awaited {
            session: session.into(),
            delegation: Runtime::skill_command_delegation_id(session, &message),
            job: None,
        });
        Ok(Submit::Command(Some(message)))
    }

    /// The command or run was not admitted, so nothing will end.
    pub fn refused(&mut self) {
        self.awaited = None;
        self.run = None;
        self.ended.clear();
    }

    /// The review run was admitted as `id`; it may already have ended.
    pub fn run_started(&mut self, id: &str) -> Ended {
        let ended = match self.run.take() {
            Some(AwaitedRun::Starting(ended)) => ended.iter().any(|run| run == id),
            _ => return Ended::Unrelated,
        };
        self.run = Some(AwaitedRun::Run(id.to_owned()));
        if ended {
            Ended::FetchRun(id.to_owned())
        } else {
            Ended::Unrelated
        }
    }

    /// A Workflow Run event: the awaited run's end fetches it.
    pub fn run_event(&mut self, kind: &str, run: &Value) -> Ended {
        if !RUN_ENDS.iter().any(|end| kind.starts_with(end)) {
            return Ended::Unrelated;
        }
        let id = run["id"].as_str().unwrap_or_default();
        match &mut self.run {
            Some(AwaitedRun::Run(awaited)) if awaited == id => Ended::FetchRun(id.to_owned()),
            Some(AwaitedRun::Starting(ended)) if !id.is_empty() => {
                ended.push(id.to_owned());
                Ended::Unrelated
            }
            _ => Ended::Unrelated,
        }
    }

    /// The awaited run, fetched after it ended: show its findings (partial ones too) or
    /// its error.
    pub fn run_settled(&mut self, run: &Value) -> Ended {
        let awaited = matches!(&self.run, Some(AwaitedRun::Run(id)) if run["id"] == id.as_str());
        if !awaited {
            return Ended::Unrelated;
        }
        self.refused();
        let result = &run["result"];
        let ended = matches!(
            run["status"].as_str(),
            Some("completed" | "budget_exceeded")
        );
        if !ended || !result.is_object() {
            let error = run["error"]["message"]
                .as_str()
                .or(run["reason"].as_str())
                .unwrap_or("The review run did not complete");
            return Ended::Failed(escape(error));
        }
        match Findings::from_workflow_result(result) {
            Ok(findings) => {
                self.show(&findings);
                if result["partial"] == true {
                    self.lines
                        .insert(1, "partial: a budget stopped the review".into());
                }
                Ended::Findings
            }
            Err(error) => Ended::Failed(escape(&error.to_string())),
        }
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
