//! `cyber attach <session>@<peer>` (`cli-commands` → Attach command,
//! `cross-session-messaging` → Peer attach): the TUI on one Session of a paired machine,
//! through its peer listener.
//!
//! The peer must be an enabled remote with an `attach` grant, and the user there approves
//! each attach (`remote.attach`); until then nothing of the Session is shown. The attached
//! TUI reaches only that Session's transcript, events, prompt, interrupt and permission
//! replies.

use std::io::IsTerminal;

use cyber_client::{Client, ClientError};
use cyber_server::runtime::messaging::PeerError;
use cyber_server::runtime::messaging::address::{self, Address};
use cyber_tui::{Start, TuiOptions};

use crate::context::Context;
use crate::error::CliError;

#[derive(Debug, clap::Args)]
pub struct AttachArgs {
    /// `<session>@<peer>`: a Session (ID or name) of a paired machine.
    pub target: String,
}

pub fn run(args: &AttachArgs, ctx: &Context) -> Result<(), CliError> {
    let (session, peer) = peer_session(&args.target)?;
    let settings = super::peers::settings(ctx)?;
    let remote = settings
        .remotes
        .iter()
        .find(|remote| remote.enabled && remote.name == peer)
        .ok_or_else(|| not_configured(&peer))?;
    if !std::io::stdout().is_terminal() {
        return Err(CliError::usage("the TUI needs a terminal")
            .with_hint("run cyber attach from an interactive terminal"));
    }
    let target = super::peers::target(remote, ctx)?;
    let summary = super::serve::runtime()?.block_on(async {
        eprintln!("waiting for {peer} to approve attaching to {session} (up to 2 minutes)");
        let client = Client::attach_peer(target, &session)
            .await
            .map_err(|error| refused(&error, &peer))?;
        let opts = TuiOptions {
            client: client.clone(),
            directory: ctx.location.display().to_string(),
            state_dir: ctx.paths.state.clone(),
            config_dir: ctx.paths.config.clone(),
            start: Start::Resume(client.attached_session().unwrap_or_default().to_owned()),
            fork: false,
            model: None,
            agent: None,
            mode: None,
            prompt: None,
        };
        let summary = cyber_tui::run(opts).await.map_err(CliError::runtime);
        let _ = client.detach().await;
        summary
    })?;
    println!("{} on {peer}", summary.title);
    println!("attach again with: cyber attach {session}@{peer}");
    Ok(())
}

/// `<session>@<peer>`; a server URL or a local Session is not attached this way yet.
fn peer_session(target: &str) -> Result<(String, String), CliError> {
    if target.contains("://") || !target.contains('@') {
        return Err(CliError::unavailable(
            "`cyber attach <url|session>`",
            "P0 or later",
        ));
    }
    match address::parse(target) {
        Ok(Address::Remote { session, peer }) => Ok((session, peer)),
        _ => Err(CliError::usage(format!(
            "`{target}` is not a <session>@<peer> address"
        ))),
    }
}

fn not_configured(peer: &str) -> CliError {
    let error = PeerError::NotConfigured { peer: peer.into() };
    CliError::usage(error.to_string())
        .with_hint(format!("pair it with `cyber peers add {peer} '<code>'`"))
}

/// Why the peer did not open the attach, with what to do about it.
fn refused(error: &ClientError, peer: &str) -> CliError {
    let ClientError::Api {
        tag, message, body, ..
    } = error
    else {
        return CliError::runtime(error.to_string());
    };
    let text = super::tagged(tag, message);
    match tag.as_str() {
        "PermissionDeniedError" => CliError::runtime(text).with_hint(format!(
            "the user on {peer} must approve remote.attach within 2 minutes, or allow it there for this machine's grant"
        )),
        "PeerAuthError" if body["reason"] == "scope" => CliError::runtime(text).with_hint(format!(
            "this machine's grant on {peer} lacks the attach scope: on {peer}, cyber peers invite <name> --scope message --scope attach, then cyber peers add {peer} '<code>' here"
        )),
        "PeerAuthError" => CliError::runtime(text).with_hint(format!(
            "pair it again: cyber peers rm {peer}, then cyber peers add {peer} '<new code>'"
        )),
        "NotFoundError" => CliError::usage(text)
            .with_hint(format!("cyber messages list --reachable lists the Sessions of {peer} this machine can reach")),
        _ => CliError::runtime(text),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_peer_sessions_are_attached() {
        assert_eq!(
            peer_session("migration@desk").unwrap(),
            ("migration".into(), "desk".into())
        );
        assert_eq!(peer_session("@ses_1@desk").unwrap().0, "ses_1");
        for unavailable in ["http://10.0.0.5:4747", "ses_1", "migration"] {
            let error = peer_session(unavailable).unwrap_err();
            assert!(error.message.contains("not available"), "{unavailable}");
        }
        for malformed in ["a@b@c", "@@x", "x@", "x@bad name"] {
            let error = peer_session(malformed).unwrap_err();
            assert!(
                error.message.contains("is not a <session>@<peer> address"),
                "{malformed}"
            );
        }
    }
}
