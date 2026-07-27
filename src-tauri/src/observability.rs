use crate::{app_error::AppErrorCode, providers::ProviderKind};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventName {
    CompatibilityProbe,
}

impl EventName {
    const fn as_str(self) -> &'static str {
        match self {
            Self::CompatibilityProbe => "compatibility_probe",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OperationName {
    Open,
    Inspect,
}

impl OperationName {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Inspect => "inspect",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OperationResult {
    Succeeded,
    Failed,
}

impl OperationResult {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LocalLogEvent {
    pub event: EventName,
    pub operation: OperationName,
    pub result: OperationResult,
    pub duration_ms: u64,
    pub error_code: Option<AppErrorCode>,
    pub retryable: bool,
    pub provider_kind: Option<ProviderKind>,
    pub item_count: u64,
    pub byte_count: u64,
}

impl LocalLogEvent {
    pub fn render(self) -> String {
        let mut fields = vec![
            format!("event={}", self.event.as_str()),
            format!("operation={}", self.operation.as_str()),
            format!("result={}", self.result.as_str()),
            format!("duration_ms={}", self.duration_ms),
            format!("retryable={}", self.retryable),
            format!("item_count={}", self.item_count),
            format!("byte_count={}", self.byte_count),
        ];

        if let Some(error_code) = self.error_code {
            fields.push(format!("error_code={}", error_code.as_str()));
        }
        if let Some(provider_kind) = self.provider_kind {
            fields.push(format!("provider_kind={provider_kind:?}"));
        }

        fields.join(" ")
    }
}

#[cfg(test)]
mod tests {
    use crate::{app_error::AppErrorCode, providers::ProviderKind};

    use super::{EventName, LocalLogEvent, OperationName, OperationResult};

    #[test]
    fn rendered_log_only_contains_allowlisted_fields() {
        let event = LocalLogEvent {
            event: EventName::CompatibilityProbe,
            operation: OperationName::Inspect,
            result: OperationResult::Succeeded,
            duration_ms: 12,
            error_code: None,
            retryable: false,
            provider_kind: Some(ProviderKind::LegacyUser),
            item_count: 3,
            byte_count: 64,
        };

        assert_eq!(
            event.render(),
            "event=compatibility_probe operation=inspect result=succeeded duration_ms=12 retryable=false item_count=3 byte_count=64 provider_kind=LegacyUser"
        );
    }

    #[test]
    fn failed_log_contains_only_a_stable_error_code() {
        let event = LocalLogEvent {
            event: EventName::CompatibilityProbe,
            operation: OperationName::Open,
            result: OperationResult::Failed,
            duration_ms: 0,
            error_code: Some(AppErrorCode::DatabaseNotFound),
            retryable: true,
            provider_kind: None,
            item_count: 0,
            byte_count: 0,
        };

        let rendered = event.render();
        let absolute_path_marker = format!("{}{}{}", '/', "Users", '/');
        assert!(rendered.contains("error_code=database_not_found"));
        assert!(!rendered.contains("fixture-sensitive-marker"));
        assert!(!rendered.contains(&absolute_path_marker));
    }

    #[test]
    fn log_event_has_no_arbitrary_text_field() {
        let event = LocalLogEvent {
            event: EventName::CompatibilityProbe,
            operation: OperationName::Open,
            result: OperationResult::Succeeded,
            duration_ms: 1,
            error_code: None,
            retryable: false,
            provider_kind: None,
            item_count: 0,
            byte_count: 0,
        };

        assert!(!event.render().contains("message="));
    }
}
