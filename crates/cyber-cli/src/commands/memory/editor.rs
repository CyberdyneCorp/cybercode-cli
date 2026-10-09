//! Editors mutate retained private drafts; validated memory commits remain directory-bound.
use super::{mutation_admission, report_mutation, storage_error};
use crate::{cli::GlobalArgs, context::Context, error::CliError, output};
use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt};
use cap_std::fs::{Dir, OpenOptions};
use cyber_core::{env::EnvSource, memory::MemoryScope};
use serde_json::json;
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    process::Command,
};

pub(super) fn edit(
    owner: &mut MemoryScope<'_>,
    name: &str,
    ctx: &Context,
    global: &GlobalArgs,
) -> Result<(), CliError> {
    let command = editor_command(ctx)?;
    let review = owner.review_edit(name).map_err(storage_error)?;
    let initial=review.original().map(str::to_owned).unwrap_or_else(||format!("---\nname: {name}\ndescription: Replace with a one-line description\ntype: reference\n---\n\nReplace with a durable fact.\n"));
    let draft = Draft::new(&ctx.paths.data, &initial)?;
    let result = run_editor(&command, &draft.path).and_then(|()| {
        mutation_admission(ctx)?;
        let edited = draft.read()?;
        if edited == initial {
            return if output::is_json(global.format) {
                output::json(&json!({"name":name,"changed":false,"draft":draft.path}))
            } else {
                println!("Memory unchanged");
                Ok(())
            };
        }
        report_mutation(review.commit(&edited).map_err(storage_error)?, global)
    });
    result.map_err(|error| {
        error.with_hint(format!("Editor draft retained at {}", draft.path.display()))
    })
}

fn editor_command(ctx: &Context) -> Result<Vec<String>, CliError> {
    let editor = ctx
        .env
        .get("EDITOR")
        .filter(|s| !s.trim().is_empty())
        .or_else(|| ctx.env.get("VISUAL").filter(|s| !s.trim().is_empty()))
        .ok_or_else(|| CliError::usage("Set EDITOR or VISUAL to edit memory"))?;
    let args = shell_words::split(&editor)
        .map_err(|_| CliError::usage("Invalid EDITOR or VISUAL command quoting"))?;
    if args.is_empty() {
        return Err(CliError::usage("Empty editor command"));
    }
    Ok(args)
}

fn run_editor(args: &[String], path: &Path) -> Result<(), CliError> {
    let status = Command::new(&args[0])
        .args(&args[1..])
        .arg(path)
        .status()
        .map_err(|_| CliError::runtime("Could not start memory editor"))?;
    if !status.success() {
        return Err(CliError::runtime("Memory editor exited unsuccessfully"));
    }
    Ok(())
}

struct Draft {
    dir: Dir,
    path: PathBuf,
}
impl Draft {
    fn new(data: &Path, text: &str) -> Result<Self, CliError> {
        let root = Dir::open_ambient_dir(data, cap_std::ambient_authority())?;
        let mut builder = tempfile::Builder::new();
        builder.prefix(".memory-edit-");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            builder.permissions(std::fs::Permissions::from_mode(0o700));
        }
        let temp = builder.tempdir_in(data)?;
        let path = temp.keep();
        let name = path
            .file_name()
            .ok_or_else(|| CliError::runtime("Invalid editor draft directory"))?;
        use cap_fs_ext::DirExt;
        let dir = root.open_dir_nofollow(name)?;
        let mut options = OpenOptions::new();
        options
            .write(true)
            .create_new(true)
            .follow(FollowSymlinks::No);
        #[cfg(unix)]
        {
            use cap_std::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = dir.open_with("note.md", &options)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        Ok(Self {
            dir,
            path: path.join("note.md"),
        })
    }
    fn read(&self) -> Result<String, CliError> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if self
                .dir
                .try_clone()?
                .into_std_file()
                .metadata()?
                .permissions()
                .mode()
                & 0o077
                != 0
            {
                return Err(CliError::runtime(
                    "Editor draft directory must remain private",
                ));
            }
        }
        if !self.dir.symlink_metadata("note.md")?.is_file() {
            return Err(CliError::runtime("Editor draft must be a regular file"));
        }
        let mut options = OpenOptions::new();
        #[cfg(unix)]
        {
            use cap_std::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NONBLOCK);
        }
        options.read(true).follow(FollowSymlinks::No);
        let file = self.dir.open_with("note.md", &options)?.into_std();
        let metadata = file.metadata()?;
        if !metadata.is_file() {
            return Err(CliError::runtime("Editor draft must be a regular file"));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::{MetadataExt, PermissionsExt};
            if metadata.nlink() != 1 {
                return Err(CliError::runtime(
                    "Editor draft must not have hard-link aliases",
                ));
            }
            file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        }
        let mut bytes = Vec::new();
        file.take(1_048_577).read_to_end(&mut bytes)?;
        if bytes.len() > 1_048_576 {
            return Err(CliError::usage("Memory file exceeds 1 MiB"));
        }
        String::from_utf8(bytes).map_err(|_| CliError::usage("Editor draft must be UTF-8"))
    }
}
