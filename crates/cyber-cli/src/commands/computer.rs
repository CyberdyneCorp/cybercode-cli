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
    /// Review and arm one application window or an explicitly selected display, or
    /// confirm a pending review by its identifier.
    Allow(AllowArgs),
    /// Immediately revoke native control; without a Session, revoke all host control.
    Stop { session: Option<String> },
}
#[derive(Debug, Args)]
#[command(group(clap::ArgGroup::new("target").required(true).args(["app", "display", "window", "pick", "preview"])))]
pub struct AllowArgs {
    pub session: String,
    /// Bundle ID of the application (for example com.apple.TextEdit).
    #[arg(long)]
    pub app: Option<String>,
    /// Native display ID; 0 explicitly selects the main display.
    #[arg(long)]
    pub display: Option<u32>,
    /// Candidate token from an owner-only window listing (platforms that offer one).
    #[arg(long)]
    pub window: Option<String>,
    /// Let the platform picker choose a window or output (platforms that offer one).
    #[arg(long, value_parser = ["window", "output"])]
    pub pick: Option<String>,
    /// Confirm this pending review by its identifier; starts no new review or picker.
    #[arg(long, requires = "yes")]
    pub preview: Option<String>,
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
                if let Some(id)=&args.preview {
                    call(&client,"POST",&format!("{path}/allow"),Some(allow_body(id,&args))).await?
                } else {
                let preview=call(&client,"POST",&format!("{path}/preview"),Some(selector_body(&args))).await?;
                if !args.yes {
                    output::json(&preview)?;
                    return Err(CliError::usage("review this exact target, then rerun with --yes to arm it; keep this Session attached and use `cyber computer stop` to revoke control"));
                }
                // Review is printed even in noninteractive mode: --yes is an owner
                // confirmation, never a model tool parameter or persisted permission.
                if !output::is_json(global.format) { output::json(&preview)?; }
                call(&client,"POST",&format!("{path}/allow"),Some(allow_body(preview["data"]["id"].as_str().unwrap_or_default(),&args))).await?
                }
            }
        };
        output::json(&value)
    })
}
fn selector_body(args: &AllowArgs) -> Value {
    // macOS selectors keep their exact request shape.
    let mut body = json!({"app":args.app,"display":args.display});
    if let Some(window) = &args.window {
        body["window"] = json!(window);
    }
    if let Some(pick) = &args.pick {
        body["pick"] = json!(pick);
    }
    body
}
fn allow_body(preview: &str, args: &AllowArgs) -> Value {
    json!({"preview_id":preview,"confirm":true,"ttl_seconds":args.ttl,"acknowledge_unknown":args.acknowledge_unknown})
}
fn owner_command(command: &ComputerCmd, agent_session: bool) -> Result<(), CliError> {
    // A picked source or candidate token cannot be re-resolved: the owner must see
    // the review first, then confirm it with --preview <id> --yes.
    if let ComputerCmd::Allow(args) = command
        && args.yes
        && args.preview.is_none()
        && (args.window.is_some() || args.pick.is_some())
    {
        return Err(CliError::usage(
            "interactive_review_required: review a picked or listed target first, then run `cyber computer allow <session> --preview <id> --yes`",
        ));
    }
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
    let message = match value["reason"].as_str() {
        Some(reason) if !message.contains(reason) => format!("{message} [{reason}]"),
        _ => message.to_owned(),
    };
    Err(CliError::runtime(super::tagged(tag, &message)))
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
    fn allow(args: &[&str]) -> Result<AllowArgs, clap::Error> {
        let mut argv = vec!["cyber", "computer", "allow", "ses_one"];
        argv.extend_from_slice(args);
        match Cli::try_parse_from(argv)?.command {
            Some(Command::Computer {
                cmd: ComputerCmd::Allow(args),
            }) => Ok(args),
            other => panic!("unexpected {other:?}"),
        }
    }
    #[test]
    fn preview_confirmation_and_non_re_resolvable_selectors_need_explicit_review() {
        let args = allow(&["--preview", "cup_one", "--yes"]).unwrap();
        assert_eq!(args.preview.as_deref(), Some("cup_one"));
        assert_eq!(
            allow_body("cup_one", &args),
            json!({"preview_id":"cup_one","confirm":true,"ttl_seconds":null,"acknowledge_unknown":false})
        );
        assert!(
            allow(&["--preview", "cup_one"]).is_err(),
            "--preview needs --yes"
        );
        assert!(allow(&["--preview", "cup_one", "--app", "x", "--yes"]).is_err());
        assert!(allow(&["--pick", "screen"]).is_err());
        for selector in [vec!["--pick", "window"], vec!["--window", "cuc_token"]] {
            let mut confirmed = selector.clone();
            confirmed.push("--yes");
            let error = owner_command(&ComputerCmd::Allow(allow(&confirmed).unwrap()), false)
                .unwrap_err()
                .message;
            assert!(error.contains("interactive_review_required"), "{error}");
            assert!(owner_command(&ComputerCmd::Allow(allow(&selector).unwrap()), false).is_ok());
        }
        let app = allow(&["--app", "com.apple.TextEdit", "--yes"]).unwrap();
        assert!(owner_command(&ComputerCmd::Allow(app), false).is_ok());
        let app = allow(&["--app", "com.apple.TextEdit"]).unwrap();
        assert_eq!(
            selector_body(&app),
            json!({"app":"com.apple.TextEdit","display":null}),
            "macOS request shape is unchanged"
        );
        let pick = allow(&["--pick", "output"]).unwrap();
        assert_eq!(selector_body(&pick)["pick"], "output");
    }
}
