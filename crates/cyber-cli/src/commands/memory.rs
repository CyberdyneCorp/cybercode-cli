//! Explicit local memory management without model or database startup.
mod editor;

use crate::{cli::GlobalArgs, context::Context, error::CliError, output};
use clap::{Args, Subcommand};
use cyber_core::memory::{self, MemoryCatalog, MemorySettings, MemoryStorageError, MemoryStore};
use serde_json::json;

#[derive(Debug, Args)]
pub struct MemoryArgs {
    /// Manage global memory instead of this repository's memory.
    #[arg(long, global = true)]
    pub global: bool,
    #[command(subcommand)]
    pub command: MemoryCmd,
}

#[derive(Debug, Subcommand)]
pub enum MemoryCmd {
    /// List note metadata and invalid-file diagnostics.
    List,
    /// Print a memory's content.
    Show { name: String },
    /// Edit a private draft with EDITOR (or VISUAL), then validate and commit it.
    Edit { name: String },
    /// Delete a note and update its index through the recovery journal.
    Delete { name: String },
    /// Print the memory directory without creating it.
    Path,
}

pub fn run(args: MemoryArgs, ctx: &Context, global: &GlobalArgs) -> Result<(), CliError> {
    let mutation = matches!(
        args.command,
        MemoryCmd::Edit { .. } | MemoryCmd::Delete { .. }
    );
    if mutation {
        mutation_admission(ctx)?;
    }
    match &args.command {
        MemoryCmd::Show { name } | MemoryCmd::Edit { name } | MemoryCmd::Delete { name } => {
            memory::validate_name(name).map_err(|e| CliError::usage(e.to_string()))?
        }
        _ => {}
    }
    let project = if args.global {
        "global".into()
    } else {
        cyber_core::project::identify(&ctx.location).id
    };
    let path =
        memory::directory(&ctx.paths.data, &project).map_err(|e| CliError::usage(e.to_string()))?;
    if matches!(args.command, MemoryCmd::Path) {
        return if output::is_json(global.format) {
            output::json(&json!({"path":path,"project_id":project}))
        } else {
            println!("{}", path.display());
            Ok(())
        };
    }
    let existing = MemoryStore::existing(&ctx.paths.data, &project).map_err(storage_error)?;
    let store = match existing {
        Some(store) => store,
        None if matches!(args.command, MemoryCmd::List) => {
            return report_catalog(MemoryCatalog::default(), global);
        }
        None if matches!(args.command, MemoryCmd::Edit { .. }) => {
            MemoryStore::open(&ctx.paths.data, &project).map_err(storage_error)?
        }
        None => return Err(storage_error(MemoryStorageError::NotFound)),
    };
    let mut owner = store.claim().map_err(storage_error)?;
    match args.command {
        MemoryCmd::List => report_catalog(owner.list().map_err(storage_error)?, global),
        MemoryCmd::Show { name } => {
            let note = owner.read(&name).map_err(storage_error)?;
            if output::is_json(global.format) {
                output::json(&json!({"metadata":note.metadata,"body":note.body}))
            } else {
                println!("{}", note.body);
                Ok(())
            }
        }
        MemoryCmd::Delete { name } => {
            report_mutation(owner.delete(&name).map_err(storage_error)?, global)
        }
        MemoryCmd::Edit { name } => editor::edit(&mut owner, &name, ctx, global),
        MemoryCmd::Path => unreachable!("handled before storage admission"),
    }
}

pub fn debug(ctx: &Context, global: &GlobalArgs, use_global: bool) -> Result<(), CliError> {
    run(
        MemoryArgs {
            global: use_global,
            command: MemoryCmd::List,
        },
        ctx,
        global,
    )
}

fn mutation_admission(ctx: &Context) -> Result<(), CliError> {
    let resolved = ctx.config()?;
    let settings = MemorySettings::from_config(&resolved.value, &ctx.env)
        .map_err(|e| CliError::usage(e.to_string()))?;
    if !settings.enabled {
        return Err(CliError::usage("Memory is disabled"));
    }
    if !settings.generate {
        return Err(CliError::usage("Memory is read-only"));
    }
    if !cfg!(unix) {
        return Err(CliError::unavailable(
            "Memory mutations",
            "M1.4 native storage privacy",
        ));
    }
    Ok(())
}

fn report_catalog(catalog: MemoryCatalog, global: &GlobalArgs) -> Result<(), CliError> {
    if output::is_json(global.format) {
        return output::json(
            &json!({"memories":catalog.memories,"invalid":catalog.invalid.into_iter().map(|e|json!({"filename":e.filename,"diagnostic":e.diagnostic})).collect::<Vec<_>>()}),
        );
    }
    for note in catalog.memories {
        println!("{} — {}", note.name, note.description);
    }
    for entry in catalog.invalid {
        eprintln!("Invalid {}: {}", entry.filename, entry.diagnostic);
    }
    Ok(())
}

fn report_mutation(receipt: memory::MemoryMutation, global: &GlobalArgs) -> Result<(), CliError> {
    if output::is_json(global.format) {
        return output::json(&receipt);
    }
    println!(
        "{} {}",
        if receipt.deleted { "Deleted" } else { "Saved" },
        receipt.name
    );
    Ok(())
}

fn storage_error(error: MemoryStorageError) -> CliError {
    CliError::runtime(error.to_string())
}
