//! Runtime settings, secret-safe AI configuration, and environment health.

use std::{
    fs::{self, OpenOptions},
    io::Write,
    net::IpAddr,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, RwLock,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use reqwest::Url;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, State};
use tauri_plugin_dialog::DialogExt;

use crate::{
    analysis::{
        AiProvider, AiProviderConfig, AiProviderKind, AnalysisContext, AnalysisProviderErrorCode,
        AnalysisRequest, AnalysisSection, AnalysisSectionKind, AnalysisService, HttpAiProvider,
        RedactionCounts,
    },
    catalog::SkillCatalog,
    codex_fixture,
    db::{DatabaseDiagnosticCode, DatabaseStatus},
    providers::AdditionalRoot,
    secrets::{
        ProviderSecretId, SecretStore, SecretStoreErrorCode, SecretValue, SystemSecretStore,
    },
};

const DEFAULT_TIMEOUT_SECONDS: u64 = 45;
const MAX_API_KEY_BYTES: usize = 8192;
const CONFIG_FILE_NAME: &str = "ai-config.json";
const ADDITIONAL_ROOTS_FILE_NAME: &str = "additional-roots.json";
static TEMPORARY_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AiSecretAction {
    Keep,
    Replace,
    Clear,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiConfigInput {
    pub kind: AiProviderKind,
    pub base_url: String,
    pub model: String,
    pub language: String,
    pub timeout_seconds: u64,
    pub privacy_mode: bool,
    pub secret_action: AiSecretAction,
    pub api_key: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AiConfigView {
    pub configured: bool,
    pub kind: AiProviderKind,
    pub base_url: String,
    pub model: String,
    pub language: String,
    pub timeout_seconds: u64,
    pub privacy_mode: bool,
    pub has_api_key: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
struct StoredAiConfig {
    kind: AiProviderKind,
    base_url: String,
    model: String,
    language: String,
    timeout_seconds: u64,
    privacy_mode: bool,
    credential_slot: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SettingsErrorCode {
    InvalidConfiguration,
    SecretRequired,
    SecretUnavailable,
    StorageUnavailable,
    PrivacyRemoteBlocked,
    AiNotConfigured,
    PathNotAllowed,
    PathSymlinkDenied,
    RootDuplicate,
    SelectionUnavailable,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SettingsError {
    pub code: SettingsErrorCode,
    pub message: &'static str,
    pub recovery: &'static str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionStatus {
    Ready,
    Failed,
    Blocked,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ConnectionTestResult {
    pub status: ConnectionStatus,
    pub code: &'static str,
    pub latency_ms: u64,
    pub recommendation: &'static str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthStatus {
    Ready,
    Warning,
    Error,
    Unavailable,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct HealthItem {
    pub id: &'static str,
    pub status: HealthStatus,
    pub code: &'static str,
    pub recommendation: &'static str,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct EnvironmentHealth {
    pub items: Vec<HealthItem>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AdditionalRootView {
    pub id: String,
    pub display_name: String,
    pub read_only: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
struct StoredAdditionalRoot {
    id: String,
    path: PathBuf,
}

struct SettingsStorage {
    config_path: PathBuf,
    additional_roots_path: PathBuf,
}

pub struct SettingsService {
    storage: Option<SettingsStorage>,
    config: RwLock<Option<StoredAiConfig>>,
    additional_roots: RwLock<Vec<StoredAdditionalRoot>>,
    secrets: Arc<dyn SecretStore + Send + Sync>,
    analysis_service: Arc<AnalysisService>,
    catalog: SkillCatalog,
    database_status: DatabaseStatus,
    codex_database_path: Option<PathBuf>,
    credential_counter: AtomicU64,
}

impl SettingsService {
    pub fn new(
        config_path: PathBuf,
        secrets: Arc<dyn SecretStore + Send + Sync>,
        analysis_service: Arc<AnalysisService>,
        catalog: SkillCatalog,
        database_status: DatabaseStatus,
        codex_database_path: Option<PathBuf>,
    ) -> Self {
        let config = load_config(&config_path);
        let additional_roots_path = config_path.with_file_name(ADDITIONAL_ROOTS_FILE_NAME);
        let additional_roots = load_additional_roots(&additional_roots_path);
        let service = Self {
            storage: Some(SettingsStorage {
                config_path,
                additional_roots_path,
            }),
            config: RwLock::new(config),
            additional_roots: RwLock::new(additional_roots),
            secrets,
            analysis_service,
            catalog,
            database_status,
            codex_database_path,
            credential_counter: AtomicU64::new(0),
        };
        service.revalidate_loaded_additional_roots();
        service.refresh_analysis_provider();
        service
    }

    pub fn without_storage(
        secrets: Arc<dyn SecretStore + Send + Sync>,
        analysis_service: Arc<AnalysisService>,
        catalog: SkillCatalog,
        database_status: DatabaseStatus,
        codex_database_path: Option<PathBuf>,
    ) -> Self {
        // Missing app-local storage is a read-only degraded mode, never permission to write elsewhere.
        let service = Self {
            storage: None,
            config: RwLock::new(None),
            additional_roots: RwLock::new(Vec::new()),
            secrets,
            analysis_service,
            catalog,
            database_status,
            codex_database_path,
            credential_counter: AtomicU64::new(0),
        };
        service.refresh_analysis_provider();
        service
    }

    pub fn get_ai_config(&self) -> AiConfigView {
        self.current_config()
            .map(|config| self.view_for(&config))
            .unwrap_or_else(default_ai_config_view)
    }

    pub fn save_ai_config(&self, input: AiConfigInput) -> Result<AiConfigView, SettingsError> {
        let storage = self.storage.as_ref().ok_or_else(storage_unavailable)?;
        validate_api_key_input(&input)?;
        let previous = self.current_config();
        let previous_file = fs::read(&storage.config_path).ok();
        let previous_slot = previous
            .as_ref()
            .and_then(|config| config.credential_slot.clone());
        let mut created_slot = None;
        let credential_slot = match input.secret_action {
            AiSecretAction::Keep => previous_slot.clone(),
            AiSecretAction::Replace => {
                let slot = self.next_credential_slot();
                let secret_id = secret_id(&slot)?;
                let api_key = input.api_key.as_deref().ok_or_else(secret_required)?;
                self.secrets
                    .set(&secret_id, SecretValue::new(api_key))
                    .map_err(|_| secret_unavailable())?;
                created_slot = Some(slot.clone());
                Some(slot)
            }
            AiSecretAction::Clear => None,
        };
        let next = StoredAiConfig {
            kind: input.kind,
            base_url: input.base_url,
            model: input.model,
            language: input.language,
            timeout_seconds: input.timeout_seconds,
            privacy_mode: input.privacy_mode,
            credential_slot,
        };

        // Clear is an explicit request to retain non-secret settings in a disabled remote state.
        if let Err(error) =
            self.validate_stored_config(&next, input.secret_action == AiSecretAction::Clear)
        {
            self.cleanup_created_slot(created_slot.as_deref());
            return Err(error);
        }
        if write_config(&storage.config_path, &next).is_err() {
            self.cleanup_created_slot(created_slot.as_deref());
            return Err(storage_unavailable());
        }

        if previous_slot != next.credential_slot {
            if let Some(slot) = previous_slot.as_deref() {
                if let Err(error) = self.secrets.delete(&secret_id(slot)?) {
                    if error.code != SecretStoreErrorCode::NotFound {
                        restore_config(&storage.config_path, previous_file.as_deref());
                        self.cleanup_created_slot(created_slot.as_deref());
                        return Err(secret_unavailable());
                    }
                }
            }
        }

        *self
            .config
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(next.clone());
        self.refresh_analysis_provider();
        Ok(self.view_for(&next))
    }

    pub async fn test_ai_connection(&self) -> Result<ConnectionTestResult, SettingsError> {
        let config = self.current_config().ok_or_else(ai_not_configured)?;
        if !self.has_required_secret(&config) {
            return Err(secret_required());
        }
        if privacy_blocks_remote(&config) {
            return Ok(ConnectionTestResult {
                status: ConnectionStatus::Blocked,
                code: "privacy_remote_blocked",
                latency_ms: 0,
                recommendation: "Disable privacy mode or use a loopback Ollama endpoint.",
            });
        }
        let provider = self.provider_for(&config)?;
        let started = Instant::now();
        let result = provider
            .analyze(AnalysisRequest::new(
                connection_test_context(),
                RedactionCounts::default(),
                config.language.clone(),
            ))
            .await;
        let latency_ms = started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
        Ok(match result {
            Ok(_) => ConnectionTestResult {
                status: ConnectionStatus::Ready,
                code: "ai_connection_ready",
                latency_ms,
                recommendation: "The active AI configuration is ready.",
            },
            Err(error) => connection_failure(error.code, latency_ms),
        })
    }

    pub fn get_environment_health(&self) -> EnvironmentHealth {
        EnvironmentHealth {
            items: vec![
                database_health(self.database_status),
                self.keyring_health(),
                self.ai_health(),
                catalog_health(&self.catalog),
                codex_health(self.codex_database_path.as_deref()),
            ],
        }
    }

    pub fn list_additional_roots(&self) -> Vec<AdditionalRootView> {
        self.additional_roots
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .map(additional_root_view)
            .collect()
    }

    pub fn add_additional_root_path(
        &self,
        selected_path: PathBuf,
    ) -> Result<Vec<AdditionalRootView>, SettingsError> {
        let storage = self.storage.as_ref().ok_or_else(storage_unavailable)?;
        let canonical = self.validate_additional_root_path(&selected_path)?;
        let mut roots = self
            .additional_roots
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if roots.iter().any(|root| root.path == canonical) {
            return Err(root_duplicate());
        }
        let next = StoredAdditionalRoot {
            id: additional_root_id(&canonical),
            path: canonical,
        };
        let mut updated = roots.clone();
        updated.push(next);
        updated.sort_by(|left, right| left.id.cmp(&right.id));
        write_additional_roots(&storage.additional_roots_path, &updated)
            .map_err(|_| storage_unavailable())?;
        self.catalog.set_additional_roots(
            updated
                .iter()
                .filter_map(|root| AdditionalRoot::new(root.id.clone(), root.path.clone()))
                .collect(),
        );
        *roots = updated;
        Ok(roots.iter().map(additional_root_view).collect())
    }

    pub fn remove_additional_root(
        &self,
        root_id: &str,
    ) -> Result<Vec<AdditionalRootView>, SettingsError> {
        let storage = self.storage.as_ref().ok_or_else(storage_unavailable)?;
        if root_id.is_empty()
            || root_id.len() > 64
            || !root_id.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(path_not_allowed());
        }
        let mut roots = self
            .additional_roots
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if !roots.iter().any(|root| root.id == root_id) {
            return Err(path_not_allowed());
        }
        let updated = roots
            .iter()
            .filter(|root| root.id != root_id)
            .cloned()
            .collect::<Vec<_>>();
        write_additional_roots(&storage.additional_roots_path, &updated)
            .map_err(|_| storage_unavailable())?;
        self.catalog.set_additional_roots(
            updated
                .iter()
                .filter_map(|root| AdditionalRoot::new(root.id.clone(), root.path.clone()))
                .collect(),
        );
        *roots = updated;
        Ok(roots.iter().map(additional_root_view).collect())
    }

    fn current_config(&self) -> Option<StoredAiConfig> {
        self.config
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn view_for(&self, config: &StoredAiConfig) -> AiConfigView {
        AiConfigView {
            configured: true,
            kind: config.kind,
            base_url: config.base_url.clone(),
            model: config.model.clone(),
            language: config.language.clone(),
            timeout_seconds: config.timeout_seconds,
            privacy_mode: config.privacy_mode,
            has_api_key: config
                .credential_slot
                .as_deref()
                .is_some_and(|slot| self.secret_exists(slot)),
        }
    }

    fn validate_stored_config(
        &self,
        config: &StoredAiConfig,
        allow_missing_secret: bool,
    ) -> Result<(), SettingsError> {
        if !allow_missing_secret && !self.has_required_secret(config) {
            return Err(secret_required());
        }
        self.provider_for(config).map(|_| ())
    }

    fn has_required_secret(&self, config: &StoredAiConfig) -> bool {
        !requires_secret(config.kind)
            || config
                .credential_slot
                .as_deref()
                .is_some_and(|slot| self.secret_exists(slot))
    }

    fn provider_for(&self, config: &StoredAiConfig) -> Result<Arc<dyn AiProvider>, SettingsError> {
        let credential_id = config
            .credential_slot
            .as_deref()
            .map(secret_id)
            .transpose()?;
        let mut provider_config = AiProviderConfig::new(
            provider_id(config.kind),
            config.kind,
            config.base_url.clone(),
            config.model.clone(),
            config.language.clone(),
            credential_id,
        );
        provider_config.timeout = Duration::from_secs(config.timeout_seconds);
        HttpAiProvider::new(provider_config, Arc::clone(&self.secrets))
            .map(|provider| Arc::new(provider) as Arc<dyn AiProvider>)
            .map_err(|_| invalid_configuration())
    }

    fn refresh_analysis_provider(&self) {
        let provider = self.current_config().and_then(|config| {
            // A cleared remote credential keeps editable settings but must never enable requests.
            (self.has_required_secret(&config) && !privacy_blocks_remote(&config))
                .then(|| self.provider_for(&config).ok())
                .flatten()
        });
        self.analysis_service.set_provider(provider);
    }

    fn keyring_health(&self) -> HealthItem {
        let Some(config) = self.current_config() else {
            return health(
                "keyring",
                HealthStatus::Ready,
                "keyring_not_required",
                "Save an AI configuration when a remote provider is needed.",
            );
        };
        let Some(slot) = config.credential_slot.as_deref() else {
            if requires_secret(config.kind) {
                return health(
                    "keyring",
                    HealthStatus::Error,
                    "keyring_secret_missing",
                    "Replace the API key in Settings.",
                );
            }
            return health(
                "keyring",
                HealthStatus::Ready,
                "keyring_not_required",
                "The active provider does not use a stored API key.",
            );
        };
        match self.secrets.exists(&secret_id(slot).unwrap_or_else(|_| {
            ProviderSecretId::new("invalid").expect("static identifier is valid")
        })) {
            Ok(true) => health(
                "keyring",
                HealthStatus::Ready,
                "keyring_ready",
                "The configured API key is available to the Rust backend.",
            ),
            Ok(false) => health(
                "keyring",
                HealthStatus::Error,
                "keyring_secret_missing",
                "Replace the API key in Settings.",
            ),
            Err(_) => health(
                "keyring",
                HealthStatus::Unavailable,
                "keyring_unavailable",
                "Check system credential-store access, then retry.",
            ),
        }
    }

    fn ai_health(&self) -> HealthItem {
        let Some(config) = self.current_config() else {
            return health(
                "ai",
                HealthStatus::Warning,
                "ai_not_configured",
                "Static Skill details remain available without AI.",
            );
        };
        if privacy_blocks_remote(&config) {
            return health(
                "ai",
                HealthStatus::Warning,
                "privacy_remote_blocked",
                "Use loopback Ollama or disable privacy mode for remote AI.",
            );
        }
        if self.analysis_service.is_configured() {
            health(
                "ai",
                HealthStatus::Ready,
                "ai_ready",
                "AI analysis runs only after an explicit user request.",
            )
        } else {
            health(
                "ai",
                HealthStatus::Error,
                "ai_configuration_invalid",
                "Review the provider URL, model, timeout, and API key.",
            )
        }
    }

    fn secret_exists(&self, slot: &str) -> bool {
        secret_id(slot)
            .ok()
            .and_then(|id| self.secrets.exists(&id).ok())
            .unwrap_or(false)
    }

    fn next_credential_slot(&self) -> String {
        let sequence = self.credential_counter.fetch_add(1, Ordering::Relaxed);
        let epoch = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        format!("runtime-{epoch:x}-{sequence:x}")
    }

    fn cleanup_created_slot(&self, slot: Option<&str>) {
        if let Some(id) = slot.and_then(|value| secret_id(value).ok()) {
            let _ = self.secrets.delete(&id);
        }
    }

    fn validate_additional_root_path(
        &self,
        selected_path: &Path,
    ) -> Result<PathBuf, SettingsError> {
        let metadata = fs::symlink_metadata(selected_path).map_err(|_| path_not_allowed())?;
        if metadata.file_type().is_symlink() {
            return Err(path_symlink_denied());
        }
        if !metadata.is_dir() {
            return Err(path_not_allowed());
        }
        let canonical = fs::canonicalize(selected_path).map_err(|_| path_not_allowed())?;
        if path_contains_symlink(&canonical)
            || canonical.parent().is_none()
            || is_system_root(&canonical)
        {
            return Err(path_not_allowed());
        }

        let roots = self.catalog.roots_snapshot();
        if canonical_path(&roots.home_directory).as_ref() == Some(&canonical) {
            return Err(path_not_allowed());
        }
        for protected in [
            roots.home_directory.join(".agents/skills"),
            roots.home_directory.join(".codex/skills"),
            roots.repository_directory.join(".agents/skills"),
            roots.plugin_cache_directory,
        ] {
            if let Some(protected) = canonical_path(&protected) {
                if canonical.starts_with(&protected) || protected.starts_with(&canonical) {
                    return Err(path_not_allowed());
                }
            }
        }
        Ok(canonical)
    }

    fn revalidate_loaded_additional_roots(&self) {
        let loaded = self
            .additional_roots
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let mut validated = Vec::new();
        for root in loaded {
            if let Ok(path) = self.validate_additional_root_path(&root.path) {
                if !validated
                    .iter()
                    .any(|existing: &StoredAdditionalRoot| existing.path == path)
                {
                    validated.push(StoredAdditionalRoot {
                        id: additional_root_id(&path),
                        path,
                    });
                }
            }
        }
        self.catalog.set_additional_roots(
            validated
                .iter()
                .filter_map(|root| AdditionalRoot::new(root.id.clone(), root.path.clone()))
                .collect(),
        );
        *self
            .additional_roots
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = validated;
    }
}

#[tauri::command]
pub fn get_ai_config(settings: State<'_, Arc<SettingsService>>) -> AiConfigView {
    settings.get_ai_config()
}

#[tauri::command]
pub fn save_ai_config(
    settings: State<'_, Arc<SettingsService>>,
    input: AiConfigInput,
) -> Result<AiConfigView, SettingsError> {
    settings.save_ai_config(input)
}

#[tauri::command]
pub async fn test_ai_connection(
    settings: State<'_, Arc<SettingsService>>,
) -> Result<ConnectionTestResult, SettingsError> {
    settings.test_ai_connection().await
}

#[tauri::command]
pub fn get_environment_health(settings: State<'_, Arc<SettingsService>>) -> EnvironmentHealth {
    settings.get_environment_health()
}

#[tauri::command]
pub fn list_additional_roots(settings: State<'_, Arc<SettingsService>>) -> Vec<AdditionalRootView> {
    settings.list_additional_roots()
}

#[tauri::command]
pub fn select_additional_root(
    app: AppHandle,
    settings: State<'_, Arc<SettingsService>>,
) -> Result<Vec<AdditionalRootView>, SettingsError> {
    let selected = app
        .dialog()
        .file()
        .blocking_pick_folder()
        .ok_or_else(selection_unavailable)?;
    let path = selected.into_path().map_err(|_| path_not_allowed())?;
    settings.add_additional_root_path(path)
}

#[tauri::command]
pub fn remove_additional_root(
    settings: State<'_, Arc<SettingsService>>,
    root_id: String,
) -> Result<Vec<AdditionalRootView>, SettingsError> {
    settings.remove_additional_root(&root_id)
}

pub fn config_path(app_local_data_directory: &Path) -> PathBuf {
    app_local_data_directory.join(CONFIG_FILE_NAME)
}

pub fn system_secret_store() -> Arc<dyn SecretStore + Send + Sync> {
    Arc::new(SystemSecretStore::new())
}

fn default_ai_config_view() -> AiConfigView {
    AiConfigView {
        configured: false,
        kind: AiProviderKind::OpenAiCompatible,
        base_url: "https://api.openai.com/v1/".to_owned(),
        model: String::new(),
        language: "zh-CN".to_owned(),
        timeout_seconds: DEFAULT_TIMEOUT_SECONDS,
        privacy_mode: false,
        has_api_key: false,
    }
}

fn load_config(path: &Path) -> Option<StoredAiConfig> {
    fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
}

fn load_additional_roots(path: &Path) -> Vec<StoredAdditionalRoot> {
    fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn write_additional_roots(path: &Path, roots: &[StoredAdditionalRoot]) -> Result<(), ()> {
    let bytes = serde_json::to_vec(roots).map_err(|_| ())?;
    write_bytes_atomically(path, &bytes)
}

fn write_config(path: &Path, config: &StoredAiConfig) -> Result<(), ()> {
    let bytes = serde_json::to_vec(config).map_err(|_| ())?;
    write_bytes_atomically(path, &bytes)
}

fn restore_config(path: &Path, previous: Option<&[u8]>) {
    match previous {
        Some(bytes) => {
            let _ = write_bytes_atomically(path, bytes);
        }
        None => {
            let _ = fs::remove_file(path);
        }
    }
}

fn write_bytes_atomically(path: &Path, bytes: &[u8]) -> Result<(), ()> {
    let parent = path.parent().ok_or(())?;
    fs::create_dir_all(parent).map_err(|_| ())?;
    let nonce = TEMPORARY_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let temporary = temporary_path(path, nonce)?;
    // create_new plus a per-write nonce prevents concurrent settings files from sharing a staging file.
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|_| ())?;
        file.write_all(bytes).map_err(|_| ())?;
        file.sync_all().map_err(|_| ())?;
        fs::rename(&temporary, path).map_err(|_| ())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn temporary_path(path: &Path, nonce: u64) -> Result<PathBuf, ()> {
    let parent = path.parent().ok_or(())?;
    let file_name = path.file_name().and_then(|name| name.to_str()).ok_or(())?;
    Ok(parent.join(format!(".{file_name}.{}.{}.tmp", std::process::id(), nonce)))
}

fn validate_api_key_input(input: &AiConfigInput) -> Result<(), SettingsError> {
    match input.secret_action {
        AiSecretAction::Replace => {
            let value = input.api_key.as_deref().ok_or_else(secret_required)?;
            if value.trim().is_empty()
                || value.len() > MAX_API_KEY_BYTES
                || value.chars().any(char::is_control)
            {
                return Err(invalid_configuration());
            }
        }
        AiSecretAction::Keep | AiSecretAction::Clear if input.api_key.is_some() => {
            return Err(invalid_configuration());
        }
        AiSecretAction::Keep | AiSecretAction::Clear => {}
    }
    Ok(())
}

fn secret_id(slot: &str) -> Result<ProviderSecretId, SettingsError> {
    ProviderSecretId::new(format!("codex-o-{slot}")).map_err(|_| invalid_configuration())
}

fn additional_root_id(path: &Path) -> String {
    let mut hasher = Sha256::new();
    hasher.update(path.to_string_lossy().as_bytes());
    hasher
        .finalize()
        .iter()
        .take(12)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn additional_root_view(root: &StoredAdditionalRoot) -> AdditionalRootView {
    AdditionalRootView {
        id: root.id.clone(),
        display_name: root
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .unwrap_or("Additional Root")
            .chars()
            .take(120)
            .collect(),
        read_only: true,
    }
}

fn canonical_path(path: &Path) -> Option<PathBuf> {
    fs::canonicalize(path).ok()
}

fn path_contains_symlink(path: &Path) -> bool {
    path.ancestors().any(|ancestor| {
        fs::symlink_metadata(ancestor)
            .map(|metadata| metadata.file_type().is_symlink())
            .unwrap_or(false)
    })
}

fn is_system_root(path: &Path) -> bool {
    #[cfg(unix)]
    {
        [
            "/", "/System", "/Library", "/usr", "/bin", "/sbin", "/etc", "/var", "/private",
        ]
        .iter()
        .any(|root| path == Path::new(root))
    }
    #[cfg(not(unix))]
    {
        path.parent().is_none()
    }
}

fn provider_id(kind: AiProviderKind) -> &'static str {
    match kind {
        AiProviderKind::OpenAiCompatible => "openai_compatible",
        AiProviderKind::Anthropic => "anthropic",
        AiProviderKind::Ollama => "ollama",
    }
}

fn requires_secret(kind: AiProviderKind) -> bool {
    kind != AiProviderKind::Ollama
}

fn privacy_blocks_remote(config: &StoredAiConfig) -> bool {
    config.privacy_mode
        && (config.kind != AiProviderKind::Ollama || !is_loopback_url(&config.base_url))
}

fn is_loopback_url(value: &str) -> bool {
    Url::parse(value).ok().is_some_and(|url| {
        url.host_str().is_some_and(|host| {
            host.eq_ignore_ascii_case("localhost")
                || host
                    .parse::<IpAddr>()
                    .is_ok_and(|address| address.is_loopback())
        })
    })
}

fn connection_test_context() -> AnalysisContext {
    AnalysisContext {
        skill_id: "connection-test".to_owned(),
        content_hash: "connection-test".to_owned(),
        parser_version: "connection-test".to_owned(),
        sections: vec![AnalysisSection {
            id: "connection-test".to_owned(),
            kind: AnalysisSectionKind::Manifest,
            relative_path: "connection-test".to_owned(),
            line_start: 1,
            line_end: 1,
            title: "Connection test".to_owned(),
            content: "Return a schema-valid response for connectivity verification.".to_owned(),
        }],
        omitted_sections: Vec::new(),
        used_chars: 65,
        budget_chars: 128,
    }
}

fn connection_failure(code: AnalysisProviderErrorCode, latency_ms: u64) -> ConnectionTestResult {
    let (stable_code, recommendation) = match code {
        AnalysisProviderErrorCode::InvalidConfiguration => (
            "ai_configuration_invalid",
            "Review the provider URL, model, and timeout.",
        ),
        AnalysisProviderErrorCode::SecretUnavailable => (
            "ai_secret_unavailable",
            "Replace the API key or check system credential-store access.",
        ),
        AnalysisProviderErrorCode::RequestRejected => (
            "ai_request_rejected",
            "Check credentials, model access, and provider compatibility.",
        ),
        AnalysisProviderErrorCode::TransportUnavailable => (
            "ai_transport_unavailable",
            "Check the endpoint and local network, then retry.",
        ),
        AnalysisProviderErrorCode::ResponseTooLarge
        | AnalysisProviderErrorCode::InvalidResponse => (
            "ai_response_invalid",
            "Check that the endpoint implements the selected provider protocol.",
        ),
    };
    ConnectionTestResult {
        status: ConnectionStatus::Failed,
        code: stable_code,
        latency_ms,
        recommendation,
    }
}

fn database_health(status: DatabaseStatus) -> HealthItem {
    match status {
        DatabaseStatus::Ready { .. } => health(
            "app_database",
            HealthStatus::Ready,
            "app_database_ready",
            "Codex-O local storage is ready.",
        ),
        DatabaseStatus::Diagnostic(diagnostic) => {
            let (status, code) = match diagnostic.code {
                DatabaseDiagnosticCode::CorruptDatabase => {
                    (HealthStatus::Error, "app_database_corrupt")
                }
                DatabaseDiagnosticCode::UnsupportedSchemaVersion => {
                    (HealthStatus::Error, "app_database_schema_unsupported")
                }
                DatabaseDiagnosticCode::MigrationFailed => {
                    (HealthStatus::Error, "app_database_migration_failed")
                }
                DatabaseDiagnosticCode::StorageUnavailable => {
                    (HealthStatus::Unavailable, "app_database_unavailable")
                }
            };
            health("app_database", status, code, diagnostic.recovery)
        }
    }
}

fn catalog_health(catalog: &SkillCatalog) -> HealthItem {
    let providers = catalog.list_providers();
    if catalog.load_catalog().is_none() {
        return health(
            "skill_catalog",
            HealthStatus::Warning,
            "skill_catalog_not_scanned",
            "Run a manual Skill scan to create the local index.",
        );
    }
    if providers
        .providers
        .iter()
        .any(|provider| provider.availability == crate::catalog::ProviderAvailability::Available)
    {
        health(
            "skill_catalog",
            HealthStatus::Ready,
            "skill_catalog_ready",
            "Available providers can be browsed without AI.",
        )
    } else {
        health(
            "skill_catalog",
            HealthStatus::Warning,
            "skill_providers_unavailable",
            "Review provider settings and run another manual scan.",
        )
    }
}

fn codex_health(path: Option<&Path>) -> HealthItem {
    let Some(path) = path else {
        return health(
            "codex_data_source",
            HealthStatus::Unavailable,
            "codex_data_source_not_found",
            "Select a compatible Codex data source when session features are enabled.",
        );
    };
    match codex_fixture::inspect(path) {
        Ok(report) if report.is_compatible_for_listing() => health(
            "codex_data_source",
            HealthStatus::Ready,
            "codex_data_source_compatible",
            "The data source is compatible with read-only listing.",
        ),
        Ok(_) => health(
            "codex_data_source",
            HealthStatus::Warning,
            "codex_data_source_incompatible",
            "Upgrade Codex-O or select a compatible data source.",
        ),
        Err(_) => health(
            "codex_data_source",
            HealthStatus::Unavailable,
            "codex_data_source_unavailable",
            "Check that the Codex data source exists and is readable.",
        ),
    }
}

const fn health(
    id: &'static str,
    status: HealthStatus,
    code: &'static str,
    recommendation: &'static str,
) -> HealthItem {
    HealthItem {
        id,
        status,
        code,
        recommendation,
    }
}

const fn invalid_configuration() -> SettingsError {
    SettingsError {
        code: SettingsErrorCode::InvalidConfiguration,
        message: "The AI configuration is invalid.",
        recovery: "Review the provider URL, model, language, timeout, and secret action.",
    }
}

const fn secret_required() -> SettingsError {
    SettingsError {
        code: SettingsErrorCode::SecretRequired,
        message: "The selected provider requires an API key.",
        recovery: "Choose Replace key and provide a valid API key.",
    }
}

const fn secret_unavailable() -> SettingsError {
    SettingsError {
        code: SettingsErrorCode::SecretUnavailable,
        message: "The API key could not be stored safely.",
        recovery: "Check system credential-store access, then retry.",
    }
}

const fn storage_unavailable() -> SettingsError {
    SettingsError {
        code: SettingsErrorCode::StorageUnavailable,
        message: "The AI configuration could not be saved.",
        recovery: "Check the Codex-O data directory, then retry.",
    }
}

const fn ai_not_configured() -> SettingsError {
    SettingsError {
        code: SettingsErrorCode::AiNotConfigured,
        message: "AI is not configured.",
        recovery: "Save an AI provider configuration before testing the connection.",
    }
}

const fn path_not_allowed() -> SettingsError {
    SettingsError {
        code: SettingsErrorCode::PathNotAllowed,
        message: "The selected directory is not allowed.",
        recovery: "Choose a regular directory outside system, home, and existing provider roots.",
    }
}

const fn path_symlink_denied() -> SettingsError {
    SettingsError {
        code: SettingsErrorCode::PathSymlinkDenied,
        message: "Symbolic-link directories are not allowed.",
        recovery: "Choose the real directory instead of a symbolic link.",
    }
}

const fn root_duplicate() -> SettingsError {
    SettingsError {
        code: SettingsErrorCode::RootDuplicate,
        message: "The selected directory is already configured.",
        recovery: "Choose a different directory or remove the existing entry first.",
    }
}

const fn selection_unavailable() -> SettingsError {
    SettingsError {
        code: SettingsErrorCode::SelectionUnavailable,
        message: "No directory was selected.",
        recovery: "Open the directory picker and choose a folder.",
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashMap,
        io::{Read, Write},
        net::TcpListener,
        sync::{Arc, Mutex},
        thread,
    };

    use tempfile::TempDir;

    use crate::{
        analysis::{AnalysisService, UnavailableAnalysisCache},
        catalog::SkillCatalog,
        db::DatabaseStatus,
        providers::ProviderRoots,
        secrets::{SecretStoreError, SecretStoreErrorCode},
    };

    use super::*;

    #[derive(Default)]
    struct FixtureSecretStore {
        values: Mutex<HashMap<String, String>>,
        fail_set: Mutex<bool>,
    }

    impl SecretStore for FixtureSecretStore {
        fn set(
            &self,
            provider_id: &ProviderSecretId,
            secret: SecretValue,
        ) -> Result<(), SecretStoreError> {
            if *self.fail_set.lock().unwrap() {
                return Err(SecretStoreError {
                    code: SecretStoreErrorCode::Unavailable,
                });
            }
            self.values
                .lock()
                .unwrap()
                .insert(provider_id.account_name(), secret.expose().to_owned());
            Ok(())
        }

        fn get(&self, provider_id: &ProviderSecretId) -> Result<SecretValue, SecretStoreError> {
            self.values
                .lock()
                .unwrap()
                .get(&provider_id.account_name())
                .cloned()
                .map(SecretValue::new)
                .ok_or(SecretStoreError {
                    code: SecretStoreErrorCode::NotFound,
                })
        }

        fn delete(&self, provider_id: &ProviderSecretId) -> Result<(), SecretStoreError> {
            self.values
                .lock()
                .unwrap()
                .remove(&provider_id.account_name());
            Ok(())
        }
    }

    struct Fixture {
        _temp: TempDir,
        service: SettingsService,
        secrets: Arc<FixtureSecretStore>,
        home: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let temp = TempDir::new().unwrap();
            let home = temp.path().join("home");
            let repository = temp.path().join("repo");
            let cache = temp.path().join("cache");
            fs::create_dir_all(home.join(".agents/skills")).unwrap();
            fs::create_dir_all(repository.join(".agents/skills")).unwrap();
            fs::create_dir_all(&cache).unwrap();
            let roots = ProviderRoots::new(home.clone(), repository, cache);
            let catalog = SkillCatalog::new(roots);
            let analysis = Arc::new(AnalysisService::new(
                catalog.clone(),
                Arc::new(UnavailableAnalysisCache),
                None,
            ));
            let secrets = Arc::new(FixtureSecretStore::default());
            let service = SettingsService::new(
                temp.path().join(CONFIG_FILE_NAME),
                secrets.clone(),
                analysis,
                catalog,
                DatabaseStatus::Ready { schema_version: 3 },
                None,
            );
            Self {
                _temp: temp,
                service,
                secrets,
                home,
            }
        }
    }

    fn remote_input(secret_action: AiSecretAction, api_key: Option<&str>) -> AiConfigInput {
        AiConfigInput {
            kind: AiProviderKind::OpenAiCompatible,
            base_url: "https://example.com/v1/".to_owned(),
            model: "model".to_owned(),
            language: "zh-CN".to_owned(),
            timeout_seconds: 45,
            privacy_mode: false,
            secret_action,
            api_key: api_key.map(str::to_owned),
        }
    }

    #[test]
    fn ai_config_view_never_serializes_the_api_key_or_credential_slot() {
        let fixture = Fixture::new();
        let view = fixture
            .service
            .save_ai_config(remote_input(AiSecretAction::Replace, Some("top-secret")))
            .unwrap();
        let json = serde_json::to_string(&view).unwrap();

        assert!(view.has_api_key);
        assert!(!json.contains("top-secret"));
        assert!(!json.contains("credential"));
    }

    #[test]
    fn remote_provider_requires_an_explicit_key_replacement() {
        let fixture = Fixture::new();
        let error = fixture
            .service
            .save_ai_config(remote_input(AiSecretAction::Keep, None))
            .unwrap_err();

        assert_eq!(error.code, SettingsErrorCode::SecretRequired);
        assert!(!fixture.service.get_ai_config().configured);
    }

    #[test]
    fn successful_save_updates_runtime_without_a_restart() {
        let fixture = Fixture::new();
        let view = fixture
            .service
            .save_ai_config(remote_input(AiSecretAction::Replace, Some("fixture-key")))
            .unwrap();

        assert!(view.configured);
        assert!(view.has_api_key);
        assert!(fixture.service.analysis_service.is_configured());
    }

    #[test]
    fn clearing_a_remote_key_preserves_settings_and_disables_requests() {
        let fixture = Fixture::new();
        fixture
            .service
            .save_ai_config(remote_input(AiSecretAction::Replace, Some("fixture-key")))
            .unwrap();

        let view = fixture
            .service
            .save_ai_config(remote_input(AiSecretAction::Clear, None))
            .unwrap();
        let connection_error = runtime()
            .block_on(fixture.service.test_ai_connection())
            .unwrap_err();

        assert!(view.configured);
        assert!(!view.has_api_key);
        assert!(fixture.secrets.values.lock().unwrap().is_empty());
        assert!(!fixture.service.analysis_service.is_configured());
        assert_eq!(connection_error.code, SettingsErrorCode::SecretRequired);
        assert!(fixture
            .service
            .get_environment_health()
            .items
            .iter()
            .any(|item| item.code == "keyring_secret_missing"));
    }

    #[test]
    fn secret_write_failure_preserves_the_old_config_and_key() {
        let fixture = Fixture::new();
        let old = fixture
            .service
            .save_ai_config(remote_input(AiSecretAction::Replace, Some("old-key")))
            .unwrap();
        *fixture.secrets.fail_set.lock().unwrap() = true;

        let error = fixture
            .service
            .save_ai_config(remote_input(AiSecretAction::Replace, Some("new-key")))
            .unwrap_err();

        assert_eq!(error.code, SettingsErrorCode::SecretUnavailable);
        assert_eq!(fixture.service.get_ai_config(), old);
        assert_eq!(fixture.secrets.values.lock().unwrap().len(), 1);
        assert_eq!(
            fixture
                .secrets
                .values
                .lock()
                .unwrap()
                .values()
                .next()
                .map(String::as_str),
            Some("old-key")
        );
    }

    #[test]
    fn config_write_failure_preserves_the_old_config_and_key() {
        let fixture = Fixture::new();
        let old = fixture
            .service
            .save_ai_config(remote_input(AiSecretAction::Replace, Some("old-key")))
            .unwrap();
        let config_path = &fixture.service.storage.as_ref().unwrap().config_path;
        fs::remove_file(config_path).unwrap();
        fs::create_dir(config_path).unwrap();

        let error = fixture
            .service
            .save_ai_config(remote_input(AiSecretAction::Replace, Some("new-key")))
            .unwrap_err();

        assert_eq!(error.code, SettingsErrorCode::StorageUnavailable);
        assert_eq!(fixture.service.get_ai_config(), old);
        assert_eq!(fixture.secrets.values.lock().unwrap().len(), 1);
    }

    #[test]
    fn unavailable_app_storage_rejects_writes_before_touching_keyring() {
        let fixture = Fixture::new();
        let service = SettingsService::without_storage(
            fixture.secrets.clone(),
            Arc::clone(&fixture.service.analysis_service),
            fixture.service.catalog.clone(),
            fixture.service.database_status,
            None,
        );

        let error = service
            .save_ai_config(remote_input(AiSecretAction::Replace, Some("fixture-key")))
            .unwrap_err();

        assert_eq!(error.code, SettingsErrorCode::StorageUnavailable);
        assert!(fixture.secrets.values.lock().unwrap().is_empty());
        assert!(!service.get_ai_config().configured);
    }

    #[test]
    fn atomic_writes_use_distinct_target_specific_temporary_paths() {
        let directory = TempDir::new().unwrap();
        let config = directory.path().join(CONFIG_FILE_NAME);
        let roots = directory.path().join(ADDITIONAL_ROOTS_FILE_NAME);

        let config_temporary = temporary_path(&config, 7).unwrap();
        let roots_temporary = temporary_path(&roots, 7).unwrap();
        let next_config_temporary = temporary_path(&config, 8).unwrap();

        assert_ne!(config_temporary, roots_temporary);
        assert_ne!(config_temporary, next_config_temporary);
        assert!(config_temporary
            .file_name()
            .unwrap()
            .to_string_lossy()
            .contains(CONFIG_FILE_NAME));
        assert!(roots_temporary
            .file_name()
            .unwrap()
            .to_string_lossy()
            .contains(ADDITIONAL_ROOTS_FILE_NAME));
    }

    #[test]
    fn privacy_mode_blocks_remote_connection_tests_before_network_access() {
        let fixture = Fixture::new();
        let mut input = remote_input(AiSecretAction::Replace, Some("fixture-key"));
        input.privacy_mode = true;
        fixture.service.save_ai_config(input).unwrap();

        let result = runtime()
            .block_on(fixture.service.test_ai_connection())
            .unwrap();

        assert_eq!(result.status, ConnectionStatus::Blocked);
        assert_eq!(result.code, "privacy_remote_blocked");
    }

    #[test]
    fn loopback_ollama_connection_test_returns_only_safe_status() {
        let (base_url, handle) = ollama_server();
        let fixture = Fixture::new();
        fixture
            .service
            .save_ai_config(AiConfigInput {
                kind: AiProviderKind::Ollama,
                base_url,
                model: "local-model".to_owned(),
                language: "zh-CN".to_owned(),
                timeout_seconds: 5,
                privacy_mode: true,
                secret_action: AiSecretAction::Clear,
                api_key: None,
            })
            .unwrap();

        let result = runtime()
            .block_on(fixture.service.test_ai_connection())
            .unwrap();
        handle.join().unwrap();

        assert_eq!(result.status, ConnectionStatus::Ready);
        assert_eq!(result.code, "ai_connection_ready");
        assert!(!serde_json::to_string(&result)
            .unwrap()
            .contains("provider-body"));
    }

    #[test]
    fn environment_health_contains_no_paths_or_database_content() {
        let fixture = Fixture::new();
        let json = serde_json::to_string(&fixture.service.get_environment_health()).unwrap();

        assert!(!json.contains(fixture._temp.path().to_string_lossy().as_ref()));
        assert!(!json.contains("threads"));
        assert!(json.contains("app_database_ready"));
        assert!(json.contains("codex_data_source_not_found"));
    }

    #[test]
    fn additional_root_returns_only_an_opaque_id_and_safe_display_name() {
        let fixture = Fixture::new();
        let root = fixture._temp.path().join("team-skills");
        fs::create_dir(&root).unwrap();

        let views = fixture
            .service
            .add_additional_root_path(root.clone())
            .unwrap();
        let json = serde_json::to_string(&views).unwrap();

        assert_eq!(views.len(), 1);
        assert_eq!(views[0].display_name, "team-skills");
        assert!(views[0].read_only);
        assert!(!json.contains(root.to_string_lossy().as_ref()));
    }

    #[test]
    fn duplicate_additional_root_is_rejected_without_changing_the_store() {
        let fixture = Fixture::new();
        let root = fixture._temp.path().join("team-skills");
        fs::create_dir(&root).unwrap();
        fixture
            .service
            .add_additional_root_path(root.clone())
            .unwrap();

        let error = fixture.service.add_additional_root_path(root).unwrap_err();

        assert_eq!(error.code, SettingsErrorCode::RootDuplicate);
        assert_eq!(fixture.service.list_additional_roots().len(), 1);
    }

    #[test]
    fn home_and_existing_provider_roots_are_rejected() {
        let fixture = Fixture::new();

        let home_error = fixture
            .service
            .add_additional_root_path(fixture.home.clone())
            .unwrap_err();
        let provider_error = fixture
            .service
            .add_additional_root_path(fixture.home.join(".agents/skills"))
            .unwrap_err();

        assert_eq!(home_error.code, SettingsErrorCode::PathNotAllowed);
        assert_eq!(provider_error.code, SettingsErrorCode::PathNotAllowed);
        assert!(fixture.service.list_additional_roots().is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_additional_root_is_rejected() {
        use std::os::unix::fs::symlink;

        let fixture = Fixture::new();
        let target = fixture._temp.path().join("target");
        let link = fixture._temp.path().join("linked-root");
        fs::create_dir(&target).unwrap();
        symlink(target, &link).unwrap();

        let error = fixture.service.add_additional_root_path(link).unwrap_err();

        assert_eq!(error.code, SettingsErrorCode::PathSymlinkDenied);
        assert!(fixture.service.list_additional_roots().is_empty());
    }

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    }

    fn ollama_server() -> (String, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut buffer = [0_u8; 4096];
            let _ = stream.read(&mut buffer);
            let body = r#"{"message":{"content":"provider-body"}}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).unwrap();
        });
        (format!("http://{address}/"), handle)
    }
}
