use serde::Serialize;
use tauri::{AppHandle, Manager};
use tauri_plugin_dialog::DialogExt;

use super::{
    emitter::emit,
    model::{
        DiagnosticDomain, DiagnosticEventCode, DiagnosticLevel, DiagnosticResult, SafeLogEvent,
    },
    viewer::{
        clear_log_logically as write_log_clear_marker, read_snapshot, set_physical_cleanup,
        LogQuery, LogSnapshot,
    },
};

#[tauri::command]
pub fn read_log_snapshot(app: AppHandle, query: LogQuery) -> LogSnapshot {
    read_snapshot(app.path().app_log_dir().ok(), &query)
}

#[tauri::command]
pub fn clear_log_logical() -> String {
    write_log_clear_marker()
}

#[tauri::command]
pub fn set_log_physical_cleanup_on_start(app: AppHandle, requested: bool) -> Result<(), String> {
    let directory = app
        .path()
        .app_local_data_dir()
        .map_err(|_| "settings_unavailable".to_owned())?;
    set_physical_cleanup(&directory, requested).map_err(|_| "settings_unavailable".to_owned())?;
    emit(SafeLogEvent::new(
        DiagnosticLevel::Info,
        DiagnosticDomain::Diagnostics,
        DiagnosticEventCode::PhysicalLogCleanupRequested,
        DiagnosticResult::Succeeded,
    ));
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DiagnosticBundleResult {
    pub record_count: usize,
    pub file_name: String,
}

#[tauri::command]
pub async fn export_diagnostic_bundle(
    app: AppHandle,
    query: LogQuery,
) -> Result<DiagnosticBundleResult, String> {
    let snapshot = read_snapshot(app.path().app_log_dir().ok(), &query);
    let Some(path) = app
        .dialog()
        .file()
        .add_filter("诊断包", &["jsonl"])
        .set_file_name("codex-o-diagnostics.jsonl")
        .blocking_save_file()
    else {
        return Err("selection_unavailable".to_owned());
    };
    let path = path
        .into_path()
        .map_err(|_| "selection_unavailable".to_owned())?;
    let mut content = String::new();
    for event in snapshot.records.iter() {
        content
            .push_str(&serde_json::to_string(event).map_err(|_| "log_export_failed".to_owned())?);
        content.push('\n');
    }
    std::fs::write(&path, content).map_err(|_| "log_export_failed".to_owned())?;
    Ok(DiagnosticBundleResult {
        record_count: snapshot.records.len(),
        file_name: "codex-o-diagnostics.jsonl".to_owned(),
    })
}
