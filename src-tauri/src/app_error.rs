use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppErrorCode {
    DatabaseNotFound,
    DatabaseBusy,
    DatabaseSchemaIncompatible,
    SessionFieldUnavailable,
    ProviderUnavailable,
    PermissionDenied,
}

impl AppErrorCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::DatabaseNotFound => "database_not_found",
            Self::DatabaseBusy => "database_busy",
            Self::DatabaseSchemaIncompatible => "database_schema_incompatible",
            Self::SessionFieldUnavailable => "session_field_unavailable",
            Self::ProviderUnavailable => "provider_unavailable",
            Self::PermissionDenied => "permission_denied",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SafeErrorContext {
    Schema {
        missing_required_columns: u8,
        unknown_relation_tables: u8,
    },
    Items {
        count: u64,
    },
}

#[derive(Clone, PartialEq, Eq)]
pub struct AppError {
    pub code: AppErrorCode,
    pub message: &'static str,
    pub recovery: Option<&'static str>,
    pub retryable: bool,
    pub context: Option<SafeErrorContext>,
}

impl AppError {
    pub const fn database_not_found() -> Self {
        Self {
            code: AppErrorCode::DatabaseNotFound,
            message: "The selected Codex data source is unavailable.",
            recovery: Some("Select a compatible Codex data source and try again."),
            retryable: true,
            context: None,
        }
    }

    pub const fn database_schema_incompatible(
        missing_required_columns: u8,
        unknown_relation_tables: u8,
    ) -> Self {
        Self {
            code: AppErrorCode::DatabaseSchemaIncompatible,
            message: "The Codex data source schema is not compatible.",
            recovery: Some("Update Codex-O or select a supported data source."),
            retryable: false,
            context: Some(SafeErrorContext::Schema {
                missing_required_columns,
                unknown_relation_tables,
            }),
        }
    }

    pub const fn database_busy() -> Self {
        Self {
            code: AppErrorCode::DatabaseBusy,
            message: "The Codex data source is busy.",
            recovery: Some("Wait for Codex activity to finish, then try again."),
            retryable: true,
            context: None,
        }
    }
}

impl fmt::Debug for AppError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AppError")
            .field("code", &self.code.as_str())
            .field("message", &self.message)
            .field("recovery", &self.recovery)
            .field("retryable", &self.retryable)
            .field("context", &self.context)
            .finish()
    }
}

impl fmt::Display for AppError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.message)
    }
}

impl std::error::Error for AppError {}

#[cfg(test)]
mod tests {
    use super::{AppError, AppErrorCode, SafeErrorContext};

    #[test]
    fn stable_codes_match_the_documented_contract() {
        assert_eq!(
            AppErrorCode::DatabaseSchemaIncompatible.as_str(),
            "database_schema_incompatible"
        );
        assert_eq!(AppErrorCode::DatabaseBusy.as_str(), "database_busy");
    }

    #[test]
    fn schema_error_has_safe_recovery_and_context() {
        let error = AppError::database_schema_incompatible(2, 1);

        assert_eq!(
            error.recovery,
            Some("Update Codex-O or select a supported data source.")
        );
        assert!(!error.retryable);
        assert_eq!(
            error.context,
            Some(SafeErrorContext::Schema {
                missing_required_columns: 2,
                unknown_relation_tables: 1,
            })
        );
    }

    #[test]
    fn debug_and_display_do_not_expose_sensitive_markers() {
        let error = AppError::database_not_found();
        let rendered = format!("{error:?} {}", error);
        let absolute_path_marker = format!("{}{}", '/', "private/");

        assert!(!rendered.contains("fixture-sensitive-marker"));
        assert!(!rendered.contains(&absolute_path_marker));
        assert!(!rendered.contains("Authorization"));
    }

    #[test]
    fn busy_database_error_is_retryable() {
        let error = AppError::database_busy();

        assert!(error.retryable);
        assert_eq!(error.code, AppErrorCode::DatabaseBusy);
    }
}
