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
    let start = start(launch)?;
    let mut global = GlobalArgs {
        cwd: global.cwd.clone(),
        profile: global.profile.clone(),
        config: global.config.clone(),
        model: global.model.clone(),
        mode: global.mode.clone(),
        format: global.format,
        log_level: global.log_level.clone(),
        print_logs: global.print_logs,
    };
    if let Some(p) = project {
        global.cwd = Some(global.cwd.clone().unwrap_or_default().join(p));
    }
    let ctx = Context::new(&global)?;
    super::start_logging(&ctx, &global)?;
    let prompt = prompt(launch)?;
    if !std::io::stdout().is_terminal() {
        return Err(
            CliError::usage("the TUI needs a terminal").with_hint("use \"cyber exec\" for scripts")
        );
    }
    let summary = super::serve::runtime()?.block_on(async {
        let (client, _app) = connect(&ctx, launch).await?;
        let opts = TuiOptions {
            client,
            directory: ctx.location.display().to_string(),
            state_dir: ctx.paths.state.clone(),
            start,
            fork: launch.fork,
            model: global.model.clone(),
            agent: launch.agent.clone(),
            mode: global.mode.clone(),
            prompt,
        };
        cyber_tui::run(opts).await.map_err(CliError::runtime)
    })?;
    println!(
        "{} · {} in / {} out · ${:.4}",
        summary.title, summary.input_tokens, summary.output_tokens, summary.cost
    );
    println!("resume with: cyber -r {}", summary.session_id);
    Ok(())
}

fn start(launch: &LaunchArgs) -> Result<Start, CliError> {
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
