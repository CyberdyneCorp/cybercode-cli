//! Provider sign-in (`provider-credentials` → ChatGPT subscription login, GitHub Copilot
//! subscription login). Only the ChatGPT and GitHub Copilot subscriptions sign in today;
//! every other provider takes its API key from the environment.
mod copilot;

use std::time::Duration;

use clap::{Subcommand, ValueEnum};
use cyber_llm::catalog::{BuildInputs, Catalog, SourceOptions, load_source};
use cyber_llm::chatgpt::{self, AuthFile, ChatGptError, Issuer, Login, ModelsFile};
use cyber_tools::mcp::oauth::{self, CallbackListener, Pkce};
use serde_json::{Value, json};

use crate::cli::GlobalArgs;
use crate::context::Context;
use crate::error::CliError;
use crate::output;

#[derive(Debug, Subcommand)]
pub enum ProvidersCmd {
    /// Sign in to a provider. `chatgpt` signs in with a ChatGPT Plus/Pro subscription:
    /// open the printed URL in a browser to authorize. `copilot` signs in with a GitHub
    /// Copilot plan: open the printed URL and enter the printed code.
    Login {
        provider: String,
        /// `browser` waits for the redirect on localhost:1455; `code` reads the redirect
        /// URL pasted from the browser (for SSH and other headless machines). ChatGPT only.
        #[arg(long, value_enum, default_value_t = Method::Browser)]
        method: Method,
        /// Sign in to a GitHub Enterprise host (a bare hostname such as example.ghe.com)
        /// instead of github.com. Copilot only.
        #[arg(long, value_name = "HOST")]
        enterprise: Option<String>,
    },
    /// Forget a provider login. Tokens are not revoked remotely.
    Logout { provider: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Method {
    Browser,
    Code,
}

/// How long `login --method browser` waits for the redirect.
const CALLBACK_TIMEOUT: Duration = Duration::from_secs(300);

pub fn run(cmd: ProvidersCmd, ctx: &Context, global: &GlobalArgs) -> Result<(), CliError> {
    match cmd {
        ProvidersCmd::Login {
            provider,
            enterprise,
            ..
        } if provider == cyber_llm::copilot::PROVIDER_ID => {
            copilot::login(ctx, global, enterprise.as_deref())
        }
        ProvidersCmd::Logout { provider } if provider == cyber_llm::copilot::PROVIDER_ID => {
            copilot::logout(ctx, global)
        }
        ProvidersCmd::Login {
            provider,
            method,
            enterprise,
        } => {
            only_chatgpt(&provider, ctx)?;
            if enterprise.is_some() {
                return Err(CliError::usage("--enterprise applies only to copilot"));
            }
            login(ctx, global, &chatgpt_file(ctx), method)
        }
        ProvidersCmd::Logout { provider } => {
            only_chatgpt(&provider, ctx)?;
            let file = chatgpt_file(ctx);
            let removed = file.remove().map_err(failed)?;
            ModelsFile::beside(file.path()).remove();
            if output::is_json(global.format) {
                return output::json(&json!({"provider": provider, "removed": removed}));
            }
            println!(
                "{}",
                if removed {
                    "Signed out of ChatGPT"
                } else {
                    "Not signed in to ChatGPT"
                }
            );
            Ok(())
        }
    }
}

fn chatgpt_file(ctx: &Context) -> AuthFile {
    AuthFile::new(chatgpt::auth_path(&ctx.paths.data))
}

fn login(
    ctx: &Context,
    global: &GlobalArgs,
    file: &AuthFile,
    method: Method,
) -> Result<(), CliError> {
    if cfg!(windows) {
        return Err(
            CliError::usage("ChatGPT login is not supported on Windows yet")
                .with_hint("use the openai provider with OPENAI_API_KEY"),
        );
    }
    let issuer = Issuer::from_env(&ctx.env).map_err(|error| CliError::usage(error.to_string()))?;
    let pkce = Pkce::generate().map_err(|error| CliError::runtime(error.to_string()))?;
    let state = oauth::random_state().map_err(|error| CliError::runtime(error.to_string()))?;
    let url = chatgpt::authorize_url(&issuer, &pkce.challenge, &state);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let code = match method {
        Method::Browser => runtime.block_on(await_callback(url.as_str(), &state))?,
        Method::Code => pasted_code(url.as_str(), &state)?,
    };
    let http = cyber_llm::Endpoint::client_builder()
        .build()
        .map_err(|_| CliError::runtime("HTTP client unavailable"))?;
    let signed_in = runtime
        .block_on(chatgpt::complete_login(
            &http,
            &issuer,
            chatgpt::backend_url(&ctx.env),
            file,
            &code,
            &pkce.verifier,
        ))
        .map_err(login_failed)?;
    let models = signed_in.models.unwrap_or_else(|error| {
        eprintln!(
            "warning: could not list the plan's models ({error}); run this login again to retry"
        );
        Vec::new()
    });
    report(global, &signed_in.login, &models)
}

async fn await_callback(url: &str, state: &str) -> Result<String, CliError> {
    let listener =
        CallbackListener::bind_path(Some(chatgpt::CALLBACK_PORT), chatgpt::CALLBACK_PATH)
            .await
            .map_err(|_| {
                CliError::runtime("localhost:1455 is in use").with_hint(
                    "finish or cancel the other ChatGPT/Codex login, or use --method code",
                )
            })?;
    eprintln!("Open this URL in a browser to sign in to ChatGPT:\n{url}");
    eprintln!("Waiting for the browser to return to localhost:1455...");
    listener
        .wait(state, CALLBACK_TIMEOUT)
        .await
        .map_err(|error| CliError::runtime(error.to_string()))
}

fn pasted_code(url: &str, state: &str) -> Result<String, CliError> {
    eprintln!("Open this URL in a browser to sign in to ChatGPT:\n{url}");
    eprintln!(
        "The browser then fails to load a localhost:1455 page; paste that page's full URL here:"
    );
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    oauth::callback_code(&line, state).map_err(|error| CliError::usage(error.to_string()))
}

fn report(global: &GlobalArgs, login: &Login, models: &[String]) -> Result<(), CliError> {
    if output::is_json(global.format) {
        return output::json(&json!({
            "provider": chatgpt::PROVIDER_ID,
            "email": login.email,
            "plan": login.plan,
            "expires_at": login.expires_at,
            "models": models,
        }));
    }
    let email = login.email.as_deref().unwrap_or("unknown account");
    let plan = login.plan.as_deref().unwrap_or("unknown plan");
    println!("Signed in to ChatGPT as {email} ({plan})");
    if !models.is_empty() {
        println!("Models: {}", models.join(", "));
    }
    Ok(())
}

/// Past `copilot`, login exists only for `chatgpt`; others are told which variables carry
/// their key.
fn only_chatgpt(provider: &str, ctx: &Context) -> Result<(), CliError> {
    if provider == chatgpt::PROVIDER_ID {
        return Ok(());
    }
    let error = CliError::usage(format!(
        "Provider login is not available for {provider} yet"
    ));
    let names = key_variables(provider, ctx);
    Err(if names.is_empty() {
        error.with_hint("configure its key under providers in cyber.jsonc")
    } else {
        error.with_hint(format!("set {}", names.join(" or ")))
    })
}

/// The provider's API-key environment variables from the cached or bundled catalog.
fn key_variables(provider: &str, ctx: &Context) -> Vec<String> {
    let mut source = SourceOptions::from_env(&ctx.env, &ctx.paths.cache);
    source.allow_fetch = false;
    let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        return Vec::new();
    };
    let Ok(loaded) = runtime.block_on(load_source(&source)) else {
        return Vec::new();
    };
    let catalog = Catalog::build(&BuildInputs {
        data: &loaded.data,
        config: &Value::Null,
        env: &ctx.env,
    });
    catalog
        .providers
        .get(provider)
        .map(|provider| provider.env.clone())
        .unwrap_or_default()
}

fn failed(error: ChatGptError) -> CliError {
    CliError::runtime(error.to_string())
}

fn login_failed(error: ChatGptError) -> CliError {
    match error {
        ChatGptError::NoAccount => CliError::runtime(error.to_string())
            .with_hint("sign in with an account that has a ChatGPT Plus or Pro plan"),
        error => failed(error),
    }
}
