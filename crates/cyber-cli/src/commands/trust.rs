//! `cyber trust` (`workspace-trust` → Checkout-scoped trust).

use clap::Subcommand;
use cyber_core::trust::TrustStore;

use crate::cli::GlobalArgs;
use crate::context::Context;
use crate::error::CliError;
use crate::output;

#[derive(Debug, Subcommand)]
pub enum TrustCmd {
    /// Show the checkout, the digest of its security-sensitive definitions and their state.
    Inspect,
    /// Approve exactly the inspected definitions.
    Approve {
        /// Digest printed by `cyber trust inspect`.
        #[arg(long)]
        digest: String,
    },
    /// Revoke this checkout's approval.
    Revoke,
}

pub fn run(cmd: TrustCmd, ctx: &Context, global: &GlobalArgs) -> Result<(), CliError> {
    match cmd {
        TrustCmd::Inspect => inspect(ctx, global),
        TrustCmd::Approve { digest } => approve(ctx, &digest),
        TrustCmd::Revoke => revoke(ctx),
    }
}

fn inspect(ctx: &Context, global: &GlobalArgs) -> Result<(), CliError> {
    let report = ctx.trust()?;
    if output::is_json(global.format) {
        return output::json(&report);
    }
    let state = match (&report.digest, report.trusted) {
        (None, _) => "nothing to approve",
        (Some(_), true) => "trusted",
        (Some(_), false) => "untrusted (definitions inactive)",
    };
    output::rows(&[
        ("checkout", report.checkout_root.display().to_string()),
        (
            "digest",
            report.digest.clone().unwrap_or_else(|| "-".into()),
        ),
        ("state", state.into()),
    ]);
    for definition in &report.definitions {
        println!("  {definition}");
    }
    if let (Some(digest), false) = (&report.digest, report.trusted) {
        println!("\napprove with: cyber trust approve --digest {digest}");
    }
    Ok(())
}

fn approve(ctx: &Context, digest: &str) -> Result<(), CliError> {
    let report = ctx.trust()?;
    let Some(current) = report.digest else {
        println!("nothing to approve in {}", report.checkout_root.display());
        return Ok(());
    };
    if current != digest {
        return Err(
            CliError::usage("the digest does not match the current definitions").with_hint(
                format!("run \"cyber trust inspect\"; current digest is {current}"),
            ),
        );
    }
    TrustStore::new(ctx.paths.trust_file()).approve(&report.checkout_root, &current)?;
    println!("approved {current} for {}", report.checkout_root.display());
    Ok(())
}

fn revoke(ctx: &Context) -> Result<(), CliError> {
    let root = ctx.trust()?.checkout_root;
    if TrustStore::new(ctx.paths.trust_file()).revoke(&root)? {
        println!("revoked trust for {}", root.display());
    } else {
        println!("no approval recorded for {}", root.display());
    }
    Ok(())
}
