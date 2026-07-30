//! Security boundary for the plan → confirm → execute lifecycle of Skill writes.

mod github;

use std::{
    collections::{HashMap, HashSet},
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
    catalog::{QuarantineCandidate, SkillCatalog},
    observability::{
        DiagnosticDomain, DiagnosticErrorCode, DiagnosticEventCode, DiagnosticLevel,
        DiagnosticRecord, DiagnosticRecoveryCode, DiagnosticResult, DiagnosticService,
    },
};

const SKILL_MARKDOWN_FILE: &str = "SKILL.md";
const SELECTION_TTL_MS: u64 = 10 * 60 * 1000;
const CONFIRMATION_TTL_MS: u64 = 5 * 60 * 1000;
pub(crate) const MAX_IMPORT_FILES: usize = 256;
pub(crate) const MAX_IMPORT_TOTAL_BYTES: u64 = 32 * 1024 * 1024;
pub(crate) const MAX_IMPORT_TEXT_BYTES: u64 = 1024 * 1024;
pub(crate) const MAX_IMPORT_RESOURCE_BYTES: u64 = 16 * 1024 * 1024;

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
    SkillQuarantine,
    SkillRestore,
    QuarantinePurge,
    QuarantineKeepActive,
    QuarantineComplete,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationPlanStatus {
    Ready,
    Conflict,
    Partial,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationResultStatus {
    Succeeded,
    Partial,
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
    pub relative_files: Vec<String>,
    pub entry_id: Option<String>,
    pub requires_acknowledgement: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct OperationPlan {
    pub id: String,
    pub operation: ManagementOperation,
    pub status: OperationPlanStatus,
    pub impact: OperationImpact,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<OperationSource>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct OperationSource {
    pub source_type: String,
    pub repository_url: String,
    pub repo_ref: String,
    pub commit_sha: String,
    pub subdirectory: String,
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
    #[serde(skip_serializing)]
    pub installed_hash: String,
    pub entry_id: Option<String>,
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

    pub(crate) const fn import_failed() -> Self {
        Self {
            code: "import_failed",
            message: "The Skill import did not complete.",
            recovery: "Review the import plan and try again.",
        }
    }

    pub(crate) const fn market_item_unavailable() -> Self {
        Self {
            code: "market_item_unavailable",
            message: "The selected market Skill is no longer available.",
            recovery: "Refresh the official market and review the Skill again.",
        }
    }

    pub(crate) const fn market_source_changed() -> Self {
        Self {
            code: "market_source_changed",
            message: "The selected market source changed after synchronization.",
            recovery: "Refresh the official market and review the updated Skill.",
        }
    }

    pub(crate) const fn market_source_invalid() -> Self {
        Self {
            code: "market_source_invalid",
            message: "The selected market Skill failed safety validation.",
            recovery: "Refresh the official market or use a reviewed local/GitHub source.",
        }
    }

    pub(crate) const fn market_offline() -> Self {
        Self {
            code: "market_offline",
            message: "The official market could not be reached.",
            recovery: "Check the network connection and review the import again.",
        }
    }

    pub(crate) const fn market_timeout() -> Self {
        Self {
            code: "market_timeout",
            message: "The market import request timed out.",
            recovery: "Try again later or use local/GitHub install.",
        }
    }

    pub(crate) const fn market_rate_limited() -> Self {
        Self {
            code: "market_rate_limited",
            message: "GitHub temporarily limited the market import request.",
            recovery: "Wait before reviewing the market import again.",
        }
    }

    pub(crate) const fn market_protocol_error() -> Self {
        Self {
            code: "market_protocol_error",
            message: "The official market returned an unsupported response.",
            recovery: "Refresh the market later or use local/GitHub install.",
        }
    }

    const fn quarantine_unavailable() -> Self {
        Self {
            code: "quarantine_unavailable",
            message: "The Codex-O quarantine storage is unavailable.",
            recovery: "Restore app-local storage before managing this Skill.",
        }
    }

    const fn quarantine_not_allowed() -> Self {
        Self {
            code: "quarantine_not_allowed",
            message: "This Skill provider does not permit quarantine.",
            recovery: "Only writable User Skills can be quarantined.",
        }
    }

    const fn quarantine_entry_not_found() -> Self {
        Self {
            code: "quarantine_entry_not_found",
            message: "The quarantine entry is unavailable.",
            recovery: "Refresh the quarantine list and try again.",
        }
    }

    const fn quarantine_content_changed() -> Self {
        Self {
            code: "quarantine_content_changed",
            message: "The quarantined Skill changed after it was isolated.",
            recovery: "Do not restore or purge it until the content is reviewed.",
        }
    }

    const fn acknowledgement_required() -> Self {
        Self {
            code: "acknowledgement_required",
            message: "The Skill name acknowledgement does not match.",
            recovery: "Enter the displayed Skill name exactly before continuing.",
        }
    }

    const fn quarantine_partial() -> Self {
        Self {
            code: "quarantine_partial",
            message: "The operation kept both copies to avoid data loss.",
            recovery: "Review the partial quarantine entry before trying another operation.",
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
    source: PendingImportSource,
    expires_at_ms: u64,
    consumed: bool,
}

#[derive(Clone)]
enum PendingImportSource {
    Local {
        source_path: PathBuf,
        kind: ImportSourceKind,
        source_hash: String,
    },
    Remote {
        operation_root: PathBuf,
        staging_home: PathBuf,
        staged_skill: PathBuf,
        source_hash: String,
        provenance: OperationSource,
    },
}

impl PendingImportSource {
    fn cleanup(&self) {
        if let Self::Remote { operation_root, .. } = self {
            let _ = fs::remove_dir_all(operation_root);
        }
    }
}

#[derive(Clone)]
enum PendingManagedAction {
    Quarantine {
        candidate: QuarantineCandidate,
        summary: TreeSummary,
        requires_acknowledgement: bool,
    },
    Restore {
        entry: QuarantineEntry,
        summary: TreeSummary,
    },
    Purge {
        entry: QuarantineEntry,
        summary: TreeSummary,
    },
    KeepActive {
        entry: QuarantineEntry,
    },
    CompleteQuarantine {
        entry: QuarantineEntry,
    },
}

#[derive(Clone)]
struct PendingManagedConfirmation {
    plan: OperationPlan,
    action: PendingManagedAction,
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

#[derive(Clone, Debug)]
struct TreeSummary {
    content_hash: String,
    file_count: usize,
    total_size_bytes: u64,
    relative_files: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct QuarantineEntry {
    pub id: String,
    pub operation_id: String,
    pub skill_id: String,
    pub provider_id: String,
    #[serde(skip_serializing)]
    pub original_relative_path: String,
    #[serde(skip_serializing)]
    pub content_hash: String,
    pub display_name: String,
    pub file_count: usize,
    pub total_size_bytes: u64,
    pub status: String,
    pub quarantined_at: u64,
    pub restored_at: Option<u64>,
}

pub struct OperationsService {
    database_path: Option<PathBuf>,
    quarantine_root: Option<PathBuf>,
    github_staging_root: Option<PathBuf>,
    target_root: PathBuf,
    catalog: SkillCatalog,
    diagnostics: Arc<DiagnosticService>,
    selections: Mutex<HashMap<String, SelectedSource>>,
    confirmations: Mutex<HashMap<String, PendingConfirmation>>,
    managed_confirmations: Mutex<HashMap<String, PendingManagedConfirmation>>,
    import_allowed: bool,
    #[cfg(test)]
    force_copy_fallback: Mutex<bool>,
    #[cfg(test)]
    force_remove_failure: Mutex<bool>,
    #[cfg(test)]
    force_copy_verification_failure: Mutex<bool>,
    #[cfg(test)]
    force_rename_verification_failure: Mutex<bool>,
    #[cfg(test)]
    force_status_update_failures: Mutex<usize>,
    #[cfg(test)]
    force_delete_entry_failures: Mutex<usize>,
    #[cfg(test)]
    force_move_failure_after: Mutex<Option<usize>>,
}

impl OperationsService {
    pub fn new(
        database_path: Option<PathBuf>,
        app_local_data_root: Option<PathBuf>,
        catalog: SkillCatalog,
        diagnostics: Arc<DiagnosticService>,
    ) -> Self {
        let target_root = catalog.managed_user_root();
        let github_staging_root = app_local_data_root
            .as_ref()
            .map(|root| root.join("github-staging"));
        if let Some(root) = github_staging_root.as_deref() {
            github::cleanup_abandoned_staging(root);
        }
        Self {
            database_path,
            quarantine_root: app_local_data_root
                .as_ref()
                .map(|root| root.join("quarantine")),
            github_staging_root,
            target_root,
            catalog,
            diagnostics,
            selections: Mutex::new(HashMap::new()),
            confirmations: Mutex::new(HashMap::new()),
            managed_confirmations: Mutex::new(HashMap::new()),
            import_allowed: true,
            #[cfg(test)]
            force_copy_fallback: Mutex::new(false),
            #[cfg(test)]
            force_remove_failure: Mutex::new(false),
            #[cfg(test)]
            force_copy_verification_failure: Mutex::new(false),
            #[cfg(test)]
            force_rename_verification_failure: Mutex::new(false),
            #[cfg(test)]
            force_status_update_failures: Mutex::new(0),
            #[cfg(test)]
            force_delete_entry_failures: Mutex::new(0),
            #[cfg(test)]
            force_move_failure_after: Mutex::new(None),
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
                relative_files: source
                    .files
                    .iter()
                    .filter_map(|(relative, _)| relative.to_str().map(str::to_owned))
                    .collect(),
                entry_id: None,
                requires_acknowledgement: false,
            },
            source: None,
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
                source: PendingImportSource::Local {
                    source_path: selected.source_path,
                    kind: selected.kind,
                    source_hash: source.source_hash,
                },
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

    pub(crate) async fn plan_market_import(
        &self,
        source: &crate::market::MarketSelection,
        endpoints: &crate::market::MarketEndpoints,
    ) -> Result<PlannedImport, OperationError> {
        let operation_id = random_token().ok_or_else(OperationError::selection_unavailable)?;
        let operation_root = self.create_github_operation_root(&operation_id)?;
        let downloaded = tokio::time::timeout(
            std::time::Duration::from_secs(45),
            crate::market::download_market_skill(endpoints, source, &operation_root),
        )
        .await;
        let selected = match downloaded {
            Ok(Ok(path)) => path,
            Ok(Err(error)) => {
                let _ = fs::remove_dir_all(&operation_root);
                return Err(error);
            }
            Err(_) => {
                let _ = fs::remove_dir_all(&operation_root);
                return Err(OperationError::market_timeout());
            }
        };
        let prepared = (|| {
            let selected_summary = inspect_source(ImportSourceKind::Directory, &selected)
                .map_err(|_| OperationError::market_source_invalid())?;
            let staging_home = operation_root.join("home");
            let staged_skill = staging_home
                .join(".agents")
                .join("skills")
                .join(&selected_summary.target_name);
            copy_source_to_staging(&selected_summary, &staged_skill)?;
            let staged = inspect_source(ImportSourceKind::Directory, &staged_skill)?;
            if staged.source_hash != selected_summary.source_hash {
                return Err(OperationError::source_changed());
            }
            let facts = self
                .catalog
                .validate_import_staging(staging_home.clone(), &staged.target_name)
                .map_err(|_| OperationError::import_source_invalid())?;
            validate_import_metadata(&facts.name, &facts.description, &staged.target_name)?;
            let provenance = OperationSource {
                source_type: "market".to_owned(),
                repository_url: source.repository_url.clone(),
                repo_ref: "main".to_owned(),
                commit_sha: source.commit_sha.clone(),
                subdirectory: source.subdirectory.clone(),
            };
            let conflict = path_exists(&self.target_root.join(&staged.target_name));
            let plan = OperationPlan {
                id: operation_id,
                operation: ManagementOperation::SkillImport,
                status: if conflict {
                    OperationPlanStatus::Conflict
                } else {
                    OperationPlanStatus::Ready
                },
                impact: OperationImpact {
                    target_provider_id: "user_global".to_owned(),
                    skill_name: staged.target_name.clone(),
                    file_count: staged.file_count,
                    total_size_bytes: staged.total_size_bytes,
                    relative_files: staged
                        .files
                        .iter()
                        .filter_map(|(relative, _)| relative.to_str().map(str::to_owned))
                        .collect(),
                    entry_id: None,
                    requires_acknowledgement: false,
                },
                source: Some(provenance.clone()),
            };
            Ok((
                plan,
                PendingImportSource::Remote {
                    operation_root: operation_root.clone(),
                    staging_home,
                    staged_skill,
                    source_hash: staged.source_hash,
                    provenance,
                },
            ))
        })();
        match prepared {
            Ok((plan, pending)) => self.finalize_remote_plan(plan, pending, &operation_root),
            Err(error) => {
                let _ = fs::remove_dir_all(&operation_root);
                Err(error)
            }
        }
    }

    fn finalize_remote_plan(
        &self,
        plan: OperationPlan,
        pending: PendingImportSource,
        operation_root: &Path,
    ) -> Result<PlannedImport, OperationError> {
        if plan.status == OperationPlanStatus::Conflict {
            let _ = fs::remove_dir_all(operation_root);
            return Ok(PlannedImport {
                plan,
                confirmation_token: None,
            });
        }
        let token = match random_token() {
            Some(token) => token,
            None => {
                let _ = fs::remove_dir_all(operation_root);
                return Err(OperationError::selection_unavailable());
            }
        };
        let expires_at_ms = now_ms().saturating_add(CONFIRMATION_TTL_MS);
        lock_unpoisoned(&self.confirmations).insert(
            token.clone(),
            PendingConfirmation {
                plan: plan.clone(),
                source: pending,
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

    pub(crate) fn execute_import(
        &self,
        confirmation_token: &str,
    ) -> Result<OperationResult, OperationError> {
        if !self.import_allowed {
            self.emit_failure(DiagnosticErrorCode::OperationExecutionFailed);
            return Err(OperationError::provider_read_only());
        }
        let confirmation = self.consume_confirmation(confirmation_token)?;
        if confirmation.plan.status != OperationPlanStatus::Ready {
            self.emit_failure(DiagnosticErrorCode::OperationConflict);
            return Err(OperationError::conflict_detected());
        }
        let mut cleanup_root = match &confirmation.source {
            PendingImportSource::Local { .. } => None,
            PendingImportSource::Remote { operation_root, .. } => Some(operation_root.clone()),
        };
        let execution = (|| {
            let (staging_home, staged_skill, expected_hash, target_name, provenance) =
                match &confirmation.source {
                    PendingImportSource::Local {
                        source_path,
                        kind,
                        source_hash,
                    } => {
                        let source = inspect_source(*kind, source_path)?;
                        if source.source_hash != *source_hash {
                            return Err(OperationError::source_changed());
                        }
                        if source.target_name != confirmation.plan.impact.skill_name
                            || path_exists(&self.target_root.join(&source.target_name))
                        {
                            return Err(OperationError::conflict_detected());
                        }
                        let staging_home = self.create_staging_home(&confirmation.plan.id)?;
                        cleanup_root = Some(staging_home.clone());
                        let staged_skill = staging_home
                            .join(".agents")
                            .join("skills")
                            .join(&source.target_name);
                        copy_source_to_staging(&source, &staged_skill)?;
                        (
                            staging_home,
                            staged_skill,
                            source_hash.clone(),
                            source.target_name,
                            None,
                        )
                    }
                    PendingImportSource::Remote {
                        operation_root,
                        staging_home,
                        staged_skill,
                        source_hash,
                        provenance,
                    } => {
                        self.validate_github_staging(operation_root, staging_home, staged_skill)?;
                        github::validate_remote_provenance(provenance)?;
                        if confirmation.plan.source.as_ref() != Some(provenance) {
                            return Err(OperationError::source_changed());
                        }
                        (
                            staging_home.clone(),
                            staged_skill.clone(),
                            source_hash.clone(),
                            confirmation.plan.impact.skill_name.clone(),
                            Some(provenance.clone()),
                        )
                    }
                };
            self.ensure_audit_store()?;
            let staged = inspect_source(ImportSourceKind::Directory, &staged_skill)?;
            if staged.source_hash != expected_hash || staged.target_name != target_name {
                return Err(OperationError::source_changed());
            }
            let facts = self
                .catalog
                .validate_import_staging(staging_home.clone(), &target_name)
                .map_err(|_| OperationError::import_source_invalid())?;
            validate_import_metadata(&facts.name, &facts.description, &target_name)?;

            // GitHub staging lives under app-local data, so the managed target may not exist yet.
            fs::create_dir_all(&self.target_root).map_err(|_| OperationError::import_failed())?;
            let target = self.target_root.join(&target_name);
            if path_exists(&target) {
                return Err(OperationError::conflict_detected());
            }
            fs::rename(&staged_skill, &target).map_err(|_| OperationError::import_failed())?;
            if let Some(root) = cleanup_root.as_ref() {
                let _ = fs::remove_dir_all(root);
            }
            self.catalog.scan_skills();
            let skill_id = match self.catalog.managed_skill_id(&target_name) {
                Some(skill_id) => skill_id,
                None => {
                    // A failed Catalog refresh must not leave an unaudited installed directory.
                    let _ = fs::remove_dir_all(&target);
                    self.catalog.scan_skills();
                    return Err(OperationError::import_failed());
                }
            };
            if let Err(error) = self.persist_success(
                &confirmation.plan,
                &skill_id,
                &facts.content_hash,
                provenance.as_ref(),
            ) {
                let _ = fs::remove_dir_all(&target);
                self.catalog.scan_skills();
                return Err(error);
            }
            Ok(OperationResult {
                operation_id: confirmation.plan.id.clone(),
                status: OperationResultStatus::Succeeded,
                skill_id,
                installed_hash: facts.content_hash,
                entry_id: None,
            })
        })();
        if execution.is_err() {
            if let Some(root) = cleanup_root.as_ref() {
                let _ = fs::remove_dir_all(root);
            }
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

    fn plan_quarantine(&self, skill_id: &str) -> Result<PlannedImport, OperationError> {
        self.ensure_audit_store()?;
        let candidate = self
            .catalog
            .quarantine_candidate(skill_id)
            .map_err(|_| OperationError::quarantine_entry_not_found())?;
        if !candidate.can_quarantine {
            return Err(OperationError::quarantine_not_allowed());
        }
        let summary = summarize_tree(&candidate.directory)?;
        let requires_acknowledgement = !self.is_managed_skill(&candidate.id)?;
        let plan = OperationPlan {
            id: random_token().ok_or_else(OperationError::selection_unavailable)?,
            operation: ManagementOperation::SkillQuarantine,
            status: OperationPlanStatus::Ready,
            impact: OperationImpact {
                target_provider_id: candidate.provider_id.clone(),
                skill_name: candidate.display_name.clone(),
                file_count: summary.file_count,
                total_size_bytes: summary.total_size_bytes,
                relative_files: summary.relative_files.clone(),
                entry_id: None,
                requires_acknowledgement,
            },
            source: None,
        };
        self.issue_managed_confirmation(
            plan,
            PendingManagedAction::Quarantine {
                candidate,
                summary,
                requires_acknowledgement,
            },
        )
    }

    fn plan_restore(&self, entry_id: &str) -> Result<PlannedImport, OperationError> {
        // A failed restore rollback can leave an active copy with a stale quarantined row.
        self.converge_quarantined_entries()?;
        let entry = self.load_quarantine_entry(entry_id)?;
        if entry.status != "quarantined" || !self.catalog.provider_can_restore(&entry.provider_id) {
            return Err(OperationError::quarantine_not_allowed());
        }
        let source = self.quarantine_entry_path(&entry)?;
        let summary = summarize_tree(&source)?;
        if summary.content_hash != entry.content_hash
            || summary.file_count != entry.file_count
            || summary.total_size_bytes != entry.total_size_bytes
        {
            return Err(OperationError::quarantine_content_changed());
        }
        let target = self.target_root.join(&entry.original_relative_path);
        let conflict = path_exists(&target);
        let plan = OperationPlan {
            id: random_token().ok_or_else(OperationError::selection_unavailable)?,
            operation: ManagementOperation::SkillRestore,
            status: if conflict {
                OperationPlanStatus::Conflict
            } else {
                OperationPlanStatus::Ready
            },
            impact: OperationImpact {
                target_provider_id: entry.provider_id.clone(),
                skill_name: entry.display_name.clone(),
                file_count: summary.file_count,
                total_size_bytes: summary.total_size_bytes,
                relative_files: summary.relative_files.clone(),
                entry_id: Some(entry.id.clone()),
                requires_acknowledgement: false,
            },
            source: None,
        };
        if conflict {
            self.emit_failure(DiagnosticErrorCode::OperationConflict);
            return Ok(PlannedImport {
                plan,
                confirmation_token: None,
            });
        }
        self.issue_managed_confirmation(plan, PendingManagedAction::Restore { entry, summary })
    }

    fn plan_purge(&self, entry_id: &str) -> Result<PlannedImport, OperationError> {
        let entry = self.load_quarantine_entry(entry_id)?;
        if entry.status == "partial" {
            return Err(OperationError::quarantine_partial());
        }
        if entry.status != "quarantined" {
            return Err(OperationError::quarantine_not_allowed());
        }
        let source = self.quarantine_entry_path(&entry)?;
        let summary = summarize_tree(&source)?;
        if summary.content_hash != entry.content_hash
            || summary.file_count != entry.file_count
            || summary.total_size_bytes != entry.total_size_bytes
        {
            return Err(OperationError::quarantine_content_changed());
        }
        let plan = OperationPlan {
            id: random_token().ok_or_else(OperationError::selection_unavailable)?,
            operation: ManagementOperation::QuarantinePurge,
            status: OperationPlanStatus::Ready,
            impact: OperationImpact {
                target_provider_id: entry.provider_id.clone(),
                skill_name: entry.display_name.clone(),
                file_count: entry.file_count,
                total_size_bytes: entry.total_size_bytes,
                relative_files: summary.relative_files.clone(),
                entry_id: Some(entry.id.clone()),
                requires_acknowledgement: true,
            },
            source: None,
        };
        self.issue_managed_confirmation(plan, PendingManagedAction::Purge { entry, summary })
    }

    fn plan_keep_active(&self, entry_id: &str) -> Result<PlannedImport, OperationError> {
        self.plan_partial_resolution(entry_id, ManagementOperation::QuarantineKeepActive, true)
    }

    fn plan_complete_quarantine(&self, entry_id: &str) -> Result<PlannedImport, OperationError> {
        self.plan_partial_resolution(entry_id, ManagementOperation::QuarantineComplete, false)
    }

    fn plan_partial_resolution(
        &self,
        entry_id: &str,
        operation: ManagementOperation,
        keep_active: bool,
    ) -> Result<PlannedImport, OperationError> {
        let entry = self.load_quarantine_entry(entry_id)?;
        let summary = self.verify_partial_entry(&entry)?;
        let plan = OperationPlan {
            id: random_token().ok_or_else(OperationError::selection_unavailable)?,
            operation,
            status: OperationPlanStatus::Ready,
            impact: OperationImpact {
                target_provider_id: entry.provider_id.clone(),
                skill_name: entry.display_name.clone(),
                file_count: summary.file_count,
                total_size_bytes: summary.total_size_bytes,
                relative_files: summary.relative_files,
                entry_id: Some(entry.id.clone()),
                requires_acknowledgement: false,
            },
            source: None,
        };
        let action = if keep_active {
            PendingManagedAction::KeepActive { entry }
        } else {
            PendingManagedAction::CompleteQuarantine { entry }
        };
        self.issue_managed_confirmation(plan, action)
    }

    fn execute_managed(
        &self,
        confirmation_token: &str,
        acknowledgement: Option<&str>,
    ) -> Result<OperationResult, OperationError> {
        let confirmation = self.consume_managed_confirmation(confirmation_token)?;
        match confirmation.action {
            PendingManagedAction::Quarantine {
                candidate,
                summary,
                requires_acknowledgement,
            } => {
                if requires_acknowledgement
                    && acknowledgement.map(str::trim) != Some(candidate.display_name.as_str())
                {
                    return Err(OperationError::acknowledgement_required());
                }
                let current = summarize_tree(&candidate.directory)?;
                if current.content_hash != summary.content_hash {
                    return Err(OperationError::source_changed());
                }
                let entry = QuarantineEntry {
                    id: confirmation.plan.id.clone(),
                    operation_id: confirmation.plan.id.clone(),
                    skill_id: candidate.id.clone(),
                    provider_id: candidate.provider_id.clone(),
                    original_relative_path: candidate.relative_path.clone(),
                    content_hash: summary.content_hash.clone(),
                    display_name: candidate.display_name.clone(),
                    file_count: summary.file_count,
                    total_size_bytes: summary.total_size_bytes,
                    status: "pending".to_owned(),
                    quarantined_at: now_ms(),
                    restored_at: None,
                };
                self.insert_quarantine_entry(&entry, &confirmation.plan)?;
                let destination = self.quarantine_entry_path(&entry)?;
                let outcome = match self.move_tree(&candidate.directory, &destination) {
                    Ok(outcome) => outcome,
                    Err(error) => {
                        let _ = self.delete_quarantine_entry(&entry.id);
                        return Err(error);
                    }
                };
                let status = if outcome == MoveOutcome::Partial {
                    "partial"
                } else {
                    "quarantined"
                };
                if let Err(error) = self.update_quarantine_status(&entry.id, status, None) {
                    if outcome == MoveOutcome::Succeeded
                        && self.move_tree(&destination, &candidate.directory).is_ok()
                    {
                        let _ = self.delete_quarantine_entry(&entry.id);
                        return Err(error);
                    }
                    let _ = self.update_quarantine_status(&entry.id, "partial", None);
                    return Err(OperationError::quarantine_partial());
                }
                self.catalog.scan_skills();
                Ok(OperationResult {
                    operation_id: confirmation.plan.id,
                    status: if outcome == MoveOutcome::Partial {
                        OperationResultStatus::Partial
                    } else {
                        OperationResultStatus::Succeeded
                    },
                    skill_id: candidate.id,
                    installed_hash: summary.content_hash,
                    entry_id: Some(entry.id),
                })
            }
            PendingManagedAction::Restore { entry, summary } => {
                let source = self.quarantine_entry_path(&entry)?;
                let current = summarize_tree(&source)?;
                if current.content_hash != summary.content_hash
                    || current.content_hash != entry.content_hash
                {
                    return Err(OperationError::quarantine_content_changed());
                }
                let target = self.target_root.join(&entry.original_relative_path);
                if path_exists(&target) {
                    return Err(OperationError::conflict_detected());
                }
                let outcome = self.move_tree(&source, &target)?;
                let restored_at = now_ms();
                let status = if outcome == MoveOutcome::Partial {
                    "partial"
                } else {
                    "restored"
                };
                if let Err(error) =
                    self.update_quarantine_status(&entry.id, status, Some(restored_at))
                {
                    if outcome == MoveOutcome::Succeeded {
                        // A failed audit finalization must not strand the only copy outside quarantine.
                        if self.move_tree(&target, &source) == Ok(MoveOutcome::Succeeded) {
                            self.catalog.scan_skills();
                            return Err(error);
                        }
                    }
                    let _ = self.update_quarantine_status(&entry.id, "partial", None);
                    self.catalog.scan_skills();
                    return Err(OperationError::quarantine_partial());
                }
                self.catalog.scan_skills();
                Ok(OperationResult {
                    operation_id: confirmation.plan.id,
                    status: if outcome == MoveOutcome::Partial {
                        OperationResultStatus::Partial
                    } else {
                        OperationResultStatus::Succeeded
                    },
                    skill_id: entry.skill_id,
                    installed_hash: entry.content_hash,
                    entry_id: Some(entry.id),
                })
            }
            PendingManagedAction::Purge { entry, summary } => {
                if acknowledgement.map(str::trim) != Some(entry.display_name.as_str()) {
                    return Err(OperationError::acknowledgement_required());
                }
                let source = self.quarantine_entry_path(&entry)?;
                let active_target = self.target_root.join(&entry.original_relative_path);
                if source.is_symlink()
                    || path_exists(&active_target)
                    || self
                        .catalog
                        .managed_skill_id(&entry.original_relative_path)
                        .is_some()
                {
                    return Err(OperationError::quarantine_not_allowed());
                }
                let current = summarize_tree(&source)?;
                // Permanent deletion stays bound to the exact tree shown by the purge plan.
                if current.content_hash != summary.content_hash
                    || current.file_count != summary.file_count
                    || current.total_size_bytes != summary.total_size_bytes
                    || summary.content_hash != entry.content_hash
                {
                    return Err(OperationError::quarantine_content_changed());
                }
                self.update_quarantine_status(&entry.id, "purging", None)?;
                if fs::remove_dir_all(&source).is_err() {
                    let _ = self.update_quarantine_status(&entry.id, "quarantined", None);
                    return Err(OperationError::import_failed());
                }
                self.delete_quarantine_entry(&entry.id)?;
                Ok(OperationResult {
                    operation_id: confirmation.plan.id,
                    status: OperationResultStatus::Succeeded,
                    skill_id: entry.skill_id,
                    installed_hash: entry.content_hash,
                    entry_id: Some(entry.id),
                })
            }
            PendingManagedAction::KeepActive { entry } => {
                let source = self.quarantine_entry_path(&entry)?;
                self.verify_partial_entry(&entry)?;
                fs::remove_dir_all(&source).map_err(|_| OperationError::import_failed())?;
                self.update_quarantine_status(&entry.id, "restored", Some(now_ms()))?;
                self.catalog.scan_skills();
                Ok(OperationResult {
                    operation_id: confirmation.plan.id,
                    status: OperationResultStatus::Succeeded,
                    skill_id: entry.skill_id,
                    installed_hash: entry.content_hash,
                    entry_id: Some(entry.id),
                })
            }
            PendingManagedAction::CompleteQuarantine { entry } => {
                let target = self.active_entry_path(&entry)?;
                self.verify_partial_entry(&entry)?;
                fs::remove_dir_all(&target).map_err(|_| OperationError::import_failed())?;
                self.update_quarantine_status(&entry.id, "quarantined", None)?;
                self.catalog.scan_skills();
                Ok(OperationResult {
                    operation_id: confirmation.plan.id,
                    status: OperationResultStatus::Succeeded,
                    skill_id: entry.skill_id,
                    installed_hash: entry.content_hash,
                    entry_id: Some(entry.id),
                })
            }
        }
    }

    fn issue_managed_confirmation(
        &self,
        plan: OperationPlan,
        action: PendingManagedAction,
    ) -> Result<PlannedImport, OperationError> {
        let token = random_token().ok_or_else(OperationError::selection_unavailable)?;
        let expires_at_ms = now_ms().saturating_add(CONFIRMATION_TTL_MS);
        lock_unpoisoned(&self.managed_confirmations).insert(
            token.clone(),
            PendingManagedConfirmation {
                plan: plan.clone(),
                action,
                expires_at_ms,
                consumed: false,
            },
        );
        Ok(PlannedImport {
            plan,
            confirmation_token: Some(ConfirmationToken {
                token,
                expires_at_ms,
            }),
        })
    }

    fn consume_managed_confirmation(
        &self,
        confirmation_token: &str,
    ) -> Result<PendingManagedConfirmation, OperationError> {
        let now = now_ms();
        let mut confirmations = lock_unpoisoned(&self.managed_confirmations);
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
            if let Some(expired) = confirmations.remove(confirmation_token) {
                expired.source.cleanup();
            }
            return Err(OperationError::confirmation_token_expired());
        }
        if confirmation.consumed {
            return Err(OperationError::confirmation_token_replayed());
        }
        confirmation.consumed = true;
        Ok(confirmation.clone())
    }

    pub(crate) fn cancel_import(&self, confirmation_token: &str) -> Result<(), OperationError> {
        let confirmation = {
            let mut confirmations = lock_unpoisoned(&self.confirmations);
            let confirmation = confirmations
                .get(confirmation_token)
                .ok_or_else(OperationError::confirmation_token_invalid)?;
            if confirmation.consumed {
                return Err(OperationError::confirmation_token_replayed());
            }
            confirmations
                .remove(confirmation_token)
                .ok_or_else(OperationError::confirmation_token_invalid)?
        };
        // The token binds the internal staging path; callers never provide a filesystem path.
        confirmation.source.cleanup();
        Ok(())
    }

    fn is_managed_skill(&self, skill_id: &str) -> Result<bool, OperationError> {
        let path = self
            .database_path
            .as_deref()
            .ok_or_else(OperationError::database_unavailable)?;
        let connection =
            Connection::open(path).map_err(|_| OperationError::database_unavailable())?;
        connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM install_receipts WHERE skill_id = ?1 AND managed_by = 'codex-o')",
                params![skill_id],
                |row| row.get::<_, bool>(0),
            )
        .map_err(|_| OperationError::database_unavailable())
    }

    pub(crate) fn installed_market_sources(&self, repository_url: &str) -> HashSet<String> {
        let Some(path) = self.database_path.as_deref() else {
            return HashSet::new();
        };
        let Ok(connection) = Connection::open(path) else {
            return HashSet::new();
        };
        let Ok(mut statement) = connection.prepare(
            "SELECT skill_id, subdirectory FROM install_receipts WHERE source_type = 'market' AND source_url = ?1 AND managed_by = 'codex-o' AND subdirectory IS NOT NULL",
        ) else {
            return HashSet::new();
        };
        let Ok(rows) = statement.query_map([repository_url], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        }) else {
            return HashSet::new();
        };
        rows.filter_map(Result::ok)
            .filter(|(skill_id, _)| {
                self.catalog
                    .get_skill_detail(skill_id, false)
                    .is_ok_and(|detail| detail.summary.provider.id == "user_global")
            })
            .map(|(_, subdirectory)| subdirectory)
            .collect()
    }

    fn quarantine_root(&self) -> Result<&Path, OperationError> {
        self.quarantine_root
            .as_deref()
            .ok_or_else(OperationError::quarantine_unavailable)
    }

    fn quarantine_entry_path(&self, entry: &QuarantineEntry) -> Result<PathBuf, OperationError> {
        if entry.id != entry.operation_id || !valid_operation_id(&entry.operation_id) {
            return Err(OperationError::quarantine_not_allowed());
        }
        self.quarantine_operation_path(&entry.operation_id)
    }

    fn quarantine_operation_path(&self, operation_id: &str) -> Result<PathBuf, OperationError> {
        if !valid_operation_id(operation_id) {
            return Err(OperationError::quarantine_not_allowed());
        }
        let root = self.quarantine_root()?;
        let root_metadata = fs::symlink_metadata(root).ok();
        if root_metadata.is_some_and(|metadata| metadata.file_type().is_symlink()) {
            return Err(OperationError::quarantine_not_allowed());
        }
        let path = root.join(operation_id);
        if path.parent() != Some(root) {
            return Err(OperationError::quarantine_not_allowed());
        }
        Ok(path)
    }

    fn active_entry_path(&self, entry: &QuarantineEntry) -> Result<PathBuf, OperationError> {
        self.active_relative_path(&entry.original_relative_path)
    }

    fn active_relative_path(
        &self,
        original_relative_path: &str,
    ) -> Result<PathBuf, OperationError> {
        if !valid_relative_path(original_relative_path) {
            return Err(OperationError::quarantine_not_allowed());
        }
        let path = self.target_root.join(original_relative_path);
        if path.strip_prefix(&self.target_root).is_err() || path == self.target_root {
            return Err(OperationError::quarantine_not_allowed());
        }
        Ok(path)
    }

    fn verify_partial_entry(&self, entry: &QuarantineEntry) -> Result<TreeSummary, OperationError> {
        if entry.status != "partial" || !self.catalog.provider_can_restore(&entry.provider_id) {
            return Err(OperationError::quarantine_not_allowed());
        }
        let quarantine = summarize_tree(&self.quarantine_entry_path(entry)?)?;
        let active = summarize_tree(&self.active_entry_path(entry)?)?;
        if quarantine.content_hash != entry.content_hash
            || active.content_hash != entry.content_hash
            || quarantine.content_hash != active.content_hash
            || quarantine.file_count != active.file_count
            || quarantine.total_size_bytes != active.total_size_bytes
        {
            return Err(OperationError::quarantine_content_changed());
        }
        Ok(quarantine)
    }

    fn load_quarantine_entry(&self, entry_id: &str) -> Result<QuarantineEntry, OperationError> {
        self.converge_purging_entries()?;
        self.converge_partial_entries()?;
        let path = self
            .database_path
            .as_deref()
            .ok_or_else(OperationError::database_unavailable)?;
        let connection =
            Connection::open(path).map_err(|_| OperationError::database_unavailable())?;
        connection
            .query_row(
                "SELECT id, operation_id, skill_id, provider_id, original_relative_path, content_hash, display_name, file_count, total_size_bytes, status, quarantined_at, restored_at FROM quarantine_entries WHERE id = ?1",
                params![entry_id],
                |row| {
                    Ok(QuarantineEntry {
                        id: row.get(0)?,
                        operation_id: row.get(1)?,
                        skill_id: row.get(2)?,
                        provider_id: row.get(3)?,
                        original_relative_path: row.get(4)?,
                        content_hash: row.get(5)?,
                        display_name: row.get(6)?,
                        file_count: row.get::<_, i64>(7)? as usize,
                        total_size_bytes: row.get::<_, i64>(8)? as u64,
                        status: row.get(9)?,
                        quarantined_at: row.get::<_, i64>(10)? as u64,
                        restored_at: row.get::<_, Option<i64>>(11)?.map(|value| value as u64),
                    })
                },
            )
            .map_err(|_| OperationError::quarantine_entry_not_found())
    }

    fn list_quarantine_entries(&self) -> Result<Vec<QuarantineEntry>, OperationError> {
        self.converge_purging_entries()?;
        self.converge_quarantined_entries()?;
        self.converge_partial_entries()?;
        let path = self
            .database_path
            .as_deref()
            .ok_or_else(OperationError::database_unavailable)?;
        let connection =
            Connection::open(path).map_err(|_| OperationError::database_unavailable())?;
        let mut statement = connection
            .prepare("SELECT id, operation_id, skill_id, provider_id, original_relative_path, content_hash, display_name, file_count, total_size_bytes, status, quarantined_at, restored_at FROM quarantine_entries ORDER BY quarantined_at DESC")
            .map_err(|_| OperationError::database_unavailable())?;
        let entries = statement
            .query_map([], |row| {
                Ok(QuarantineEntry {
                    id: row.get(0)?,
                    operation_id: row.get(1)?,
                    skill_id: row.get(2)?,
                    provider_id: row.get(3)?,
                    original_relative_path: row.get(4)?,
                    content_hash: row.get(5)?,
                    display_name: row.get(6)?,
                    file_count: row.get::<_, i64>(7)? as usize,
                    total_size_bytes: row.get::<_, i64>(8)? as u64,
                    status: row.get(9)?,
                    quarantined_at: row.get::<_, i64>(10)? as u64,
                    restored_at: row.get::<_, Option<i64>>(11)?.map(|value| value as u64),
                })
            })
            .map_err(|_| OperationError::database_unavailable())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| OperationError::database_unavailable())?;
        Ok(entries)
    }

    fn converge_purging_entries(&self) -> Result<(), OperationError> {
        let path = self
            .database_path
            .as_deref()
            .ok_or_else(OperationError::database_unavailable)?;
        let connection =
            Connection::open(path).map_err(|_| OperationError::database_unavailable())?;
        let mut statement = connection
            .prepare(
                "SELECT id, operation_id, content_hash FROM quarantine_entries WHERE status = 'purging'",
            )
            .map_err(|_| OperationError::database_unavailable())?;
        let entries = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .map_err(|_| OperationError::database_unavailable())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| OperationError::database_unavailable())?;
        drop(statement);
        for (entry_id, operation_id, content_hash) in entries {
            let source = match self.quarantine_operation_path(&operation_id) {
                Ok(source) => source,
                Err(_) => continue,
            };
            match fs::symlink_metadata(&source) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    self.delete_quarantine_entry(&entry_id)?;
                }
                Ok(metadata)
                    if !metadata.file_type().is_symlink()
                        && metadata.is_dir()
                        && summarize_tree(&source)
                            .map(|summary| summary.content_hash == content_hash)
                            .unwrap_or(false) =>
                {
                    self.update_quarantine_status(&entry_id, "quarantined", None)?;
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn converge_quarantined_entries(&self) -> Result<(), OperationError> {
        let path = self
            .database_path
            .as_deref()
            .ok_or_else(OperationError::database_unavailable)?;
        let connection =
            Connection::open(path).map_err(|_| OperationError::database_unavailable())?;
        let mut statement = connection
            .prepare(
                "SELECT id, operation_id, original_relative_path, content_hash FROM quarantine_entries WHERE status = 'quarantined'",
            )
            .map_err(|_| OperationError::database_unavailable())?;
        let entries = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })
            .map_err(|_| OperationError::database_unavailable())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| OperationError::database_unavailable())?;
        drop(statement);
        for (entry_id, operation_id, relative_path, content_hash) in entries {
            let quarantine = match self.quarantine_operation_path(&operation_id) {
                Ok(path) => path,
                Err(_) => continue,
            };
            let active = match self.active_relative_path(&relative_path) {
                Ok(path) => path,
                Err(_) => continue,
            };
            let active_summary = summarize_tree(&active)
                .ok()
                .filter(|summary| summary.content_hash == content_hash);
            let quarantine_summary = summarize_tree(&quarantine)
                .ok()
                .filter(|summary| summary.content_hash == content_hash);
            if quarantine_summary.is_some() && active_summary.is_some() {
                self.update_quarantine_status(&entry_id, "partial", None)?;
            } else if fs::symlink_metadata(&quarantine)
                .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
                && active_summary.is_some()
            {
                self.update_quarantine_status(&entry_id, "restored", Some(now_ms()))?;
            }
        }
        Ok(())
    }

    fn converge_partial_entries(&self) -> Result<(), OperationError> {
        let path = self
            .database_path
            .as_deref()
            .ok_or_else(OperationError::database_unavailable)?;
        let connection =
            Connection::open(path).map_err(|_| OperationError::database_unavailable())?;
        let mut statement = connection
            .prepare(
                "SELECT id, operation_id, original_relative_path, content_hash FROM quarantine_entries WHERE status = 'partial'",
            )
            .map_err(|_| OperationError::database_unavailable())?;
        let entries = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })
            .map_err(|_| OperationError::database_unavailable())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| OperationError::database_unavailable())?;
        drop(statement);
        for (entry_id, operation_id, relative_path, content_hash) in entries {
            let quarantine = match self.quarantine_operation_path(&operation_id) {
                Ok(path) => path,
                Err(_) => continue,
            };
            let active = match self.active_relative_path(&relative_path) {
                Ok(path) => path,
                Err(_) => continue,
            };
            let quarantine_missing = fs::symlink_metadata(&quarantine)
                .map(|metadata| !metadata.is_dir() || metadata.file_type().is_symlink())
                .unwrap_or(true);
            let active_missing = fs::symlink_metadata(&active)
                .map(|metadata| !metadata.is_dir() || metadata.file_type().is_symlink())
                .unwrap_or(true);
            if quarantine_missing
                && !active_missing
                && summarize_tree(&active)
                    .map(|summary| summary.content_hash == content_hash)
                    .unwrap_or(false)
            {
                self.update_quarantine_status(&entry_id, "restored", Some(now_ms()))?;
            } else if active_missing
                && !quarantine_missing
                && summarize_tree(&quarantine)
                    .map(|summary| summary.content_hash == content_hash)
                    .unwrap_or(false)
            {
                self.update_quarantine_status(&entry_id, "quarantined", None)?;
            }
        }
        Ok(())
    }

    fn insert_quarantine_entry(
        &self,
        entry: &QuarantineEntry,
        plan: &OperationPlan,
    ) -> Result<(), OperationError> {
        let path = self
            .database_path
            .as_deref()
            .ok_or_else(OperationError::database_unavailable)?;
        let mut connection =
            Connection::open(path).map_err(|_| OperationError::database_unavailable())?;
        let plan_json = serde_json::to_string(plan).map_err(|_| OperationError::import_failed())?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| OperationError::database_unavailable())?;
        transaction.execute(
            "INSERT INTO quarantine_entries(id, operation_id, skill_id, provider_id, original_relative_path, content_hash, display_name, file_count, total_size_bytes, status, quarantined_at, restored_at) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'pending', ?10, NULL)",
            params![entry.id, entry.operation_id, entry.skill_id, entry.provider_id, entry.original_relative_path, entry.content_hash, entry.display_name, entry.file_count as i64, entry.total_size_bytes as i64, entry.quarantined_at as i64],
        ).map_err(|_| OperationError::database_unavailable())?;
        transaction.execute(
            "INSERT INTO management_operations(id, skill_id, operation, status, plan_json, result_json, created_at, completed_at) VALUES(?1, ?2, 'skill_quarantine', 'pending', ?3, NULL, ?4, NULL)",
            params![plan.id, entry.skill_id, plan_json, entry.quarantined_at as i64],
        ).map_err(|_| OperationError::database_unavailable())?;
        transaction
            .commit()
            .map_err(|_| OperationError::database_unavailable())
    }

    fn update_quarantine_status(
        &self,
        entry_id: &str,
        status: &str,
        restored_at: Option<u64>,
    ) -> Result<(), OperationError> {
        let path = self
            .database_path
            .as_deref()
            .ok_or_else(OperationError::database_unavailable)?;
        #[cfg(test)]
        {
            let mut remaining = lock_unpoisoned(&self.force_status_update_failures);
            if *remaining > 0 {
                *remaining -= 1;
                return Err(OperationError::database_unavailable());
            }
        }
        let mut connection =
            Connection::open(path).map_err(|_| OperationError::database_unavailable())?;
        let result_json =
            serde_json::to_string(status).map_err(|_| OperationError::import_failed())?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| OperationError::database_unavailable())?;
        let entries_updated = transaction.execute(
            "UPDATE quarantine_entries SET status = ?2, restored_at = COALESCE(?3, restored_at) WHERE id = ?1",
            params![entry_id, status, restored_at.map(|value| value as i64)],
        ).map_err(|_| OperationError::database_unavailable())?;
        if entries_updated != 1 {
            return Err(OperationError::quarantine_entry_not_found());
        }
        let operations_updated = transaction.execute(
            "UPDATE management_operations SET status = ?2, result_json = ?3, completed_at = ?4 WHERE id = ?1",
            params![entry_id, status, result_json, now_ms() as i64],
        ).map_err(|_| OperationError::database_unavailable())?;
        if operations_updated != 1 {
            return Err(OperationError::quarantine_entry_not_found());
        }
        transaction
            .commit()
            .map_err(|_| OperationError::database_unavailable())
    }

    fn delete_quarantine_entry(&self, entry_id: &str) -> Result<(), OperationError> {
        let path = self
            .database_path
            .as_deref()
            .ok_or_else(OperationError::database_unavailable)?;
        #[cfg(test)]
        {
            let mut remaining = lock_unpoisoned(&self.force_delete_entry_failures);
            if *remaining > 0 {
                *remaining -= 1;
                return Err(OperationError::database_unavailable());
            }
        }
        let mut connection =
            Connection::open(path).map_err(|_| OperationError::database_unavailable())?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| OperationError::database_unavailable())?;
        let entries_deleted = transaction
            .execute(
                "DELETE FROM quarantine_entries WHERE id = ?1",
                params![entry_id],
            )
            .map_err(|_| OperationError::database_unavailable())?;
        if entries_deleted != 1 {
            return Err(OperationError::quarantine_entry_not_found());
        }
        let operations_deleted = transaction
            .execute(
                "DELETE FROM management_operations WHERE id = ?1",
                params![entry_id],
            )
            .map_err(|_| OperationError::database_unavailable())?;
        if operations_deleted != 1 {
            return Err(OperationError::quarantine_entry_not_found());
        }
        transaction
            .commit()
            .map_err(|_| OperationError::database_unavailable())
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

    fn move_tree(&self, source: &Path, destination: &Path) -> Result<MoveOutcome, OperationError> {
        #[cfg(test)]
        let force_copy_fallback = *lock_unpoisoned(&self.force_copy_fallback);
        #[cfg(not(test))]
        let force_copy_fallback = false;
        #[cfg(test)]
        let force_remove_failure = *lock_unpoisoned(&self.force_remove_failure);
        #[cfg(not(test))]
        let force_remove_failure = false;
        #[cfg(test)]
        let force_copy_verification_failure =
            *lock_unpoisoned(&self.force_copy_verification_failure);
        #[cfg(not(test))]
        let force_copy_verification_failure = false;
        #[cfg(test)]
        let force_rename_verification_failure =
            *lock_unpoisoned(&self.force_rename_verification_failure);
        #[cfg(not(test))]
        let force_rename_verification_failure = false;
        #[cfg(test)]
        let force_move_failure = {
            let mut after = lock_unpoisoned(&self.force_move_failure_after);
            match after.as_mut() {
                Some(remaining) if *remaining == 0 => {
                    *after = None;
                    true
                }
                Some(remaining) => {
                    *remaining -= 1;
                    false
                }
                None => false,
            }
        };
        #[cfg(not(test))]
        let force_move_failure = false;
        move_tree(
            source,
            destination,
            force_copy_fallback,
            force_remove_failure,
            force_copy_verification_failure,
            force_rename_verification_failure,
            force_move_failure,
        )
    }

    fn persist_success(
        &self,
        plan: &OperationPlan,
        skill_id: &str,
        installed_hash: &str,
        provenance: Option<&OperationSource>,
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
        let (source_type, source_url, repo_ref, commit_sha, subdirectory) = match provenance {
            Some(source) => (
                source.source_type.as_str(),
                Some(source.repository_url.as_str()),
                Some(source.repo_ref.as_str()),
                Some(source.commit_sha.as_str()),
                Some(source.subdirectory.as_str()),
            ),
            None => ("local", None, None, None, None),
        };
        let now = now_ms() as i64;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| OperationError::database_unavailable())?;
        transaction
            .execute(
                "INSERT INTO install_receipts(skill_id, source_type, source_url, repo_ref, commit_sha, subdirectory, installed_hash, installed_at, managed_by) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![skill_id, source_type, source_url, repo_ref, commit_sha, subdirectory, installed_hash, now, "codex-o"],
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

    #[cfg(test)]
    fn expire_managed_confirmation(&self, token: &str) {
        if let Some(confirmation) = lock_unpoisoned(&self.managed_confirmations).get_mut(token) {
            confirmation.expires_at_ms = 0;
        }
    }

    #[cfg(test)]
    fn force_copy_fallback(&self) {
        *lock_unpoisoned(&self.force_copy_fallback) = true;
    }

    #[cfg(test)]
    fn force_remove_failure(&self) {
        *lock_unpoisoned(&self.force_remove_failure) = true;
    }

    #[cfg(test)]
    fn force_copy_verification_failure(&self) {
        *lock_unpoisoned(&self.force_copy_verification_failure) = true;
    }

    #[cfg(test)]
    fn force_rename_verification_failure(&self) {
        *lock_unpoisoned(&self.force_rename_verification_failure) = true;
    }

    #[cfg(test)]
    fn force_status_update_failures(&self, failures: usize) {
        *lock_unpoisoned(&self.force_status_update_failures) = failures;
    }

    #[cfg(test)]
    fn force_delete_entry_failures(&self, failures: usize) {
        *lock_unpoisoned(&self.force_delete_entry_failures) = failures;
    }

    #[cfg(test)]
    fn force_move_failure_after(&self, successful_moves: usize) {
        *lock_unpoisoned(&self.force_move_failure_after) = Some(successful_moves);
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
pub async fn plan_github_import(
    operations: State<'_, Arc<OperationsService>>,
    repository_url: String,
    repo_ref: String,
    subdirectory: String,
) -> Result<PlannedImport, OperationError> {
    operations
        .plan_github_import(&repository_url, &repo_ref, &subdirectory)
        .await
}

#[tauri::command]
pub fn execute_skill_import(
    operations: State<'_, Arc<OperationsService>>,
    confirmation_token: String,
) -> Result<OperationResult, OperationError> {
    operations.execute_import(&confirmation_token)
}

#[tauri::command]
pub fn cancel_skill_import(
    operations: State<'_, Arc<OperationsService>>,
    confirmation_token: String,
) -> Result<(), OperationError> {
    operations.cancel_import(&confirmation_token)
}

#[tauri::command]
pub fn plan_skill_quarantine(
    operations: State<'_, Arc<OperationsService>>,
    skill_id: String,
) -> Result<PlannedImport, OperationError> {
    operations.plan_quarantine(&skill_id)
}

#[tauri::command]
pub fn execute_skill_quarantine(
    operations: State<'_, Arc<OperationsService>>,
    confirmation_token: String,
    acknowledgement: Option<String>,
) -> Result<OperationResult, OperationError> {
    operations.execute_managed(&confirmation_token, acknowledgement.as_deref())
}

#[tauri::command]
pub fn list_quarantine_entries(
    operations: State<'_, Arc<OperationsService>>,
) -> Result<Vec<QuarantineEntry>, OperationError> {
    operations.list_quarantine_entries()
}

#[tauri::command]
pub fn plan_skill_restore(
    operations: State<'_, Arc<OperationsService>>,
    entry_id: String,
) -> Result<PlannedImport, OperationError> {
    operations.plan_restore(&entry_id)
}

#[tauri::command]
pub fn execute_skill_restore(
    operations: State<'_, Arc<OperationsService>>,
    confirmation_token: String,
) -> Result<OperationResult, OperationError> {
    operations.execute_managed(&confirmation_token, None)
}

#[tauri::command]
pub fn plan_quarantine_keep_active(
    operations: State<'_, Arc<OperationsService>>,
    entry_id: String,
) -> Result<PlannedImport, OperationError> {
    operations.plan_keep_active(&entry_id)
}

#[tauri::command]
pub fn execute_quarantine_keep_active(
    operations: State<'_, Arc<OperationsService>>,
    confirmation_token: String,
) -> Result<OperationResult, OperationError> {
    operations.execute_managed(&confirmation_token, None)
}

#[tauri::command]
pub fn plan_quarantine_complete(
    operations: State<'_, Arc<OperationsService>>,
    entry_id: String,
) -> Result<PlannedImport, OperationError> {
    operations.plan_complete_quarantine(&entry_id)
}

#[tauri::command]
pub fn execute_quarantine_complete(
    operations: State<'_, Arc<OperationsService>>,
    confirmation_token: String,
) -> Result<OperationResult, OperationError> {
    operations.execute_managed(&confirmation_token, None)
}

#[tauri::command]
pub fn plan_quarantine_purge(
    operations: State<'_, Arc<OperationsService>>,
    entry_id: String,
) -> Result<PlannedImport, OperationError> {
    operations.plan_purge(&entry_id)
}

#[tauri::command]
pub fn execute_quarantine_purge(
    operations: State<'_, Arc<OperationsService>>,
    confirmation_token: String,
    acknowledgement: String,
) -> Result<OperationResult, OperationError> {
    operations.execute_managed(&confirmation_token, Some(&acknowledgement))
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MoveOutcome {
    Succeeded,
    Partial,
}

fn move_tree(
    source: &Path,
    destination: &Path,
    force_copy_fallback: bool,
    force_remove_failure: bool,
    force_copy_verification_failure: bool,
    force_rename_verification_failure: bool,
    force_move_failure: bool,
) -> Result<MoveOutcome, OperationError> {
    if force_move_failure {
        return Err(OperationError::import_failed());
    }
    let source_summary = summarize_tree(source)?;
    let parent = destination
        .parent()
        .ok_or_else(OperationError::quarantine_unavailable)?;
    fs::create_dir_all(parent).map_err(|_| OperationError::quarantine_unavailable())?;
    if fs::symlink_metadata(destination).is_ok() {
        return Err(OperationError::conflict_detected());
    }
    if !force_copy_fallback && fs::rename(source, destination).is_ok() {
        let verified = summarize_tree(destination)
            .map(|destination_summary| {
                !force_rename_verification_failure
                    && destination_summary.content_hash == source_summary.content_hash
                    && destination_summary.file_count == source_summary.file_count
                    && destination_summary.total_size_bytes == source_summary.total_size_bytes
            })
            .unwrap_or(false);
        if verified {
            return Ok(MoveOutcome::Succeeded);
        }
        // A rename removes the source immediately; restore it before reporting verification failure.
        return if fs::rename(destination, source).is_ok() {
            Err(OperationError::import_failed())
        } else {
            Ok(MoveOutcome::Partial)
        };
    }
    if let Err(error) = copy_tree(source, destination) {
        let _ = fs::remove_dir_all(destination);
        return Err(error);
    }
    if force_copy_verification_failure {
        let path = destination.join(SKILL_MARKDOWN_FILE);
        let mut bytes = fs::read(&path).map_err(|_| OperationError::import_failed())?;
        let first = bytes
            .first_mut()
            .ok_or_else(OperationError::import_failed)?;
        *first ^= 1;
        fs::write(path, bytes).map_err(|_| OperationError::import_failed())?;
    }
    let copied_summary = summarize_tree(destination)?;
    if copied_summary.content_hash != source_summary.content_hash
        || copied_summary.file_count != source_summary.file_count
        || copied_summary.total_size_bytes != source_summary.total_size_bytes
    {
        let _ = fs::remove_dir_all(destination);
        return Err(OperationError::import_failed());
    }
    if force_remove_failure {
        return Ok(MoveOutcome::Partial);
    }
    match fs::remove_dir_all(source) {
        Ok(()) => Ok(MoveOutcome::Succeeded),
        Err(_) => Ok(MoveOutcome::Partial),
    }
}

fn summarize_tree(root: &Path) -> Result<TreeSummary, OperationError> {
    let metadata =
        fs::symlink_metadata(root).map_err(|_| OperationError::quarantine_entry_not_found())?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(OperationError::quarantine_not_allowed());
    }
    let mut files = Vec::new();
    collect_tree_files(root, Path::new(""), &mut files)?;
    if !files
        .iter()
        .any(|(relative, _)| relative == Path::new(SKILL_MARKDOWN_FILE))
    {
        return Err(OperationError::import_source_invalid());
    }
    files.sort_by(|left, right| left.0.cmp(&right.0));
    let mut digest = Sha256::new();
    let mut total_size_bytes = 0_u64;
    let mut relative_files = Vec::with_capacity(files.len());
    for (relative, path) in files {
        let relative_text = relative
            .to_str()
            .filter(|value| valid_relative_path(value))
            .ok_or_else(OperationError::import_source_invalid)?;
        let metadata = fs::metadata(&path).map_err(|_| OperationError::import_source_invalid())?;
        total_size_bytes = total_size_bytes.saturating_add(metadata.len());
        digest.update((relative_text.len() as u64).to_be_bytes());
        digest.update(relative_text.as_bytes());
        hash_file(&path, &mut digest)?;
        relative_files.push(relative_text.to_owned());
    }
    Ok(TreeSummary {
        content_hash: format!("{:x}", digest.finalize()),
        file_count: relative_files.len(),
        total_size_bytes,
        relative_files,
    })
}

fn collect_tree_files(
    root: &Path,
    relative_directory: &Path,
    files: &mut Vec<(PathBuf, PathBuf)>,
) -> Result<(), OperationError> {
    for entry in fs::read_dir(root.join(relative_directory))
        .map_err(|_| OperationError::import_source_invalid())?
    {
        let entry = entry.map_err(|_| OperationError::import_source_invalid())?;
        let relative = relative_directory.join(entry.file_name());
        let metadata = fs::symlink_metadata(entry.path())
            .map_err(|_| OperationError::import_source_invalid())?;
        if metadata.file_type().is_symlink() {
            return Err(OperationError::quarantine_not_allowed());
        }
        if metadata.is_dir() {
            collect_tree_files(root, &relative, files)?;
        } else if metadata.is_file() {
            files.push((relative, entry.path()));
        } else {
            return Err(OperationError::import_source_invalid());
        }
    }
    Ok(())
}

fn copy_tree(source: &Path, destination: &Path) -> Result<(), OperationError> {
    let summary = summarize_tree(source)?;
    for relative in summary.relative_files {
        let from = source.join(&relative);
        let to = destination.join(&relative);
        let parent = to.parent().ok_or_else(OperationError::import_failed)?;
        fs::create_dir_all(parent).map_err(|_| OperationError::import_failed())?;
        fs::copy(from, to).map_err(|_| OperationError::import_failed())?;
    }
    Ok(())
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

fn valid_operation_id(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
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
