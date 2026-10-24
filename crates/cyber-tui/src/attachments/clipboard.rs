//! Reading an image from the system clipboard (`tui` → Large paste and image input).
//! Terminals never send image bytes, so `composer.paste_image` asks a platform tool.
//! Each tool runs from a fixed argument vector, never a shell string, and no user data
//! reaches it. The reader is picked at run time with `cfg!`, so every platform builds
//! and tests every argument vector.

use std::io::Read as _;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use base64::Engine as _;

/// How long a clipboard tool may run.
const TIMEOUT: Duration = Duration::from_secs(5);

/// The Windows reader: the clipboard image as base64 PNG, exit code 3 without one.
/// `Get-Clipboard -Format Image` exists only in Windows PowerShell 5.1, so the .NET
/// clipboard is read directly.
const POWERSHELL_SCRIPT: &str = "Add-Type -AssemblyName System.Windows.Forms; \
Add-Type -AssemblyName System.Drawing; \
$image = [System.Windows.Forms.Clipboard]::GetImage(); \
if ($image -eq $null) { exit 3 }; \
$stream = New-Object System.IO.MemoryStream; \
$image.Save($stream, [System.Drawing.Imaging.ImageFormat]::Png); \
[Convert]::ToBase64String($stream.ToArray())";

/// Why the clipboard gave no image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClipboardError {
    /// The platform tool is not installed; the text is the toast.
    MissingTool(&'static str),
    /// The image is over the byte cap.
    TooLarge,
    Failed(String),
}

/// The system clipboard's image, as file bytes.
pub trait Clipboard: Send + Sync {
    /// `Ok(None)` when the clipboard holds no image.
    fn image(&self, max_bytes: u64) -> Result<Option<Vec<u8>>, ClipboardError>;
}

/// How a reader prints the image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encoding {
    /// The raw bytes.
    Raw,
    /// AppleScript's `«data PNGf…»` hex literal.
    AppleHex,
    Base64,
}

/// One platform reader.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reader {
    pub argv: Vec<&'static str>,
    pub encoding: Encoding,
    /// The toast when the program is missing.
    pub missing: &'static str,
}

/// The operating systems with a reader.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    MacOs,
    Linux,
    Windows,
    Other,
}

impl Platform {
    pub fn current() -> Self {
        if cfg!(target_os = "macos") {
            Self::MacOs
        } else if cfg!(windows) {
            Self::Windows
        } else if cfg!(unix) {
            Self::Linux
        } else {
            Self::Other
        }
    }
}

/// The reader for `platform`; on Linux `wl-paste` under Wayland, else `xclip`.
pub fn reader(platform: Platform, wayland: bool) -> Option<Reader> {
    const LINUX_MISSING: &str = "Clipboard image needs wl-paste or xclip";
    Some(match platform {
        Platform::MacOs => Reader {
            argv: vec!["/usr/bin/osascript", "-e", "the clipboard as «class PNGf»"],
            encoding: Encoding::AppleHex,
            missing: "Clipboard image needs osascript",
        },
        Platform::Linux if wayland => Reader {
            argv: vec!["wl-paste", "--no-newline", "--type", "image/png"],
            encoding: Encoding::Raw,
            missing: LINUX_MISSING,
        },
        Platform::Linux => Reader {
            argv: vec!["xclip", "-selection", "clipboard", "-t", "image/png", "-o"],
            encoding: Encoding::Raw,
            missing: LINUX_MISSING,
        },
        Platform::Windows => Reader {
            argv: vec![
                "powershell.exe",
                "-NoProfile",
                "-NonInteractive",
                "-Sta",
                "-Command",
                POWERSHELL_SCRIPT,
            ],
            encoding: Encoding::Base64,
            missing: "Clipboard image needs powershell.exe",
        },
        Platform::Other => return None,
    })
}

/// The clipboard of this machine.
pub struct System;

impl Clipboard for System {
    fn image(&self, max_bytes: u64) -> Result<Option<Vec<u8>>, ClipboardError> {
        let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some();
        let reader = reader(Platform::current(), wayland).ok_or(ClipboardError::Failed(
            "no clipboard reader on this system".into(),
        ))?;
        read(&reader, max_bytes)
    }
}

/// Run `reader` and decode what it printed. A failing exit means no image: each tool
/// exits non-zero when the clipboard holds none.
pub fn read(reader: &Reader, max_bytes: u64) -> Result<Option<Vec<u8>>, ClipboardError> {
    let printed_cap = match reader.encoding {
        Encoding::Raw => max_bytes + 1,
        Encoding::AppleHex => max_bytes * 2 + 64,
        Encoding::Base64 => max_bytes.div_ceil(3) * 4 + 64,
    };
    let Some(printed) = run(reader, printed_cap)? else {
        return Ok(None);
    };
    if printed.len() as u64 > printed_cap {
        return Err(ClipboardError::TooLarge);
    }
    let bytes = match reader.encoding {
        Encoding::Raw => Some(printed),
        Encoding::AppleHex => apple_hex(&String::from_utf8_lossy(&printed)),
        Encoding::Base64 => base64::engine::general_purpose::STANDARD
            .decode(String::from_utf8_lossy(&printed).trim())
            .ok(),
    };
    match bytes {
        Some(bytes) if bytes.len() as u64 > max_bytes => Err(ClipboardError::TooLarge),
        Some(bytes) if crate::attachments::sniff(&bytes).is_some() => Ok(Some(bytes)),
        _ => Ok(None),
    }
}

/// What the program printed, at most `cap + 1` bytes; `None` when it exited non-zero.
fn run(reader: &Reader, cap: u64) -> Result<Option<Vec<u8>>, ClipboardError> {
    let mut child = Command::new(reader.argv[0])
        .args(&reader.argv[1..])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => ClipboardError::MissingTool(reader.missing),
            _ => ClipboardError::Failed(e.to_string()),
        })?;
    let stdout = child.stdout.take().expect("stdout is piped");
    // Reading stops at the cap and drops the pipe, so a larger image ends the tool.
    let output = std::thread::spawn(move || {
        let mut printed = Vec::new();
        let _ = stdout.take(cap + 1).read_to_end(&mut printed);
        printed
    });
    let deadline = Instant::now() + TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(ClipboardError::Failed(
                    "the clipboard tool timed out".into(),
                ));
            }
            Err(e) => return Err(ClipboardError::Failed(e.to_string())),
        }
    };
    let printed = output.join().unwrap_or_default();
    if printed.len() as u64 > cap {
        return Err(ClipboardError::TooLarge);
    }
    Ok(status.success().then_some(printed))
}

/// The bytes of AppleScript's `«data PNGf89504E47…»`.
pub fn apple_hex(printed: &str) -> Option<Vec<u8>> {
    let hex = printed
        .trim()
        .strip_prefix("«data PNGf")?
        .strip_suffix('»')?;
    if hex.len() % 2 != 0 {
        return None;
    }
    hex.as_bytes()
        .chunks(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok())
        .collect()
}

/// A clipboard with a fixed answer.
#[cfg(test)]
pub struct Fake(pub Result<Option<Vec<u8>>, ClipboardError>);

#[cfg(test)]
impl Clipboard for Fake {
    fn image(&self, max_bytes: u64) -> Result<Option<Vec<u8>>, ClipboardError> {
        match &self.0 {
            Ok(Some(bytes)) if bytes.len() as u64 > max_bytes => Err(ClipboardError::TooLarge),
            other => other.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_apple_hex_literal_decodes_to_png_bytes() {
        assert_eq!(
            apple_hex("«data PNGf89504E470D0A1A0A»\n"),
            Some(vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A])
        );
        assert_eq!(apple_hex("«data PNGf89504»"), None, "odd length");
        assert_eq!(apple_hex("«data PNGfZZ»"), None, "not hex");
        assert_eq!(apple_hex("«data TIFF4D4D»"), None, "another class");
        assert_eq!(apple_hex(""), None);
    }

    #[test]
    fn readers_use_fixed_argument_vectors() {
        let mac = reader(Platform::MacOs, false).unwrap();
        assert_eq!(
            mac.argv,
            ["/usr/bin/osascript", "-e", "the clipboard as «class PNGf»"]
        );
        assert_eq!(mac.encoding, Encoding::AppleHex);
        let wayland = reader(Platform::Linux, true).unwrap();
        assert_eq!(
            wayland.argv,
            ["wl-paste", "--no-newline", "--type", "image/png"]
        );
        let x11 = reader(Platform::Linux, false).unwrap();
        assert_eq!(
            x11.argv,
            ["xclip", "-selection", "clipboard", "-t", "image/png", "-o"]
        );
        assert_eq!(x11.missing, "Clipboard image needs wl-paste or xclip");
        let windows = reader(Platform::Windows, false).unwrap();
        assert_eq!(
            windows.argv[..5],
            [
                "powershell.exe",
                "-NoProfile",
                "-NonInteractive",
                "-Sta",
                "-Command"
            ]
        );
        assert!(windows.argv[5].contains("Clipboard]::GetImage()"));
        assert_eq!(windows.encoding, Encoding::Base64);
        assert_eq!(reader(Platform::Other, false), None);
    }

    #[test]
    fn a_missing_tool_names_itself() {
        let reader = Reader {
            argv: vec!["cyber-no-such-clipboard-tool"],
            encoding: Encoding::Raw,
            missing: "Clipboard image needs wl-paste or xclip",
        };
        assert_eq!(
            read(&reader, 1024),
            Err(ClipboardError::MissingTool(
                "Clipboard image needs wl-paste or xclip"
            ))
        );
    }
}
