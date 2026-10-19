//! `cyber [project]`: the terminal UI (`tui` → Launch and server transport, Launch flags).

use std::io::{IsTerminal, Read};

use cyber_app::{App, AppOptions};
use cyber_client::Client;
use cyber_tui::{Start, TuiOptions};

use crate::cli::{GlobalArgs, LaunchArgs};
use crate::context::Context;
use crate::error::CliError;

pub fn run(
    launch: &LaunchArgs,
    global: &GlobalArgs,
    project: Option<&str>,
) -> Result<(), CliError> {
    let mut mode = global.mode.clone();
    if launch.dangerously_skip_permissions {
        super::exec::setup::skip_permissions(mode.as_deref(), launch.embedded, "--embedded")?;
        mode = Some("bypass".into());
    }
    let mut start = start(launch)?;
    let mut global = GlobalArgs {
        cwd: global.cwd.clone(),
        profile: global.profile.clone(),
        config: global.config.clone(),
        model: global.model.clone(),
        mode,
        format: global.format,
        log_level: global.log_level.clone(),
        print_logs: global.print_logs,
        pure: global.pure,
    };
    if let Some(p) = project {
        global.cwd = Some(global.cwd.clone().unwrap_or_default().join(p));
    }
    let ctx = Context::new(&global)?;
    super::start_logging(&ctx, &global)?;
    if let Some(pr) = &launch.from_pr {
        let number = super::pr::parse(pr)?;
        if crate::output::is_json(global.format) {
            return super::pr::report_linked(&ctx, number);
        }
        start = cyber_tui::Start::PickPr(number);
    }
    let prompt = prompt(launch)?;
    if !std::io::stdout().is_terminal() {
        return Err(
            CliError::usage("the TUI needs a terminal").with_hint("use \"cyber exec\" for scripts")
        );
    }
    let summary = super::serve::runtime()?.block_on(async {
        let (client, _app) = connect(&ctx, launch).await?;
        if let Start::Resume(wanted) = &start {
            let at = client.at(&ctx.location.display().to_string());
            let found = super::sessions::resolve(&at, wanted).await?;
            start = Start::Resume(found["id"].as_str().unwrap_or_default().to_owned());
        }
        let source = client.at(&ctx.location.display().to_string());
        let mut owned = None;
        let directory = if let Some(name) = &launch.worktree {
            let session = super::worktrees::start(&source, name, serde_json::json!({"model": global.model, "agent": launch.agent, "mode": global.mode}), false).await?;
            let id: String = session["id"].as_str().ok_or_else(|| CliError::runtime("Worktree Session has no ID"))?.into();
            owned = Some(id.clone());
            start = Start::Resume(id);
            session["directory"].as_str().ok_or_else(|| CliError::runtime("Worktree Session has no Location"))?.to_owned()
        } else { ctx.location.display().to_string() };
        let opts = TuiOptions {
            client,
            directory,
            state_dir: ctx.paths.state.clone(),
            config_dir: ctx.paths.config.clone(),
            start,
            fork: launch.fork,
            model: global.model.clone(),
            agent: launch.agent.clone(),
            mode: global.mode.clone(),
            prompt,
        };
        let summary = cyber_tui::run(opts).await.map_err(CliError::runtime);
        if let Some(id) = &owned {
            super::worktrees::cleanup(&source, id).await;
        }
        if let Ok(summary) = &summary
            && owned.as_ref() != Some(&summary.session_id)
        {
            super::worktrees::cleanup_entered(&source, &summary.session_id).await;
        }
        summary
    })?;
    println!(
        "{} · {} in / {} out · ${:.4}",
        summary.title, summary.input_tokens, summary.output_tokens, summary.cost
    );
    println!("resume with: cyber -r {}", summary.session_id);
    Ok(())
}

fn start(launch: &LaunchArgs) -> Result<Start, CliError> {
    if launch.from_pr.is_some()
        && (launch.resume_last || launch.resume.is_some() || launch.worktree.is_some())
    {
        return Err(CliError::usage(
            "--from-pr opens the picker and cannot be combined with --continue, --resume or --worktree",
        ));
    }
    if launch.worktree.is_some() && (launch.resume_last || launch.resume.is_some() || launch.fork) {
        return Err(CliError::usage(
            "--worktree starts a fresh Session and cannot be combined with --continue, --resume or --fork",
        ));
    }
    if let Some(name) = launch.worktree.as_deref().filter(|name| !name.is_empty()) {
        cyber_core::worktrees::Name::parse(name).map_err(CliError::usage)?;
    }
    if launch.resume_last && launch.resume.is_some() {
        return Err(CliError::usage(
            "--continue and --resume cannot be combined",
        ));
    }
    if launch.fork && !(launch.resume_last || launch.resume.is_some()) {
        return Err(CliError::usage("--fork needs --continue or --resume"));
    }
    Ok(match (&launch.resume, launch.resume_last) {
        (Some(id), _) if id.is_empty() => Start::Pick,
        (Some(id), _) => Start::Resume(id.clone()),
        (None, true) => Start::Last,
        (None, false) => Start::New,
    })
}

/// `--prompt`, with piped stdin prepended.
fn prompt(launch: &LaunchArgs) -> Result<Option<String>, CliError> {
    let stdin = std::io::stdin();
    let piped = if stdin.is_terminal() {
        String::new()
    } else {
        let mut s = String::new();
        stdin.lock().read_to_string(&mut s)?;
        s.trim_end().to_string()
    };
    Ok(match (piped.is_empty(), &launch.prompt) {
        (true, p) => p.clone(),
        (false, None) => Some(piped),
        (false, Some(p)) => Some(format!("{piped}\n\n{p}")),
    })
}

async fn connect(ctx: &Context, launch: &LaunchArgs) -> Result<(Client, Option<App>), CliError> {
    if launch.embedded {
        let dir = ctx.paths.data.join("embedded");
        std::fs::create_dir_all(&dir)?;
        let database = cyber_core::paths::DatabaseLocation::File(dir.join(format!(
            "{}.db",
            ulid::Ulid::new().to_string().to_lowercase()
        )));
        let app = App::build(AppOptions {
            paths: ctx.paths.clone(),
            home: ctx.home.clone(),
            database,
            default_directory: ctx.location.clone(),
            sandbox_policy: launch.sandbox.clone(),
            snapshots: true,
            interactive: true,
            password: None,
            mcp_credentials: None,
            remote_skill_fetcher: None,
            workflow_cores: None,
            isolation: crate::commands::exec::setup::isolation(
                launch.dangerously_bypass_permissions || launch.dangerously_skip_permissions,
            ),
        })
        .await
        .map_err(CliError::runtime)?;
        return Ok((Client::embedded(app.embedded()), Some(app)));
    }
    let info = super::serve::start(ctx).await?;
    Ok((
        Client::http(&info.registration.url, Some(info.password)),
        None,
    ))
}
