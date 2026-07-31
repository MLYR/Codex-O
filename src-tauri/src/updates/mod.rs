//! Manual update checks and fixed-commit update planning for managed User Skills.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
    time::{SystemTime, UNIX_EPOCH},
};

use serde::Serialize;
use tauri::State;

#[cfg(test)]
mod tests;

use crate::{
    catalog::SkillCatalog,
    market::{latest_market_selection, MarketEndpoints},
    operations::{
        copy_source_to_staging,
        github::{download_github_skill, GithubEndpoints},
        inspect_source, ImportSourceKind, InstallReceiptRecord, OperationError, OperationResult,
        OperationsService, PlannedImport, PreparedSkillUpdate,
    },
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillUpdateStatus {
    Current,
    Available,
    Conflict,
    Unavailable,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SkillUpdateSummary {
    pub skill_id: String,
    pub display_name: String,
    pub source_type: String,
    pub status: SkillUpdateStatus,
    pub installed_commit: Option<String>,
    pub available_commit: Option<String>,
    pub checked_at_ms: u64,
    pub reason: String,
    pub changed_files: Vec<String>,
}

#[derive(Clone)]
struct CheckedUpdate {
    receipt: InstallReceiptRecord,
    skill_id: String,
    target_name: String,
    current_hash: String,
    remote_hash: String,
    provenance: crate::operations::OperationSource,
    changed_files: Vec<String>,
    file_count: usize,
    total_size_bytes: u64,
}

pub struct UpdateService {
    operations: Arc<OperationsService>,
    catalog: SkillCatalog,
    github_endpoints: GithubEndpoints,
    market_endpoints: MarketEndpoints,
    checked: Mutex<HashMap<String, CheckedUpdate>>,
}

impl UpdateService {
    pub fn new(operations: Arc<OperationsService>, catalog: SkillCatalog) -> Self {
        Self {
            operations,
            catalog,
            github_endpoints: GithubEndpoints::production(),
            market_endpoints: MarketEndpoints::production(),
            checked: Mutex::new(HashMap::new()),
        }
    }

    #[cfg(test)]
    fn with_endpoints(
        operations: Arc<OperationsService>,
        catalog: SkillCatalog,
        github_endpoints: GithubEndpoints,
        market_endpoints: MarketEndpoints,
    ) -> Self {
        Self {
            operations,
            catalog,
            github_endpoints,
            market_endpoints,
            checked: Mutex::new(HashMap::new()),
        }
    }

    pub async fn check_updates(&self) -> Result<Vec<SkillUpdateSummary>, OperationError> {
        let receipts = self.operations.list_install_receipts()?;
        let mut summaries = Vec::with_capacity(receipts.len());
        let mut available = HashMap::new();
        for receipt in receipts {
            let (summary, checked) = self.check_receipt(receipt).await;
            if let Some(checked) = checked {
                available.insert(checked.skill_id.clone(), checked);
            }
            summaries.push(summary);
        }
        summaries.sort_by(|left, right| left.display_name.cmp(&right.display_name));
        *lock_unpoisoned(&self.checked) = available;
        Ok(summaries)
    }

    async fn check_receipt(
        &self,
        receipt: InstallReceiptRecord,
    ) -> (SkillUpdateSummary, Option<CheckedUpdate>) {
        let checked_at_ms = now_ms();
        let installed_commit = receipt.commit_sha.as_deref().map(short_commit);
        let source_type = receipt.source_type.clone();
        let unavailable = |display_name: String, reason: &str| {
            (
                SkillUpdateSummary {
                    skill_id: receipt.skill_id.clone(),
                    display_name,
                    source_type: source_type.clone(),
                    status: SkillUpdateStatus::Unavailable,
                    installed_commit: installed_commit.clone(),
                    available_commit: None,
                    checked_at_ms,
                    reason: reason.to_owned(),
                    changed_files: Vec::new(),
                },
                None,
            )
        };

        let candidate = match self.catalog.quarantine_candidate(&receipt.skill_id) {
            Ok(candidate) => candidate,
            Err(_) => return unavailable("已移除的 Skill".to_owned(), "本地 Skill 已不存在。"),
        };
        let display_name = candidate.display_name.clone();
        if !candidate.can_quarantine || candidate.provider_id != "user_global" {
            return unavailable(display_name, "此 Provider 为只读，不能更新。");
        }
        if receipt.managed_by != "codex-o"
            || !matches!(receipt.source_type.as_str(), "github" | "market")
            || receipt.source_url.as_deref().is_none_or(str::is_empty)
            || receipt.repo_ref.as_deref().is_none_or(str::is_empty)
            || receipt.commit_sha.as_deref().is_none_or(str::is_empty)
            || receipt.subdirectory.is_none()
            || receipt.installed_hash.is_empty()
        {
            return unavailable(display_name, "安装凭据不完整，无法确认更新来源。");
        }
        let current_hash = match self.catalog.current_content_hash(&receipt.skill_id) {
            Ok(hash) => hash,
            Err(_) => return unavailable(display_name, "无法验证本地 Skill 内容。"),
        };
        if current_hash != receipt.installed_hash {
            return (
                SkillUpdateSummary {
                    skill_id: receipt.skill_id.clone(),
                    display_name,
                    source_type,
                    status: SkillUpdateStatus::Conflict,
                    installed_commit,
                    available_commit: None,
                    checked_at_ms,
                    reason: "检测到本地修改，已保留且不会覆盖。".to_owned(),
                    changed_files: Vec::new(),
                },
                None,
            );
        }

        let (operation_id, operation_root) = match self.operations.create_update_operation_root() {
            Ok(value) => value,
            Err(_) => return unavailable(display_name, "更新暂存区不可用。"),
        };
        let result = self
            .download_and_prepare(&receipt, &candidate.relative_path, &operation_root, None)
            .await;
        let prepared = match result {
            Ok(prepared) => prepared,
            Err(error) => {
                let _ = fs::remove_dir_all(&operation_root);
                return unavailable(display_name, update_reason(&error));
            }
        };
        let changed_files = match changed_files(&candidate.directory, &prepared.staged_skill) {
            Ok(files) => files,
            Err(_) => {
                let _ = fs::remove_dir_all(&operation_root);
                return unavailable(display_name, "无法生成安全的变更摘要。");
            }
        };
        // Checks never retain remote content; planning downloads the fixed commit again.
        let _ = fs::remove_dir_all(&operation_root);
        let status = if prepared.remote_hash == current_hash {
            SkillUpdateStatus::Current
        } else {
            SkillUpdateStatus::Available
        };
        let summary = SkillUpdateSummary {
            skill_id: receipt.skill_id.clone(),
            display_name,
            source_type,
            status,
            installed_commit,
            available_commit: Some(short_commit(&prepared.provenance.commit_sha)),
            checked_at_ms,
            reason: if status == SkillUpdateStatus::Current {
                "已是来源中的最新内容。".to_owned()
            } else {
                "发现可安全预览的新版本。".to_owned()
            },
            changed_files: changed_files.clone(),
        };
        let checked = (status == SkillUpdateStatus::Available).then_some(CheckedUpdate {
            receipt,
            skill_id: summary.skill_id.clone(),
            target_name: candidate.relative_path,
            current_hash,
            remote_hash: prepared.remote_hash,
            provenance: prepared.provenance,
            changed_files,
            file_count: prepared.file_count,
            total_size_bytes: prepared.total_size_bytes,
        });
        let _ = operation_id;
        (summary, checked)
    }

    pub async fn plan_update(&self, skill_id: &str) -> Result<PlannedImport, OperationError> {
        let checked = lock_unpoisoned(&self.checked)
            .get(skill_id)
            .cloned()
            .ok_or_else(OperationError::update_unavailable)?;
        let receipt = self
            .operations
            .list_install_receipts()?
            .into_iter()
            .find(|receipt| receipt.skill_id == skill_id)
            .ok_or_else(OperationError::update_receipt_changed)?;
        if receipt != checked.receipt
            || self.catalog.current_content_hash(skill_id).ok().as_deref()
                != Some(checked.current_hash.as_str())
        {
            return Err(OperationError::update_receipt_changed());
        }

        let (operation_id, operation_root) = self.operations.create_update_operation_root()?;
        let prepared = self
            .download_and_prepare(
                &receipt,
                &checked.target_name,
                &operation_root,
                Some(&checked.provenance.commit_sha),
            )
            .await;
        let downloaded = match prepared {
            Ok(value) => value,
            Err(error) => {
                let _ = fs::remove_dir_all(&operation_root);
                return Err(error);
            }
        };
        let current = match self.catalog.quarantine_candidate(skill_id) {
            Ok(current) => current,
            Err(_) => {
                let _ = fs::remove_dir_all(&operation_root);
                return Err(OperationError::update_unavailable());
            }
        };
        let actual_changes = match changed_files(&current.directory, &downloaded.staged_skill) {
            Ok(changes) => changes,
            Err(error) => {
                let _ = fs::remove_dir_all(&operation_root);
                return Err(error);
            }
        };
        if downloaded.remote_hash != checked.remote_hash
            || downloaded.provenance != checked.provenance
            || actual_changes != checked.changed_files
        {
            let _ = fs::remove_dir_all(&operation_root);
            return Err(OperationError::update_receipt_changed());
        }
        let prepared = PreparedSkillUpdate {
            operation_id,
            operation_root,
            staging_home: downloaded.staging_home,
            staged_skill: downloaded.staged_skill,
            skill_id: checked.skill_id,
            target_name: checked.target_name,
            installed_hash: checked.receipt.installed_hash.clone(),
            current_hash: checked.current_hash,
            remote_hash: downloaded.remote_hash,
            receipt: checked.receipt,
            provenance: downloaded.provenance,
            relative_files: checked.changed_files,
            file_count: checked.file_count,
            total_size_bytes: checked.total_size_bytes,
        };
        let cleanup_root = prepared.operation_root.clone();
        let plan = self.operations.plan_update(prepared);
        if plan.is_err() {
            let _ = fs::remove_dir_all(cleanup_root);
        }
        plan
    }

    pub fn execute_update(
        &self,
        confirmation_token: &str,
    ) -> Result<OperationResult, OperationError> {
        let result = self.operations.execute_update(confirmation_token);
        if let Ok(result) = &result {
            lock_unpoisoned(&self.checked).remove(&result.skill_id);
        }
        result
    }

    async fn download_and_prepare(
        &self,
        receipt: &InstallReceiptRecord,
        target_name: &str,
        operation_root: &Path,
        fixed_commit: Option<&str>,
    ) -> Result<DownloadedUpdate, OperationError> {
        let source_url = receipt
            .source_url
            .as_deref()
            .ok_or_else(OperationError::update_unavailable)?;
        let repo_ref = receipt
            .repo_ref
            .as_deref()
            .ok_or_else(OperationError::update_unavailable)?;
        let subdirectory = receipt
            .subdirectory
            .as_deref()
            .ok_or_else(OperationError::update_unavailable)?;
        let (repository_url, commit) = if receipt.source_type == "market" {
            let selection = latest_market_selection(subdirectory, &self.market_endpoints).await?;
            if selection.repository_url != source_url || selection.subdirectory != subdirectory {
                return Err(OperationError::update_receipt_changed());
            }
            (selection.repository_url, Some(selection.commit_sha))
        } else {
            (source_url.to_owned(), None)
        };
        let commit = fixed_commit.or(commit.as_deref());
        let (selected, mut provenance) = download_github_skill(
            &repository_url,
            repo_ref,
            subdirectory,
            commit,
            operation_root,
            &self.github_endpoints,
        )
        .await?;
        provenance.source_type = receipt.source_type.clone();
        let selected_summary = inspect_source(ImportSourceKind::Directory, &selected)?;
        if selected_summary.target_name != target_name {
            return Err(OperationError::update_receipt_changed());
        }
        let staging_home = operation_root.join("home");
        let staged_skill = staging_home
            .join(".agents")
            .join("skills")
            .join(target_name);
        copy_source_to_staging(&selected_summary, &staged_skill)?;
        let staged = inspect_source(ImportSourceKind::Directory, &staged_skill)?;
        if staged.source_hash != selected_summary.source_hash {
            return Err(OperationError::update_failed());
        }
        let facts = self
            .catalog
            .validate_import_staging(staging_home.clone(), target_name)
            .map_err(|_| OperationError::update_failed())?;
        Ok(DownloadedUpdate {
            staging_home,
            staged_skill,
            // Catalog hashes represent parsed Skill content; tree hashes only verify byte-for-byte copies.
            remote_hash: facts.content_hash,
            provenance,
            file_count: staged.file_count,
            total_size_bytes: staged.total_size_bytes,
        })
    }
}

struct DownloadedUpdate {
    staging_home: PathBuf,
    staged_skill: PathBuf,
    remote_hash: String,
    provenance: crate::operations::OperationSource,
    file_count: usize,
    total_size_bytes: u64,
}

fn changed_files(current: &Path, remote: &Path) -> Result<Vec<String>, OperationError> {
    let current = file_map(current)?;
    let remote = file_map(remote)?;
    let paths = current
        .keys()
        .chain(remote.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    let mut changed = Vec::new();
    for path in paths {
        let differs = match (current.get(&path), remote.get(&path)) {
            (Some(left), Some(right)) => {
                fs::read(left).map_err(|_| OperationError::update_failed())?
                    != fs::read(right).map_err(|_| OperationError::update_failed())?
            }
            _ => true,
        };
        if differs {
            changed.push(path);
        }
    }
    Ok(changed)
}

fn file_map(root: &Path) -> Result<BTreeMap<String, PathBuf>, OperationError> {
    let summary = inspect_source(ImportSourceKind::Directory, root)?;
    summary
        .files
        .into_iter()
        .map(|(relative, path)| {
            relative
                .to_str()
                .map(|relative| (relative.to_owned(), path))
                .ok_or_else(OperationError::update_failed)
        })
        .collect()
}

fn short_commit(commit: &str) -> String {
    commit.chars().take(8).collect()
}

fn update_reason(error: &OperationError) -> &'static str {
    match error.code {
        "github_rate_limited" => "GitHub 已限流，请稍后手动重试。",
        "github_offline" | "github_timeout" => "当前无法连接更新来源。",
        "market_item_unavailable" | "market_protocol_error" => "市场来源暂时不可用。",
        _ => "无法安全验证远端更新。",
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn lock_unpoisoned<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[tauri::command]
pub async fn check_skill_updates(
    updates: State<'_, Arc<UpdateService>>,
) -> Result<Vec<SkillUpdateSummary>, OperationError> {
    updates.check_updates().await
}

#[tauri::command]
pub async fn plan_skill_update(
    updates: State<'_, Arc<UpdateService>>,
    skill_id: String,
) -> Result<PlannedImport, OperationError> {
    updates.plan_update(&skill_id).await
}

#[tauri::command]
pub fn execute_skill_update(
    updates: State<'_, Arc<UpdateService>>,
    confirmation_token: String,
) -> Result<OperationResult, OperationError> {
    updates.execute_update(&confirmation_token)
}
