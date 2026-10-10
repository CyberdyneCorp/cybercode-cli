//! Language server discovery without installation or process execution.
mod catalogue;
mod discovery;
pub use catalogue::{InstallMethod, ServerDefinition, builtin_servers};
pub use discovery::{DetectedServer, ExecutableSearch, detect_servers, server_root};

mod formatters;
pub use formatters::{
    DetectedFormatter, FormatterDefinition, FormatterDetection, builtin_formatters,
    detect_formatters,
};
