//! `cyber providers login|logout copilot` (`provider-credentials` → GitHub Copilot
//! subscription login): GitHub's OAuth device flow, so it also works over SSH. The CLI
//! prints the URL and code (it never opens a browser), polls until the user approves, and
//! stores the GitHub login only once a Copilot token mint proves the plan. Ctrl+C, a denial
//! or an expired code store nothing.
use cyber_llm::copilot::{
    self, AuthFile, CopilotError, CopilotToken, Host, Login, ModelsFile, Urls,
};
use serde_json::json;

use crate::cli::GlobalArgs;
use crate::context::Context;
use crate::error::CliError;
use crate::output;

pub fn login(ctx: &Context, global: &GlobalArgs, enterprise: Option<&str>) -> Result<(), CliError> {
    if cfg!(windows) {
        return Err(
            CliError::usage("GitHub Copilot login is not supported on Windows yet")
                .with_hint("run Cyber Code on macOS, Linux or WSL to use a Copilot plan"),
        );
    }
    let host = enterprise
        .map_or_else(|| Ok(Host::default()), Host::parse)
        .map_err(usage)?;
    let urls = Urls::resolve(&ctx.env, &host).map_err(usage)?;
    let http = cyber_llm::Endpoint::client_builder()
        .build()
        .map_err(|_| CliError::runtime("HTTP client unavailable"))?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let (login, token) = runtime.block_on(async {
        tokio::select! {
            signed_in = sign_in(ctx, &http, &urls, &host) => signed_in,
            _ = tokio::signal::ctrl_c() => Err(CliError::runtime("GitHub sign-in cancelled")),
        }
    })?;
    let file = AuthFile::new(copilot::auth_path(&ctx.paths.data));
    file.save(&login).map_err(failed)?;
    let models = runtime.block_on(plan_models(&http, &file, &token));
    report(global, &login, &models)
}

/// The device flow and the subscription check; nothing is stored here.
async fn sign_in(
    ctx: &Context,
    http: &reqwest::Client,
    urls: &Urls,
    host: &Host,
) -> Result<(Login, CopilotToken), CliError> {
    let client_id = copilot::client_id(&ctx.env);
    let code = copilot::request_code(http, urls, &client_id)
        .await
        .map_err(failed)?;
    eprintln!(
        "Open {} and enter {}",
        code.verification_uri, code.user_code
    );
    eprintln!("Waiting for GitHub to approve the sign-in...");
    let github_token = copilot::poll(http, urls, &client_id, &code, tokio::time::sleep)
        .await
        .map_err(failed)?;
    copilot::confirm(http, urls, host, github_token)
        .await
        .map_err(login_failed)
}

/// Fetch and cache the plan's models. A failure is only a warning: the login is kept, and
/// the next request or signing in again retries.
async fn plan_models(http: &reqwest::Client, file: &AuthFile, token: &CopilotToken) -> Vec<String> {
    let cache = ModelsFile::beside(file.path());
    let fetched = copilot::fetch_models(http, token).await;
    match fetched.and_then(|models| {
        let slugs = models.iter().map(|m| m.slug.clone()).collect();
        cache.save(models).map(|()| slugs)
    }) {
        Ok(slugs) => slugs,
        Err(error) => {
            eprintln!(
                "warning: could not list the plan's models ({error}); run this login again to retry"
            );
            Vec::new()
        }
    }
}

fn report(global: &GlobalArgs, login: &Login, models: &[String]) -> Result<(), CliError> {
    if output::is_json(global.format) {
        return output::json(&json!({
            "provider": copilot::PROVIDER_ID,
            "user": login.user,
            "host": login.host,
            "models": models,
        }));
    }
    println!(
        "Signed in to GitHub Copilot as {} ({})",
        login.user, login.host
    );
    if !models.is_empty() {
        println!("Models: {}", models.join(", "));
    }
    Ok(())
}

/// Forget the login and the cached model list; the GitHub token is not revoked.
pub fn logout(ctx: &Context, global: &GlobalArgs) -> Result<(), CliError> {
    let file = AuthFile::new(copilot::auth_path(&ctx.paths.data));
    let removed = file.remove().map_err(failed)?;
    ModelsFile::beside(file.path()).remove();
    if output::is_json(global.format) {
        return output::json(&json!({"provider": copilot::PROVIDER_ID, "removed": removed}));
    }
    println!(
        "{}",
        if removed {
            "Signed out of GitHub Copilot"
        } else {
            "Not signed in to GitHub Copilot"
        }
    );
    Ok(())
}

fn usage(error: CopilotError) -> CliError {
    CliError::usage(error.to_string())
}

fn failed(error: CopilotError) -> CliError {
    CliError::runtime(error.to_string())
}

fn login_failed(error: CopilotError) -> CliError {
    match error {
        CopilotError::NoSubscription => CliError::runtime(error.to_string())
            .with_hint("sign in with a GitHub account that has a Copilot Pro, Pro+, Business or Enterprise plan"),
        error => failed(error),
    }
}
