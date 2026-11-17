use serde::{Deserialize, Serialize};

use std::{collections::BTreeMap, sync::Arc};

use crate::{
    Error, Geometry, ReasonCode, Rect,
    capability::SelectorKind,
    control::{InputPermit, Target},
};

/// The platform's own picker chooses a window or a whole output (design F4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
#[schemars(rename = "ComputerPickKind")]
pub enum PickKind {
    Window,
    Output,
}

/// Exactly one of `app`, `display`, `window` or `pick`. A selector value is never
/// reinterpreted as a different kind of identifier on another platform.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
#[schemars(rename = "ComputerSelector")]
pub struct Selector {
    /// macOS application bundle ID.
    pub app: Option<String>,
    /// macOS display index.
    pub display: Option<u32>,
    /// Short-lived candidate token from an owner-only target listing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<String>,
    /// Let the platform picker choose the source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pick: Option<PickKind>,
}

fn token_like(value: &str, extra: &[u8]) -> bool {
    !value.is_empty()
        && value.len() <= 255
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || extra.contains(&byte))
}

impl Selector {
    pub fn app(app: impl Into<String>) -> Self {
        Self {
            app: Some(app.into()),
            ..Self::default()
        }
    }
    pub fn display(display: u32) -> Self {
        Self {
            display: Some(display),
            ..Self::default()
        }
    }

    /// The kind of a well-formed selector.
    pub fn kind(&self) -> Result<SelectorKind, Error> {
        let named = [
            self.app.as_ref().map(|_| SelectorKind::App),
            self.display.map(|_| SelectorKind::Display),
            self.window.as_ref().map(|_| SelectorKind::Window),
            self.pick.map(|_| SelectorKind::Pick),
        ];
        let mut kinds = named.into_iter().flatten();
        match (kinds.next(), kinds.next()) {
            (Some(kind), None) => Ok(kind),
            _ => Err(Error::Input(
                "select exactly one application, display, window candidate or picker source",
            )),
        }
    }

    pub fn validate(&self) -> Result<(), Error> {
        let valid = match self.kind()? {
            SelectorKind::App => self
                .app
                .as_deref()
                .is_some_and(|app| token_like(app, b".-_")),
            SelectorKind::Window => self
                .window
                .as_deref()
                .is_some_and(|token| token.len() <= 128 && token_like(token, b"-_")),
            SelectorKind::Display | SelectorKind::Pick => true,
        };
        if !valid {
            return Err(Error::Input(
                "select exactly one application bundle ID or display",
            ));
        }
        Ok(())
    }

    /// Validates the selector and refuses kinds the running platform does not honor,
    /// before any native call.
    pub fn validate_for(&self, honored: &[SelectorKind]) -> Result<SelectorKind, Error> {
        self.validate()?;
        let kind = self.kind()?;
        if !honored.contains(&kind) {
            return Err(Error::Refused(
                ReasonCode::SelectorUnsupportedOnPlatform,
                "this platform does not honor the selected target kind",
            ));
        }
        Ok(kind)
    }
}

pub use crate::capability::Health;

/// What kind of source a target is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
#[schemars(rename = "ComputerSourceKind")]
pub enum SourceKind {
    App,
    Window,
    Display,
    Output,
}

/// Typed description of a target's source. `detail` is for owner display and
/// diagnostics only; clients never parse it for decisions, and it is never journaled.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[schemars(rename = "ComputerTargetSource")]
pub struct TargetSource {
    pub platform: String,
    pub kind: SourceKind,
    pub backend: String,
    #[serde(default)]
    pub detail: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[schemars(rename = "ComputerNativeTarget")]
pub struct NativeTarget {
    /// Server-internal comparison string; never accepted from a caller.
    pub identity: String,
    #[schemars(extend("deprecated" = true))]
    pub app: Option<String>,
    #[schemars(extend("deprecated" = true))]
    pub pid: i32,
    #[schemars(extend("deprecated" = true))]
    pub window: u32,
    #[schemars(extend("deprecated" = true))]
    pub display: u32,
    #[schemars(extend("deprecated" = true))]
    pub launch_ms: u64,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub image_width: u32,
    pub image_height: u32,
    pub rotation: u16,
    /// Server-issued, opaque, unique per review and meaningless after restart.
    #[serde(default)]
    pub target_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<TargetSource>,
    /// Display-topology epoch observed when this target was resolved or captured.
    #[serde(default)]
    pub topology_epoch: u64,
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
        match &self.app {
            Some(app) => Selector::app(app.clone()),
            None => Selector::display(self.display),
        }
    }

    /// The native binding without server-issued references or owner-display detail.
    pub fn binding(&self) -> Self {
        Self {
            target_id: String::new(),
            source: None,
            topology_epoch: 0,
            ..self.clone()
        }
    }

    /// Whether two resolutions name the same native binding and geometry.
    pub fn same_binding(&self, other: &Self) -> bool {
        self.binding() == other.binding()
    }

    /// The source description, derived from the macOS fields when an adapter gave none.
    pub fn described_source(&self) -> TargetSource {
        if let Some(source) = &self.source {
            return source.clone();
        }
        let (kind, detail) = match &self.app {
            Some(app) => (SourceKind::App, ("bundle_id", app.clone())),
            None => (SourceKind::Display, ("display", self.display.to_string())),
        };
        TargetSource {
            platform: crate::capability::FOUNDATION_PLATFORM.into(),
            kind,
            backend: "macos-screencapturekit".into(),
            detail: BTreeMap::from([(detail.0.into(), detail.1)]),
        }
    }

    /// The copy written to durable records: `source.detail` is owner-display only.
    pub fn for_journal(&self) -> Self {
        let mut target = self.clone();
        if let Some(source) = target.source.as_mut() {
            source.detail.clear();
        }
        target
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

pub type HostLease = Arc<dyn Send + Sync>;
pub trait Backend: Send + Sync {
    /// Native implementations coordinate all server processes of this OS user.
    /// Fake adapters use a process-local no-op unless testing lease exclusion.
    fn acquire_lease(&self) -> Result<HostLease, Error> {
        Ok(Arc::new(()))
    }

    /// Prompting is allowed only from the explicit owner setup endpoint.
    fn health(&self, prompt: bool) -> Result<Health, Error>;
    fn resolve(&self, selector: &Selector) -> Result<NativeTarget, Error>;
    fn capture(&self, target: &NativeTarget) -> Result<Capture, Error>;
    fn input(&self, target: &NativeTarget, permit: &InputPermit) -> Result<(), Error>;

    /// Owner preview only; may show a platform picker. Never called by `allow`.
    fn review(
        &self,
        selector: &Selector,
        cancelled: &(dyn Fn() -> bool + Sync),
    ) -> Result<NativeTarget, Error> {
        let _ = cancelled;
        self.resolve(selector)
    }

    /// Idempotent, non-blocking teardown of a review's or grant's platform scope.
    fn end_scope(&self, target: &NativeTarget) {
        let _ = target;
    }

    /// The backend reports binding-loss and topology events through this sink.
    fn set_revocation_sink(&self, sink: RevocationSink) {
        let _ = sink;
    }
}

/// How a platform event affects a grant (design F6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EventClass {
    /// The reviewed target, its session or the platform authority is gone or changed:
    /// revoke through the controller fence first, then tear down.
    BindingLoss,
    /// The display layout changed with the binding intact: older frames become stale.
    TopologyChange,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Revocation {
    pub class: EventClass,
    pub reason: ReasonCode,
    /// The affected target identity, when the platform knows it.
    pub identity: Option<String>,
}

impl Revocation {
    /// An event the platform cannot classify is treated as binding loss.
    pub fn unclassified(reason: ReasonCode) -> Self {
        Self {
            class: EventClass::BindingLoss,
            reason,
            identity: None,
        }
    }
}

pub type RevocationSink = Arc<dyn Fn(Revocation) + Send + Sync>;

pub struct Native;

#[cfg(not(target_os = "macos"))]
impl Backend for Native {
    fn health(&self, _prompt: bool) -> Result<Health, Error> {
        Ok(Health::unsupported(
            std::env::consts::OS,
            std::env::current_exe()
                .ok()
                .map(|path| path.display().to_string())
                .unwrap_or_default(),
        ))
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

#[cfg(all(test, not(target_os = "macos")))]
mod unsupported_tests {
    use super::*;
    #[test]
    fn unsupported_adapter_has_no_native_fallback_or_consent_prompt() {
        for prompt in [false, true] {
            let health = Native.health(prompt).unwrap();
            assert!(!health.supported);
            assert!(!health.screen_capture);
            assert!(!health.accessibility);
        }
        assert!(matches!(
            Native.resolve(&Selector {
                app: Some("fixture.app".into()),
                display: None,
                ..Selector::default()
            }),
            Err(Error::UnsupportedPlatform)
        ));
        let target = NativeTarget {
            identity: "fixture".into(),
            app: None,
            pid: 0,
            window: 0,
            display: 0,
            launch_ms: 0,
            x: 0.0,
            y: 0.0,
            width: 1.0,
            height: 1.0,
            image_width: 1,
            image_height: 1,
            rotation: 0,
            target_id: String::new(),
            source: None,
            topology_epoch: 0,
        };
        assert!(matches!(
            Native.capture(&target),
            Err(Error::UnsupportedPlatform)
        ));
    }
}

#[cfg(test)]
mod selector_tests {
    use super::*;

    fn target() -> NativeTarget {
        NativeTarget {
            identity: "app:test.fixture:42:100:7".into(),
            app: Some("test.fixture".into()),
            pid: 42,
            window: 7,
            display: 1,
            launch_ms: 100,
            x: 0.0,
            y: 0.0,
            width: 10.0,
            height: 10.0,
            image_width: 10,
            image_height: 10,
            rotation: 0,
            target_id: String::new(),
            source: None,
            topology_epoch: 0,
        }
    }

    #[test]
    fn exactly_one_selector_kind_is_accepted() {
        assert_eq!(Selector::app("com.example").kind(), Ok(SelectorKind::App));
        assert_eq!(Selector::display(0).kind(), Ok(SelectorKind::Display));
        let window = Selector {
            window: Some("cuc_token-1".into()),
            ..Selector::default()
        };
        assert_eq!(
            window.validate_for(&[SelectorKind::Window]),
            Ok(SelectorKind::Window)
        );
        let pick = Selector {
            pick: Some(PickKind::Output),
            ..Selector::default()
        };
        assert_eq!(pick.kind(), Ok(SelectorKind::Pick));
        for invalid in [
            Selector::default(),
            Selector {
                app: Some("a".into()),
                display: Some(1),
                ..Selector::default()
            },
            Selector {
                display: Some(1),
                pick: Some(PickKind::Window),
                ..Selector::default()
            },
            Selector::app("bad bundle"),
            Selector {
                window: Some("../../etc".into()),
                ..Selector::default()
            },
            Selector {
                window: Some("x".repeat(129)),
                ..Selector::default()
            },
        ] {
            assert!(invalid.validate().is_err(), "{invalid:?}");
        }
    }

    #[test]
    fn selectors_a_platform_does_not_honor_are_refused_with_a_code() {
        let macos = [SelectorKind::App, SelectorKind::Display];
        for selector in [
            Selector {
                window: Some("cuc_one".into()),
                ..Selector::default()
            },
            Selector {
                pick: Some(PickKind::Window),
                ..Selector::default()
            },
        ] {
            let error = selector.validate_for(&macos).unwrap_err();
            assert_eq!(
                error.reason(),
                Some(ReasonCode::SelectorUnsupportedOnPlatform)
            );
        }
        let error = Selector::app("com.example")
            .validate_for(&[SelectorKind::Pick])
            .unwrap_err();
        assert_eq!(
            error.reason(),
            Some(ReasonCode::SelectorUnsupportedOnPlatform),
            "a bundle ID is never reinterpreted on another platform"
        );
        assert!(SelectorKind::App.re_resolvable() && SelectorKind::Display.re_resolvable());
        assert!(!SelectorKind::Window.re_resolvable() && !SelectorKind::Pick.re_resolvable());
    }

    #[test]
    fn requests_cannot_name_numeric_handles_or_identities() {
        for body in [
            serde_json::json!({"pid": 42}),
            serde_json::json!({"window": 7}),
            serde_json::json!({"identity": "app:test.fixture:42:100:7"}),
            serde_json::json!({"hwnd": 1234}),
        ] {
            assert!(serde_json::from_value::<Selector>(body).is_err());
        }
        let legacy: Selector =
            serde_json::from_value(serde_json::json!({"app":"com.example","display":null}))
                .unwrap();
        assert_eq!(legacy, Selector::app("com.example"));
        assert_eq!(
            serde_json::to_value(&legacy).unwrap(),
            serde_json::json!({"app":"com.example","display":null}),
            "the native request shape is unchanged for macOS selectors"
        );
    }

    #[test]
    fn binding_comparison_ignores_server_references_and_journal_drops_detail() {
        let resolved = target();
        let mut reviewed = resolved.clone();
        reviewed.target_id = "cut_one".into();
        reviewed.source = Some(TargetSource {
            detail: BTreeMap::from([("title".into(), "Private Document.txt".into())]),
            ..resolved.described_source()
        });
        reviewed.topology_epoch = 3;
        assert!(reviewed.same_binding(&resolved));
        let mut moved = resolved.clone();
        moved.x = 1.0;
        assert!(!moved.same_binding(&reviewed));
        let durable = serde_json::to_string(&reviewed.for_journal()).unwrap();
        assert!(!durable.contains("Private Document"));
        assert!(durable.contains("cut_one"));
        let source = resolved.described_source();
        assert_eq!(source.kind, SourceKind::App);
        assert_eq!(source.platform, "macos");
        let display = NativeTarget {
            app: None,
            ..target()
        };
        assert_eq!(display.described_source().kind, SourceKind::Display);
    }

    #[test]
    fn version_one_target_rows_deserialize_with_empty_references() {
        let mut value = serde_json::to_value(target()).unwrap();
        for added in ["target_id", "source", "topology_epoch"] {
            value.as_object_mut().unwrap().remove(added);
        }
        let parsed: NativeTarget = serde_json::from_value(value).unwrap();
        assert_eq!(parsed, target());
    }

    #[test]
    fn default_backend_review_resolves_and_scope_hooks_are_no_ops() {
        struct Legacy;
        impl Backend for Legacy {
            fn health(&self, _: bool) -> Result<Health, Error> {
                Ok(Health::default())
            }
            fn resolve(&self, _: &Selector) -> Result<NativeTarget, Error> {
                Ok(target())
            }
            fn capture(&self, _: &NativeTarget) -> Result<Capture, Error> {
                Err(Error::UnsupportedPlatform)
            }
            fn input(&self, _: &NativeTarget, _: &InputPermit) -> Result<(), Error> {
                Err(Error::UnsupportedPlatform)
            }
        }
        assert_eq!(
            Legacy.review(&Selector::app("test.fixture"), &|| false),
            Ok(target())
        );
        Legacy.end_scope(&target());
        Legacy.set_revocation_sink(Arc::new(|_| {}));
        assert_eq!(
            Revocation::unclassified(ReasonCode::SessionLocked).class,
            EventClass::BindingLoss
        );
    }
}
