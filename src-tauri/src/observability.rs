use std::{
    collections::VecDeque,
    fs::{self, File, OpenOptions},
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering},
        mpsc::{self, Receiver, SyncSender, TrySendError},
        Arc, Mutex,
    },
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, State};
use tauri_plugin_dialog::DialogExt;

use crate::{app_error::AppErrorCode, providers::ProviderKind};

const MEMORY_CAPACITY: usize = 500;
const WRITER_QUEUE_CAPACITY: usize = 256;
const FILE_LIMIT: usize = 5;
const FILE_BYTES_LIMIT: u64 = 2 * 1024 * 1024;
const TOTAL_BYTES_LIMIT: u64 = 10 * 1024 * 1024;
const SETTINGS_FILE_NAME: &str = "developer-settings.json";
const EXPORT_FILE_NAME: &str = "codex-o-diagnostics.jsonl";
const WRITER_RESPONSE_TIMEOUT: Duration = Duration::from_secs(2);
static EVENT_COUNTER: AtomicU64 = AtomicU64::new(0);
static SETTINGS_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticLevel {
    Info,
    Warning,
    Error,
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
    DiagnosticQueueDropped,
    DiagnosticAccessDenied,
    DiagnosticsExported,
    DiagnosticsCleared,
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
    ContinueWithMemoryDiagnostics,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticErrorCode {
    DeveloperModeRequired,
    LogStoreUnavailable,
    LogExportFailed,
    SelectionUnavailable,
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

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticRecord {
    id: String,
    occurred_at: i64,
    level: DiagnosticLevel,
    domain: DiagnosticDomain,
    event_code: DiagnosticEventCode,
    result: DiagnosticResult,
    duration_ms: Option<u64>,
    error_code: Option<DiagnosticErrorCode>,
    retryable: bool,
    recovery_code: Option<DiagnosticRecoveryCode>,
    provider_kind: Option<DiagnosticProviderKind>,
    item_count: Option<u64>,
    byte_count: Option<u64>,
    dropped_count: Option<u64>,
    entity_ref: Option<String>,
}

impl DiagnosticRecord {
    pub fn new(
        level: DiagnosticLevel,
        domain: DiagnosticDomain,
        event_code: DiagnosticEventCode,
        result: DiagnosticResult,
    ) -> Self {
        let occurred_at = unix_millis();
        let sequence = EVENT_COUNTER.fetch_add(1, Ordering::Relaxed);
        Self {
            id: format!("evt-{occurred_at:016x}-{sequence:016x}"),
            occurred_at,
            level,
            domain,
            event_code,
            result,
            duration_ms: None,
            error_code: None,
            retryable: false,
            recovery_code: None,
            provider_kind: None,
            item_count: None,
            byte_count: None,
            dropped_count: None,
            entity_ref: None,
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
        self.provider_kind = Some(provider_kind);
        self
    }

    pub fn with_counts(mut self, item_count: Option<u64>, byte_count: Option<u64>) -> Self {
        self.item_count = item_count;
        self.byte_count = byte_count;
        self
    }

    pub fn with_entity_ref(mut self, internal_id: &str) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(b"codex-o-diagnostic-entity");
        hasher.update(internal_id.as_bytes());
        self.entity_ref = Some(
            hasher
                .finalize()
                .iter()
                .take(10)
                .map(|byte| format!("{byte:02x}"))
                .collect(),
        );
        self
    }

    fn dropped(count: u64) -> Self {
        let mut record = Self::new(
            DiagnosticLevel::Warning,
            DiagnosticDomain::Diagnostics,
            DiagnosticEventCode::DiagnosticQueueDropped,
            DiagnosticResult::Degraded,
        );
        record.dropped_count = Some(count);
        record.recovery_code = Some(DiagnosticRecoveryCode::ContinueWithMemoryDiagnostics);
        record
    }

    fn is_safe(&self) -> bool {
        safe_identifier(&self.id, "evt-", 80)
            && self
                .entity_ref
                .as_deref()
                .is_none_or(|value| safe_hex(value, 20))
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticStoreStatus {
    Available,
    MemoryOnly,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DeveloperSettingsView {
    pub developer_mode_enabled: bool,
    pub store_status: DiagnosticStoreStatus,
    pub memory_capacity: usize,
    pub file_limit: usize,
    pub total_bytes_limit: u64,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(default, rename_all = "camelCase")]
pub struct DiagnosticQuery {
    pub level: Option<DiagnosticLevel>,
    pub domain: Option<DiagnosticDomain>,
    pub result: Option<DiagnosticResult>,
    pub error_code: Option<DiagnosticErrorCode>,
    pub event_id: Option<String>,
    pub limit: Option<usize>,
}

impl DiagnosticQuery {
    fn matches(&self, record: &DiagnosticRecord) -> bool {
        self.level.is_none_or(|level| record.level == level)
            && self.domain.is_none_or(|domain| record.domain == domain)
            && self.result.is_none_or(|result| record.result == result)
            && self
                .error_code
                .is_none_or(|error_code| record.error_code == Some(error_code))
            && self
                .event_id
                .as_deref()
                .is_none_or(|event_id| record.id == event_id)
    }

    fn limit(&self) -> usize {
        self.limit.unwrap_or(100).clamp(1, MEMORY_CAPACITY)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DiagnosticPage {
    pub records: Vec<DiagnosticRecord>,
    pub total: usize,
    pub store_status: DiagnosticStoreStatus,
    pub dropped_count: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DiagnosticExportResult {
    pub record_count: usize,
    pub file_name: &'static str,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DiagnosticClearResult {
    pub memory_records_cleared: usize,
    pub files_cleared: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticServiceErrorCode {
    DeveloperModeRequired,
    SettingsUnavailable,
    SelectionUnavailable,
    LogStoreUnavailable,
    LogExportFailed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DiagnosticServiceError {
    pub code: DiagnosticServiceErrorCode,
    pub message: &'static str,
    pub recovery: &'static str,
    pub retryable: bool,
    pub event_id: Option<String>,
}

impl DiagnosticServiceError {
    fn with_event_id(mut self, event_id: String) -> Self {
        self.event_id = Some(event_id);
        self
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct StoredDeveloperSettings {
    developer_mode_enabled: bool,
}

enum WriterCommand {
    Append(Vec<u8>),
    Clear(mpsc::Sender<Result<usize, ()>>),
    #[cfg(test)]
    Flush(mpsc::Sender<Result<(), ()>>),
}

pub struct DiagnosticService {
    memory: Mutex<VecDeque<DiagnosticRecord>>,
    developer_mode_enabled: AtomicBool,
    settings_path: Option<PathBuf>,
    store_status: Arc<AtomicU8>,
    writer: Option<SyncSender<WriterCommand>>,
    dropped_count: AtomicU64,
}

impl DiagnosticService {
    pub fn new(log_directory: Option<PathBuf>, settings_path: Option<PathBuf>) -> Arc<Self> {
        Self::with_limits(
            log_directory,
            settings_path,
            WRITER_QUEUE_CAPACITY,
            MEMORY_CAPACITY,
        )
    }

    fn with_limits(
        log_directory: Option<PathBuf>,
        settings_path: Option<PathBuf>,
        queue_capacity: usize,
        memory_capacity: usize,
    ) -> Arc<Self> {
        let developer_mode_enabled = settings_path
            .as_deref()
            .and_then(load_developer_settings)
            .is_some_and(|settings| settings.developer_mode_enabled);
        let store_status = Arc::new(AtomicU8::new(store_status_value(
            DiagnosticStoreStatus::MemoryOnly,
        )));
        let mut memory = VecDeque::with_capacity(memory_capacity);
        let writer = log_directory.and_then(|directory| {
            if prepare_log_directory(&directory).is_err() {
                return None;
            }
            load_existing_records(&directory, &mut memory, memory_capacity);
            let (sender, receiver) = mpsc::sync_channel(queue_capacity);
            store_status.store(
                store_status_value(DiagnosticStoreStatus::Available),
                Ordering::Release,
            );
            let worker_status = Arc::clone(&store_status);
            thread::Builder::new()
                .name("codex-o-diagnostics".to_owned())
                .spawn(move || writer_loop(directory, receiver, worker_status))
                .ok()
                .map(|_| sender)
        });
        if writer.is_none() {
            store_status.store(
                store_status_value(DiagnosticStoreStatus::MemoryOnly),
                Ordering::Release,
            );
        }
        Arc::new(Self {
            memory: Mutex::new(memory),
            developer_mode_enabled: AtomicBool::new(developer_mode_enabled),
            settings_path,
            store_status,
            writer,
            dropped_count: AtomicU64::new(0),
        })
    }

    pub fn emit(&self, record: DiagnosticRecord) -> String {
        let event_id = record.id.clone();
        self.push_memory(record.clone());
        let Some(writer) = &self.writer else {
            return event_id;
        };

        let dropped = self.dropped_count.swap(0, Ordering::AcqRel);
        if dropped > 0 {
            let aggregate = DiagnosticRecord::dropped(dropped);
            self.push_memory(aggregate.clone());
            if self.try_append(writer, aggregate).is_err() {
                self.dropped_count.fetch_add(dropped, Ordering::Relaxed);
            }
        }
        if self.try_append(writer, record).is_err() {
            self.dropped_count.fetch_add(1, Ordering::Relaxed);
        }
        event_id
    }

    pub fn developer_settings(&self) -> DeveloperSettingsView {
        DeveloperSettingsView {
            developer_mode_enabled: self.developer_mode_enabled.load(Ordering::Acquire),
            store_status: self.store_status(),
            memory_capacity: MEMORY_CAPACITY,
            file_limit: FILE_LIMIT,
            total_bytes_limit: TOTAL_BYTES_LIMIT,
        }
    }

    pub fn set_developer_mode(
        &self,
        enabled: bool,
    ) -> Result<DeveloperSettingsView, DiagnosticServiceError> {
        let path = self
            .settings_path
            .as_deref()
            .ok_or_else(settings_unavailable)?;
        write_developer_settings(
            path,
            &StoredDeveloperSettings {
                developer_mode_enabled: enabled,
            },
        )
        .map_err(|_| settings_unavailable())?;
        self.developer_mode_enabled
            .store(enabled, Ordering::Release);
        Ok(self.developer_settings())
    }

    pub fn list(&self, query: &DiagnosticQuery) -> Result<DiagnosticPage, DiagnosticServiceError> {
        self.require_developer_mode()?;
        Ok(self.list_authorized(query))
    }

    pub fn clear(&self) -> Result<DiagnosticClearResult, DiagnosticServiceError> {
        self.require_developer_mode()?;
        let memory_records_cleared = {
            let mut memory = self
                .memory
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let count = memory.len();
            memory.clear();
            count
        };
        let files_cleared = match &self.writer {
            Some(writer) => {
                let (sender, receiver) = mpsc::channel();
                writer
                    .send(WriterCommand::Clear(sender))
                    .map_err(|_| log_store_unavailable())?;
                receiver
                    .recv_timeout(WRITER_RESPONSE_TIMEOUT)
                    .map_err(|_| log_store_unavailable())?
                    .map_err(|_| log_store_unavailable())?
            }
            None => 0,
        };
        self.dropped_count.store(0, Ordering::Release);
        Ok(DiagnosticClearResult {
            memory_records_cleared,
            files_cleared,
        })
    }

    pub fn export_to(
        &self,
        query: &DiagnosticQuery,
        path: &Path,
    ) -> Result<DiagnosticExportResult, DiagnosticServiceError> {
        self.require_developer_mode()?;
        let page = self.list_authorized(query);
        let mut file = File::create(path).map_err(|_| log_export_failed())?;
        for record in &page.records {
            serde_json::to_writer(&mut file, record).map_err(|_| log_export_failed())?;
            file.write_all(b"\n").map_err(|_| log_export_failed())?;
        }
        file.flush().map_err(|_| log_export_failed())?;
        Ok(DiagnosticExportResult {
            record_count: page.records.len(),
            file_name: EXPORT_FILE_NAME,
        })
    }

    pub fn require_developer_mode(&self) -> Result<(), DiagnosticServiceError> {
        if self.developer_mode_enabled.load(Ordering::Acquire) {
            Ok(())
        } else {
            Err(developer_mode_required())
        }
    }

    fn list_authorized(&self, query: &DiagnosticQuery) -> DiagnosticPage {
        let memory = self
            .memory
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let total = memory.iter().filter(|record| query.matches(record)).count();
        let records = memory
            .iter()
            .rev()
            .filter(|record| query.matches(record))
            .take(query.limit())
            .cloned()
            .collect();
        DiagnosticPage {
            records,
            total,
            store_status: self.store_status(),
            dropped_count: self.dropped_count.load(Ordering::Acquire),
        }
    }

    fn store_status(&self) -> DiagnosticStoreStatus {
        store_status_from_value(self.store_status.load(Ordering::Acquire))
    }

    fn push_memory(&self, record: DiagnosticRecord) {
        let mut memory = self
            .memory
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if memory.len() == MEMORY_CAPACITY {
            memory.pop_front();
        }
        memory.push_back(record);
    }

    fn try_append(
        &self,
        writer: &SyncSender<WriterCommand>,
        record: DiagnosticRecord,
    ) -> Result<(), ()> {
        let mut bytes = serde_json::to_vec(&record).map_err(|_| ())?;
        bytes.push(b'\n');
        match writer.try_send(WriterCommand::Append(bytes)) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => Err(()),
            Err(TrySendError::Disconnected(_)) => {
                self.store_status.store(
                    store_status_value(DiagnosticStoreStatus::MemoryOnly),
                    Ordering::Release,
                );
                Err(())
            }
        }
    }

    #[cfg(test)]
    fn flush(&self) -> Result<(), ()> {
        let Some(writer) = &self.writer else {
            return Ok(());
        };
        let (sender, receiver) = mpsc::channel();
        writer.send(WriterCommand::Flush(sender)).map_err(|_| ())?;
        receiver
            .recv_timeout(WRITER_RESPONSE_TIMEOUT)
            .map_err(|_| ())?
    }
}

#[tauri::command]
pub fn get_developer_settings(
    diagnostics: State<'_, Arc<DiagnosticService>>,
) -> DeveloperSettingsView {
    diagnostics.developer_settings()
}

#[tauri::command]
pub fn set_developer_mode(
    diagnostics: State<'_, Arc<DiagnosticService>>,
    enabled: bool,
) -> Result<DeveloperSettingsView, DiagnosticServiceError> {
    match diagnostics.set_developer_mode(enabled) {
        Ok(view) => {
            diagnostics.emit(DiagnosticRecord::new(
                DiagnosticLevel::Info,
                DiagnosticDomain::Settings,
                DiagnosticEventCode::SettingsSaved,
                DiagnosticResult::Succeeded,
            ));
            Ok(view)
        }
        Err(error) => {
            let event_id = diagnostics.emit(
                DiagnosticRecord::new(
                    DiagnosticLevel::Error,
                    DiagnosticDomain::Settings,
                    DiagnosticEventCode::SettingsSaved,
                    DiagnosticResult::Failed,
                )
                .with_error(
                    DiagnosticErrorCode::SettingsUnavailable,
                    true,
                    DiagnosticRecoveryCode::Retry,
                ),
            );
            Err(error.with_event_id(event_id))
        }
    }
}

#[tauri::command]
pub fn list_diagnostics(
    diagnostics: State<'_, Arc<DiagnosticService>>,
    query: DiagnosticQuery,
) -> Result<DiagnosticPage, DiagnosticServiceError> {
    diagnostics.list(&query).map_err(|error| {
        let event_id = diagnostics.emit(
            DiagnosticRecord::new(
                DiagnosticLevel::Warning,
                DiagnosticDomain::Diagnostics,
                DiagnosticEventCode::DiagnosticAccessDenied,
                DiagnosticResult::Failed,
            )
            .with_error(
                service_error_diagnostic_code(error.code),
                error.retryable,
                DiagnosticRecoveryCode::CheckSettings,
            ),
        );
        error.with_event_id(event_id)
    })
}

#[tauri::command]
pub async fn export_diagnostics(
    app: AppHandle,
    diagnostics: State<'_, Arc<DiagnosticService>>,
    query: DiagnosticQuery,
) -> Result<DiagnosticExportResult, DiagnosticServiceError> {
    if let Err(error) = diagnostics.require_developer_mode() {
        return Err(record_diagnostic_command_error(
            diagnostics.inner(),
            error,
            DiagnosticEventCode::DiagnosticAccessDenied,
        ));
    }
    let (sender, receiver) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .add_filter("JSON Lines", &["jsonl"])
        .set_file_name(EXPORT_FILE_NAME)
        .save_file(move |selection| {
            let _ = sender.send(selection);
        });
    let selection = receiver.await.map_err(|_| {
        record_diagnostic_command_error(
            diagnostics.inner(),
            selection_unavailable(),
            DiagnosticEventCode::DiagnosticsExported,
        )
    })?;
    let path = selection
        .ok_or_else(selection_unavailable)
        .and_then(|selection| selection.into_path().map_err(|_| selection_unavailable()))
        .map_err(|error| {
            record_diagnostic_command_error(
                diagnostics.inner(),
                error,
                DiagnosticEventCode::DiagnosticsExported,
            )
        })?;
    let diagnostics = Arc::clone(diagnostics.inner());
    let export_diagnostics = Arc::clone(&diagnostics);
    let result =
        tauri::async_runtime::spawn_blocking(move || export_diagnostics.export_to(&query, &path))
            .await
            .map_err(|_| log_export_failed())?;
    match result {
        Ok(export) => {
            diagnostics.emit(
                DiagnosticRecord::new(
                    DiagnosticLevel::Info,
                    DiagnosticDomain::Diagnostics,
                    DiagnosticEventCode::DiagnosticsExported,
                    DiagnosticResult::Succeeded,
                )
                .with_counts(Some(export.record_count as u64), None),
            );
            Ok(export)
        }
        Err(error) => Err(record_diagnostic_command_error(
            &diagnostics,
            error,
            DiagnosticEventCode::DiagnosticsExported,
        )),
    }
}

#[tauri::command]
pub fn clear_diagnostics(
    diagnostics: State<'_, Arc<DiagnosticService>>,
) -> Result<DiagnosticClearResult, DiagnosticServiceError> {
    match diagnostics.clear() {
        Ok(result) => Ok(result),
        Err(error) => Err(record_diagnostic_command_error(
            diagnostics.inner(),
            error,
            DiagnosticEventCode::DiagnosticsCleared,
        )),
    }
}

pub fn settings_path(app_local_data_directory: &Path) -> PathBuf {
    app_local_data_directory.join(SETTINGS_FILE_NAME)
}

fn writer_loop(directory: PathBuf, receiver: Receiver<WriterCommand>, store_status: Arc<AtomicU8>) {
    while let Ok(command) = receiver.recv() {
        let result = match command {
            WriterCommand::Append(bytes) => append_record(&directory, &bytes),
            WriterCommand::Clear(response) => {
                let result = clear_log_files(&directory);
                let _ = response.send(result);
                continue;
            }
            #[cfg(test)]
            WriterCommand::Flush(response) => {
                let _ = response.send(Ok(()));
                continue;
            }
        };
        if result.is_err() {
            store_status.store(
                store_status_value(DiagnosticStoreStatus::MemoryOnly),
                Ordering::Release,
            );
        }
    }
}

fn prepare_log_directory(directory: &Path) -> Result<(), ()> {
    if directory
        .symlink_metadata()
        .is_ok_and(|metadata| metadata.file_type().is_symlink())
    {
        return Err(());
    }
    fs::create_dir_all(directory).map_err(|_| ())
}

fn append_record(directory: &Path, bytes: &[u8]) -> Result<(), ()> {
    let current = log_path(directory, 0);
    let current_size = fs::metadata(&current)
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    if current_size.saturating_add(bytes.len() as u64) > FILE_BYTES_LIMIT {
        rotate_files(directory)?;
    }
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path(directory, 0))
        .and_then(|mut file| file.write_all(bytes))
        .map_err(|_| ())?;
    enforce_total_limit(directory)
}

fn rotate_files(directory: &Path) -> Result<(), ()> {
    remove_if_exists(&log_path(directory, FILE_LIMIT - 1))?;
    for index in (0..FILE_LIMIT - 1).rev() {
        let source = log_path(directory, index);
        if source.exists() {
            fs::rename(source, log_path(directory, index + 1)).map_err(|_| ())?;
        }
    }
    Ok(())
}

fn enforce_total_limit(directory: &Path) -> Result<(), ()> {
    let mut total = log_files(directory)
        .iter()
        .filter_map(|path| fs::metadata(path).ok())
        .map(|metadata| metadata.len())
        .sum::<u64>();
    for index in (0..FILE_LIMIT).rev() {
        if total <= TOTAL_BYTES_LIMIT {
            break;
        }
        let path = log_path(directory, index);
        if let Ok(metadata) = fs::metadata(&path) {
            remove_if_exists(&path)?;
            total = total.saturating_sub(metadata.len());
        }
    }
    Ok(())
}

fn clear_log_files(directory: &Path) -> Result<usize, ()> {
    let mut cleared = 0;
    for path in log_files(directory) {
        if path.exists() {
            fs::remove_file(path).map_err(|_| ())?;
            cleared += 1;
        }
    }
    Ok(cleared)
}

fn remove_if_exists(path: &Path) -> Result<(), ()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(()),
    }
}

fn log_files(directory: &Path) -> Vec<PathBuf> {
    (0..FILE_LIMIT)
        .map(|index| log_path(directory, index))
        .collect()
}

fn log_path(directory: &Path, index: usize) -> PathBuf {
    directory.join(format!("diagnostics-{index}.jsonl"))
}

fn load_existing_records(
    directory: &Path,
    memory: &mut VecDeque<DiagnosticRecord>,
    memory_capacity: usize,
) {
    for index in (0..FILE_LIMIT).rev() {
        let Ok(file) = File::open(log_path(directory, index)) else {
            continue;
        };
        for line in BufReader::new(file).lines().map_while(Result::ok) {
            let Ok(record) = serde_json::from_str::<DiagnosticRecord>(&line) else {
                continue;
            };
            if !record.is_safe() {
                continue;
            }
            if memory.len() == memory_capacity {
                memory.pop_front();
            }
            memory.push_back(record);
        }
    }
}

fn load_developer_settings(path: &Path) -> Option<StoredDeveloperSettings> {
    fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
}

fn write_developer_settings(path: &Path, settings: &StoredDeveloperSettings) -> Result<(), ()> {
    let bytes = serde_json::to_vec(settings).map_err(|_| ())?;
    let parent = path.parent().ok_or(())?;
    fs::create_dir_all(parent).map_err(|_| ())?;
    let nonce = SETTINGS_COUNTER.fetch_add(1, Ordering::Relaxed);
    let temporary = parent.join(format!(
        ".developer-settings.{}.{}.tmp",
        std::process::id(),
        nonce
    ));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|_| ())?;
        file.write_all(&bytes).map_err(|_| ())?;
        file.sync_all().map_err(|_| ())?;
        fs::rename(&temporary, path).map_err(|_| ())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

fn unix_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}

fn safe_identifier(value: &str, prefix: &str, max_len: usize) -> bool {
    value.starts_with(prefix)
        && value.len() <= max_len
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-')
}

fn safe_hex(value: &str, length: usize) -> bool {
    value.len() == length && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

const fn store_status_value(status: DiagnosticStoreStatus) -> u8 {
    match status {
        DiagnosticStoreStatus::Available => 0,
        DiagnosticStoreStatus::MemoryOnly => 1,
    }
}

const fn store_status_from_value(value: u8) -> DiagnosticStoreStatus {
    if value == 0 {
        DiagnosticStoreStatus::Available
    } else {
        DiagnosticStoreStatus::MemoryOnly
    }
}

fn developer_mode_required() -> DiagnosticServiceError {
    DiagnosticServiceError {
        code: DiagnosticServiceErrorCode::DeveloperModeRequired,
        message: "Developer mode is required to access diagnostics.",
        recovery: "Enable developer mode in Settings and try again.",
        retryable: false,
        event_id: None,
    }
}

fn settings_unavailable() -> DiagnosticServiceError {
    DiagnosticServiceError {
        code: DiagnosticServiceErrorCode::SettingsUnavailable,
        message: "Developer settings could not be saved.",
        recovery: "Check the application data directory and try again.",
        retryable: true,
        event_id: None,
    }
}

fn selection_unavailable() -> DiagnosticServiceError {
    DiagnosticServiceError {
        code: DiagnosticServiceErrorCode::SelectionUnavailable,
        message: "No export destination was selected.",
        recovery: "Choose a destination and try again.",
        retryable: true,
        event_id: None,
    }
}

fn log_store_unavailable() -> DiagnosticServiceError {
    DiagnosticServiceError {
        code: DiagnosticServiceErrorCode::LogStoreUnavailable,
        message: "The diagnostic file store is unavailable.",
        recovery: "Continue with memory diagnostics or restart the application.",
        retryable: true,
        event_id: None,
    }
}

fn log_export_failed() -> DiagnosticServiceError {
    DiagnosticServiceError {
        code: DiagnosticServiceErrorCode::LogExportFailed,
        message: "The diagnostic export could not be written.",
        recovery: "Choose another destination and try again.",
        retryable: true,
        event_id: None,
    }
}

fn record_diagnostic_command_error(
    diagnostics: &DiagnosticService,
    error: DiagnosticServiceError,
    event_code: DiagnosticEventCode,
) -> DiagnosticServiceError {
    let event_id = diagnostics.emit(
        DiagnosticRecord::new(
            DiagnosticLevel::Error,
            DiagnosticDomain::Diagnostics,
            event_code,
            DiagnosticResult::Failed,
        )
        .with_error(
            service_error_diagnostic_code(error.code),
            error.retryable,
            DiagnosticRecoveryCode::Retry,
        ),
    );
    error.with_event_id(event_id)
}

const fn service_error_diagnostic_code(code: DiagnosticServiceErrorCode) -> DiagnosticErrorCode {
    match code {
        DiagnosticServiceErrorCode::DeveloperModeRequired => {
            DiagnosticErrorCode::DeveloperModeRequired
        }
        DiagnosticServiceErrorCode::SettingsUnavailable => DiagnosticErrorCode::SettingsUnavailable,
        DiagnosticServiceErrorCode::SelectionUnavailable => {
            DiagnosticErrorCode::SelectionUnavailable
        }
        DiagnosticServiceErrorCode::LogStoreUnavailable => DiagnosticErrorCode::LogStoreUnavailable,
        DiagnosticServiceErrorCode::LogExportFailed => DiagnosticErrorCode::LogExportFailed,
    }
}

// Compatibility model retained for the M1 safety gate.
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
    use std::{
        fs,
        sync::Arc,
        thread,
        time::{Duration, Instant},
    };

    use tempfile::tempdir;

    use crate::{app_error::AppErrorCode, providers::ProviderKind};

    use super::{
        log_files, DiagnosticDomain, DiagnosticErrorCode, DiagnosticEventCode, DiagnosticLevel,
        DiagnosticQuery, DiagnosticRecord, DiagnosticResult, DiagnosticService,
        DiagnosticServiceErrorCode, DiagnosticStoreStatus, EventName, LocalLogEvent, OperationName,
        OperationResult, FILE_BYTES_LIMIT, FILE_LIMIT, MEMORY_CAPACITY, TOTAL_BYTES_LIMIT,
    };

    fn event(sequence: u64) -> DiagnosticRecord {
        DiagnosticRecord::new(
            DiagnosticLevel::Info,
            DiagnosticDomain::Catalog,
            DiagnosticEventCode::CatalogCacheLoaded,
            DiagnosticResult::Succeeded,
        )
        .with_counts(Some(sequence), None)
        .with_entity_ref(&format!("fixture-{sequence}"))
    }

    fn enabled_service() -> (tempfile::TempDir, Arc<DiagnosticService>) {
        let directory = tempdir().unwrap();
        let service = DiagnosticService::new(
            Some(directory.path().join("logs")),
            Some(directory.path().join("settings.json")),
        );
        service.set_developer_mode(true).unwrap();
        (directory, service)
    }

    #[test]
    fn developer_mode_defaults_to_disabled_and_persists() {
        let directory = tempdir().unwrap();
        let settings = directory.path().join("settings.json");
        let service = DiagnosticService::new(None, Some(settings.clone()));
        assert!(!service.developer_settings().developer_mode_enabled);

        service.set_developer_mode(true).unwrap();
        let restored = DiagnosticService::new(None, Some(settings));
        assert!(restored.developer_settings().developer_mode_enabled);
    }

    #[test]
    fn protected_operations_reject_when_developer_mode_is_disabled() {
        let directory = tempdir().unwrap();
        let service = DiagnosticService::new(None, Some(directory.path().join("settings.json")));

        assert_eq!(
            service.list(&DiagnosticQuery::default()).unwrap_err().code,
            DiagnosticServiceErrorCode::DeveloperModeRequired
        );
        assert_eq!(
            service.clear().unwrap_err().code,
            DiagnosticServiceErrorCode::DeveloperModeRequired
        );
        assert_eq!(
            service
                .export_to(
                    &DiagnosticQuery::default(),
                    &directory.path().join("export.jsonl")
                )
                .unwrap_err()
                .code,
            DiagnosticServiceErrorCode::DeveloperModeRequired
        );
    }

    #[test]
    fn event_ids_are_unique_and_entity_references_are_irreversible() {
        let first = event(1);
        let second = event(1);
        let first_json = serde_json::to_string(&first).unwrap();

        assert_ne!(first.id, second.id);
        assert!(!first_json.contains("fixture-1"));
        assert!(first
            .entity_ref
            .as_deref()
            .is_some_and(|value| value.len() == 20));
    }

    #[test]
    fn memory_buffer_keeps_only_the_latest_five_hundred_records() {
        let (_directory, service) = enabled_service();
        for sequence in 0..(MEMORY_CAPACITY + 12) as u64 {
            service.emit(event(sequence));
        }
        let page = service
            .list(&DiagnosticQuery {
                limit: Some(MEMORY_CAPACITY),
                ..DiagnosticQuery::default()
            })
            .unwrap();

        assert_eq!(page.total, MEMORY_CAPACITY);
        assert_eq!(page.records.len(), MEMORY_CAPACITY);
    }

    #[test]
    fn unavailable_log_directory_degrades_to_memory_without_losing_events() {
        let directory = tempdir().unwrap();
        let blocked = directory.path().join("not-a-directory");
        fs::write(&blocked, b"fixture").unwrap();
        let service =
            DiagnosticService::new(Some(blocked), Some(directory.path().join("settings.json")));
        service.set_developer_mode(true).unwrap();
        service.emit(event(1));

        let page = service.list(&DiagnosticQuery::default()).unwrap();
        assert_eq!(page.store_status, DiagnosticStoreStatus::MemoryOnly);
        assert_eq!(page.records.len(), 1);
    }

    #[test]
    fn concurrent_emitters_preserve_unique_safe_records() {
        let (_directory, service) = enabled_service();
        let mut workers = Vec::new();
        for worker in 0..8 {
            let service = Arc::clone(&service);
            workers.push(thread::spawn(move || {
                for sequence in 0..40 {
                    service.emit(event(worker * 100 + sequence));
                }
            }));
        }
        for worker in workers {
            worker.join().unwrap();
        }

        let page = service
            .list(&DiagnosticQuery {
                limit: Some(MEMORY_CAPACITY),
                ..DiagnosticQuery::default()
            })
            .unwrap();
        let mut ids = page
            .records
            .iter()
            .filter(|record| record.event_code == DiagnosticEventCode::CatalogCacheLoaded)
            .map(|record| record.id.as_str())
            .collect::<Vec<_>>();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), 320);
    }

    #[test]
    fn clear_removes_memory_and_rotated_files() {
        let (directory, service) = enabled_service();
        service.emit(event(1));
        service.flush().unwrap();
        let result = service.clear().unwrap();

        assert_eq!(result.memory_records_cleared, 1);
        assert!(result.files_cleared <= FILE_LIMIT);
        assert!(log_files(&directory.path().join("logs"))
            .iter()
            .all(|path| !path.exists()));
    }

    #[test]
    fn export_contains_only_structured_allowlisted_fields() {
        let (directory, service) = enabled_service();
        service.emit(event(1));
        let export = directory.path().join("export.jsonl");
        let result = service
            .export_to(&DiagnosticQuery::default(), &export)
            .unwrap();
        let contents = fs::read_to_string(export).unwrap();

        assert_eq!(result.record_count, 1);
        assert!(contents.contains("\"event_code\":\"catalog_cache_loaded\""));
        for forbidden in [
            "message",
            "payload",
            "Authorization",
            "fixture-1",
            "/Users/",
        ] {
            assert!(!contents.contains(forbidden));
        }
    }

    #[test]
    fn file_writer_rotates_within_file_and_total_limits() {
        let directory = tempdir().unwrap();
        let logs = directory.path().join("logs");
        fs::create_dir_all(&logs).unwrap();
        let mut line = vec![b'x'; 8191];
        line.push(b'\n');
        for _ in 0..1500 {
            super::append_record(&logs, &line).unwrap();
        }

        let files = log_files(&logs)
            .into_iter()
            .filter(|path| path.exists())
            .collect::<Vec<_>>();
        let total = files
            .iter()
            .map(|path| fs::metadata(path).unwrap().len())
            .sum::<u64>();
        assert!(files.len() > 1);
        assert!(files.len() <= FILE_LIMIT);
        assert!(files
            .iter()
            .all(|path| fs::metadata(path).unwrap().len() <= FILE_BYTES_LIMIT));
        assert!(total <= TOTAL_BYTES_LIMIT);
    }

    #[test]
    fn malformed_or_unsafe_persisted_lines_are_ignored() {
        let directory = tempdir().unwrap();
        let logs = directory.path().join("logs");
        fs::create_dir_all(&logs).unwrap();
        fs::write(
            logs.join("diagnostics-0.jsonl"),
            b"{\"id\":\"/Users/private\",\"occurred_at\":0,\"level\":\"info\",\"domain\":\"app\",\"event_code\":\"app_started\",\"result\":\"succeeded\",\"duration_ms\":null,\"error_code\":null,\"retryable\":false,\"recovery_code\":null,\"provider_kind\":null,\"item_count\":null,\"byte_count\":null,\"dropped_count\":null,\"entity_ref\":null}\nnot-json\n",
        )
        .unwrap();
        let service =
            DiagnosticService::new(Some(logs), Some(directory.path().join("settings.json")));
        service.set_developer_mode(true).unwrap();

        assert_eq!(service.list(&DiagnosticQuery::default()).unwrap().total, 0);
    }

    #[test]
    fn query_filters_by_enumerated_fields_and_event_id() {
        let (_directory, service) = enabled_service();
        let failed = DiagnosticRecord::new(
            DiagnosticLevel::Error,
            DiagnosticDomain::SkillScan,
            DiagnosticEventCode::SkillScanFailed,
            DiagnosticResult::Failed,
        )
        .with_error(
            DiagnosticErrorCode::ScanFailed,
            true,
            super::DiagnosticRecoveryCode::Rescan,
        );
        let event_id = failed.id.clone();
        service.emit(event(1));
        service.emit(failed);

        let page = service
            .list(&DiagnosticQuery {
                level: Some(DiagnosticLevel::Error),
                event_id: Some(event_id),
                ..DiagnosticQuery::default()
            })
            .unwrap();
        assert_eq!(page.total, 1);
        assert_eq!(
            page.records[0].error_code,
            Some(DiagnosticErrorCode::ScanFailed)
        );
    }

    #[test]
    fn asynchronous_writer_does_not_block_event_submission() {
        let (_directory, service) = enabled_service();
        let started = Instant::now();
        for sequence in 0..500 {
            service.emit(event(sequence));
        }
        assert!(started.elapsed() < Duration::from_secs(1));
    }

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
