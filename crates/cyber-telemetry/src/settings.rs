//! The effective export settings (`observability-costs` → OpenTelemetry configuration),
//! resolved once at server start with a source for every value.
//!
//! Precedence, highest first:
//! 1. the off switches `CYBER_DISABLE_TELEMETRY` and `OTEL_SDK_DISABLED=true`;
//! 2. a malformed managed policy document (fail closed). A managed telemetry setting, when
//!    org policy defines one, belongs here;
//! 3. enablement: only `telemetry.otel.enabled: true` turns export on, never an `OTEL_*`
//!    variable, so a machine-wide `OTEL_EXPORTER_OTLP_ENDPOINT` set for other programs
//!    cannot send Cyber Sessions anywhere;
//! 4. per value, the per-signal variable over the generic one over `telemetry.otel.*` over
//!    the default; a headers variable replaces the configured map whole;
//! 5. `OTEL_{TRACES,METRICS,LOGS}_EXPORTER=none` turns one signal off.
//!
//! An invalid value keeps export off (fail closed) with the reason, never the value: an
//! endpoint can carry credentials in its userinfo or query, and header values are secrets.

use std::fmt;
use std::time::Duration;

use cyber_core::config::{
    OtlpProtocol, TelemetryLoad, TelemetrySettings, check_endpoint, valid_header_name,
    valid_header_value,
};
use cyber_core::env::EnvSource;

/// The default OTLP timeout.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);
const HTTP_ENDPOINT: &str = "http://localhost:4318";
const GRPC_ENDPOINT: &str = "http://localhost:4317";
/// The reason of a configuration that never turned export on.
pub const NOT_ENABLED: &str = "telemetry.otel.enabled is not true";

/// An OpenTelemetry signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signal {
    Traces,
    Metrics,
    Logs,
}

impl Signal {
    pub const ALL: [Self; 3] = [Self::Traces, Self::Metrics, Self::Logs];

    pub fn name(self) -> &'static str {
        match self {
            Self::Traces => "traces",
            Self::Metrics => "metrics",
            Self::Logs => "logs",
        }
    }

    /// The infix of its `OTEL_*` variables, such as `TRACES`.
    fn variable(self) -> String {
        self.name().to_ascii_uppercase()
    }
}

/// Where a value came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    Default,
    /// A configuration layer label.
    Config(String),
    /// An environment variable name.
    Env(String),
    /// A managed policy source.
    Policy(String),
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Default => f.write_str("default"),
            Self::Config(label) => write!(f, "config ({label})"),
            Self::Env(name) => write!(f, "env ({name})"),
            Self::Policy(label) => write!(f, "policy ({label})"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sourced<T> {
    pub value: T,
    pub source: Source,
}

impl<T> Sourced<T> {
    fn new(value: T, source: Source) -> Self {
        Self { value, source }
    }
}

/// One signal's effective settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignalSettings {
    pub signal: Signal,
    /// Whether this signal is exported: export is on and its switch leaves it on.
    pub enabled: bool,
    /// Why its `OTEL_<SIGNAL>_EXPORTER` switch turns it off.
    pub switched_off: Option<String>,
    pub endpoint: Sourced<String>,
    /// The protocol text; [`Self::protocol`] parses it.
    pub protocol: Sourced<String>,
    /// Header names and values. The values are secrets.
    pub headers: Sourced<Vec<(String, String)>>,
    pub timeout: Sourced<Duration>,
    /// Invalid values, each named without its value.
    issues: Vec<String>,
}

impl SignalSettings {
    pub fn protocol(&self) -> Option<OtlpProtocol> {
        OtlpProtocol::parse(&self.protocol.value)
    }
}

/// The effective export settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    /// Whether anything is exported.
    pub enabled: bool,
    /// Why export is off.
    pub reason: Option<String>,
    /// Export is off because a value or the managed policy is unusable, not by choice.
    pub fail_closed: bool,
    pub log_prompts: Sourced<bool>,
    /// In [`Signal::ALL`] order.
    pub signals: Vec<SignalSettings>,
    /// Resource attributes, the protected ones first.
    pub resource: Vec<(String, String)>,
}

impl Settings {
    pub fn signal(&self, signal: Signal) -> &SignalSettings {
        &self.signals[signal as usize]
    }

    /// The configured header values, for exact-value redaction.
    pub fn header_values(&self) -> impl Iterator<Item = &str> {
        self.signals
            .iter()
            .flat_map(|s| s.headers.value.iter().map(|(_, value)| value.as_str()))
    }
}

/// Resolve the settings of `load` over `env`. `grpc` says whether this build exports gRPC.
pub fn resolve(load: &TelemetryLoad, env: &dyn EnvSource, grpc: bool) -> Settings {
    let config = load.settings.clone().unwrap_or_default();
    let mut signals: Vec<SignalSettings> = Signal::ALL
        .into_iter()
        .map(|signal| resolve_signal(signal, &config, load, env))
        .collect();
    let off = off_reason(load, env, &config, &signals, grpc);
    let enabled = off.is_none();
    for signal in &mut signals {
        signal.enabled = enabled && signal.switched_off.is_none();
    }
    let (reason, fail_closed) =
        off.map_or((None, false), |off| (Some(off.reason), off.fail_closed));
    Settings {
        enabled,
        reason,
        fail_closed,
        log_prompts: Sourced::new(
            config.log_prompts,
            config_source(load, "/telemetry/log_prompts"),
        ),
        signals,
        resource: resource(env),
    }
}

struct Off {
    reason: String,
    fail_closed: bool,
}

impl Off {
    fn chosen(reason: impl Into<String>) -> Option<Self> {
        Some(Self {
            reason: reason.into(),
            fail_closed: false,
        })
    }

    fn closed(reason: impl Into<String>) -> Option<Self> {
        Some(Self {
            reason: reason.into(),
            fail_closed: true,
        })
    }
}

/// The first precedence step that keeps export off.
fn off_reason(
    load: &TelemetryLoad,
    env: &dyn EnvSource,
    config: &TelemetrySettings,
    signals: &[SignalSettings],
    grpc: bool,
) -> Option<Off> {
    if env.flag("CYBER_DISABLE_TELEMETRY") {
        return Off::chosen("CYBER_DISABLE_TELEMETRY is set");
    }
    if env
        .get("OTEL_SDK_DISABLED")
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("true"))
    {
        return Off::chosen("OTEL_SDK_DISABLED=true");
    }
    if let Some(error) = &load.managed.error {
        return Off::closed(format!("managed policy is malformed ({error})"));
    }
    if let Err(error) = &load.settings {
        return Off::closed(error.clone());
    }
    if !config.enabled {
        return Off::chosen(NOT_ENABLED);
    }
    let active: Vec<&SignalSettings> = signals
        .iter()
        .filter(|signal| signal.switched_off.is_none())
        .collect();
    if let Some(issue) = active.iter().find_map(|signal| first_issue(signal, grpc)) {
        return Off::closed(issue);
    }
    if active.is_empty() {
        return Off::chosen(
            "OTEL_TRACES_EXPORTER, OTEL_METRICS_EXPORTER and OTEL_LOGS_EXPORTER turn every signal off",
        );
    }
    None
}

fn first_issue(signal: &SignalSettings, grpc: bool) -> Option<String> {
    let name = signal.signal.name();
    if let Some(issue) = signal.issues.first() {
        return Some(format!("{name}: {issue}"));
    }
    if signal.protocol() == Some(OtlpProtocol::Grpc) && !grpc {
        return Some(format!(
            "{name}: protocol grpc ({}) is not supported by this build",
            signal.protocol.source
        ));
    }
    None
}

fn resolve_signal(
    signal: Signal,
    config: &TelemetrySettings,
    load: &TelemetryLoad,
    env: &dyn EnvSource,
) -> SignalSettings {
    let mut issues = Vec::new();
    let protocol = protocol(signal, config, load, env, &mut issues);
    let parsed = OtlpProtocol::parse(&protocol.value).unwrap_or(OtlpProtocol::HttpProtobuf);
    let endpoint = endpoint(signal, parsed, config, load, env);
    if let Err(error) = check_endpoint(&endpoint.value) {
        issues.push(format!(
            "the endpoint from {} is invalid: {error}",
            endpoint.source
        ));
    }
    let headers = headers(signal, config, load, env, &mut issues);
    if !headers.value.is_empty() && cleartext_remote(&endpoint.value) {
        issues.push(
            "headers need an https or loopback endpoint; they are never sent in clear text off this machine"
                .into(),
        );
    }
    SignalSettings {
        signal,
        enabled: false,
        switched_off: switched_off(signal, env),
        endpoint,
        protocol,
        headers,
        timeout: timeout(signal, env, &mut issues),
        issues,
    }
}

/// The per-signal variable, else the generic one, as `(name, value)`.
fn variable(env: &dyn EnvSource, signal: Signal, key: &str) -> Option<(String, String)> {
    let specific = format!("OTEL_EXPORTER_OTLP_{}_{key}", signal.variable());
    if let Some(value) = env.get(&specific) {
        return Some((specific, value));
    }
    let generic = format!("OTEL_EXPORTER_OTLP_{key}");
    env.get(&generic).map(|value| (generic, value))
}

/// The label of the layer that set `pointer` or a leaf under it.
fn config_source(load: &TelemetryLoad, pointer: &str) -> Source {
    let prefix = format!("{pointer}/");
    let mut labels: Vec<&str> = load
        .sources
        .iter()
        .filter(|(key, _)| *key == pointer || key.starts_with(&prefix))
        .map(|(_, label)| label.as_str())
        .collect();
    labels.dedup();
    if labels.is_empty() {
        Source::Default
    } else {
        Source::Config(labels.join(", "))
    }
}

fn protocol(
    signal: Signal,
    config: &TelemetrySettings,
    load: &TelemetryLoad,
    env: &dyn EnvSource,
    issues: &mut Vec<String>,
) -> Sourced<String> {
    if let Some((name, value)) = variable(env, signal, "PROTOCOL") {
        let value = value.trim().to_string();
        if OtlpProtocol::parse(&value).is_none() {
            issues.push(format!("{name}: expected one of {}", OtlpProtocol::names()));
        }
        return Sourced::new(value, Source::Env(name));
    }
    match config.protocol {
        Some(protocol) => Sourced::new(
            protocol.as_str().into(),
            config_source(load, "/telemetry/otel/protocol"),
        ),
        None => Sourced::new(OtlpProtocol::HttpProtobuf.as_str().into(), Source::Default),
    }
}

/// A per-signal variable is used as given; for HTTP, `/v1/<signal>` is appended to a
/// generic, configured or default base URL.
fn endpoint(
    signal: Signal,
    protocol: OtlpProtocol,
    config: &TelemetrySettings,
    load: &TelemetryLoad,
    env: &dyn EnvSource,
) -> Sourced<String> {
    let specific = format!("OTEL_EXPORTER_OTLP_{}_ENDPOINT", signal.variable());
    if let Some(value) = env.get(&specific) {
        return Sourced::new(value.trim().to_string(), Source::Env(specific));
    }
    let (base, source) = if let Some(value) = env.get("OTEL_EXPORTER_OTLP_ENDPOINT") {
        (value, Source::Env("OTEL_EXPORTER_OTLP_ENDPOINT".into()))
    } else if let Some(value) = &config.endpoint {
        (
            value.clone(),
            config_source(load, "/telemetry/otel/endpoint"),
        )
    } else if protocol.is_http() {
        (HTTP_ENDPOINT.into(), Source::Default)
    } else {
        (GRPC_ENDPOINT.into(), Source::Default)
    };
    let base = base.trim();
    let value = if protocol.is_http() {
        format!("{}/v1/{}", base.trim_end_matches('/'), signal.name())
    } else {
        base.to_string()
    };
    Sourced::new(value, source)
}

fn headers(
    signal: Signal,
    config: &TelemetrySettings,
    load: &TelemetryLoad,
    env: &dyn EnvSource,
    issues: &mut Vec<String>,
) -> Sourced<Vec<(String, String)>> {
    if let Some((name, value)) = variable(env, signal, "HEADERS") {
        let mut headers = parse_pairs(&value).unwrap_or_else(|error| {
            issues.push(format!("{name}: {error}"));
            Vec::new()
        });
        // By position, and none are kept to show: a mistyped entry can hold the secret
        // in its name.
        if let Some(position) = headers
            .iter()
            .position(|(header, value)| !valid_header_name(header) || !valid_header_value(value))
        {
            issues.push(format!(
                "{name}: header {} is not a valid header",
                position + 1
            ));
            headers.clear();
        }
        return Sourced::new(headers, Source::Env(name));
    }
    match &config.headers {
        Some(map) => Sourced::new(
            map.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
            config_source(load, "/telemetry/otel/headers"),
        ),
        None => Sourced::new(Vec::new(), Source::Default),
    }
}

fn timeout(signal: Signal, env: &dyn EnvSource, issues: &mut Vec<String>) -> Sourced<Duration> {
    let Some((name, value)) = variable(env, signal, "TIMEOUT") else {
        return Sourced::new(DEFAULT_TIMEOUT, Source::Default);
    };
    let millis = value.trim().parse::<u64>().unwrap_or_else(|_| {
        issues.push(format!("{name}: expected a whole number of milliseconds"));
        DEFAULT_TIMEOUT.as_millis() as u64
    });
    Sourced::new(Duration::from_millis(millis), Source::Env(name))
}

fn switched_off(signal: Signal, env: &dyn EnvSource) -> Option<String> {
    let name = format!("OTEL_{}_EXPORTER", signal.variable());
    let value = env.get(&name)?;
    match value.trim() {
        "otlp" => None,
        "none" => Some(format!("{name}=none")),
        other => Some(format!(
            "{name}={other} is not supported; only otlp and none are"
        )),
    }
}

/// A plain `http` URL whose host is not this machine.
fn cleartext_remote(endpoint: &str) -> bool {
    let Ok(url) = check_endpoint(endpoint) else {
        return false;
    };
    url.scheme() == "http" && !loopback(&url)
}

/// Whether the host of `url` is this machine.
pub(crate) fn loopback(url: &url::Url) -> bool {
    match url.host() {
        Some(url::Host::Domain(name)) => name.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    }
}

/// `name=value` pairs separated by commas, percent-decoded (`OTEL_EXPORTER_OTLP_HEADERS`,
/// `OTEL_RESOURCE_ATTRIBUTES`). Errors never echo a value.
pub fn parse_pairs(text: &str) -> Result<Vec<(String, String)>, String> {
    const EXPECTED: &str = "expected comma-separated name=value pairs";
    let mut pairs = Vec::new();
    for entry in text.split(',').map(str::trim).filter(|e| !e.is_empty()) {
        let (name, value) = entry.split_once('=').ok_or(EXPECTED)?;
        let name = percent_decode(name.trim()).ok_or(EXPECTED)?;
        let value = percent_decode(value.trim()).ok_or(EXPECTED)?;
        if name.is_empty() {
            return Err(EXPECTED.into());
        }
        pairs.push((name, value));
    }
    Ok(pairs)
}

fn percent_decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = text.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// Keys `OTEL_RESOURCE_ATTRIBUTES` cannot set.
fn protected(key: &str) -> bool {
    matches!(key, "service.name" | "service.version" | "os.type") || key.starts_with("cyber.")
}

/// `service.name` (`OTEL_SERVICE_NAME` overrides `cyber`), `service.version`,
/// `cyber.build` and `os.type`, then what `OTEL_RESOURCE_ATTRIBUTES` adds. An unparsable
/// `OTEL_RESOURCE_ATTRIBUTES` adds nothing.
fn resource(env: &dyn EnvSource) -> Vec<(String, String)> {
    let info = cyber_core::version::build_info();
    let mut attributes = vec![
        (
            "service.name".to_string(),
            env.get("OTEL_SERVICE_NAME")
                .unwrap_or_else(|| "cyber".into()),
        ),
        ("service.version".into(), info.version.into()),
        ("cyber.build".into(), info.build.into()),
        ("os.type".into(), os_type().into()),
    ];
    let extra = env
        .get("OTEL_RESOURCE_ATTRIBUTES")
        .and_then(|text| parse_pairs(&text).ok())
        .unwrap_or_default();
    for (key, value) in extra {
        if !protected(&key) && !attributes.iter().any(|(k, _)| *k == key) {
            attributes.push((key, value));
        }
    }
    attributes
}

/// The OpenTelemetry `os.type` of this build.
pub fn os_type() -> &'static str {
    match std::env::consts::OS {
        "macos" => "darwin",
        other => other,
    }
}

#[cfg(test)]
#[path = "settings_tests.rs"]
mod tests;
