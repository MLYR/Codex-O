//! Security boundary for the plan → confirm → execute lifecycle of Skill writes.

use std::{
    collections::HashMap,
    fs,
    io::Read,
    path::{Component, Path, PathBuf},
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

use rusqlite::{params, Connection, TransactionBehavior};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, State};
use tauri_plugin_dialog::DialogExt;

use crate::{
    catalog::SkillCatalog,
    observability::{
        DiagnosticDomain, DiagnosticErrorCode, DiagnosticEventCode, DiagnosticLevel,
        DiagnosticRecord, DiagnosticRecoveryCode, DiagnosticResult, DiagnosticService,
    },
};

const SKILL_MARKDOWN_FILE: &str = "SKILL.md";
const SELECTION_TTL_MS: u64 = 10 * 60 * 1000;
const CONFIRMATION_TTL_MS: u64 = 5 * 60 * 1000;
const MAX_IMPORT_FILES: usize = 256;
const MAX_IMPORT_TOTAL_BYTES: u64 = 32 * 1024 * 1024;
const MAX_IMPORT_TEXT_BYTES: u64 = 1024 * 1024;
const MAX_IMPORT_RESOURCE_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportSourceKind {
    File,
    Directory,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ManagementOperation {
    SkillImport,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationPlanStatus {
    Ready,
    Conflict,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationResultStatus {
    Succeeded,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SelectionToken {
    pub token: String,
    pub expires_at_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ConfirmationToken {
    pub token: String,
    pub expires_at_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct OperationImpact {
    pub target_provider_id: String,
    pub skill_name: String,
    pub file_count: usize,
    pub total_size_bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct OperationPlan {
    pub id: String,
    pub operation: ManagementOperation,
    pub status: OperationPlanStatus,
    pub impact: OperationImpact,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PlannedImport {
    pub plan: OperationPlan,
    pub confirmation_token: Option<ConfirmationToken>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct OperationResult {
    pub operation_id: String,
    pub status: OperationResultStatus,
    pub skill_id: String,
    pub installed_hash: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct OperationError {
    pub code: &'static str,
    pub message: &'static str,
    pub recovery: &'static str,
}

impl OperationError {
    const fn selection_unavailable() -> Self {
        Self {
            code: "selection_unavailable",
            message: "The source selection could not be completed.",
            recovery: "Choose a local SKILL.md file or Skill directory and try again.",
        }
    }

    const fn selection_token_invalid() -> Self {
        Self {
            code: "selection_token_invalid",
            message: "The selected source is no longer available.",
            recovery: "Select the local Skill again.",
        }
    }

    const fn selection_token_expired() -> Self {
        Self {
            code: "selection_token_expired",
            message: "The source selection expired.",
            recovery: "Select the local Skill again within ten minutes.",
        }
    }

    const fn confirmation_token_invalid() -> Self {
        Self {
            code: "confirmation_token_invalid",
            message: "The import confirmation is no longer available.",
            recovery: "Review the import plan again.",
        }
    }

    const fn confirmation_token_expired() -> Self {
        Self {
            code: "confirmation_token_expired",
            message: "The import confirmation expired.",
            recovery: "Review the import plan again within five minutes.",
        }
    }

    const fn confirmation_token_replayed() -> Self {
        Self {
            code: "confirmation_token_replayed",
            message: "This import confirmation was already used.",
            recovery: "Review the import plan again before retrying.",
        }
    }

    const fn provider_read_only() -> Self {
        Self {
            code: "provider_read_only",
            message: "This Skill provider does not permit imports.",
            recovery: "Choose the managed User provider.",
        }
    }

    const fn conflict_detected() -> Self {
        Self {
            code: "conflict_detected",
            message: "A Skill with this name already exists.",
            recovery: "Choose a different local Skill name; existing Skills are never overwritten.",
        }
    }

    const fn source_changed() -> Self {
        Self {
            code: "source_changed",
            message: "The selected Skill changed after planning.",
            recovery: "Review the updated local Skill before importing it.",
        }
    }

    const fn import_source_invalid() -> Self {
        Self {
            code: "import_source_invalid",
            message: "The selected Skill cannot be imported safely.",
            recovery: "Fix the Skill structure, paths, encoding, or size and try again.",
        }
    }

    const fn database_unavailable() -> Self {
        Self {
            code: "database_unavailable",
            message: "The import audit store is unavailable.",
            recovery: "Restore Codex-O storage before importing a Skill.",
        }
    }

    const fn import_failed() -> Self {
        Self {
            code: "import_failed",
            message: "The Skill import did not complete.",
            recovery: "Review the import plan and try again.",
        }
    }
}

#[derive(Clone)]
struct SelectedSource {
    source_path: PathBuf,
    kind: ImportSourceKind,
    expires_at_ms: u64,
}

#[derive(Clone)]
struct PendingConfirmation {
    plan: OperationPlan,
    source_path: PathBuf,
    kind: ImportSourceKind,
    source_hash: String,
    expires_at_ms: u64,
    consumed: bool,
}

#[derive(Debug)]
struct ImportSourceSummary {
    source_hash: String,
    target_name: String,
    file_count: usize,
    total_size_bytes: u64,
    files: Vec<(PathBuf, PathBuf)>,
}

pub struct OperationsService {
    database_path: Option<PathBuf>,
    target_root: PathBuf,
    catalog: SkillCatalog,
    diagnostics: Arc<DiagnosticService>,
    selections: Mutex<HashMap<String, SelectedSource>>,
    confirmations: Mutex<HashMap<String, PendingConfirmation>>,
    import_allowed: bool,
}

impl OperationsService {
    pub fn new(
        database_path: Option<PathBuf>,
        catalog: SkillCatalog,
        diagnostics: Arc<DiagnosticService>,
    ) -> Self {
        let target_root = catalog.managed_user_root();
        Self {
            database_path,
            target_root,
            catalog,
            diagnostics,
            selections: Mutex::new(HashMap::new()),
            confirmations: Mutex::new(HashMap::new()),
            import_allowed: true,
        }
    }

    fn select_source(
        &self,
        kind: ImportSourceKind,
        source_path: PathBuf,
    ) -> Result<SelectionToken, OperationError> {
        let token = random_token().ok_or_else(OperationError::selection_unavailable)?;
        let expires_at_ms = now_ms().saturating_add(SELECTION_TTL_MS);
        let mut selections = lock_unpoisoned(&self.selections);
        remove_expired_selections(&mut selections, now_ms());
        selections.insert(
            token.clone(),
            SelectedSource {
                source_path,
                kind,
                expires_at_ms,
            },
        );
        Ok(SelectionToken {
            token,
            expires_at_ms,
        })
    }

    fn plan_import(&self, selection_token: &str) -> Result<PlannedImport, OperationError> {
        let now = now_ms();
        let selected = {
            let mut selections = lock_unpoisoned(&self.selections);
            let selected = selections
                .get(selection_token)
                .cloned()
                .ok_or_else(OperationError::selection_token_invalid)?;
            if selected.expires_at_ms <= now {
                selections.remove(selection_token);
                return Err(OperationError::selection_token_expired());
            }
            remove_expired_selections(&mut selections, now);
            selected
        };
        let source = inspect_source(selected.kind, &selected.source_path)?;
        let conflict = path_exists(&self.target_root.join(&source.target_name));
        let plan = OperationPlan {
            id: random_token().ok_or_else(OperationError::selection_unavailable)?,
            operation: ManagementOperation::SkillImport,
            status: if conflict {
                OperationPlanStatus::Conflict
            } else {
                OperationPlanStatus::Ready
            },
            impact: OperationImpact {
                target_provider_id: "user_global".to_owned(),
                skill_name: source.target_name,
                file_count: source.file_count,
                total_size_bytes: source.total_size_bytes,
            },
        };
        if conflict {
            self.emit_failure(DiagnosticErrorCode::OperationConflict);
            return Ok(PlannedImport {
                plan,
                confirmation_token: None,
            });
        }

        let token = random_token().ok_or_else(OperationError::selection_unavailable)?;
        let expires_at_ms = now.saturating_add(CONFIRMATION_TTL_MS);
        lock_unpoisoned(&self.confirmations).insert(
            token.clone(),
            PendingConfirmation {
                plan: plan.clone(),
                source_path: selected.source_path,
                kind: selected.kind,
                source_hash: source.source_hash,
                expires_at_ms,
                consumed: false,
            },
        );
        self.diagnostics.emit(
            DiagnosticRecord::new(
                DiagnosticLevel::Info,
                DiagnosticDomain::Operations,
                DiagnosticEventCode::OperationPlanned,
                DiagnosticResult::Succeeded,
            )
            .with_counts(Some(plan.impact.file_count as u64), None),
        );
        Ok(PlannedImport {
            plan,
            confirmation_token: Some(ConfirmationToken {
                token,
                expires_at_ms,
            }),
        })
    }

    fn execute_import(&self, confirmation_token: &str) -> Result<OperationResult, OperationError> {
        if !self.import_allowed {
            self.emit_failure(DiagnosticErrorCode::OperationExecutionFailed);
            return Err(OperationError::provider_read_only());
        }
        let confirmation = self.consume_confirmation(confirmation_token)?;
        if confirmation.plan.status != OperationPlanStatus::Ready {
            self.emit_failure(DiagnosticErrorCode::OperationConflict);
            return Err(OperationError::conflict_detected());
        }
        let source = inspect_source(confirmation.kind, &confirmation.source_path)?;
        if source.source_hash != confirmation.source_hash {
            self.emit_failure(DiagnosticErrorCode::OperationSourceChanged);
            return Err(OperationError::source_changed());
        }
        if source.target_name != confirmation.plan.impact.skill_name
            || path_exists(&self.target_root.join(&source.target_name))
        {
            self.emit_failure(DiagnosticErrorCode::OperationConflict);
            return Err(OperationError::conflict_detected());
        }
        self.ensure_audit_store()?;

        let staging_home = self.create_staging_home(&confirmation.plan.id)?;
        let staged_skill = staging_home
            .join(".agents")
            .join("skills")
            .join(&source.target_name);
        let execution = (|| {
            copy_source_to_staging(&source, &staged_skill)?;
            let staged = inspect_source(ImportSourceKind::Directory, &staged_skill)?;
            if staged.source_hash != confirmation.source_hash {
                return Err(OperationError::source_changed());
            }
            let facts = self
                .catalog
                .validate_import_staging(staging_home.clone(), &source.target_name)
                .map_err(|_| OperationError::import_source_invalid())?;
            validate_import_metadata(&facts.name, &facts.description, &source.target_name)?;

            let target = self.target_root.join(&source.target_name);
            if path_exists(&target) {
                return Err(OperationError::conflict_detected());
            }
            fs::rename(&staged_skill, &target).map_err(|_| OperationError::import_failed())?;
            let _ = fs::remove_dir_all(&staging_home);
            self.catalog.scan_skills();
            let skill_id = self
                .catalog
                .managed_skill_id(&source.target_name)
                .ok_or_else(OperationError::import_failed)?;
            if let Err(error) =
                self.persist_success(&confirmation.plan, &skill_id, &facts.content_hash)
            {
                let _ = fs::remove_dir_all(&target);
                self.catalog.scan_skills();
                return Err(error);
            }
            Ok(OperationResult {
                operation_id: confirmation.plan.id.clone(),
                status: OperationResultStatus::Succeeded,
                skill_id,
                installed_hash: facts.content_hash,
            })
        })();
        if execution.is_err() {
            let _ = fs::remove_dir_all(&staging_home);
        }
        match &execution {
            Ok(result) => {
                self.diagnostics.emit(
                    DiagnosticRecord::new(
                        DiagnosticLevel::Info,
                        DiagnosticDomain::Operations,
                        DiagnosticEventCode::OperationExecuted,
                        DiagnosticResult::Succeeded,
                    )
                    .with_entity_ref(&result.skill_id),
                );
            }
            Err(error) => self.emit_failure(error_to_diagnostic(error)),
        }
        execution
    }

    fn consume_confirmation(
        &self,
        confirmation_token: &str,
    ) -> Result<PendingConfirmation, OperationError> {
        let now = now_ms();
        let mut confirmations = lock_unpoisoned(&self.confirmations);
        let confirmation = confirmations
            .get_mut(confirmation_token)
            .ok_or_else(OperationError::confirmation_token_invalid)?;
        if confirmation.expires_at_ms <= now {
            confirmations.remove(confirmation_token);
            return Err(OperationError::confirmation_token_expired());
        }
        if confirmation.consumed {
            return Err(OperationError::confirmation_token_replayed());
        }
        confirmation.consumed = true;
        Ok(confirmation.clone())
    }

    fn ensure_audit_store(&self) -> Result<(), OperationError> {
        let path = self
            .database_path
            .as_deref()
            .ok_or_else(OperationError::database_unavailable)?;
        Connection::open(path)
            .and_then(|connection| connection.execute_batch("PRAGMA foreign_keys = ON;"))
            .map_err(|_| OperationError::database_unavailable())
    }

    fn create_staging_home(&self, operation_id: &str) -> Result<PathBuf, OperationError> {
        fs::create_dir_all(&self.target_root).map_err(|_| OperationError::import_failed())?;
        let staging_home = self
            .target_root
            .join(format!(".codex-o-import-{operation_id}"));
        fs::create_dir(&staging_home).map_err(|_| OperationError::import_failed())?;
        Ok(staging_home)
    }

    fn persist_success(
        &self,
        plan: &OperationPlan,
        skill_id: &str,
        installed_hash: &str,
    ) -> Result<(), OperationError> {
        let path = self
            .database_path
            .as_deref()
            .ok_or_else(OperationError::database_unavailable)?;
        let mut connection =
            Connection::open(path).map_err(|_| OperationError::database_unavailable())?;
        connection
            .execute_batch("PRAGMA foreign_keys = ON;")
            .map_err(|_| OperationError::database_unavailable())?;
        let plan_json = serde_json::to_string(plan).map_err(|_| OperationError::import_failed())?;
        let result_json = serde_json::to_string(&OperationResultStatus::Succeeded)
            .map_err(|_| OperationError::import_failed())?;
        let now = now_ms() as i64;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| OperationError::database_unavailable())?;
        transaction
            .execute(
                "INSERT INTO install_receipts(skill_id, source_type, source_url, repo_ref, commit_sha, subdirectory, installed_hash, installed_at, managed_by) VALUES(?1, ?2, NULL, NULL, NULL, NULL, ?3, ?4, ?5)",
                params![skill_id, "local", installed_hash, now, "codex-o"],
            )
            .map_err(|_| OperationError::import_failed())?;
        transaction
            .execute(
                "INSERT INTO management_operations(id, skill_id, operation, status, plan_json, result_json, created_at, completed_at) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
                params![plan.id, skill_id, "skill_import", "succeeded", plan_json, result_json, now],
            )
            .map_err(|_| OperationError::import_failed())?;
        transaction
            .commit()
            .map_err(|_| OperationError::import_failed())
    }

    fn emit_failure(&self, error_code: DiagnosticErrorCode) {
        self.diagnostics.emit(
            DiagnosticRecord::new(
                DiagnosticLevel::Warning,
                DiagnosticDomain::Operations,
                DiagnosticEventCode::OperationFailed,
                DiagnosticResult::Failed,
            )
            .with_error(error_code, false, DiagnosticRecoveryCode::Retry),
        );
    }

    #[cfg(test)]
    fn expire_selection(&self, token: &str) {
        if let Some(selection) = lock_unpoisoned(&self.selections).get_mut(token) {
            selection.expires_at_ms = 0;
        }
    }

    #[cfg(test)]
    fn expire_confirmation(&self, token: &str) {
        if let Some(confirmation) = lock_unpoisoned(&self.confirmations).get_mut(token) {
            confirmation.expires_at_ms = 0;
        }
    }
}

#[tauri::command]
pub fn select_import_source(
    app: AppHandle,
    operations: State<'_, Arc<OperationsService>>,
    kind: ImportSourceKind,
) -> Result<SelectionToken, OperationError> {
    let selected = match kind {
        ImportSourceKind::File => app.dialog().file().blocking_pick_file(),
        ImportSourceKind::Directory => app.dialog().file().blocking_pick_folder(),
    }
    .ok_or_else(OperationError::selection_unavailable)?;
    let path = selected
        .into_path()
        .map_err(|_| OperationError::selection_unavailable())?;
    operations.select_source(kind, path)
}

#[tauri::command]
pub fn plan_skill_import(
    operations: State<'_, Arc<OperationsService>>,
    selection_token: String,
) -> Result<PlannedImport, OperationError> {
    operations.plan_import(&selection_token)
}

#[tauri::command]
pub fn execute_skill_import(
    operations: State<'_, Arc<OperationsService>>,
    confirmation_token: String,
) -> Result<OperationResult, OperationError> {
    operations.execute_import(&confirmation_token)
}

fn inspect_source(
    kind: ImportSourceKind,
    selected_path: &Path,
) -> Result<ImportSourceSummary, OperationError> {
    let source_root = match kind {
        ImportSourceKind::File => {
            let metadata = fs::symlink_metadata(selected_path)
                .map_err(|_| OperationError::import_source_invalid())?;
            if metadata.file_type().is_symlink()
                || !metadata.is_file()
                || selected_path.file_name().and_then(|name| name.to_str())
                    != Some(SKILL_MARKDOWN_FILE)
            {
                return Err(OperationError::import_source_invalid());
            }
            selected_path
                .parent()
                .map(Path::to_path_buf)
                .ok_or_else(OperationError::import_source_invalid)?
        }
        ImportSourceKind::Directory => selected_path.to_path_buf(),
    };
    let metadata =
        fs::symlink_metadata(&source_root).map_err(|_| OperationError::import_source_invalid())?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(OperationError::import_source_invalid());
    }
    let target_name = source_root
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| valid_skill_name(name))
        .map(str::to_owned)
        .ok_or_else(OperationError::import_source_invalid)?;
    let mut files = Vec::new();
    collect_source_files(&source_root, Path::new(""), &mut files)?;
    if files.is_empty()
        || !files
            .iter()
            .any(|(relative, _)| relative == Path::new(SKILL_MARKDOWN_FILE))
    {
        return Err(OperationError::import_source_invalid());
    }
    if files.len() > MAX_IMPORT_FILES {
        return Err(OperationError::import_source_invalid());
    }
    files.sort_by(|left, right| left.0.cmp(&right.0));

    let mut digest = Sha256::new();
    let mut total_size_bytes: u64 = 0;
    for (relative, path) in &files {
        let relative_text = relative
            .to_str()
            .filter(|value| valid_relative_path(value))
            .ok_or_else(OperationError::import_source_invalid)?;
        if !allowed_import_file(relative_text) {
            return Err(OperationError::import_source_invalid());
        }
        let metadata = fs::metadata(path).map_err(|_| OperationError::import_source_invalid())?;
        let limit = if relative_text == SKILL_MARKDOWN_FILE || relative_text == "agents/openai.yaml"
        {
            MAX_IMPORT_TEXT_BYTES
        } else {
            MAX_IMPORT_RESOURCE_BYTES
        };
        if metadata.len() > limit {
            return Err(OperationError::import_source_invalid());
        }
        total_size_bytes = total_size_bytes.saturating_add(metadata.len());
        if total_size_bytes > MAX_IMPORT_TOTAL_BYTES {
            return Err(OperationError::import_source_invalid());
        }
        digest.update((relative_text.len() as u64).to_be_bytes());
        digest.update(relative_text.as_bytes());
        hash_file(path, &mut digest)?;
    }
    Ok(ImportSourceSummary {
        source_hash: format!("{:x}", digest.finalize()),
        target_name,
        file_count: files.len(),
        total_size_bytes,
        files,
    })
}

fn collect_source_files(
    root: &Path,
    relative_directory: &Path,
    files: &mut Vec<(PathBuf, PathBuf)>,
) -> Result<(), OperationError> {
    let directory = root.join(relative_directory);
    let entries = fs::read_dir(&directory).map_err(|_| OperationError::import_source_invalid())?;
    for entry in entries {
        let entry = entry.map_err(|_| OperationError::import_source_invalid())?;
        let name = entry.file_name();
        let name = name
            .to_str()
            .ok_or_else(OperationError::import_source_invalid)?;
        let relative_path = relative_directory.join(name);
        let metadata = fs::symlink_metadata(entry.path())
            .map_err(|_| OperationError::import_source_invalid())?;
        if metadata.file_type().is_symlink() {
            return Err(OperationError::import_source_invalid());
        }
        if metadata.is_dir() {
            if !allowed_import_directory(&relative_path) {
                return Err(OperationError::import_source_invalid());
            }
            collect_source_files(root, &relative_path, files)?;
        } else if metadata.is_file() {
            files.push((relative_path, entry.path()));
        } else {
            return Err(OperationError::import_source_invalid());
        }
    }
    Ok(())
}

fn copy_source_to_staging(
    source: &ImportSourceSummary,
    staged_skill: &Path,
) -> Result<(), OperationError> {
    for (relative, path) in &source.files {
        let destination = staged_skill.join(relative);
        let parent = destination
            .parent()
            .ok_or_else(OperationError::import_failed)?;
        fs::create_dir_all(parent).map_err(|_| OperationError::import_failed())?;
        fs::copy(path, destination).map_err(|_| OperationError::import_failed())?;
    }
    Ok(())
}

fn hash_file(path: &Path, digest: &mut Sha256) -> Result<(), OperationError> {
    let mut file = fs::File::open(path).map_err(|_| OperationError::import_source_invalid())?;
    let mut buffer = [0_u8; 8192];
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|_| OperationError::import_source_invalid())?;
        if count == 0 {
            return Ok(());
        }
        digest.update(&buffer[..count]);
    }
}

fn valid_skill_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && !name.starts_with('-')
        && !name.ends_with('-')
}

fn valid_relative_path(path: &str) -> bool {
    !path.is_empty()
        && Path::new(path)
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

fn allowed_import_directory(relative: &Path) -> bool {
    let components = relative.components().collect::<Vec<_>>();
    matches!(
        components.as_slice(),
        [Component::Normal(name)] if *name == "agents"
            || *name == "resources"
            || *name == "scripts"
            || *name == "references"
    ) || matches!(components.first(), Some(Component::Normal(name)) if *name == "resources" || *name == "scripts" || *name == "references")
}

fn allowed_import_file(relative: &str) -> bool {
    if relative != SKILL_MARKDOWN_FILE
        && Path::new(relative)
            .file_name()
            .and_then(|name| name.to_str())
            == Some(SKILL_MARKDOWN_FILE)
    {
        return false;
    }
    relative == SKILL_MARKDOWN_FILE
        || relative == "agents/openai.yaml"
        || ["resources/", "scripts/", "references/"]
            .iter()
            .any(|prefix| relative.starts_with(prefix) && relative.len() > prefix.len())
}

fn validate_import_metadata(
    name: &Option<String>,
    description: &Option<String>,
    target_name: &str,
) -> Result<(), OperationError> {
    let name = name
        .as_deref()
        .filter(|value| valid_skill_name(value))
        .ok_or_else(OperationError::import_source_invalid)?;
    let description = description
        .as_deref()
        .filter(|value| !value.trim().is_empty() && value.len() <= 512)
        .filter(|value| !value.chars().any(char::is_control))
        .ok_or_else(OperationError::import_source_invalid)?;
    if name != target_name || description.is_empty() {
        return Err(OperationError::import_source_invalid());
    }
    Ok(())
}

fn path_exists(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}

fn random_token() -> Option<String> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).ok()?;
    Some(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or_default()
}

fn remove_expired_selections(selections: &mut HashMap<String, SelectedSource>, now: u64) {
    selections.retain(|_, selection| selection.expires_at_ms > now);
}

fn lock_unpoisoned<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn error_to_diagnostic(error: &OperationError) -> DiagnosticErrorCode {
    match error.code {
        "conflict_detected" => DiagnosticErrorCode::OperationConflict,
        "source_changed" => DiagnosticErrorCode::OperationSourceChanged,
        "selection_token_invalid"
        | "selection_token_expired"
        | "confirmation_token_invalid"
        | "confirmation_token_expired"
        | "confirmation_token_replayed" => DiagnosticErrorCode::OperationTokenInvalid,
        _ => DiagnosticErrorCode::OperationExecutionFailed,
    }
}

#[cfg(test)]
mod tests;
