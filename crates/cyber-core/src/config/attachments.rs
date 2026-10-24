//! `attachments`: limits and normalization of prompt attachments (`session-runtime` →
//! Attachment normalization).

use serde::Serialize;
use serde_json::{Map, Value};

const KEY: &str = "attachments";
const MIB: u64 = 1024 * 1024;

/// The resolved `attachments` key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct AttachmentSettings {
    /// Largest decoded attachment, in bytes (`AttachmentTooLargeError` above it).
    pub max_bytes: u64,
    /// Images larger than this on either side are downscaled to fit.
    pub image_max_dimension: u32,
    /// Images larger than this, after any downscale, are re-encoded as JPEG quality 85.
    pub image_max_bytes: u64,
    /// PDFs longer than this are attached with a page-range note.
    pub pdf_max_pages: u32,
    /// Also store the original of a normalized image.
    pub keep_original: bool,
}

impl Default for AttachmentSettings {
    fn default() -> Self {
        Self {
            max_bytes: 20 * MIB,
            image_max_dimension: 2000,
            image_max_bytes: 5 * MIB,
            pdf_max_pages: 100,
            keep_original: false,
        }
    }
}

impl AttachmentSettings {
    /// Validate `attachments` in a configuration value and apply its defaults.
    pub fn from_config(config: &Value) -> Result<Self, String> {
        let mut settings = Self::default();
        let Some(value) = config.get(KEY) else {
            return Ok(settings);
        };
        for (key, value) in object(KEY, value)? {
            match key.as_str() {
                "max_bytes" => settings.max_bytes = integer(key, value, 1, 512 * MIB)?,
                "keep_original" => settings.keep_original = boolean(key, value)?,
                "image" => settings.image(value)?,
                "pdf" => settings.pdf(value)?,
                other => return Err(format!("{KEY}.{other}: unknown key")),
            }
        }
        Ok(settings)
    }

    fn image(&mut self, value: &Value) -> Result<(), String> {
        for (key, value) in object(&format!("{KEY}.image"), value)? {
            let path = format!("image.{key}");
            match key.as_str() {
                "max_dimension" => {
                    self.image_max_dimension = integer(&path, value, 64, 16_384)? as u32;
                }
                "max_bytes" => self.image_max_bytes = integer(&path, value, 64 * 1024, 512 * MIB)?,
                _ => return Err(format!("{KEY}.{path}: unknown key")),
            }
        }
        Ok(())
    }

    fn pdf(&mut self, value: &Value) -> Result<(), String> {
        for (key, value) in object(&format!("{KEY}.pdf"), value)? {
            let path = format!("pdf.{key}");
            match key.as_str() {
                "max_pages" => self.pdf_max_pages = integer(&path, value, 1, 100_000)? as u32,
                _ => return Err(format!("{KEY}.{path}: unknown key")),
            }
        }
        Ok(())
    }
}

fn object<'a>(path: &str, value: &'a Value) -> Result<&'a Map<String, Value>, String> {
    value
        .as_object()
        .ok_or_else(|| format!("{path}: expected an object, got {value}"))
}

fn integer(key: &str, value: &Value, min: u64, max: u64) -> Result<u64, String> {
    value
        .as_u64()
        .filter(|n| (min..=max).contains(n))
        .ok_or_else(|| format!("{KEY}.{key}: expected an integer from {min} to {max}, got {value}"))
}

fn boolean(key: &str, value: &Value) -> Result<bool, String> {
    value
        .as_bool()
        .ok_or_else(|| format!("{KEY}.{key}: expected true or false, got {value}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn defaults_apply_and_nested_limits_are_read() {
        assert_eq!(
            AttachmentSettings::from_config(&json!({})).unwrap(),
            AttachmentSettings::default()
        );
        let settings = AttachmentSettings::from_config(&json!({"attachments": {
            "max_bytes": 1048576, "keep_original": true,
            "image": {"max_dimension": 1000, "max_bytes": 524288}, "pdf": {"max_pages": 10},
        }}))
        .unwrap();
        assert_eq!(settings.max_bytes, MIB);
        assert!(settings.keep_original);
        assert_eq!(settings.image_max_dimension, 1000);
        assert_eq!(settings.image_max_bytes, 512 * 1024);
        assert_eq!(settings.pdf_max_pages, 10);
    }

    #[test]
    fn invalid_values_name_their_key() {
        let read = |value: Value| AttachmentSettings::from_config(&json!({"attachments": value}));
        assert!(
            read(json!({"max_bytes": 0}))
                .unwrap_err()
                .contains("attachments.max_bytes")
        );
        assert!(
            read(json!({"image": {"max_dimension": "big"}}))
                .unwrap_err()
                .contains("attachments.image.max_dimension")
        );
        assert!(
            read(json!({"pdf": {"pages": 1}}))
                .unwrap_err()
                .contains("attachments.pdf.pages")
        );
        assert!(
            read(json!({"keep": true}))
                .unwrap_err()
                .contains("attachments.keep")
        );
    }
}
