//! OpenTelemetry export of the server (`observability-costs` → OpenTelemetry export):
//! the settings, resolved once at server start, the event-to-signal mapper, the export
//! pipeline and the status `cyber telemetry show` reports. Only the server exports;
//! clients never do.

pub mod counted;
pub mod logs;
pub mod mapper;
pub mod pipeline;
pub mod privacy;
pub mod settings;
pub mod status;

pub use pipeline::{AccountIds, SessionLookup};
pub use settings::{Settings, Signal, resolve};
pub use status::Telemetry;

/// Whether this build exports OTLP over gRPC (the `otlp-grpc` feature).
pub const GRPC: bool = cfg!(feature = "otlp-grpc");
