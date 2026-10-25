//! Literal manifest input and owner-reviewed integration actions over the server API.
use super::api;
use crate::{cli::GlobalArgs, context::Context, error::CliError, output};
use clap::Subcommand;
use cyber_client::Client;
use serde_json::{Value, json};
use std::{io::Read, path::Path};

#[derive(Debug, Subcommand)]
pub enum IntegrationCmd {
    /// Pin a JSON manifest from a file (or - for stdin); does not run checks yet.
    Submit {
        file: String,
        #[arg(long)]
        session: Option<String>,
        #[arg(long)]
        budget: Option<String>,
    },
    List,
    Show {
        id: String,
    },
    Report {
        id: String,
    },
    /// Start a fresh immutable attempt; --wait reports final host evidence.
    Run {
        id: String,
        #[arg(long)]
        wait: bool,
        #[arg(long)]
        expected_revision: Option<u64>,
    },
    Cancel {
        id: String,
        #[arg(long)]
        expected_revision: Option<u64>,
    },
    /// Explicitly push the integration branch and create/reconcile its GitHub PR.
    Publish {
        id: String,
        #[arg(long, default_value = "origin")]
        remote: String,
        #[arg(long)]
        repository: String,
        #[arg(long)]
        title: String,
        #[arg(long)]
        body_file: String,
        #[arg(long)]
        operation_id: Option<String>,
        #[arg(long)]
        acknowledge_review_unavailable: bool,
        #[arg(long)]
        expected_revision: Option<u64>,
    },
}
fn read(file: &str) -> Result<String, CliError> {
    let mut bytes = Vec::new();
    if file == "-" {
        std::io::stdin()
            .take(1024 * 1024 + 1)
            .read_to_end(&mut bytes)?;
    } else {
        std::fs::File::open(file)?
            .take(1024 * 1024 + 1)
            .read_to_end(&mut bytes)?;
    }
    if bytes.len() > 1024 * 1024 {
        return Err(CliError::usage("integration input exceeds 1 MiB"));
    }
    String::from_utf8(bytes).map_err(|_| CliError::usage("integration input must be UTF-8"))
}
fn path(id: &str) -> Result<String, CliError> {
    if !id.starts_with("intg_")
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return Err(CliError::usage("expected an intg_ batch ID"));
    }
    Ok(format!("/workflows/integrations/{id}"))
}
async fn current(client: &Client, id: &str) -> Result<Value, CliError> {
    Ok(client.get(&path(id)?).await.map_err(api)?["data"].clone())
}
async fn revision(client: &Client, id: &str, expected: Option<u64>) -> Result<u64, CliError> {
    if let Some(revision) = expected {
        return Ok(revision);
    }
    current(client, id).await?["revision"]
        .as_u64()
        .ok_or_else(|| CliError::runtime("integration revision missing"))
}
fn print(value: &Value, global: &GlobalArgs) -> Result<(), CliError> {
    if output::is_json(global.format) {
        return output::json(value);
    }
    println!(
        "{}",
        serde_json::to_string_pretty(value).map_err(|e| CliError::runtime(e.to_string()))?
    );
    Ok(())
}

pub(super) async fn run(
    command: IntegrationCmd,
    client: &Client,
    _ctx: &Context,
    global: &GlobalArgs,
) -> Result<(), CliError> {
    match command {
        IntegrationCmd::Submit {
            file,
            session,
            budget,
        } => {
            let manifest: cyber_workflow::integration::Manifest =
                serde_json::from_str(&read(&file)?)
                    .map_err(|error| CliError::usage(error.to_string()))?;
            manifest.validate().map_err(CliError::usage)?;
            let session=match session {Some(id)=>id,None=>client.post("/sessions",json!({"title":"Integration queue","model":global.model,"mode":global.mode.clone().unwrap_or_else(||"default".into())})).await.map_err(api)?["data"]["id"].as_str().ok_or_else(||CliError::runtime("server returned no owner Session"))?.to_string()};
            let budget = budget
                .map(|text| {
                    serde_json::from_str::<Value>(&text)
                        .map_err(|_| CliError::usage("--budget must be a JSON Budget object"))
                })
                .transpose()?;
            let batch = client
                .post(
                    "/workflows/integrations",
                    json!({"session_id":session,"manifest":manifest,"budget":budget}),
                )
                .await
                .map_err(api)?;
            print(&batch["data"], global)
        }
        IntegrationCmd::List => print(
            &client.get("/workflows/integrations").await.map_err(api)?["data"],
            global,
        ),
        IntegrationCmd::Show { id } | IntegrationCmd::Report { id } => {
            print(&current(client, &id).await?, global)
        }
        IntegrationCmd::Run {
            id,
            wait,
            expected_revision,
        } => run_attempt(client, &id, wait, expected_revision, global).await,
        IntegrationCmd::Cancel {
            id,
            expected_revision,
        } => {
            let revision = revision(client, &id, expected_revision).await?;
            print(
                &client
                    .post(
                        &format!("{}/cancel", path(&id)?),
                        json!({"expected_revision":revision}),
                    )
                    .await
                    .map_err(api)?["data"],
                global,
            )
        }
        IntegrationCmd::Publish {
            id,
            remote,
            repository,
            title,
            body_file,
            operation_id,
            acknowledge_review_unavailable,
            expected_revision,
        } => {
            let revision = revision(client, &id, expected_revision).await?;
            let operation_id = operation_id.unwrap_or_else(|| format!("cli_{}", ulid::Ulid::new()));
            let result=client.post(&format!("{}/publish",path(&id)?),json!({"expected_revision":revision,"remote":remote,"repository":repository,"title":title,"body":read(Path::new(&body_file).to_str().ok_or_else(||CliError::usage("body path is not UTF-8"))?)?,"operation_id":operation_id,"acknowledge_review_unavailable":acknowledge_review_unavailable})).await.map_err(api)?;
            print(&result["data"], global)
        }
    }
}

async fn run_attempt(
    client: &Client,
    id: &str,
    wait: bool,
    expected_revision: Option<u64>,
    global: &GlobalArgs,
) -> Result<(), CliError> {
    let revision = revision(client, id, expected_revision).await?;
    let batch = client
        .post(
            &format!("{}/run", path(id)?),
            json!({"expected_revision":revision}),
        )
        .await
        .map_err(api)?;
    if !wait {
        return print(&batch["data"], global);
    }
    loop {
        let batch = current(client, id).await?;
        if !matches!(
            batch["status"].as_str(),
            Some("queued" | "preparing" | "checking" | "reviewing")
        ) {
            let ready = batch["status"] == "ready";
            print(&batch, global)?;
            if ready {
                return Ok(());
            }
            return Err(CliError::runtime(
                "integration did not produce a current verified combined patch",
            ));
        }
        tokio::select! { _=tokio::signal::ctrl_c()=>{let revision=batch["revision"].as_u64().unwrap_or_default();let _=client.post(&format!("{}/cancel",path(id)?),json!({"expected_revision":revision})).await;return Err(CliError{code:130,message:"integration wait interrupted; cancellation requested".into(),hint:None});},_=tokio::time::sleep(std::time::Duration::from_millis(250))=>{}}
    }
}
