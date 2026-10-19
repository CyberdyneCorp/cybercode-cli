//! `messaging`: cross-session messaging settings (`cross-session-messaging` → Disable
//! switch, Inbound controls, Message chain limit).
//!
//! `messaging.enabled` comes from the ordinary layers; `policy.messaging.enabled` comes
//! only from the OS-managed policy (merged under `policy` where no other layer can write),
//! so a `false` there wins over any user or project `true`.

use serde::{Deserialize, Serialize};
use serde_json::Value;

const KEY: &str = "messaging";
/// `messaging.max_hops` when unset, and its range.
pub const DEFAULT_MAX_HOPS: u32 = 8;
const HOPS: (u32, u32) = (1, 64);

/// What a target does with an inbound message.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum InboundPolicy {
    #[default]
    Deliver,
    Hold,
    Refuse,
}

impl InboundPolicy {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "deliver" => Some(Self::Deliver),
            "hold" => Some(Self::Hold),
            "refuse" => Some(Self::Refuse),
            _ => None,
        }
    }
}

/// `messaging.inbound`: the default for same-machine senders and the sender patterns of
/// each policy, matched with the permission wildcard syntax.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct InboundSettings {
    pub default: InboundPolicy,
    pub deliver: Vec<String>,
    pub hold: Vec<String>,
    pub refuse: Vec<String>,
}

/// The resolved `messaging` key of one Location.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MessagingSettings {
    /// False when `messaging.enabled` or `policy.messaging.enabled` is false (or, applied
    /// by the host, `CYBER_DISABLE_MESSAGING=1`).
    pub enabled: bool,
    pub inbound: InboundSettings,
    /// The longest message chain (`Message chain limit`).
    pub max_hops: u32,
}

impl Default for MessagingSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            inbound: InboundSettings::default(),
            max_hops: DEFAULT_MAX_HOPS,
        }
    }
}

impl MessagingSettings {
    /// Validate `messaging` (and `policy.messaging`) in a configuration value and apply
    /// the defaults. Errors name the offending path.
    pub fn from_config(config: &Value) -> Result<Self, String> {
        let mut settings = Self::default();
        if let Some(value) = config.get(KEY) {
            let map = value
                .as_object()
                .ok_or_else(|| format!("{KEY}: expected an object, got {value}"))?;
            for (key, value) in map {
                match key.as_str() {
                    "enabled" => settings.enabled = boolean(&format!("{KEY}.enabled"), value)?,
                    "inbound" => settings.inbound = inbound(value)?,
                    "max_hops" => settings.max_hops = hops(value)?,
                    other => return Err(format!("{KEY}.{other}: unknown key")),
                }
            }
        }
        if let Some(policy) = config.pointer("/policy/messaging/enabled") {
            settings.enabled &= boolean("policy.messaging.enabled", policy)?;
        }
        Ok(settings)
    }
}

fn boolean(path: &str, value: &Value) -> Result<bool, String> {
    value
        .as_bool()
        .ok_or_else(|| format!("{path}: expected true or false, got {value}"))
}

fn hops(value: &Value) -> Result<u32, String> {
    let (min, max) = HOPS;
    value
        .as_u64()
        .and_then(|n| u32::try_from(n).ok())
        .filter(|n| (min..=max).contains(n))
        .ok_or_else(|| {
            format!("{KEY}.max_hops: expected an integer from {min} to {max}, got {value}")
        })
}

fn inbound(value: &Value) -> Result<InboundSettings, String> {
    let map = value
        .as_object()
        .ok_or_else(|| format!("{KEY}.inbound: expected an object, got {value}"))?;
    let mut settings = InboundSettings::default();
    for (key, value) in map {
        let path = format!("{KEY}.inbound.{key}");
        match key.as_str() {
            "default" => {
                settings.default =
                    value
                        .as_str()
                        .and_then(InboundPolicy::parse)
                        .ok_or_else(|| {
                            format!("{path}: expected one of deliver, hold, refuse, got {value}")
                        })?;
            }
            "deliver" => settings.deliver = patterns(&path, value)?,
            "hold" => settings.hold = patterns(&path, value)?,
            "refuse" => settings.refuse = patterns(&path, value)?,
            _ => return Err(format!("{path}: unknown key")),
        }
    }
    Ok(settings)
}

/// A list of non-empty sender patterns.
fn patterns(path: &str, value: &Value) -> Result<Vec<String>, String> {
    let list = value
        .as_array()
        .ok_or_else(|| format!("{path}: expected an array of sender patterns, got {value}"))?;
    list.iter()
        .enumerate()
        .map(|(index, pattern)| {
            pattern
                .as_str()
                .filter(|pattern| !pattern.trim().is_empty())
                .map(str::to_owned)
                .ok_or_else(|| {
                    format!("{path}[{index}]: expected a non-empty pattern, got {pattern}")
                })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn defaults_apply_without_the_key() {
        let settings = MessagingSettings::from_config(&json!({})).unwrap();
        assert_eq!(settings, MessagingSettings::default());
        assert!(settings.enabled);
        assert_eq!(settings.max_hops, 8);
        assert_eq!(settings.inbound.default, InboundPolicy::Deliver);
    }

    #[test]
    fn every_key_is_read() {
        let settings = MessagingSettings::from_config(&json!({"messaging": {
            "enabled": false,
            "inbound": {"default": "hold", "deliver": ["frontend", "ses_01J*"],
                "hold": ["*@*"], "refuse": ["noisy-*"]},
            "max_hops": 64,
        }}))
        .unwrap();
        assert!(!settings.enabled);
        assert_eq!(settings.max_hops, 64);
        assert_eq!(
            settings.inbound,
            InboundSettings {
                default: InboundPolicy::Hold,
                deliver: vec!["frontend".into(), "ses_01J*".into()],
                hold: vec!["*@*".into()],
                refuse: vec!["noisy-*".into()],
            }
        );
    }

    #[test]
    fn the_policy_key_cannot_be_overridden_by_config() {
        let read = |config| MessagingSettings::from_config(&config).unwrap().enabled;
        assert!(!read(json!({"messaging": {"enabled": true},
            "policy": {"messaging": {"enabled": false}}})));
        assert!(!read(json!({"messaging": {"enabled": false},
            "policy": {"messaging": {"enabled": true}}})));
        assert!(read(json!({"policy": {"messaging": {"enabled": true}}})));
    }

    #[test]
    fn validation_errors_name_the_path() {
        for (config, path) in [
            (json!({"messaging": true}), "messaging:"),
            (
                json!({"messaging": {"enabled": "no"}}),
                "messaging.enabled:",
            ),
            (json!({"messaging": {"max_hops": 0}}), "messaging.max_hops:"),
            (
                json!({"messaging": {"max_hops": 65}}),
                "messaging.max_hops:",
            ),
            (
                json!({"messaging": {"max_hops": "8"}}),
                "messaging.max_hops:",
            ),
            (json!({"messaging": {"inbound": []}}), "messaging.inbound:"),
            (
                json!({"messaging": {"inbound": {"default": "drop"}}}),
                "messaging.inbound.default:",
            ),
            (
                json!({"messaging": {"inbound": {"hold": "*@*"}}}),
                "messaging.inbound.hold:",
            ),
            (
                json!({"messaging": {"inbound": {"refuse": ["a", ""]}}}),
                "messaging.inbound.refuse[1]:",
            ),
            (
                json!({"messaging": {"inbound": {"allow": []}}}),
                "messaging.inbound.allow:",
            ),
            (json!({"messaging": {"peers": {}}}), "messaging.peers:"),
            (
                json!({"policy": {"messaging": {"enabled": 0}}}),
                "policy.messaging.enabled:",
            ),
        ] {
            let error = MessagingSettings::from_config(&config).unwrap_err();
            assert!(error.starts_with(path), "{config}: {error}");
        }
    }
}
