use super::{InstallMethod, ServerDefinition, builtin_servers};
use crate::{config::LspSettings, env::EnvSource};
use serde::Serialize;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

/// Explicit search inputs keep discovery independent of process-global environment.
pub struct ExecutableSearch {
    pub location: PathBuf,
    pub directories: Vec<PathBuf>,
    pub cache_bin: PathBuf,
    pub suffixes: Vec<String>,
}
impl ExecutableSearch {
    pub fn new(location: &Path, cache: &Path, env: &impl EnvSource) -> Self {
        let directories = env
            .get("PATH")
            .map(|path| std::env::split_paths(&path).collect())
            .unwrap_or_default();
        #[cfg(windows)]
        let suffixes = env
            .get("PATHEXT")
            .unwrap_or_else(|| ".COM;.EXE;.BAT;.CMD".into())
            .split(';')
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect();
        #[cfg(not(windows))]
        let suffixes = Vec::new();
        Self {
            location: location.into(),
            directories,
            cache_bin: cache.join("bin"),
            suffixes,
        }
    }

    pub fn find(&self, executable: &str) -> Option<PathBuf> {
        if executable.is_empty() {
            return None;
        }
        let path = Path::new(executable);
        if path.is_absolute() || path.components().count() > 1 {
            return self.candidate(&self.location.join(path));
        }
        self.directories
            .iter()
            .chain(std::iter::once(&self.cache_bin))
            .find_map(|directory| self.candidate(&self.location.join(directory).join(path)))
    }

    fn candidate(&self, path: &Path) -> Option<PathBuf> {
        if executable_file(path) {
            return path.canonicalize().ok();
        }
        self.suffixes.iter().find_map(|suffix| {
            let mut candidate = path.as_os_str().to_os_string();
            candidate.push(suffix);
            let candidate = PathBuf::from(candidate);
            executable_file(&candidate)
                .then(|| candidate.canonicalize().ok())
                .flatten()
        })
    }
}
fn executable_file(path: &Path) -> bool {
    let Ok(metadata) = path.metadata() else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct DetectedServer {
    pub definition: ServerDefinition,
    pub enabled: bool,
    pub installed: bool,
    pub executable: Option<PathBuf>,
}

/// Merge already-trusted overrides, then inspect local executables. Never installs/spawns.
pub fn detect_servers(settings: &LspSettings, search: &ExecutableSearch) -> Vec<DetectedServer> {
    let mut definitions: BTreeMap<_, _> = builtin_servers()
        .into_iter()
        .map(|s| (s.id.clone(), s))
        .collect();
    for (id, configured) in &settings.servers {
        let definition = definitions
            .entry(id.clone())
            .or_insert_with(|| ServerDefinition {
                id: id.clone(),
                extensions: Vec::new(),
                root_markers: Vec::new(),
                command: Vec::new(),
                env: BTreeMap::new(),
                initialization_options: None,
                install: InstallMethod::Custom,
            });
        if let Some(command) = &configured.command {
            definition.command = command.clone();
            definition.install = InstallMethod::Custom;
        }
        if let Some(extensions) = &configured.extensions {
            definition.extensions = extensions.clone();
        }
        if let Some(markers) = &configured.root_markers {
            definition.root_markers = markers.clone();
        }
        definition.env.extend(configured.env.clone());
        if configured.initialization_options.is_some() {
            definition.initialization_options = configured.initialization_options.clone();
        }
    }
    definitions
        .into_iter()
        .map(|(id, definition)| {
            let executable = definition
                .command
                .first()
                .and_then(|command| search.find(command));
            let installed = executable.is_some();
            let disabled = settings.servers.get(&id).is_some_and(|s| s.disabled);
            DetectedServer {
                definition,
                enabled: settings.enabled && installed && !disabled,
                installed,
                executable,
            }
        })
        .collect()
}

/// Nearest marker wins; external/symlink-routed files cannot select a Location root.
pub fn server_root(location: &Path, file: &Path, markers: &[String]) -> Result<PathBuf, String> {
    let location = location
        .canonicalize()
        .map_err(|_| "Language-server Location is unavailable")?;
    if !location.is_dir() {
        return Err("Language-server Location must be a directory".into());
    }
    let file = location
        .join(file)
        .canonicalize()
        .map_err(|_| "Language-server file is unavailable")?;
    if !file.starts_with(&location) || !file.is_file() {
        return Err("Language-server file must be inside its Location".into());
    }
    for marker in markers {
        if marker.trim().is_empty() || marker.contains('\0') {
            return Err("Language-server root markers must be nonempty names".into());
        }
        if Path::new(marker)
            .components()
            .any(|part| !matches!(part, std::path::Component::Normal(_)))
        {
            return Err("Language-server root markers must be relative names".into());
        }
    }
    let mut directory = file.parent().ok_or("Language-server file has no parent")?;
    loop {
        if markers.iter().any(|marker| directory.join(marker).exists()) {
            return Ok(directory.into());
        }
        if directory == location {
            return Ok(location);
        }
        directory = directory
            .parent()
            .ok_or("Language-server root is unavailable")?;
    }
}
