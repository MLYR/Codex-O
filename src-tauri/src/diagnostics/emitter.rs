use std::{
    fmt::{Arguments, Write as _},
    fs,
    path::PathBuf,
};

use log::Record;
use tauri::Runtime;
use tauri_plugin_log::{FileOpenStrategy, RotationStrategy, Target, TargetKind, TimezoneStrategy};

use super::model::{DiagnosticLevel, SafeLogEvent, MAX_LOG_LINE_BYTES};

pub const LOG_FILE_STEM: &str = "codex-o";

pub fn emit(event: SafeLogEvent) -> String {
    let event = normalize(event);
    let event_id = event.event_id.clone();
    let line = match serde_json::to_string(&event) {
        Ok(line) => line,
        Err(_) => return event_id,
    };
    match event.level {
        DiagnosticLevel::Info => log::info!(target: "codex_o", "{line}"),
        DiagnosticLevel::Warning => log::warn!(target: "codex_o", "{line}"),
        DiagnosticLevel::Error => log::error!(target: "codex_o", "{line}"),
    }
    event_id
}

pub fn build_plugin<R: Runtime>(log_directory: Option<PathBuf>) -> tauri::plugin::TauriPlugin<R> {
    let formatter = safe_formatter;
    let stdout = Target::new(TargetKind::Stdout).format(formatter);
    let targets = match log_directory.filter(|directory| prepare_log_directory(directory).is_ok()) {
        Some(directory) => vec![
            stdout,
            Target::new(TargetKind::Folder {
                path: directory,
                file_name: Some(LOG_FILE_STEM.to_owned()),
            })
            .format(safe_formatter),
        ],
        None => vec![stdout],
    };

    // ponytail: the plugin owns the only file writer and rotation policy; this function only chooses a safe target.
    tauri_plugin_log::Builder::new()
        .targets(targets)
        .level(log::LevelFilter::Info)
        .rotation_strategy(RotationStrategy::KeepSome(4))
        .timezone_strategy(TimezoneStrategy::UseUtc)
        .file_open_strategy(FileOpenStrategy::Append)
        .max_file_size(2 * 1024 * 1024)
        .build()
}

fn safe_formatter(
    out: tauri_plugin_log::fern::FormatCallback,
    _message: &Arguments,
    record: &Record<'_>,
) {
    let raw = record.args().to_string();
    let event = if raw.len() <= MAX_LOG_LINE_BYTES {
        serde_json::from_str::<SafeLogEvent>(&raw)
            .ok()
            .filter(|event| event.validate().is_ok())
            .unwrap_or_else(|| SafeLogEvent::rejection(level(record.level())))
    } else {
        SafeLogEvent::rejection(level(record.level()))
    };
    let event = normalize(event);
    let mut serialized = serde_json::to_string(&event).unwrap_or_default();
    if serialized.is_empty() {
        let _ = write!(serialized, "{{\"schema_version\":1,\"event_id\":\"evt-formatter-failed\",\"occurred_at\":1,\"level\":\"error\",\"category\":\"diagnostic\",\"domain\":\"diagnostics\",\"event_code\":\"frontend_log_rejected\",\"result\":\"degraded\",\"module\":\"diagnostics\",\"retryable\":false,\"redaction_version\":1}}");
    }
    out.finish(format_args!("{serialized}"));
}

fn normalize(event: SafeLogEvent) -> SafeLogEvent {
    if event.validate().is_ok() {
        event
    } else {
        SafeLogEvent::rejection(event.level)
    }
}

fn level(level: log::Level) -> DiagnosticLevel {
    match level {
        log::Level::Error => DiagnosticLevel::Error,
        log::Level::Warn => DiagnosticLevel::Warning,
        log::Level::Trace | log::Level::Debug | log::Level::Info => DiagnosticLevel::Info,
    }
}

fn prepare_log_directory(directory: &PathBuf) -> Result<(), ()> {
    if fs::symlink_metadata(directory)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        return Err(());
    }
    fs::create_dir_all(directory).map_err(|_| ())?;
    let metadata = fs::metadata(directory).map_err(|_| ())?;
    if metadata.is_dir() {
        Ok(())
    } else {
        Err(())
    }
}

#[cfg(test)]
mod tests {
    use super::{level, normalize};
    use crate::diagnostics::model::{
        DiagnosticDomain, DiagnosticEventCode, DiagnosticLevel, DiagnosticResult, SafeLogEvent,
    };

    #[test]
    fn invalid_events_are_replaced_without_retaining_payload() {
        let mut event = SafeLogEvent::new(
            DiagnosticLevel::Info,
            DiagnosticDomain::App,
            DiagnosticEventCode::AppStarted,
            DiagnosticResult::Succeeded,
        );
        event.module = "/Users/private".to_owned();
        let safe = normalize(event);
        assert_eq!(safe.event_code, DiagnosticEventCode::FrontendLogRejected);
        assert_eq!(safe.module, "diagnostics");
    }

    #[test]
    fn plugin_levels_map_to_safe_levels() {
        assert_eq!(level(log::Level::Warn), DiagnosticLevel::Warning);
        assert_eq!(level(log::Level::Error), DiagnosticLevel::Error);
    }
}
