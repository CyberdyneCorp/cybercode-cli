//! Cyber Account commands use global settings only and remain separate from provider login.
use crate::{cli::GlobalArgs, context::Context, error::CliError, output};
use cyber_account::{
    Account, AccountStore, Error, LoopbackLogin, OidcClient, StorageKind, TokenSet,
};
use cyber_core::env::EnvSource;
use serde_json::json;
use std::{
    io::{self, BufRead, Read, Write},
    process::Command,
};

pub(crate) type Settings = cyber_account::AccountSettings;
pub(crate) fn settings(ctx: &Context) -> Result<Settings, CliError> {
    Settings::from_global(&ctx.global_config()?, ctx.env.get("CYBER_ACCOUNT_ISSUER"))
        .map_err(failed)
}

pub fn login(
    ctx: &Context,
    global: &GlobalArgs,
    no_browser: bool,
    reauth: bool,
) -> Result<(), CliError> {
    let settings = settings(ctx)?;
    super::serve::runtime()?.block_on(login_async(ctx, global, no_browser, reauth, settings))
}
async fn login_async(
    ctx: &Context,
    global: &GlobalArgs,
    no_browser: bool,
    reauth: bool,
    settings: Settings,
) -> Result<(), CliError> {
    let epoch = store(ctx).login_epoch().await.map_err(failed)?;
    let mut client = settings.client(&ctx.paths.cache).await.map_err(failed)?;
    for scope in client.provider().login_scopes().map_err(failed)?.omitted {
        eprintln!(
            "warning: issuer does not advertise scope {scope}; its account features will be unavailable"
        );
    }
    let tokens =
        if no_browser && !reauth && client.provider().device_authorization_endpoint().is_some() {
            let device = client.start_device_authorization().await.map_err(failed)?;
            eprintln!(
                "Go to {} and enter code {}",
                device.verification_uri(),
                device.user_code()
            );
            client
                .poll_device_authorization(device, &settings.audience)
                .await
                .map_err(failed)?
        } else {
            code_login(&mut client, &settings, no_browser, reauth).await?
        };
    let account = tokens.account().clone();
    let kind = store(ctx).save_login(tokens, epoch).await.map_err(failed)?;
    if kind == StorageKind::File {
        eprintln!("warning: account tokens stored in file (no OS keyring)");
    }
    if output::is_json(global.format) {
        output::json(&json!({"logged_in":true,"account":account,"storage":kind}))
    } else {
        println!(
            "Logged in as {}",
            safe(account.email.as_deref().unwrap_or(&account.sub))
        );
        Ok(())
    }
}
async fn code_login(
    client: &mut OidcClient,
    settings: &Settings,
    no_browser: bool,
    reauth: bool,
) -> Result<TokenSet, CliError> {
    let flow =
        LoopbackLogin::start_with_reauthentication(client.provider(), &settings.client_id, reauth)
            .await
            .map_err(failed)?;
    eprintln!("Sign in at {}", flow.authorization_url());
    if no_browser {
        let handle = flow.callback_handle();
        let authorization = flow.authorization_url().clone();
        let input = tokio::task::spawn_blocking(move || pasted_callback(&authorization))
            .await
            .map_err(|_| CliError::runtime("could not read login response"))??;
        handle.submit(&input).map_err(failed)?;
    } else if !open_browser(flow.authorization_url().as_str()) {
        eprintln!("Open the printed URL in your browser to continue.");
    }
    client
        .exchange(flow.finish().await.map_err(failed)?, &settings.audience)
        .await
        .map_err(failed)
}
fn pasted_callback(authorization: &url::Url) -> Result<String, CliError> {
    let input = prompt_line("Paste the full redirect URL (or authorization code): ")?;
    if url::Url::parse(&input).is_ok() {
        return Ok(input);
    }
    let state = prompt_line("Paste the OAuth state from the same redirect URL: ")?;
    let redirect = authorization
        .query_pairs()
        .find(|(key, _)| key == "redirect_uri")
        .ok_or_else(|| failed(Error::InvalidCallback))?
        .1
        .into_owned();
    let mut redirect = url::Url::parse(&redirect).map_err(|_| failed(Error::InvalidCallback))?;
    redirect
        .query_pairs_mut()
        .extend_pairs([("code", input.as_str()), ("state", state.as_str())]);
    Ok(redirect.to_string())
}
fn prompt_line(prompt: &str) -> Result<String, CliError> {
    eprint!("{prompt}");
    io::stderr().flush()?;
    let stdin = io::stdin();
    let mut input = String::new();
    stdin.lock().take(8193).read_line(&mut input)?;
    if input.len() > 8192 || input.trim().is_empty() {
        return Err(failed(Error::InvalidCallback));
    }
    Ok(input.trim().to_owned())
}

pub fn whoami(ctx: &Context, global: &GlobalArgs, org: Option<&str>) -> Result<(), CliError> {
    super::serve::runtime()?.block_on(whoami_async(ctx, global, org))
}
async fn whoami_async(
    ctx: &Context,
    global: &GlobalArgs,
    org: Option<&str>,
) -> Result<(), CliError> {
    let Some(mut tokens) = store(ctx).load().await.map_err(failed)? else {
        return if output::is_json(global.format) {
            output::json(&json!({"logged_in":false}))
        } else {
            println!("not logged in");
            Ok(())
        };
    };
    let settings = settings(ctx)?;
    if tokens.account().issuer.trim_end_matches('/') != settings.issuer.trim_end_matches('/')
        || tokens.client_id() != settings.client_id
    {
        return Err(failed(Error::IssuerMismatch));
    }
    if tokens.account().expires_at <= now().saturating_add(60) {
        tokens = store(ctx)
            .refresh(
                settings.client(&ctx.paths.cache).await.map_err(failed)?,
                settings.audience,
            )
            .await
            .map_err(failed)?;
    }
    let mut account = tokens.account().clone();
    select_org(ctx, &mut account, org)?;
    if output::is_json(global.format) {
        output::json(&json!({"logged_in":true,"account":account}))
    } else {
        output::rows(&[
            ("subject", safe(&account.sub)),
            (
                "email",
                safe(account.email.as_deref().unwrap_or("(not provided)")),
            ),
            ("issuer", safe(&account.issuer)),
            (
                "organizations",
                account
                    .orgs
                    .iter()
                    .map(|o| safe(o.short_name.as_deref().unwrap_or(&o.id)))
                    .collect::<Vec<_>>()
                    .join(", "),
            ),
        ]);
        Ok(())
    }
}
/// A running owner must cancel connectors before issuer revocation. Only send its
/// password to a literal loopback registration that answers with the matching ID.
async fn logout_owner(ctx: &Context, all: bool) -> bool {
    let Some(registration) = cyber_app::read_registration(&ctx.paths) else {
        return false;
    };
    let Ok(url) = url::Url::parse(&registration.url) else {
        return false;
    };
    if !matches!(url.host(), Some(url::Host::Ipv4(ip)) if ip.is_loopback())
        && !matches!(url.host(), Some(url::Host::Ipv6(ip)) if ip.is_loopback())
    {
        return false;
    }
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return false;
    }
    let Some(probe) = cyber_app::probe(&registration.url).await else {
        return false;
    };
    if probe.id.as_deref() != Some(registration.id.as_str()) {
        return false;
    }
    let Ok(password) = cyber_app::password(&ctx.paths) else {
        return false;
    };
    let Ok(client) = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(12))
        .build()
    else {
        return false;
    };
    client
        .delete(format!(
            "{}/api/v1/account?all={all}",
            registration.url.trim_end_matches('/')
        ))
        .basic_auth("cyber", Some(password))
        .send()
        .await
        .is_ok_and(|response| response.status().is_success())
}
fn select_org(
    ctx: &Context,
    account: &mut Account,
    selection: Option<&str>,
) -> Result<(), CliError> {
    let path = ctx.paths.state.join("account-org.json");
    if let Some(selection) = selection {
        let selected = account
            .orgs
            .iter()
            .find(|o| o.id == selection || o.short_name.as_deref() == Some(selection))
            .cloned()
            .ok_or_else(|| {
                CliError::usage(
                    "organization is not in the current account's authorized organization list",
                )
            })?;
        let bytes = serde_json::to_vec(
            &json!({"issuer":account.issuer,"sub":account.sub,"organization_id":selected.id}),
        )
        .map_err(|_| CliError::runtime("could not encode organization selection"))?;
        cyber_core::trust::write_private_atomic(&path, &bytes)?;
        account.org = Some(selected);
    } else if let Ok(text) = cyber_core::files::read_small(&path, 8192)
        && let Ok(saved) = serde_json::from_str::<serde_json::Value>(&text)
        && saved["issuer"] == account.issuer
        && saved["sub"] == account.sub
    {
        account.org = account
            .orgs
            .iter()
            .find(|o| saved["organization_id"] == o.id)
            .cloned();
    }
    Ok(())
}

pub fn logout(ctx: &Context, global: &GlobalArgs, all: bool) -> Result<(), CliError> {
    super::serve::runtime()?.block_on(logout_async(ctx, global, all))
}
async fn logout_async(ctx: &Context, global: &GlobalArgs, all: bool) -> Result<(), CliError> {
    if !logout_owner(ctx, all).await {
        let old = store(ctx).load().await.ok().flatten();
        store(ctx).logout(all).await.map_err(failed)?;
        if let Some(old) = old
            && let Ok(settings) = settings(ctx)
            && let Ok(client) = settings.client(&ctx.paths.cache).await
        {
            client.revoke_logout(&old).await;
        }
    }
    if output::is_json(global.format) {
        output::json(&json!({"logged_in":false}))
    } else {
        println!("Logged out of Cyber Account");
        Ok(())
    }
}

pub(crate) fn store(ctx: &Context) -> AccountStore {
    AccountStore::open(ctx.paths.data.clone())
}
pub(crate) fn failed(error: Error) -> CliError {
    CliError { code: 6, message: error.to_string(), hint: Some("check global account.issuer, account.client_id and account.audience, then run cyber login".into()) }
}
fn safe(value: &str) -> String {
    value.chars().filter(|c| !c.is_control()).collect()
}
fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}
fn open_browser(url: &str) -> bool {
    let program = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(windows) {
        "explorer.exe"
    } else {
        "xdg-open"
    };
    Command::new(program)
        .arg(url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .is_ok()
}
