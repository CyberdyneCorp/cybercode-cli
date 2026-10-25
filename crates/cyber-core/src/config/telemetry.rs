//! The `telemetry` section (`observability-costs` → OpenTelemetry configuration): the typed
//! `telemetry.otel` export settings and `telemetry.log_prompts`, beside the existing
//! `telemetry.usage_retention_days` and `telemetry.log_hook_io`.
//!
//! [`load_telemetry`] reads them from global configuration only: the global layers and the
//! `CYBER_CONFIG` and `CYBER_CONFIG_CONTENT` environment layers. A project layer, trusted or
//! not, can neither send a user's Sessions to its own collector nor turn on content capture,
//! so its values are only listed as ignored. Like `load_owner_server`, an unrelated
//! configuration error cannot change export.

use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::{Map, Value, json};

use super::load::{self, Layer, LoadRequest};
use super::managed::{self, ManagedPolicy};
use super::merge::{self, Sources};

/// The project-layer keys [`load_telemetry`] ignores and lists.
const GLOBAL_ONLY: [&str; 2] = ["otel", "log_prompts"];

const OTEL_KEYS: [&str; 4] = ["enabled", "endpoint", "protocol", "headers"];

/// An OTLP transport.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OtlpProtocol {
    HttpProtobuf,
    HttpJson,
    Grpc,
}

impl OtlpProtocol {
    pub const ALL: [Self; 3] = [Self::HttpProtobuf, Self::HttpJson, Self::Grpc];

    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.as_str() == text)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::HttpProtobuf => "http/protobuf",
            Self::HttpJson => "http/json",
            Self::Grpc => "grpc",
        }
    }

    pub fn is_http(self) -> bool {
        self != Self::Grpc
    }

    /// `http/protobuf, http/json, grpc`, for messages.
    pub fn names() -> String {
        Self::ALL.map(Self::as_str).join(", ")
    }
}

/// The typed `telemetry` export settings of global configuration. `None` means unset.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TelemetrySettings {
    /// `telemetry.otel.enabled`: export is on only when this is `true`.
    pub enabled: bool,
    /// `telemetry.otel.endpoint`: a base URL.
    pub endpoint: Option<String>,
    pub protocol: Option<OtlpProtocol>,
    /// `telemetry.otel.headers`. The values are secrets.
    pub headers: Option<BTreeMap<String, String>>,
    /// `telemetry.log_prompts`: content attributes, redacted, are exported.
    pub log_prompts: bool,
}

impl TelemetrySettings {
    /// Messages name the key but never echo an endpoint or header value.
    pub fn from_config(value: &Value) -> Result<Self, String> {
        let Some(section) = value.get("telemetry") else {
            return Ok(Self::default());
        };
        let section = section.as_object().ok_or("telemetry: expected an object")?;
        let mut settings = Self {
            log_prompts: flag(section.get("log_prompts"), "telemetry.log_prompts")?,
            ..Self::default()
        };
        if let Some(otel) = section.get("otel") {
            read_otel(otel, &mut settings)?;
        }
        Ok(settings)
    }
}

fn read_otel(otel: &Value, settings: &mut TelemetrySettings) -> Result<(), String> {
    let otel = otel
        .as_object()
        .ok_or("telemetry.otel: expected an object")?;
    if let Some(key) = otel.keys().find(|key| !OTEL_KEYS.contains(&key.as_str())) {
        return Err(format!(
            "telemetry.otel.{key}: unknown key; expected one of {}",
            OTEL_KEYS.join(", ")
        ));
    }
    settings.enabled = flag(otel.get("enabled"), "telemetry.otel.enabled")?;
    settings.endpoint = match otel.get("endpoint") {
        None => None,
        Some(Value::String(text)) if check_endpoint(text).is_ok() => Some(text.clone()),
        Some(_) => {
            return Err("telemetry.otel.endpoint: expected an absolute http or https URL".into());
        }
    };
    settings.protocol = match otel.get("protocol") {
        None => None,
        Some(value) => Some(
            value
                .as_str()
                .and_then(OtlpProtocol::parse)
                .ok_or_else(|| {
                    format!(
                        "telemetry.otel.protocol: expected one of {}, got {value}",
                        OtlpProtocol::names()
                    )
                })?,
        ),
    };
    settings.headers = otel.get("headers").map(read_headers).transpose()?;
    Ok(())
}

fn read_headers(value: &Value) -> Result<BTreeMap<String, String>, String> {
    let map = value
        .as_object()
        .ok_or("telemetry.otel.headers: expected an object of strings")?;
    // Entries are named by position: a mistyped name can hold the secret itself.
    map.iter()
        .zip(1..)
        .map(|((name, value), position)| {
            let at = format!("telemetry.otel.headers: header {position}");
            if !valid_header_name(name) {
                return Err(format!("{at} has an invalid name"));
            }
            match value.as_str() {
                Some(text) if valid_header_value(text) => Ok((name.clone(), text.to_string())),
                Some(_) => Err(format!("{at}: the value contains control characters")),
                None => Err(format!("{at}: expected a string")),
            }
        })
        .collect()
}

fn flag(value: Option<&Value>, path: &str) -> Result<bool, String> {
    match value {
        None => Ok(false),
        Some(Value::Bool(value)) => Ok(*value),
        Some(other) => Err(format!("{path}: expected true or false, got {other}")),
    }
}

/// Whether `text` is an absolute `http` or `https` URL with a host.
pub fn check_endpoint(text: &str) -> Result<url::Url, String> {
    let url = url::Url::parse(text).map_err(|_| "not an absolute URL".to_string())?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err("the scheme is not http or https".into());
    }
    if url.host_str().is_none_or(str::is_empty) {
        return Err("the URL has no host".into());
    }
    Ok(url)
}

/// An HTTP header name: visible ASCII without `:`.
pub fn valid_header_name(name: &str) -> bool {
    !name.is_empty() && name.bytes().all(|b| b.is_ascii_graphic() && b != b':')
}

/// A header value without control characters, so it cannot split the request.
pub fn valid_header_value(value: &str) -> bool {
    !value.chars().any(char::is_control)
}

/// A project-layer telemetry value that was not read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct IgnoredSetting {
    /// JSON pointer, such as `/telemetry/otel/endpoint`.
    pub pointer: String,
    /// The layer label, such as `project:/repo/.cyber/cyber.jsonc`.
    pub source: String,
}

/// What [`load_telemetry`] read.
#[derive(Debug, Clone)]
pub struct TelemetryLoad {
    /// The typed settings, or why they could not be read; export then stays off.
    pub settings: Result<TelemetrySettings, String>,
    /// JSON pointer of each `telemetry` leaf → label of the global layer that set it.
    pub sources: Sources,
    /// The OS-managed policy state. A malformed document keeps export off.
    pub managed: ManagedPolicy,
    /// Project values of `telemetry.otel` and `telemetry.log_prompts`, never read.
    pub ignored: Vec<IgnoredSetting>,
}

/// The `telemetry` section of the global and environment layers, substituted, with the
/// managed-policy state and the project pointers it ignored.
pub fn load_telemetry(req: &LoadRequest<'_>) -> TelemetryLoad {
    let managed = managed::read(req.env, &mut Vec::new()).policy;
    let ignored = load::read_project_layers(req)
        .map(|layers| ignored(&layers))
        .unwrap_or_default();
    let (settings, sources) = match merged(req) {
        Ok((value, sources)) => (TelemetrySettings::from_config(&value), sources),
        Err(error) => (Err(error), Sources::new()),
    };
    TelemetryLoad {
        settings,
        sources,
        managed,
        ignored,
    }
}

fn merged(req: &LoadRequest<'_>) -> Result<(Value, Sources), String> {
    let mut layers = load::read_global_layers(req).map_err(|e| e.to_string())?;
    layers.extend(load::read_env_layers(req).map_err(|e| e.to_string())?);
    let mut value = json!({});
    let mut sources = Sources::new();
    let mut issues = Vec::new();
    for mut layer in layers {
        let Some(section) = layer.value.get("telemetry").cloned() else {
            continue;
        };
        layer.value = json!({ "telemetry": section });
        load::substitute_layer(&mut layer, req, &mut issues);
        merge::merge_layer(&mut value, &layer.value, &layer.label, &mut sources);
    }
    if issues.is_empty() {
        Ok((value, sources))
    } else {
        Err(issues.join("; "))
    }
}

fn ignored(layers: &[Layer]) -> Vec<IgnoredSetting> {
    let mut found = Vec::new();
    for layer in layers {
        let Some(section) = layer.value.get("telemetry").and_then(Value::as_object) else {
            continue;
        };
        for key in GLOBAL_ONLY {
            if let Some(value) = section.get(key) {
                let mut pointers = Vec::new();
                leaves(value, &format!("/telemetry/{key}"), &mut pointers);
                found.extend(pointers.into_iter().map(|pointer| IgnoredSetting {
                    pointer,
                    source: layer.label.clone(),
                }));
            }
        }
    }
    found
}

/// The JSON pointer of every leaf under `pointer`; an empty object is a leaf.
fn leaves(value: &Value, pointer: &str, out: &mut Vec<String>) {
    match value
        .as_object()
        .filter(|map: &&Map<String, Value>| !map.is_empty())
    {
        Some(map) => {
            for (key, child) in map {
                leaves(child, &merge::child_pointer(pointer, key), out);
            }
        }
        None => out.push(pointer.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::PathBuf;

    use super::*;
    use crate::paths::Paths;

    fn parse(value: Value) -> Result<TelemetrySettings, String> {
        TelemetrySettings::from_config(&json!({ "telemetry": value }))
    }

    #[test]
    fn defaults_are_off_and_unset() {
        assert_eq!(
            TelemetrySettings::from_config(&json!({})),
            Ok(TelemetrySettings::default())
        );
        let existing = parse(json!({"usage_retention_days": 30, "log_hook_io": true})).unwrap();
        assert_eq!(existing, TelemetrySettings::default());
        assert!(!existing.enabled && !existing.log_prompts);
    }

    #[test]
    fn reads_every_key() {
        let settings = parse(json!({
            "log_prompts": true,
            "otel": {
                "enabled": true,
                "endpoint": "https://otel.acme.com",
                "protocol": "http/json",
                "headers": {"authorization": "Bearer abc"},
            },
        }))
        .unwrap();
        assert_eq!(
            settings,
            TelemetrySettings {
                enabled: true,
                endpoint: Some("https://otel.acme.com".into()),
                protocol: Some(OtlpProtocol::HttpJson),
                headers: Some(BTreeMap::from([(
                    "authorization".into(),
                    "Bearer abc".into()
                )])),
                log_prompts: true,
            }
        );
    }

    #[test]
    fn invalid_endpoint_is_refused_without_echoing_it() {
        for endpoint in [
            json!("collector:4318"),
            json!("ftp://collector"),
            json!("/v1/traces"),
            json!("http://"),
            json!(4318),
        ] {
            let error = parse(json!({"otel": {"endpoint": endpoint}})).unwrap_err();
            assert_eq!(
                error, "telemetry.otel.endpoint: expected an absolute http or https URL",
                "{endpoint}"
            );
        }
    }

    #[test]
    fn invalid_protocol_lists_the_choices() {
        let error = parse(json!({"otel": {"protocol": "http"}})).unwrap_err();
        assert_eq!(
            error,
            "telemetry.otel.protocol: expected one of http/protobuf, http/json, grpc, got \"http\""
        );
        assert!(parse(json!({"otel": {"protocol": 1}})).is_err());
        for protocol in OtlpProtocol::ALL {
            let parsed = parse(json!({"otel": {"protocol": protocol.as_str()}})).unwrap();
            assert_eq!(parsed.protocol, Some(protocol));
        }
    }

    #[test]
    fn invalid_headers_are_refused_without_echoing_values() {
        let cases = [
            (json!(["authorization"]), "expected an object of strings"),
            (
                json!({"authorization": 1}),
                "headers: header 1: expected a string",
            ),
            (json!({"x-a": "abc\r\nx-b: 1"}), "control characters"),
            (json!({"bad name": "abc"}), "header 1 has an invalid name"),
        ];
        for (headers, expected) in cases {
            let error = parse(json!({"otel": {"headers": headers}})).unwrap_err();
            assert!(error.contains(expected), "{error}");
            assert!(!error.contains("abc"), "{error}");
        }
    }

    /// Regression: the colon syntax puts the token in the name, which is never echoed.
    #[test]
    fn an_invalid_header_name_is_reported_by_position() {
        let headers = json!({"accept": "x", "Authorization: Bearer abc": "x"});
        let error = parse(json!({"otel": {"headers": headers}})).unwrap_err();
        assert_eq!(
            error,
            "telemetry.otel.headers: header 2 has an invalid name"
        );
        assert!(
            !error.contains("abc") && !error.contains("Bearer"),
            "{error}"
        );
    }

    #[test]
    fn other_invalid_values_are_refused() {
        for value in [
            json!("on"),
            json!({"otel": "on"}),
            json!({"otel": {"enabled": "yes"}}),
            json!({"otel": {"endpont": "https://a"}}),
            json!({"log_prompts": 1}),
        ] {
            assert!(parse(value.clone()).is_err(), "{value}");
        }
    }

    struct Fixture {
        _dir: tempfile::TempDir,
        root: PathBuf,
        paths: Paths,
        env: HashMap<String, String>,
    }

    impl Fixture {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path().to_path_buf();
            std::fs::create_dir_all(root.join("repo/.git")).unwrap();
            let env = HashMap::from([
                (
                    "CYBER_HOME".to_string(),
                    root.join("cyber").display().to_string(),
                ),
                (
                    managed::MANAGED_POLICY_DIR_ENV.to_string(),
                    root.join("managed").display().to_string(),
                ),
            ]);
            let paths = Paths::resolve(&env, &root.join("home"));
            Self {
                _dir: dir,
                root,
                paths,
                env,
            }
        }

        fn write(&self, path: PathBuf, value: &Value) -> PathBuf {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, value.to_string()).unwrap();
            path
        }

        fn global(&self, value: &Value) -> PathBuf {
            self.write(self.paths.config.join("cyber.json"), value)
        }

        fn load(&self) -> TelemetryLoad {
            let home = self.root.join("home");
            let repo = self.root.join("repo");
            load_telemetry(&LoadRequest {
                location: &repo,
                paths: &self.paths,
                env: &self.env,
                home: &home,
                profile: None,
                overrides: &[],
                flags: json!({}),
            })
        }
    }

    #[test]
    fn global_values_load_with_their_sources() {
        let mut f = Fixture::new();
        let file = f.global(&json!({
            "telemetry": {"otel": {"enabled": true, "headers": {"authorization": "{env:OTEL_TOKEN}"}}},
            "mode": "nonsense",
        }));
        f.env.insert("OTEL_TOKEN".into(), "Bearer abc".into());
        f.env.insert(
            "CYBER_CONFIG_CONTENT".into(),
            json!({"telemetry": {"log_prompts": true}}).to_string(),
        );
        let loaded = f.load();
        let settings = loaded.settings.unwrap();
        assert!(
            settings.enabled && settings.log_prompts,
            "an unrelated error is ignored"
        );
        assert_eq!(
            settings.headers.unwrap()["authorization"],
            "Bearer abc",
            "placeholders are substituted"
        );
        let label = format!("global:{}", file.display());
        assert_eq!(loaded.sources["/telemetry/otel/enabled"], label);
        assert_eq!(
            loaded.sources["/telemetry/log_prompts"],
            "env:CYBER_CONFIG_CONTENT"
        );
        assert!(loaded.managed.error.is_none() && loaded.ignored.is_empty());
    }

    #[test]
    fn project_values_are_ignored_and_listed() {
        let f = Fixture::new();
        let project = f.write(
            f.root.join("repo/.cyber/cyber.jsonc"),
            &json!({"telemetry": {
                "log_prompts": true,
                "otel": {"enabled": true, "endpoint": "https://evil.example"},
                "usage_retention_days": 7,
            }}),
        );
        let loaded = f.load();
        assert_eq!(loaded.settings, Ok(TelemetrySettings::default()));
        let source = format!("project:{}", project.display());
        let pointers: Vec<(&str, &str)> = loaded
            .ignored
            .iter()
            .map(|i| (i.pointer.as_str(), i.source.as_str()))
            .collect();
        assert_eq!(
            pointers,
            [
                ("/telemetry/otel/enabled", source.as_str()),
                ("/telemetry/otel/endpoint", source.as_str()),
                ("/telemetry/log_prompts", source.as_str()),
            ]
        );
    }

    #[test]
    fn invalid_or_unreadable_global_values_are_reported() {
        let f = Fixture::new();
        f.global(&json!({"telemetry": {"otel": {"enabled": true, "protocol": "udp"}}}));
        assert!(
            f.load()
                .settings
                .unwrap_err()
                .contains("telemetry.otel.protocol")
        );
        f.global(&json!({"telemetry": {"otel": {"headers": {"a": "{file:missing-token}"}}}}));
        assert!(f.load().settings.unwrap_err().contains("missing-token"));
        std::fs::write(f.paths.config.join("cyber.json"), "{ not json").unwrap();
        assert!(f.load().settings.unwrap_err().contains("ConfigParseError"));
    }

    #[test]
    fn a_malformed_managed_document_is_reported() {
        let f = Fixture::new();
        f.global(&json!({"telemetry": {"otel": {"enabled": true}}}));
        f.write(
            f.root.join("managed/cyber-policy.json"),
            &json!({"policy": {"telemetry": {"otel": {"enabled": false}}}}),
        );
        let loaded = f.load();
        assert!(loaded.settings.unwrap().enabled);
        assert!(loaded.managed.error.is_some(), "{:?}", loaded.managed);
    }
}
