pub mod commands;
pub mod emitter;
pub mod model;
pub mod viewer;

pub use commands::{
    clear_log_logical, export_diagnostic_bundle, read_log_snapshot,
    set_log_physical_cleanup_on_start,
};
pub use emitter::{build_plugin, emit};
pub use model::{
    DiagnosticDomain, DiagnosticErrorCode, DiagnosticEventCode, DiagnosticLevel,
    DiagnosticProviderKind, DiagnosticRecoveryCode, DiagnosticResult,
    LogCategory as DiagnosticCategory, SafeLogEvent,
};
