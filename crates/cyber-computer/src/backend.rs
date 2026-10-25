use serde::{Deserialize, Serialize};

use crate::{
    Error, Geometry, Rect,
    control::{InputPermit, Target},
};

#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "ComputerSelector")]
pub struct Selector {
    pub app: Option<String>,
    pub display: Option<u32>,
}

impl Selector {
    pub fn validate(&self) -> Result<(), Error> {
        match (&self.app, self.display) {
            (Some(app), None)
                if !app.is_empty()
                    && app.len() <= 255
                    && app
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || b".-_".contains(&byte)) =>
            {
                Ok(())
            }
            (None, Some(_)) => Ok(()),
            _ => Err(Error::Input(
                "select exactly one application bundle ID or display",
            )),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[schemars(rename = "ComputerHealth")]
pub struct Health {
    pub supported: bool,
    pub platform: String,
    pub minimum_version: String,
    pub screen_capture: bool,
    pub accessibility: bool,
    pub binary: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[schemars(rename = "ComputerNativeTarget")]
pub struct NativeTarget {
    pub identity: String,
    pub app: Option<String>,
    pub pid: i32,
    pub window: u32,
    pub display: u32,
    pub launch_ms: u64,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub image_width: u32,
    pub image_height: u32,
    pub rotation: u16,
}

impl NativeTarget {
    pub fn target(&self) -> Target {
        Target {
            identity: self.identity.clone(),
        }
    }
    pub fn geometry(&self) -> Result<Geometry, Error> {
        Geometry::new(
            self.image_width,
            self.image_height,
            Rect {
                x: self.x,
                y: self.y,
                width: self.width,
                height: self.height,
            },
            self.rotation,
        )
    }
    pub fn selector(&self) -> Selector {
        if let Some(app) = &self.app {
            Selector {
                app: Some(app.clone()),
                display: None,
            }
        } else {
            Selector {
                app: None,
                display: Some(self.display),
            }
        }
    }
}

pub struct Capture {
    pub target: NativeTarget,
    pub png: Vec<u8>,
}

impl Capture {
    pub fn validate(&self) -> Result<(), Error> {
        self.target.geometry()?;
        if self.png.len() < 24
            || self.png.len() > 8 * 1024 * 1024
            || !self.png.starts_with(b"\x89PNG\r\n\x1a\n")
        {
            return Err(Error::Native("native capture is not a bounded PNG"));
        }
        let width = u32::from_be_bytes(self.png[16..20].try_into().expect("checked PNG header"));
        let height = u32::from_be_bytes(self.png[20..24].try_into().expect("checked PNG header"));
        if width != self.target.image_width || height != self.target.image_height {
            return Err(Error::Native(
                "capture dimensions do not match its coordinate frame",
            ));
        }
        Ok(())
    }
}

pub trait Backend: Send + Sync {
    /// Prompting is allowed only from the explicit owner setup endpoint.
    fn health(&self, prompt: bool) -> Result<Health, Error>;
    fn resolve(&self, selector: &Selector) -> Result<NativeTarget, Error>;
    fn capture(&self, target: &NativeTarget) -> Result<Capture, Error>;
    fn input(&self, target: &NativeTarget, permit: &InputPermit) -> Result<(), Error>;
}

pub struct Native;

#[cfg(not(target_os = "macos"))]
impl Backend for Native {
    fn health(&self, _prompt: bool) -> Result<Health, Error> {
        Ok(Health {
            supported: false,
            platform: std::env::consts::OS.into(),
            minimum_version: "macOS 14".into(),
            screen_capture: false,
            accessibility: false,
            binary: std::env::current_exe()
                .ok()
                .map(|path| path.display().to_string())
                .unwrap_or_default(),
        })
    }
    fn resolve(&self, _selector: &Selector) -> Result<NativeTarget, Error> {
        Err(Error::UnsupportedPlatform)
    }
    fn capture(&self, _target: &NativeTarget) -> Result<Capture, Error> {
        Err(Error::UnsupportedPlatform)
    }
    fn input(&self, _target: &NativeTarget, _permit: &InputPermit) -> Result<(), Error> {
        Err(Error::UnsupportedPlatform)
    }
}

#[cfg(target_os = "macos")]
#[path = "macos.rs"]
mod macos;
