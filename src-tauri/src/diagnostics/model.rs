use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const SCHEMA_VERSION: u16 = 1;
pub const REDACTION_VERSION: u16 = 1;
pub const MAX_LOG_LINE_BYTES: usize = 128 * 1024;

static EVENT_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticLevel {
    Info,
    Warning,
    Error,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LogCategory {
    System,
    Diagnostic,
    Ai,
    SkillMcp,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticDomain {
    App,
    Database,
    Catalog,
    SkillScan,
    Analysis,
    Settings,
    Environment,
    Diagnostics,
    Operations,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticResult {
    Started,
    Succeeded,
    Failed,
    Degraded,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticRecoveryCode {
    Retry,
    CheckSettings,
    Rescan,
    RestartApplication,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticProviderKind {
    OpenAiCompatible,
    Anthropic,
    Ollama,
    User,
    Repo,
    LegacyUser,
    System,
    Plugin,
    Bundled,
    AdditionalRoot,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticErrorCode {
    DatabaseUnavailable,
    DatabaseSchemaIncompatible,
    ScanFailed,
    ScanInProgress,
    AnalysisNotConfigured,
    AnalysisFailed,
    SettingsUnavailable,
    InvalidConfiguration,
    PrivacyRemoteBlocked,
    AiNotConfigured,
    SecretUnavailable,
    PathNotAllowed,
    OperationTokenInvalid,
    OperationConflict,
    OperationSourceChanged,
    OperationExecutionFailed,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticEventCode {
    AppStarted,
    DatabaseInitialized,
    CatalogCacheLoaded,
    FrontendReady,
    SkillScanStarted,
    SkillScanCompleted,
    SkillScanFailed,
    AnalysisQueued,
    AnalysisRetried,
    AnalysisCompleted,
    AnalysisFailed,
    SettingsLoaded,
    SettingsSaved,
    AiConnectionTested,
    EnvironmentHealthChecked,
    OperationPlanned,
    OperationExecuted,
    OperationFailed,
    LogEventCleared,
    FrontendLogRejected,
    DiagnosticsExported,
    PhysicalLogCleanupRequested,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SafeLogEvent {
    pub schema_version: u16,
    pub event_id: String,
    pub occurred_at: i64,
    pub level: DiagnosticLevel,
    pub category: LogCategory,
    pub domain: DiagnosticDomain,
    pub event_code: DiagnosticEventCode,
    pub result: DiagnosticResult,
    pub module: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub submodule: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_ref: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http_status: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub item_count: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<DiagnosticErrorCode>,
    pub retryable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recovery_code: Option<DiagnosticRecoveryCode>,
    pub redaction_version: u16,
}

impl SafeLogEvent {
    pub fn new(
        level: DiagnosticLevel,
        domain: DiagnosticDomain,
        event_code: DiagnosticEventCode,
        result: DiagnosticResult,
    ) -> Self {
        let occurred_at = unix_millis();
        let sequence = EVENT_COUNTER.fetch_add(1, Ordering::Relaxed);
        Self {
            schema_version: SCHEMA_VERSION,
            event_id: format!("evt-{occurred_at:016x}-{sequence:016x}"),
            occurred_at,
            level,
            category: domain.category(),
            module: domain.as_str().to_owned(),
            domain,
            event_code,
            result,
            submodule: None,
            duration_ms: None,
            trace_id: None,
            request_ref: None,
            provider: None,
            model: None,
            http_status: None,
            item_count: None,
            error_code: None,
            retryable: false,
            recovery_code: None,
            redaction_version: REDACTION_VERSION,
        }
    }

    pub fn with_duration(mut self, duration_ms: u64) -> Self {
        self.duration_ms = Some(duration_ms);
        self
    }

    pub fn with_error(
        mut self,
        error_code: DiagnosticErrorCode,
        retryable: bool,
        recovery_code: DiagnosticRecoveryCode,
    ) -> Self {
        self.error_code = Some(error_code);
        self.retryable = retryable;
        self.recovery_code = Some(recovery_code);
        self
    }

    pub fn with_provider(mut self, provider_kind: DiagnosticProviderKind) -> Self {
        self.provider = Some(provider_kind.as_str().to_owned());
        self
    }

    pub fn with_model(mut self, model: &str) -> Self {
        self.model = Some(model.to_owned());
        self
    }

    pub fn with_http_status(mut self, http_status: u16) -> Self {
        self.http_status = Some(http_status);
        self
    }

    pub fn with_counts(mut self, item_count: Option<u64>, _byte_count: Option<u64>) -> Self {
        // SafeLogEvent deliberately keeps counts but drops the old byte_count field.
        self.item_count = item_count;
        self
    }

    pub fn with_trace_id(mut self, trace_id: &str) -> Self {
        self.trace_id = Some(hash_reference("trace", trace_id));
        self
    }

    pub fn with_request_ref(mut self, request_id: &str) -> Self {
        self.request_ref = Some(hash_reference("request", request_id));
        self
    }

    pub fn with_entity_ref(self, internal_id: &str) -> Self {
        self.with_request_ref(internal_id)
    }

    pub fn validate(&self) -> Result<(), &'static str> {
        if self.schema_version != SCHEMA_VERSION || self.redaction_version != REDACTION_VERSION {
            return Err("unsupported_schema");
        }
        if !is_safe_identifier(&self.event_id, 96)
            || !self.event_id.starts_with("evt-")
            || self.occurred_at <= 0
            || !is_safe_identifier(&self.module, 64)
            || self
                .submodule
                .as_deref()
                .is_some_and(|value| !is_safe_identifier(value, 64))
            || self
                .trace_id
                .as_deref()
                .is_some_and(|value| !is_safe_hex(value, 64))
            || self
                .request_ref
                .as_deref()
                .is_some_and(|value| !is_safe_hex(value, 64))
            || self
                .provider
                .as_deref()
                .is_some_and(|value| !is_safe_identifier(value, 64))
            || self
                .model
                .as_deref()
                .is_some_and(|value| !is_safe_identifier(value, 128))
        {
            return Err("unsafe_identifier");
        }
        Ok(())
    }

    pub fn rejection(level: DiagnosticLevel) -> Self {
        Self::new(
            level,
            DiagnosticDomain::Diagnostics,
            DiagnosticEventCode::FrontendLogRejected,
            DiagnosticResult::Degraded,
        )
    }
}

impl DiagnosticDomain {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::App => "app",
            Self::Database => "database",
            Self::Catalog => "catalog",
            Self::SkillScan => "skill_scan",
            Self::Analysis => "analysis",
            Self::Settings => "settings",
            Self::Environment => "environment",
            Self::Diagnostics => "diagnostics",
            Self::Operations => "operations",
        }
    }

    pub const fn category(self) -> LogCategory {
        match self {
            Self::Analysis => LogCategory::Ai,
            Self::Catalog | Self::SkillScan | Self::Operations => LogCategory::SkillMcp,
            Self::Diagnostics => LogCategory::Diagnostic,
            Self::App | Self::Database | Self::Settings | Self::Environment => LogCategory::System,
        }
    }
}

impl DiagnosticProviderKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::OpenAiCompatible => "open_ai_compatible",
            Self::Anthropic => "anthropic",
            Self::Ollama => "ollama",
            Self::User => "user",
            Self::Repo => "repo",
            Self::LegacyUser => "legacy_user",
            Self::System => "system",
            Self::Plugin => "plugin",
            Self::Bundled => "bundled",
            Self::AdditionalRoot => "additional_root",
        }
    }
}

fn unix_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

fn hash_reference(namespace: &str, value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"codex-o-safe-log-reference");
    hasher.update(namespace.as_bytes());
    hasher.update(value.as_bytes());
    hasher
        .finalize()
        .iter()
        .take(20)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn is_safe_identifier(value: &str, max_len: usize) -> bool {
    !value.is_empty()
        && value.len() <= max_len
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b':'))
}

fn is_safe_hex(value: &str, max_len: usize) -> bool {
    !value.is_empty()
        && value.len() <= max_len
        && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::{
        DiagnosticDomain, DiagnosticEventCode, DiagnosticLevel, DiagnosticResult, SafeLogEvent,
    };

    #[test]
    fn safe_event_has_only_allowlisted_fields_and_hashed_references() {
        let event = SafeLogEvent::new(
            DiagnosticLevel::Info,
            DiagnosticDomain::Analysis,
            DiagnosticEventCode::AnalysisCompleted,
            DiagnosticResult::Succeeded,
        )
        .with_trace_id("request-123")
        .with_request_ref("request-123");
        let value = serde_json::to_value(&event).unwrap();
        assert!(value.get("prompt").is_none());
        assert_eq!(value["category"], "ai");
        assert_eq!(event.validate(), Ok(()));
        assert_ne!(event.trace_id.as_deref(), Some("request-123"));
    }

    #[test]
    fn unknown_fields_are_rejected() {
        let result = serde_json::from_str::<SafeLogEvent>(
            r#"{"schema_version":1,"event_id":"evt-1","occurred_at":1,"level":"info","category":"system","domain":"app","event_code":"app_started","result":"succeeded","module":"app","retryable":false,"redaction_version":1,"prompt":"secret"}"#,
        );
        assert!(result.is_err());
    }
}
