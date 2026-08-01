use std::{
    fs,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use super::{
    emitter::{emit, LOG_FILE_STEM},
    model::{
        DiagnosticDomain, DiagnosticEventCode, DiagnosticLevel, DiagnosticResult, SafeLogEvent,
        MAX_LOG_LINE_BYTES,
    },
    DiagnosticCategory,
};

pub const LOG_SETTINGS_FILE_NAME: &str = "log-settings.json";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogStorageError {
    Unavailable,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LogStorageStatus {
    Available,
    Unavailable,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct LogSettings {
    pub physical_cleanup_on_start: bool,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct LogQuery {
    pub level: Option<DiagnosticLevel>,
    pub category: Option<DiagnosticCategory>,
    pub module: Option<String>,
    pub result: Option<DiagnosticResult>,
    pub from_occurred_at: Option<i64>,
    pub to_occurred_at: Option<i64>,
    pub trace_id: Option<String>,
    pub event_id: Option<String>,
    pub request_ref: Option<String>,
    pub cursor: Option<String>,
    pub limit: Option<usize>,
}

impl LogQuery {
    pub fn limit(&self) -> usize {
        self.limit.unwrap_or(200).clamp(1, 1_000)
    }

    fn matches(&self, event: &SafeLogEvent) -> bool {
        self.level.is_none_or(|value| event.level == value)
            && self.category.is_none_or(|value| event.category == value)
            && self
                .module
                .as_deref()
                .is_none_or(|value| event.module == value)
            && self.result.is_none_or(|value| event.result == value)
            && self
                .from_occurred_at
                .is_none_or(|value| event.occurred_at >= value)
            && self
                .to_occurred_at
                .is_none_or(|value| event.occurred_at <= value)
            && self
                .trace_id
                .as_deref()
                .is_none_or(|value| event.trace_id.as_deref() == Some(value))
            && self
                .event_id
                .as_deref()
                .is_none_or(|value| event.event_id == value)
            && self
                .request_ref
                .as_deref()
                .is_none_or(|value| event.request_ref.as_deref() == Some(value))
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct LogStats {
    pub total: usize,
    pub errors: usize,
    pub warnings: usize,
    pub ai_calls: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct LogFilterOptions {
    pub modules: Vec<String>,
    pub categories: Vec<DiagnosticCategory>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct LogCoverage {
    pub oldest_occurred_at: Option<i64>,
    pub newest_occurred_at: Option<i64>,
    pub historical_comparison_available: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct LogSnapshot {
    pub records: Vec<SafeLogEvent>,
    pub stats: LogStats,
    pub filters: LogFilterOptions,
    pub coverage: LogCoverage,
    pub cursor: Option<String>,
    pub storage_status: LogStorageStatus,
    pub invalid_line_count: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogFileIssue {
    Unavailable,
    Rejected,
}

pub fn read_snapshot(log_directory: Option<PathBuf>, query: &LogQuery) -> LogSnapshot {
    let Some(directory) = log_directory else {
        return unavailable_snapshot();
    };
    let (mut events, invalid_line_count, rejected) = read_files(&directory);
    if rejected && events.is_empty() {
        return LogSnapshot {
            storage_status: LogStorageStatus::Unavailable,
            ..unavailable_snapshot()
        };
    }
    // Rotate files are discovered by name, so order all events before applying the logical clear marker.
    events.sort_by_key(|event| event.occurred_at);
    if let Some(marker) = events
        .iter()
        .rposition(|event| event.event_code == DiagnosticEventCode::LogEventCleared)
    {
        events = events.into_iter().skip(marker + 1).collect();
    }
    events.retain(|event| event.event_code != DiagnosticEventCode::LogEventCleared);
    let stats = stats(&events);
    let filters = filter_options(&events);
    let coverage = coverage(&events);
    let filtered = events
        .into_iter()
        .filter(|event| query.matches(event))
        .collect::<Vec<_>>();
    let start = query
        .cursor
        .as_deref()
        .and_then(|cursor| cursor.parse::<usize>().ok())
        .unwrap_or(0);
    let records = filtered
        .iter()
        .skip(start)
        .take(query.limit())
        .cloned()
        .collect::<Vec<_>>();
    let next =
        (start + records.len() < filtered.len()).then(|| (start + records.len()).to_string());
    LogSnapshot {
        records,
        stats,
        filters,
        coverage,
        cursor: next,
        storage_status: if rejected {
            LogStorageStatus::Unavailable
        } else {
            LogStorageStatus::Available
        },
        invalid_line_count,
    }
}

pub fn log_settings_path(app_local_directory: &Path) -> PathBuf {
    app_local_directory.join(LOG_SETTINGS_FILE_NAME)
}

pub fn physical_cleanup_if_requested(
    app_local_directory: &Path,
    log_directory: &Path,
) -> Result<bool, LogStorageError> {
    let settings_path = log_settings_path(app_local_directory);
    let settings = load_settings(&settings_path);
    if !settings.physical_cleanup_on_start {
        return Ok(false);
    }
    for path in log_paths(log_directory)? {
        if fs::symlink_metadata(&path)
            .map_err(|_| LogStorageError::Unavailable)?
            .file_type()
            .is_symlink()
        {
            return Err(LogStorageError::Unavailable);
        }
        fs::remove_file(path).map_err(|_| LogStorageError::Unavailable)?;
    }
    write_settings(&settings_path, &LogSettings::default())?;
    Ok(true)
}

pub fn set_physical_cleanup(
    app_local_directory: &Path,
    requested: bool,
) -> Result<(), LogStorageError> {
    let path = log_settings_path(app_local_directory);
    write_settings(
        &path,
        &LogSettings {
            physical_cleanup_on_start: requested,
        },
    )
}

pub fn clear_log_logically() -> String {
    emit(SafeLogEvent::new(
        DiagnosticLevel::Info,
        DiagnosticDomain::Diagnostics,
        DiagnosticEventCode::LogEventCleared,
        DiagnosticResult::Succeeded,
    ))
}

fn read_files(directory: &Path) -> (Vec<SafeLogEvent>, usize, bool) {
    let paths = match log_paths(directory) {
        Ok(paths) => paths,
        Err(_) => return (Vec::new(), 0, true),
    };
    let mut events = Vec::new();
    let mut invalid = 0;
    let mut rejected = false;
    for path in paths {
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(_) => {
                rejected = true;
                continue;
            }
        };
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            rejected = true;
            continue;
        }
        let file = match fs::File::open(&path) {
            Ok(file) => file,
            Err(_) => {
                rejected = true;
                continue;
            }
        };
        for line in BufReader::new(file).lines() {
            let Ok(line) = line else {
                invalid += 1;
                continue;
            };
            if line.len() > MAX_LOG_LINE_BYTES {
                invalid += 1;
                continue;
            }
            let Ok(event) = serde_json::from_str::<SafeLogEvent>(&line) else {
                invalid += 1;
                continue;
            };
            if event.validate().is_err() {
                invalid += 1;
                continue;
            }
            events.push(event);
        }
    }
    (events, invalid, rejected)
}

fn log_paths(directory: &Path) -> Result<Vec<PathBuf>, LogStorageError> {
    let entries = fs::read_dir(directory).map_err(|_| LogStorageError::Unavailable)?;
    let mut paths = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|_| LogStorageError::Unavailable)?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let is_expected = name == format!("{LOG_FILE_STEM}.log")
            || (name.starts_with(&format!("{LOG_FILE_STEM}_")) && name.ends_with(".log"));
        if is_expected {
            paths.push(entry.path());
        }
    }
    paths.sort_by_key(|path| path.file_name().map(|name| name.to_os_string()));
    Ok(paths)
}

fn stats(events: &[SafeLogEvent]) -> LogStats {
    LogStats {
        total: events.len(),
        errors: events
            .iter()
            .filter(|event| event.level == DiagnosticLevel::Error)
            .count(),
        warnings: events
            .iter()
            .filter(|event| event.level == DiagnosticLevel::Warning)
            .count(),
        ai_calls: events
            .iter()
            .filter(|event| event.category == DiagnosticCategory::Ai)
            .count(),
    }
}

fn filter_options(events: &[SafeLogEvent]) -> LogFilterOptions {
    let mut modules = events
        .iter()
        .map(|event| event.module.clone())
        .collect::<Vec<_>>();
    modules.sort();
    modules.dedup();
    LogFilterOptions {
        modules,
        categories: vec![
            DiagnosticCategory::System,
            DiagnosticCategory::Diagnostic,
            DiagnosticCategory::Ai,
            DiagnosticCategory::SkillMcp,
        ],
    }
}

fn coverage(events: &[SafeLogEvent]) -> LogCoverage {
    let oldest = events.first().map(|event| event.occurred_at);
    let newest = events.last().map(|event| event.occurred_at);
    LogCoverage {
        oldest_occurred_at: oldest,
        newest_occurred_at: newest,
        historical_comparison_available: oldest
            .is_some_and(|value| newest.unwrap_or(value) - value >= 86_400_000),
    }
}

fn unavailable_snapshot() -> LogSnapshot {
    LogSnapshot {
        records: Vec::new(),
        stats: LogStats::default(),
        filters: LogFilterOptions::default(),
        coverage: LogCoverage::default(),
        cursor: None,
        storage_status: LogStorageStatus::Unavailable,
        invalid_line_count: 0,
    }
}

fn load_settings(path: &Path) -> LogSettings {
    fs::read_to_string(path)
        .ok()
        .and_then(|content| serde_json::from_str(&content).ok())
        .unwrap_or_default()
}

fn write_settings(path: &Path, settings: &LogSettings) -> Result<(), LogStorageError> {
    let parent = path.parent().ok_or(LogStorageError::Unavailable)?;
    fs::create_dir_all(parent).map_err(|_| LogStorageError::Unavailable)?;
    let temporary = path.with_extension("tmp");
    fs::write(
        &temporary,
        serde_json::to_vec(settings).map_err(|_| LogStorageError::Unavailable)?,
    )
    .map_err(|_| LogStorageError::Unavailable)?;
    fs::rename(temporary, path).map_err(|_| LogStorageError::Unavailable)
}

#[cfg(test)]
mod tests {
    use super::{read_snapshot, set_physical_cleanup, LogQuery, LogStorageStatus};
    use crate::diagnostics::model::{
        DiagnosticDomain, DiagnosticEventCode, DiagnosticLevel, DiagnosticResult, SafeLogEvent,
    };
    use std::{fs, path::Path};
    use tempfile::tempdir;

    #[test]
    fn corrupt_and_oversized_lines_are_ignored() {
        let directory = tempdir().unwrap();
        let oversized = "x".repeat(super::MAX_LOG_LINE_BYTES + 1);
        fs::write(
            directory.path().join("codex-o.log"),
            format!("not-json\n{{}}\n{oversized}\n"),
        )
        .unwrap();
        let snapshot = read_snapshot(Some(directory.path().to_path_buf()), &LogQuery::default());
        assert_eq!(snapshot.invalid_line_count, 3);
        assert_eq!(snapshot.records.len(), 0);
    }

    #[test]
    fn logical_clear_marker_hides_events_before_marker() {
        let directory = tempdir().unwrap();
        let mut before = SafeLogEvent::new(
            DiagnosticLevel::Info,
            DiagnosticDomain::App,
            DiagnosticEventCode::AppStarted,
            DiagnosticResult::Succeeded,
        );
        before.occurred_at = 1;
        let mut marker = SafeLogEvent::new(
            DiagnosticLevel::Info,
            DiagnosticDomain::Diagnostics,
            DiagnosticEventCode::LogEventCleared,
            DiagnosticResult::Succeeded,
        );
        marker.occurred_at = 2;
        let mut after = SafeLogEvent::new(
            DiagnosticLevel::Info,
            DiagnosticDomain::App,
            DiagnosticEventCode::FrontendReady,
            DiagnosticResult::Succeeded,
        );
        after.occurred_at = 3;
        fs::write(
            directory.path().join("codex-o_2026-08-01_000000.log"),
            format!("{}\n", serde_json::to_string(&before).unwrap()),
        )
        .unwrap();
        fs::write(
            directory.path().join("codex-o.log"),
            format!(
                "{}\n{}\n",
                serde_json::to_string(&marker).unwrap(),
                serde_json::to_string(&after).unwrap()
            ),
        )
        .unwrap();

        let snapshot = read_snapshot(Some(directory.path().to_path_buf()), &LogQuery::default());
        assert_eq!(snapshot.records.len(), 1);
        assert_eq!(
            snapshot.records[0].event_code,
            DiagnosticEventCode::FrontendReady
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_log_file_is_not_read() {
        use std::os::unix::fs::symlink;

        let directory = tempdir().unwrap();
        let target = directory.path().join("outside.log");
        fs::write(&target, "not-json\n").unwrap();
        symlink(target, directory.path().join("codex-o.log")).unwrap();

        let snapshot = read_snapshot(Some(directory.path().to_path_buf()), &LogQuery::default());
        assert_eq!(snapshot.storage_status, LogStorageStatus::Unavailable);
        assert!(snapshot.records.is_empty());
    }

    #[test]
    fn physical_cleanup_is_deferred_until_start() {
        let root = tempdir().unwrap();
        let logs = root.path().join("logs");
        fs::create_dir_all(&logs).unwrap();
        fs::write(logs.join("codex-o.log"), "fixture").unwrap();
        set_physical_cleanup(root.path(), true).unwrap();
        assert!(Path::new(&logs).join("codex-o.log").exists());
        assert_eq!(
            super::physical_cleanup_if_requested(root.path(), &logs),
            Ok(true)
        );
        assert!(!Path::new(&logs).join("codex-o.log").exists());
        assert_eq!(
            read_snapshot(Some(logs), &LogQuery::default()).storage_status,
            LogStorageStatus::Available
        );
    }
}
