//! Local owner controls; models can request actions but cannot arm a native grant.
use crate::{cli::GlobalArgs, context::Context, error::CliError, output};
use clap::{Args, Subcommand};
use cyber_client::Client;
use serde_json::{Value, json};

#[derive(Debug, Subcommand)]
pub enum ComputerCmd {
    /// Inspect native OS consent and the running binary; optionally a Session's grant.
    Status { session: Option<String> },
    /// Request macOS Screen Recording and Accessibility consent for the running server.
    Setup {
        /// Explicitly allow the operating system permission prompts.
        #[arg(long)]
        yes: bool,
    },
    /// Review and arm one application window or an explicitly selected display.
    Allow(AllowArgs),
    /// Immediately revoke native control; without a Session, revoke all host control.
    Stop { session: Option<String> },
}
#[derive(Debug, Args)]
pub struct AllowArgs {
    pub session: String,
    /// Bundle ID of the application (for example com.apple.TextEdit).
    #[arg(long, required_unless_present = "display", conflicts_with = "display")]
    pub app: Option<String>,
    /// Native display ID; 0 explicitly selects the main display.
    #[arg(long, required_unless_present = "app", conflicts_with = "app")]
    pub display: Option<u32>,
    /// Lifetime in seconds (1–900); default 300.
    #[arg(long, value_parser=clap::value_parser!(u64).range(1..=900))]
    pub ttl: Option<u64>,
    /// Confirm this native scope after reviewing its details printed below.
    #[arg(long)]
    pub yes: bool,
    /// Acknowledge the reviewed unresolved native operations before rearming.
    #[arg(long)]
    pub acknowledge_unknown: bool,
}
pub fn run(command: ComputerCmd, ctx: &Context, global: &GlobalArgs) -> Result<(), CliError> {
    owner_command(&command, std::env::var_os("CYBER_SESSION_ID").is_some())?;
    let rt = super::serve::runtime()?;
    rt.block_on(async {
        let info=super::serve::start(ctx).await?;
        let client=Client::http(&info.registration.url,Some(info.password)).at(&ctx.location.display().to_string());
        let value=match command {
            ComputerCmd::Status {session} => call(&client,"GET",&match session {Some(id)=>format!("{}/computer",session_path(&id)?),None=>"/tools/computer/health".into()},None).await?,
            ComputerCmd::Setup {yes}=>{
                if !yes { return Err(CliError::usage("setup can display macOS permission prompts; rerun with --yes to request consent for the server binary")); }
                call(&client,"POST","/tools/computer/setup",Some(json!({"confirm":true}))).await?
            }
            ComputerCmd::Stop {session}=>call(&client,"POST",&match session {Some(id)=>format!("{}/computer/stop",session_path(&id)?),None=>"/tools/computer/stop".into()},None).await?,
            ComputerCmd::Allow(args)=>{
                let path=format!("{}/computer",session_path(&args.session)?);
                let preview=call(&client,"POST",&format!("{path}/preview"),Some(json!({"app":args.app,"display":args.display}))).await?;
                if !args.yes {
                    output::json(&preview)?;
                    return Err(CliError::usage("review this exact target, then rerun with --yes to arm it; keep this Session attached and use `cyber computer stop` to revoke control"));
                }
                // Review is printed even in noninteractive mode: --yes is an owner
                // confirmation, never a model tool parameter or persisted permission.
                if !output::is_json(global.format) { output::json(&preview)?; }
                call(&client,"POST",&format!("{path}/allow"),Some(json!({"preview_id":preview["data"]["id"],"confirm":true,"ttl_seconds":args.ttl,"acknowledge_unknown":args.acknowledge_unknown}))).await?
            }
        };
        output::json(&value)
    })
}
fn owner_command(command: &ComputerCmd, agent_session: bool) -> Result<(), CliError> {
    if agent_session && matches!(command, ComputerCmd::Setup { .. } | ComputerCmd::Allow(_)) {
        return Err(CliError::usage(
            "native setup and arming require the human owner's terminal or dashboard; agent subprocesses cannot grant themselves native control",
        ));
    }
    Ok(())
}
fn session_path(id: &str) -> Result<String, CliError> {
    if id.is_empty()
        || id.len() > 100
        || !id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
    {
        return Err(CliError::usage("use an exact Session ID"));
    }
    Ok(format!("/sessions/{id}"))
}
async fn call(
    client: &Client,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Result<Value, CliError> {
    let (status, value) = client
        .raw(method, path, body, &[])
        .await
        .map_err(|e| CliError::runtime(e.to_string()))?;
    if (200..300).contains(&status) {
        return Ok(if status == 204 {
            json!({"stopped":true})
        } else {
            value
        });
    }
    let tag = value["_tag"].as_str().unwrap_or("ComputerError");
    let message = value["message"]
        .as_str()
        .unwrap_or("native computer request failed");
    Err(CliError::runtime(super::tagged(tag, message)))
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::{Cli, Command};
    use clap::Parser;
    #[test]
    fn native_target_must_be_explicit_and_unique() {
        for args in [
            vec!["cyber", "computer", "allow", "ses_one"],
            vec![
                "cyber",
                "computer",
                "allow",
                "ses_one",
                "--app",
                "test.fixture",
                "--display",
                "1",
            ],
            vec![
                "cyber",
                "computer",
                "allow",
                "ses_one",
                "--display",
                "1",
                "--ttl",
                "901",
            ],
        ] {
            assert!(Cli::try_parse_from(args).is_err());
        }
        assert!(matches!(
            Cli::try_parse_from(["cyber", "computer", "stop"])
                .unwrap()
                .command,
            Some(Command::Computer {
                cmd: ComputerCmd::Stop { session: None }
            })
        ));
        assert!(session_path("ses_one/path").is_err());
        assert!(owner_command(&ComputerCmd::Setup { yes: true }, true).is_err());
        assert!(owner_command(&ComputerCmd::Stop { session: None }, true).is_ok());
    }
}
