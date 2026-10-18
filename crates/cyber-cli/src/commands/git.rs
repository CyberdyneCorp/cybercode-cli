//! `cyber git commit-msg` (`vcs-integration` → Commit and PR text generation): a
//! conventional commit message for the staged diff, written by the small model and printed
//! for the user to commit with. No co-author trailer unless `git.co_author` is set.

use std::sync::Arc;

use clap::Subcommand;
use cyber_app::git_text::{self, GitTextError};
use cyber_core::env::ProcessEnv;
use cyber_llm::catalog::{SourceOptions, load_source};
use cyber_server::runtime::{CatalogResolver, ModelResolver};
use serde_json::json;

use crate::cli::GlobalArgs;
use crate::context::Context;
use crate::error::CliError;
use crate::output;

#[derive(Debug, Subcommand)]
pub enum GitCmd {
    /// Print a conventional commit message for the staged changes.
    CommitMsg,
    /// Print a pull request title and body for BASE..HEAD.
    PrDescription {
        /// The base branch or commit (default: origin/HEAD, else main, else master).
        #[arg(long)]
        base: Option<String>,
    },
}

pub fn run(cmd: GitCmd, ctx: &Context, global: &GlobalArgs) -> Result<(), CliError> {
    let config = ctx.config()?.value;
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(async {
            let resolver = resolver(ctx, &config).await?;
            match cmd {
                GitCmd::CommitMsg => {
                    let message = git_text::commit_message(&*resolver, &config, &ctx.location)
                        .await
                        .map_err(error)?;
                    if output::is_json(global.format) {
                        return output::json(&json!({ "message": message }));
                    }
                    println!("{message}");
                    Ok(())
                }
                GitCmd::PrDescription { base } => {
                    let pr = git_text::pr_description(
                        &*resolver,
                        &config,
                        &ctx.location,
                        base.as_deref(),
                    )
                    .await
                    .map_err(error)?;
                    if output::is_json(global.format) {
                        return output::json(
                            &json!({ "base": pr.base, "title": pr.title, "body": pr.body }),
                        );
                    }
                    println!("{}\n\n{}", pr.title, pr.body);
                    Ok(())
                }
            }
        })
}

/// The model catalog and providers, as the server resolves them.
async fn resolver(
    ctx: &Context,
    config: &serde_json::Value,
) -> Result<Arc<dyn ModelResolver>, CliError> {
    let loaded = load_source(&SourceOptions::from_env(&ProcessEnv, &ctx.paths.cache))
        .await
        .map_err(|error| CliError::runtime(error.to_string()))?;
    Ok(Arc::new(CatalogResolver::new(
        &loaded.data,
        config.clone(),
        &ProcessEnv,
    )))
}

fn error(error: GitTextError) -> CliError {
    match error {
        GitTextError::Invalid(message) => CliError::usage(message),
        GitTextError::Failed(message) => CliError::runtime(message),
    }
}
