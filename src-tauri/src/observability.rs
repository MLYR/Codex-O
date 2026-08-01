// Compatibility exports keep existing feature modules focused on business events while the
// diagnostics implementation lives in model/emitter/viewer/commands modules.
pub use crate::diagnostics::emit;
pub use crate::diagnostics::{
    DiagnosticDomain, DiagnosticErrorCode, DiagnosticEventCode, DiagnosticLevel,
    DiagnosticProviderKind, DiagnosticRecoveryCode, DiagnosticResult, SafeLogEvent,
};

pub type DiagnosticRecord = SafeLogEvent;
