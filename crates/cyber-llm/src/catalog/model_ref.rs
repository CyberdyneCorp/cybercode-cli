//! `provider/model[#variant]` (`provider-catalog` → Model reference format).

use std::fmt;

use super::CatalogError;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ModelRef {
    pub provider: String,
    pub model: String,
    pub variant: Option<String>,
}

impl ModelRef {
    /// Split at the first `/` (model IDs may contain slashes) and the last `#`.
    pub fn parse(value: &str) -> Result<Self, CatalogError> {
        let invalid = || CatalogError::InvalidRef(value.to_string());
        let (provider, rest) = value.split_once('/').ok_or_else(invalid)?;
        let (model, variant) = match rest.rsplit_once('#') {
            Some((model, variant)) => (model, Some(variant.to_string())),
            None => (rest, None),
        };
        if provider.is_empty() || model.is_empty() || variant.as_deref() == Some("") {
            return Err(invalid());
        }
        Ok(Self {
            provider: provider.into(),
            model: model.into(),
            variant,
        })
    }

    /// `provider/model` without the variant.
    pub fn base(&self) -> String {
        format!("{}/{}", self.provider, self.model)
    }
}

impl fmt::Display for ModelRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.variant {
            Some(v) => write!(f, "{}/{}#{v}", self.provider, self.model),
            None => write!(f, "{}/{}", self.provider, self.model),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slashes_in_model_and_variant() {
        let r = ModelRef::parse("openrouter/meta/llama-4#high").unwrap();
        assert_eq!(
            (r.provider.as_str(), r.model.as_str(), r.variant.as_deref()),
            ("openrouter", "meta/llama-4", Some("high"))
        );
        assert_eq!(r.to_string(), "openrouter/meta/llama-4#high");
        for bad in ["nope", "/x", "a/", "a/b#"] {
            assert!(ModelRef::parse(bad).is_err(), "{bad}");
        }
    }
}
