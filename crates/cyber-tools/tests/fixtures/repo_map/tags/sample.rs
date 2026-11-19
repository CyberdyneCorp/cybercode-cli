//! Module docs.
use std::path::Path;

/// Settings.
#[derive(Debug)]
pub struct Config {
    pub name: String,
}

pub const LIMIT: usize = 10;

pub trait Loader {
    fn load(&self, path: &Path) -> Result<Config, String>;
}

impl Config {
    /// Builds one.
    pub fn new(name: &str) -> Self {
        Config { name: name.into() }
    }
}

pub fn load(path: &Path) -> Result<Config, String> { // trailing note
    let _ = path;
    Ok(Config::new("x"))
}

mod inner {
    pub fn helper() {}
}

macro_rules! shout {
    () => {};
}
