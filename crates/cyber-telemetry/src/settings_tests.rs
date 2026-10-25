use std::collections::{BTreeMap, HashMap};

use cyber_core::config::{ManagedPolicy, TelemetryLoad, TelemetrySettings};

use super::*;

const GLOBAL: &str = "global:/home/u/.config/cyber/cyber.jsonc";

/// Settings read from one global layer, every key sourced to it.
fn config(settings: TelemetrySettings) -> TelemetryLoad {
    let mut sources = BTreeMap::new();
    sources.insert("/telemetry/otel/enabled".into(), GLOBAL.into());
    if settings.endpoint.is_some() {
        sources.insert("/telemetry/otel/endpoint".into(), GLOBAL.into());
    }
    if settings.protocol.is_some() {
        sources.insert("/telemetry/otel/protocol".into(), GLOBAL.into());
    }
    for name in settings.headers.iter().flat_map(|h| h.keys()) {
        sources.insert(format!("/telemetry/otel/headers/{name}"), GLOBAL.into());
    }
    if settings.log_prompts {
        sources.insert("/telemetry/log_prompts".into(), GLOBAL.into());
    }
    TelemetryLoad {
        settings: Ok(settings),
        sources,
        managed: ManagedPolicy::default(),
        ignored: Vec::new(),
    }
}

fn enabled() -> TelemetrySettings {
    TelemetrySettings {
        enabled: true,
        ..TelemetrySettings::default()
    }
}

fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

fn resolve_http(load: &TelemetryLoad, pairs: &[(&str, &str)]) -> Settings {
    resolve(load, &env(pairs), false)
}

fn global() -> Source {
    Source::Config(GLOBAL.into())
}

fn var(name: &str) -> Source {
    Source::Env(name.into())
}

#[test]
fn defaults_keep_export_off_and_name_the_default_values() {
    let settings = resolve_http(&config(TelemetrySettings::default()), &[]);
    assert!(!settings.enabled && !settings.fail_closed);
    assert_eq!(settings.reason.as_deref(), Some(NOT_ENABLED));
    assert_eq!(settings.log_prompts, Sourced::new(false, Source::Default));
    for (signal, path) in Signal::ALL.into_iter().zip(["traces", "metrics", "logs"]) {
        let s = settings.signal(signal);
        assert!(!s.enabled);
        assert_eq!(
            s.endpoint,
            Sourced::new(format!("http://localhost:4318/v1/{path}"), Source::Default)
        );
        assert_eq!(
            s.protocol,
            Sourced::new("http/protobuf".to_string(), Source::Default)
        );
        assert_eq!(s.headers, Sourced::new(Vec::new(), Source::Default));
        assert_eq!(s.timeout, Sourced::new(DEFAULT_TIMEOUT, Source::Default));
    }
}

/// Scenario "Endpoint variable alone does not enable export".
#[test]
fn no_otel_variable_enables_export() {
    let load = config(TelemetrySettings::default());
    let settings = resolve_http(
        &load,
        &[
            ("OTEL_EXPORTER_OTLP_ENDPOINT", "http://127.0.0.1:4318"),
            ("OTEL_EXPORTER_OTLP_PROTOCOL", "http/json"),
            ("OTEL_EXPORTER_OTLP_HEADERS", "authorization=abc"),
            ("OTEL_TRACES_EXPORTER", "otlp"),
            ("OTEL_SDK_DISABLED", "false"),
        ],
    );
    assert!(!settings.enabled);
    assert_eq!(settings.reason.as_deref(), Some(NOT_ENABLED));
    assert_eq!(
        settings.signal(Signal::Traces).endpoint.value,
        "http://127.0.0.1:4318/v1/traces",
        "the values are still shown"
    );
}

#[test]
fn enabled_configuration_turns_every_signal_on() {
    let settings = resolve_http(&config(enabled()), &[]);
    assert!(settings.enabled, "{:?}", settings.reason);
    assert_eq!(settings.reason, None);
    assert!(settings.signals.iter().all(|s| s.enabled));
}

/// Step 1 beats step 2 (managed policy), step 3 (enablement) and invalid values.
#[test]
fn off_switches_win_over_everything() {
    let mut load = config(TelemetrySettings {
        endpoint: Some("http://collector.lan:4318".into()),
        headers: Some(BTreeMap::from([("a".into(), "b".into())])),
        ..enabled()
    });
    load.managed.error = Some("bad".into());
    for (pairs, reason) in [
        (
            [("CYBER_DISABLE_TELEMETRY", "1")],
            "CYBER_DISABLE_TELEMETRY is set",
        ),
        ([("OTEL_SDK_DISABLED", "TRUE")], "OTEL_SDK_DISABLED=true"),
    ] {
        let settings = resolve_http(&load, &pairs);
        assert!(!settings.enabled && !settings.fail_closed);
        assert_eq!(settings.reason.as_deref(), Some(reason));
    }
}

/// Scenario "SDK disabled".
#[test]
fn sdk_disabled_wins_over_configuration() {
    let settings = resolve_http(&config(enabled()), &[("OTEL_SDK_DISABLED", "true")]);
    assert!(!settings.enabled);
    assert_eq!(settings.reason.as_deref(), Some("OTEL_SDK_DISABLED=true"));
}

/// Scenario "Malformed managed policy keeps export off": step 2 beats enablement.
#[test]
fn malformed_managed_policy_keeps_export_off() {
    let mut load = config(enabled());
    load.managed.error = Some("cyber-policy.json: unknown key \"telemetry\"".into());
    let settings = resolve_http(&load, &[]);
    assert!(!settings.enabled && settings.fail_closed);
    assert_eq!(
        settings.reason.as_deref(),
        Some("managed policy is malformed (cyber-policy.json: unknown key \"telemetry\")")
    );
}

#[test]
fn invalid_configuration_keeps_export_off() {
    let mut load = config(enabled());
    load.settings = Err("telemetry.otel.protocol: expected one of ...".into());
    let settings = resolve_http(&load, &[]);
    assert!(!settings.enabled && settings.fail_closed);
    assert_eq!(
        settings.reason.as_deref(),
        Some("telemetry.otel.protocol: expected one of ...")
    );
}

/// Scenario "Environment overrides configuration".
#[test]
fn per_signal_variable_over_generic_over_config_over_default_for_endpoints() {
    let load = config(TelemetrySettings {
        endpoint: Some("https://otel.acme.com".into()),
        ..enabled()
    });
    let settings = resolve_http(
        &load,
        &[(
            "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT",
            "http://127.0.0.1:9999/v1/traces",
        )],
    );
    assert!(settings.enabled, "{:?}", settings.reason);
    assert_eq!(
        settings.signal(Signal::Traces).endpoint,
        Sourced::new(
            "http://127.0.0.1:9999/v1/traces".to_string(),
            var("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT")
        )
    );
    assert_eq!(
        settings.signal(Signal::Metrics).endpoint,
        Sourced::new("https://otel.acme.com/v1/metrics".to_string(), global())
    );
    let generic = resolve_http(
        &load,
        &[("OTEL_EXPORTER_OTLP_ENDPOINT", "https://env.example:4318/")],
    );
    assert_eq!(
        generic.signal(Signal::Logs).endpoint,
        Sourced::new(
            "https://env.example:4318/v1/logs".to_string(),
            var("OTEL_EXPORTER_OTLP_ENDPOINT")
        )
    );
}

#[test]
fn per_signal_variable_over_generic_over_config_for_protocol_headers_and_timeout() {
    let load = config(TelemetrySettings {
        protocol: Some(OtlpProtocol::HttpJson),
        headers: Some(BTreeMap::from([("x-config".into(), "c".into())])),
        ..enabled()
    });
    let configured = resolve_http(&load, &[]);
    let traces = configured.signal(Signal::Traces);
    assert_eq!(traces.protocol, Sourced::new("http/json".into(), global()));
    assert_eq!(
        traces.headers,
        Sourced::new(vec![("x-config".into(), "c".into())], global())
    );
    let settings = resolve_http(
        &load,
        &[
            ("OTEL_EXPORTER_OTLP_PROTOCOL", "http/protobuf"),
            ("OTEL_EXPORTER_OTLP_METRICS_PROTOCOL", "http/json"),
            ("OTEL_EXPORTER_OTLP_HEADERS", "x-generic=g"),
            ("OTEL_EXPORTER_OTLP_LOGS_HEADERS", "x-logs=l"),
            ("OTEL_EXPORTER_OTLP_TIMEOUT", "2500"),
            ("OTEL_EXPORTER_OTLP_TRACES_TIMEOUT", "500"),
        ],
    );
    assert!(settings.enabled, "{:?}", settings.reason);
    let (traces, metrics, logs) = (
        settings.signal(Signal::Traces),
        settings.signal(Signal::Metrics),
        settings.signal(Signal::Logs),
    );
    assert_eq!(traces.protocol.source, var("OTEL_EXPORTER_OTLP_PROTOCOL"));
    assert_eq!(
        metrics.protocol,
        Sourced::new(
            "http/json".into(),
            var("OTEL_EXPORTER_OTLP_METRICS_PROTOCOL")
        )
    );
    assert_eq!(
        traces.timeout,
        Sourced::new(
            Duration::from_millis(500),
            var("OTEL_EXPORTER_OTLP_TRACES_TIMEOUT")
        )
    );
    assert_eq!(
        metrics.timeout,
        Sourced::new(
            Duration::from_millis(2500),
            var("OTEL_EXPORTER_OTLP_TIMEOUT")
        )
    );
    assert_eq!(
        logs.headers,
        Sourced::new(
            vec![("x-logs".into(), "l".into())],
            var("OTEL_EXPORTER_OTLP_LOGS_HEADERS")
        )
    );
}

/// The variable replaces the configured map whole: no configured header survives.
#[test]
fn headers_variable_replaces_the_configured_map() {
    let load = config(TelemetrySettings {
        headers: Some(BTreeMap::from([
            ("authorization".into(), "Bearer config".into()),
            ("x-tenant".into(), "acme".into()),
        ])),
        ..enabled()
    });
    let settings = resolve_http(&load, &[("OTEL_EXPORTER_OTLP_HEADERS", "x-api-key=k")]);
    for signal in &settings.signals {
        assert_eq!(signal.headers.value, [("x-api-key".into(), "k".into())]);
        assert_eq!(signal.headers.source, var("OTEL_EXPORTER_OTLP_HEADERS"));
    }
}

#[test]
fn headers_variable_is_url_decoded_and_checked() {
    let load = config(enabled());
    let settings = resolve_http(
        &load,
        &[(
            "OTEL_EXPORTER_OTLP_HEADERS",
            "authorization=Bearer%20abc%3D%3D , x-team = a%2Cb,",
        )],
    );
    assert!(settings.enabled, "{:?}", settings.reason);
    assert_eq!(
        settings.signal(Signal::Traces).headers.value,
        [
            ("authorization".to_string(), "Bearer abc==".to_string()),
            ("x-team".to_string(), "a,b".to_string()),
        ]
    );
    for bad in ["authorization", "=abc", "a=%zz", "a=%0d%0ax: 1", "a b=c"] {
        let settings = resolve_http(&load, &[("OTEL_EXPORTER_OTLP_HEADERS", bad)]);
        assert!(!settings.enabled && settings.fail_closed, "{bad}");
        let reason = settings.reason.unwrap();
        assert!(reason.contains("OTEL_EXPORTER_OTLP_HEADERS"), "{reason}");
        assert!(!reason.contains("abc"), "{reason}");
    }
}

/// Regression: `name: value` syntax with `=` padding splits inside the token, so the
/// rejected name holds the secret; the reason names the entry by position only.
#[test]
fn an_invalid_header_name_never_echoes_the_secret() {
    let load = config(enabled());
    let settings = resolve_http(
        &load,
        &[(
            "OTEL_EXPORTER_OTLP_HEADERS",
            "x-team=a,Authorization: Bearer abc==",
        )],
    );
    assert!(!settings.enabled && settings.fail_closed);
    let reason = settings.reason.clone().unwrap();
    assert!(
        reason.contains("OTEL_EXPORTER_OTLP_HEADERS: header 2 is not a valid header"),
        "{reason}"
    );
    assert!(
        !reason.contains("abc") && !reason.contains("Bearer"),
        "{reason}"
    );
    // Nor do the names reach `telemetry show`.
    assert!(settings.signal(Signal::Traces).headers.value.is_empty());
}

#[test]
fn exporter_none_turns_one_signal_off() {
    let settings = resolve_http(&config(enabled()), &[("OTEL_METRICS_EXPORTER", "none")]);
    assert!(settings.enabled);
    assert!(settings.signal(Signal::Traces).enabled && settings.signal(Signal::Logs).enabled);
    let metrics = settings.signal(Signal::Metrics);
    assert!(!metrics.enabled);
    assert_eq!(
        metrics.switched_off.as_deref(),
        Some("OTEL_METRICS_EXPORTER=none")
    );
    let other = resolve_http(&config(enabled()), &[("OTEL_LOGS_EXPORTER", "console")]);
    let logs = other.signal(Signal::Logs);
    assert!(
        !logs.enabled
            && logs
                .switched_off
                .as_ref()
                .unwrap()
                .contains("not supported")
    );
    let all = resolve_http(
        &config(enabled()),
        &[
            ("OTEL_TRACES_EXPORTER", "none"),
            ("OTEL_METRICS_EXPORTER", "none"),
            ("OTEL_LOGS_EXPORTER", "none"),
        ],
    );
    assert!(!all.enabled && !all.fail_closed);
}

/// A switched-off signal's invalid values do not keep the others off.
#[test]
fn a_switched_off_signal_is_not_validated() {
    let settings = resolve_http(
        &config(enabled()),
        &[
            ("OTEL_LOGS_EXPORTER", "none"),
            ("OTEL_EXPORTER_OTLP_LOGS_ENDPOINT", "not a url"),
        ],
    );
    assert!(settings.enabled, "{:?}", settings.reason);
}

#[test]
fn http_paths_are_appended_to_base_urls_only() {
    let load = config(TelemetrySettings {
        endpoint: Some("https://otel.acme.com/ingest/".into()),
        ..enabled()
    });
    let settings = resolve_http(
        &load,
        &[(
            "OTEL_EXPORTER_OTLP_LOGS_ENDPOINT",
            "https://logs.acme.com/custom",
        )],
    );
    assert_eq!(
        settings.signal(Signal::Traces).endpoint.value,
        "https://otel.acme.com/ingest/v1/traces"
    );
    assert_eq!(
        settings.signal(Signal::Logs).endpoint.value,
        "https://logs.acme.com/custom"
    );
}

#[test]
fn grpc_uses_its_default_endpoint_without_paths_and_needs_the_feature() {
    let load = config(TelemetrySettings {
        protocol: Some(OtlpProtocol::Grpc),
        ..enabled()
    });
    let settings = resolve(&load, &env(&[]), true);
    assert!(settings.enabled, "{:?}", settings.reason);
    assert_eq!(
        settings.signal(Signal::Metrics).endpoint,
        Sourced::new("http://localhost:4317".to_string(), Source::Default)
    );
    let unsupported = resolve(&load, &env(&[]), false);
    assert!(!unsupported.enabled && unsupported.fail_closed);
    assert!(
        unsupported
            .reason
            .unwrap()
            .contains("grpc (config (global:/home/u/.config/cyber/cyber.jsonc)) is not supported")
    );
}

/// Scenario "Headers not sent in clear text".
#[test]
fn headers_need_https_or_loopback() {
    let headers = Some(BTreeMap::from([(
        "authorization".into(),
        "Bearer abc".into(),
    )]));
    let with = |endpoint: &str| {
        resolve_http(
            &config(TelemetrySettings {
                endpoint: Some(endpoint.into()),
                headers: headers.clone(),
                ..enabled()
            }),
            &[],
        )
    };
    let remote = with("http://collector.lan:4318");
    assert!(!remote.enabled && remote.fail_closed);
    assert!(
        remote
            .reason
            .unwrap()
            .contains("headers need an https or loopback endpoint")
    );
    for endpoint in [
        "https://collector.lan:4318",
        "http://localhost:4318",
        "http://127.0.0.1:4318",
        "http://[::1]:4318",
    ] {
        assert!(with(endpoint).enabled, "{endpoint}");
    }
    let plain = resolve_http(
        &config(TelemetrySettings {
            endpoint: Some("http://collector.lan:4318".into()),
            ..enabled()
        }),
        &[],
    );
    assert!(plain.enabled, "no headers, nothing to protect");
}

#[test]
fn invalid_variables_keep_export_off_without_echoing_values() {
    for (name, value) in [
        (
            "OTEL_EXPORTER_OTLP_ENDPOINT",
            "collector.lan:4318?token=s3cret",
        ),
        ("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT", "ftp://s3cret"),
        ("OTEL_EXPORTER_OTLP_PROTOCOL", "s3cret"),
        ("OTEL_EXPORTER_OTLP_TIMEOUT", "s3cret"),
    ] {
        let settings = resolve_http(&config(enabled()), &[(name, value)]);
        assert!(!settings.enabled && settings.fail_closed, "{name}");
        let reason = settings.reason.unwrap();
        assert!(reason.contains(name), "{reason}");
        assert!(!reason.contains("s3cret"), "{reason}");
    }
}

#[test]
fn log_prompts_is_sourced_from_configuration() {
    let settings = resolve_http(
        &config(TelemetrySettings {
            log_prompts: true,
            ..enabled()
        }),
        &[],
    );
    assert_eq!(settings.log_prompts, Sourced::new(true, global()));
}

#[test]
fn resource_attributes_cannot_replace_protected_keys() {
    let settings = resolve_http(
        &config(TelemetrySettings::default()),
        &[
            ("OTEL_SERVICE_NAME", "cyber-dev"),
            (
                "OTEL_RESOURCE_ATTRIBUTES",
                "service.version=9.9.9,os.type=plan9,service.name=x,cyber.build=fake,cyber.account.sub=u,deployment.environment=prod%20eu",
            ),
        ],
    );
    let attribute = |key: &str| {
        settings
            .resource
            .iter()
            .filter(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
            .collect::<Vec<_>>()
    };
    let info = cyber_core::version::build_info();
    assert_eq!(attribute("service.name"), ["cyber-dev"]);
    assert_eq!(attribute("service.version"), [info.version]);
    assert_eq!(attribute("os.type"), [os_type()]);
    assert_eq!(attribute("cyber.build"), [info.build]);
    assert!(attribute("cyber.account.sub").is_empty());
    assert_eq!(attribute("deployment.environment"), ["prod eu"]);
    assert!(["darwin", "linux", "windows"].contains(&os_type()) || cfg!(target_os = "freebsd"));
    let default = resolve_http(&config(TelemetrySettings::default()), &[]);
    assert_eq!(default.resource[0], ("service.name".into(), "cyber".into()));
}
