//! Official market discovery, cache, and fixed-commit selective downloads.

use std::{
    collections::HashSet,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    process,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex, MutexGuard,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use reqwest::{Client, Response, StatusCode, Url};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tauri::State;

use crate::operations::{OperationError, OperationsService, PlannedImport};

const CACHE_SCHEMA_VERSION: u32 = 1;
const OFFICIAL_PROVIDER_NAME: &str = "openai-curated";
const OFFICIAL_REPOSITORY_URL: &str = "https://github.com/openai/plugins";
const OFFICIAL_OWNER: &str = "openai";
const OFFICIAL_REPOSITORY: &str = "plugins";
const MARKETPLACE_LIMIT: usize = 2 * 1024 * 1024;
const TREE_RESPONSE_LIMIT: usize = 8 * 1024 * 1024;
const CACHE_RESPONSE_LIMIT: usize = 4 * 1024 * 1024;
const TREE_ENTRY_LIMIT: usize = 20_000;
const MARKET_ITEM_LIMIT: usize = 1_000;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MarketStatus {
    Ready,
    Stale,
    Unavailable,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct MarketIssue {
    pub code: &'static str,
    pub message: &'static str,
    pub recovery: &'static str,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct MarketItem {
    pub id: String,
    pub plugin_name: String,
    pub skill_name: String,
    pub category: Option<String>,
    pub description: Option<String>,
    pub installed: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct MarketCatalog {
    pub status: MarketStatus,
    pub provider_name: Option<String>,
    pub commit_sha: Option<String>,
    pub synced_at_ms: Option<u64>,
    pub items: Vec<MarketItem>,
    pub issue: Option<MarketIssue>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
struct CachedMarketItem {
    id: String,
    plugin_name: String,
    skill_name: String,
    category: Option<String>,
    description: Option<String>,
    subdirectory: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
struct CachedMarketSnapshot {
    schema_version: u32,
    provider_name: String,
    repository_url: String,
    commit_sha: String,
    synced_at_ms: u64,
    items: Vec<CachedMarketItem>,
}

#[derive(Clone, Debug)]
pub(crate) struct MarketSelection {
    pub repository_url: String,
    pub commit_sha: String,
    pub subdirectory: String,
}

#[derive(Clone)]
pub(crate) struct MarketEndpoints {
    api_base: Url,
    raw_base: Url,
    test_origin: Option<String>,
}

impl MarketEndpoints {
    pub(crate) fn production() -> Self {
        Self {
            api_base: Url::parse("https://api.github.com/").expect("static GitHub API URL"),
            raw_base: Url::parse("https://raw.githubusercontent.com/")
                .expect("static GitHub raw URL"),
            test_origin: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn loopback(origin: &str) -> Self {
        let mut base = Url::parse(origin).expect("valid loopback fixture origin");
        base.set_path("/");
        base.set_query(None);
        base.set_fragment(None);
        Self {
            test_origin: Some(origin_of(&base)),
            api_base: base.clone(),
            raw_base: base,
        }
    }

    fn allows(&self, url: &Url) -> bool {
        if self
            .test_origin
            .as_ref()
            .is_some_and(|origin| origin == &origin_of(url))
        {
            return url.host_str().is_some_and(is_loopback_host);
        }
        url.scheme() == "https"
            && url.username().is_empty()
            && url.password().is_none()
            && url.port().is_none()
            && matches!(
                url.host_str(),
                Some("api.github.com" | "raw.githubusercontent.com")
            )
    }
}

pub struct MarketService {
    cache_path: Option<PathBuf>,
    endpoints: MarketEndpoints,
    operations: Arc<OperationsService>,
    snapshot: Mutex<Option<CachedMarketSnapshot>>,
}

impl MarketService {
    pub fn new(cache_path: Option<PathBuf>, operations: Arc<OperationsService>) -> Self {
        let snapshot = cache_path.as_deref().and_then(load_cache);
        Self {
            cache_path,
            endpoints: MarketEndpoints::production(),
            operations,
            snapshot: Mutex::new(snapshot),
        }
    }

    #[cfg(test)]
    pub(crate) fn with_endpoints(
        cache_path: Option<PathBuf>,
        operations: Arc<OperationsService>,
        endpoints: MarketEndpoints,
    ) -> Self {
        let snapshot = cache_path.as_deref().and_then(load_cache);
        Self {
            cache_path,
            endpoints,
            operations,
            snapshot: Mutex::new(snapshot),
        }
    }

    pub fn catalog(&self) -> MarketCatalog {
        match lock_unpoisoned(&self.snapshot).clone() {
            Some(snapshot) => self.catalog_from_snapshot(snapshot, MarketStatus::Ready, None),
            None => unavailable_catalog(MarketFailure::CacheMissing.issue()),
        }
    }

    pub async fn refresh(&self) -> MarketCatalog {
        let fetched = fetch_snapshot(&self.endpoints).await.and_then(|snapshot| {
            let path = self
                .cache_path
                .as_deref()
                .ok_or(MarketFailure::StorageUnavailable)?;
            write_cache_atomic(path, &snapshot)?;
            Ok(snapshot)
        });
        match fetched {
            Ok(snapshot) => {
                *lock_unpoisoned(&self.snapshot) = Some(snapshot.clone());
                self.catalog_from_snapshot(snapshot, MarketStatus::Ready, None)
            }
            Err(error) => match lock_unpoisoned(&self.snapshot).clone() {
                Some(snapshot) => {
                    self.catalog_from_snapshot(snapshot, MarketStatus::Stale, Some(error.issue()))
                }
                None => unavailable_catalog(error.issue()),
            },
        }
    }

    pub async fn plan_import(&self, market_item_id: &str) -> Result<PlannedImport, OperationError> {
        let selected = lock_unpoisoned(&self.snapshot)
            .as_ref()
            .and_then(|snapshot| {
                snapshot
                    .items
                    .iter()
                    .find(|item| item.id == market_item_id)
                    .map(|item| MarketSelection {
                        repository_url: snapshot.repository_url.clone(),
                        commit_sha: snapshot.commit_sha.clone(),
                        subdirectory: item.subdirectory.clone(),
                    })
            })
            .ok_or_else(OperationError::market_item_unavailable)?;
        self.operations
            .plan_market_import(&selected, &self.endpoints)
            .await
    }

    fn catalog_from_snapshot(
        &self,
        snapshot: CachedMarketSnapshot,
        status: MarketStatus,
        issue: Option<MarketIssue>,
    ) -> MarketCatalog {
        let installed = self
            .operations
            .installed_market_sources(&snapshot.repository_url);
        MarketCatalog {
            status,
            provider_name: Some(snapshot.provider_name),
            commit_sha: Some(snapshot.commit_sha),
            synced_at_ms: Some(snapshot.synced_at_ms),
            items: snapshot
                .items
                .into_iter()
                .map(|item| MarketItem {
                    installed: installed.contains(&item.subdirectory),
                    id: item.id,
                    plugin_name: item.plugin_name,
                    skill_name: item.skill_name,
                    category: item.category,
                    description: item.description,
                })
                .collect(),
            issue,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MarketFailure {
    CacheMissing,
    StorageUnavailable,
    Offline,
    Timeout,
    RateLimited,
    NotFound,
    ResponseTooLarge,
    Protocol,
    InvalidIndex,
    TruncatedTree,
}

impl MarketFailure {
    fn issue(self) -> MarketIssue {
        match self {
            Self::CacheMissing => MarketIssue {
                code: "market_cache_missing",
                message: "No official market snapshot is available.",
                recovery: "Connect to the network and refresh the market.",
            },
            Self::StorageUnavailable => MarketIssue {
                code: "market_storage_unavailable",
                message: "The market cache cannot be stored safely.",
                recovery: "Restore app-local storage and refresh the market.",
            },
            Self::Offline => MarketIssue {
                code: "market_offline",
                message: "The official market could not be reached.",
                recovery: "Check the network connection or use local/GitHub install.",
            },
            Self::Timeout => MarketIssue {
                code: "market_timeout",
                message: "The official market request timed out.",
                recovery: "Try refreshing later or use local/GitHub install.",
            },
            Self::RateLimited => MarketIssue {
                code: "market_rate_limited",
                message: "GitHub temporarily limited the market request.",
                recovery: "Wait before refreshing the market again.",
            },
            Self::NotFound => MarketIssue {
                code: "market_not_found",
                message: "The official market source is unavailable.",
                recovery: "Try refreshing later or use local/GitHub install.",
            },
            Self::ResponseTooLarge => MarketIssue {
                code: "market_response_too_large",
                message: "The market response exceeds the safe limit.",
                recovery: "Keep the previous snapshot and try again later.",
            },
            Self::Protocol => MarketIssue {
                code: "market_protocol_error",
                message: "The market returned an unsupported response.",
                recovery: "Keep the previous snapshot and try again later.",
            },
            Self::InvalidIndex => MarketIssue {
                code: "market_index_invalid",
                message: "The market index failed safety validation.",
                recovery: "Keep the previous snapshot and try again after the source is fixed.",
            },
            Self::TruncatedTree => MarketIssue {
                code: "market_tree_truncated",
                message: "GitHub returned an incomplete repository tree.",
                recovery: "Keep the previous snapshot and try again later.",
            },
        }
    }
}

#[derive(Deserialize)]
struct CommitResponse {
    sha: String,
}

#[derive(Deserialize)]
struct MarketplaceDocument {
    name: String,
    plugins: Vec<serde_json::Value>,
}

#[derive(Deserialize)]
struct MarketplacePlugin {
    name: String,
    source: String,
    category: Option<String>,
    description: Option<String>,
    policy: MarketplacePolicy,
}

#[derive(Deserialize)]
struct MarketplacePolicy {
    installation: String,
    products: Option<Vec<String>>,
}

#[derive(Clone, Deserialize)]
struct TreeEntry {
    path: String,
    mode: String,
    #[serde(rename = "type")]
    kind: String,
    size: Option<u64>,
}

#[derive(Deserialize)]
struct TreeResponse {
    sha: String,
    truncated: bool,
    tree: Vec<TreeEntry>,
}

async fn fetch_snapshot(
    endpoints: &MarketEndpoints,
) -> Result<CachedMarketSnapshot, MarketFailure> {
    let client = market_client()?;
    let commit_url = endpoint_url(
        &endpoints.api_base,
        &[
            "repos",
            OFFICIAL_OWNER,
            OFFICIAL_REPOSITORY,
            "commits",
            "main",
        ],
    )?;
    let commit: CommitResponse =
        fetch_json(&client, commit_url, endpoints, MARKETPLACE_LIMIT).await?;
    let commit_sha = commit.sha.to_ascii_lowercase();
    if !valid_commit_sha(&commit_sha) {
        return Err(MarketFailure::Protocol);
    }

    let marketplace_url = endpoint_url(
        &endpoints.raw_base,
        &[
            OFFICIAL_OWNER,
            OFFICIAL_REPOSITORY,
            &commit_sha,
            ".agents",
            "plugins",
            "marketplace.json",
        ],
    )?;
    let document: MarketplaceDocument =
        fetch_json(&client, marketplace_url, endpoints, MARKETPLACE_LIMIT).await?;
    let tree = fetch_tree(&client, endpoints, &commit_sha).await?;
    build_snapshot(document, tree, commit_sha, now_ms())
}

pub(crate) async fn latest_market_selection(
    subdirectory: &str,
    endpoints: &MarketEndpoints,
) -> Result<MarketSelection, OperationError> {
    if !valid_market_subdirectory(subdirectory) {
        return Err(OperationError::market_item_unavailable());
    }
    let snapshot = fetch_snapshot(endpoints)
        .await
        .map_err(market_failure_to_operation)?;
    snapshot
        .items
        .iter()
        .find(|item| item.subdirectory == subdirectory)
        .map(|item| MarketSelection {
            repository_url: snapshot.repository_url.clone(),
            commit_sha: snapshot.commit_sha.clone(),
            subdirectory: item.subdirectory.clone(),
        })
        .ok_or_else(OperationError::market_item_unavailable)
}

async fn fetch_tree(
    client: &Client,
    endpoints: &MarketEndpoints,
    commit_sha: &str,
) -> Result<TreeResponse, MarketFailure> {
    let mut tree_url = endpoint_url(
        &endpoints.api_base,
        &[
            "repos",
            OFFICIAL_OWNER,
            OFFICIAL_REPOSITORY,
            "git",
            "trees",
            commit_sha,
        ],
    )?;
    tree_url.set_query(Some("recursive=1"));
    let tree: TreeResponse = fetch_json(client, tree_url, endpoints, TREE_RESPONSE_LIMIT).await?;
    if tree.truncated {
        return Err(MarketFailure::TruncatedTree);
    }
    if tree.tree.len() > TREE_ENTRY_LIMIT || tree.sha.to_ascii_lowercase() != commit_sha {
        return Err(MarketFailure::InvalidIndex);
    }
    Ok(tree)
}

fn build_snapshot(
    document: MarketplaceDocument,
    tree: TreeResponse,
    commit_sha: String,
    synced_at_ms: u64,
) -> Result<CachedMarketSnapshot, MarketFailure> {
    if document.name != OFFICIAL_PROVIDER_NAME
        || tree.truncated
        || tree.tree.len() > TREE_ENTRY_LIMIT
        || tree.sha.to_ascii_lowercase() != commit_sha
    {
        return Err(MarketFailure::InvalidIndex);
    }
    for entry in &tree.tree {
        if !valid_repository_path(&entry.path) {
            return Err(MarketFailure::InvalidIndex);
        }
    }

    let mut items = Vec::new();
    let mut ids = HashSet::new();
    for value in document.plugins {
        let Ok(plugin) = serde_json::from_value::<MarketplacePlugin>(value) else {
            continue;
        };
        let Some(plugin_root) = validate_plugin(&plugin) else {
            continue;
        };
        let skill_prefix = format!("{plugin_root}/skills/");
        for entry in tree.tree.iter().filter(|entry| {
            entry.kind == "blob"
                && entry.mode == "100644"
                && entry.path.starts_with(&skill_prefix)
                && entry.path.ends_with("/SKILL.md")
        }) {
            let Some(skill_name) = market_skill_name(&entry.path, &skill_prefix) else {
                continue;
            };
            let subdirectory = format!("{plugin_root}/skills/{skill_name}");
            if tree.tree.iter().any(|candidate| {
                candidate.path.starts_with(&format!("{subdirectory}/"))
                    && candidate.mode == "120000"
            }) {
                continue;
            }
            let id = stable_market_id(&plugin.name, &subdirectory);
            if !ids.insert(id.clone()) {
                continue;
            }
            items.push(CachedMarketItem {
                id,
                plugin_name: plugin.name.clone(),
                skill_name,
                category: clean_optional_text(plugin.category.as_deref(), 80),
                description: clean_optional_text(plugin.description.as_deref(), 1024),
                subdirectory,
            });
            if items.len() > MARKET_ITEM_LIMIT {
                return Err(MarketFailure::InvalidIndex);
            }
        }
    }
    items.sort_by(|left, right| {
        left.skill_name
            .to_ascii_lowercase()
            .cmp(&right.skill_name.to_ascii_lowercase())
            .then_with(|| left.id.cmp(&right.id))
    });
    Ok(CachedMarketSnapshot {
        schema_version: CACHE_SCHEMA_VERSION,
        provider_name: document.name,
        repository_url: OFFICIAL_REPOSITORY_URL.to_owned(),
        commit_sha,
        synced_at_ms,
        items,
    })
}

fn validate_plugin(plugin: &MarketplacePlugin) -> Option<String> {
    if !valid_name(&plugin.name, 128)
        || plugin.policy.installation != "AVAILABLE"
        || plugin
            .policy
            .products
            .as_ref()
            .is_some_and(|products| !products.iter().any(|product| product == "CODEX"))
    {
        return None;
    }
    let expected = format!("./plugins/{}", plugin.name);
    if plugin.source != expected {
        return None;
    }
    let root = plugin.source.strip_prefix("./")?.to_owned();
    valid_repository_path(&root).then_some(root)
}

fn market_skill_name(path: &str, skill_prefix: &str) -> Option<String> {
    let relative = path.strip_prefix(skill_prefix)?;
    let skill_name = relative.strip_suffix("/SKILL.md")?;
    if skill_name.contains('/') || !valid_name(skill_name, 128) {
        return None;
    }
    Some(skill_name.to_owned())
}

fn stable_market_id(plugin_name: &str, subdirectory: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(OFFICIAL_PROVIDER_NAME.as_bytes());
    digest.update([0]);
    digest.update(plugin_name.to_ascii_lowercase().as_bytes());
    digest.update([0]);
    digest.update(subdirectory.to_ascii_lowercase().as_bytes());
    format!("market:{:x}", digest.finalize())
}

fn valid_name(value: &str, max_len: usize) -> bool {
    !value.is_empty()
        && value.len() <= max_len
        && value != "."
        && value != ".."
        && !value.chars().any(char::is_control)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn clean_optional_text(value: Option<&str>, max_len: usize) -> Option<String> {
    value
        .filter(|value| !value.is_empty() && value.len() <= max_len)
        .filter(|value| !value.chars().any(char::is_control))
        .map(str::to_owned)
}

fn valid_repository_path(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 2048
        && !value.starts_with('/')
        && !value.contains('\\')
        && !value.chars().any(char::is_control)
        && value.split('/').all(|segment| {
            !segment.is_empty() && segment != "." && segment != ".." && !segment.contains(':')
        })
}

fn validate_cache(snapshot: &CachedMarketSnapshot) -> bool {
    snapshot.schema_version == CACHE_SCHEMA_VERSION
        && snapshot.provider_name == OFFICIAL_PROVIDER_NAME
        && snapshot.repository_url == OFFICIAL_REPOSITORY_URL
        && valid_commit_sha(&snapshot.commit_sha)
        && snapshot.items.len() <= MARKET_ITEM_LIMIT
        && snapshot.items.iter().all(|item| {
            valid_name(&item.plugin_name, 128)
                && valid_name(&item.skill_name, 128)
                && valid_market_subdirectory(&item.subdirectory)
                && item.id == stable_market_id(&item.plugin_name, &item.subdirectory)
        })
        && snapshot
            .items
            .iter()
            .map(|item| item.id.as_str())
            .collect::<HashSet<_>>()
            .len()
            == snapshot.items.len()
}

fn valid_market_subdirectory(value: &str) -> bool {
    if !valid_repository_path(value) {
        return false;
    }
    let segments = value.split('/').collect::<Vec<_>>();
    segments.len() == 4
        && segments[0] == "plugins"
        && segments[2] == "skills"
        && valid_name(segments[1], 128)
        && valid_name(segments[3], 128)
}

fn load_cache(path: &Path) -> Option<CachedMarketSnapshot> {
    let metadata = fs::symlink_metadata(path).ok()?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() > CACHE_RESPONSE_LIMIT as u64
    {
        return None;
    }
    let bytes = fs::read(path).ok()?;
    let snapshot = serde_json::from_slice::<CachedMarketSnapshot>(&bytes).ok()?;
    validate_cache(&snapshot).then_some(snapshot)
}

fn write_cache_atomic(path: &Path, snapshot: &CachedMarketSnapshot) -> Result<(), MarketFailure> {
    if !validate_cache(snapshot) {
        return Err(MarketFailure::InvalidIndex);
    }
    let parent = path.parent().ok_or(MarketFailure::StorageUnavailable)?;
    fs::create_dir_all(parent).map_err(|_| MarketFailure::StorageUnavailable)?;
    let parent_metadata =
        fs::symlink_metadata(parent).map_err(|_| MarketFailure::StorageUnavailable)?;
    if parent_metadata.file_type().is_symlink() || !parent_metadata.is_dir() {
        return Err(MarketFailure::StorageUnavailable);
    }
    if fs::symlink_metadata(path)
        .is_ok_and(|metadata| metadata.file_type().is_symlink() || !metadata.is_file())
    {
        return Err(MarketFailure::StorageUnavailable);
    }
    let bytes = serde_json::to_vec(snapshot).map_err(|_| MarketFailure::InvalidIndex)?;
    if bytes.len() > CACHE_RESPONSE_LIMIT {
        return Err(MarketFailure::ResponseTooLarge);
    }
    // Every writer gets a unique sibling file, so concurrent refreshes cannot overwrite each other's temporary data.
    let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("market-cache.json");
    let temporary = parent.join(format!(".{name}.{}.{}.tmp", process::id(), sequence));
    let write_result = (|| {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)
            .map_err(|_| MarketFailure::StorageUnavailable)?;
        file.write_all(&bytes)
            .map_err(|_| MarketFailure::StorageUnavailable)?;
        file.sync_all()
            .map_err(|_| MarketFailure::StorageUnavailable)?;
        let verified = fs::read(&temporary).map_err(|_| MarketFailure::StorageUnavailable)?;
        let decoded = serde_json::from_slice::<CachedMarketSnapshot>(&verified)
            .map_err(|_| MarketFailure::InvalidIndex)?;
        if decoded != *snapshot || !validate_cache(&decoded) {
            return Err(MarketFailure::InvalidIndex);
        }
        fs::rename(&temporary, path).map_err(|_| MarketFailure::StorageUnavailable)
    })();
    if write_result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    write_result
}

pub(crate) async fn download_market_skill(
    endpoints: &MarketEndpoints,
    selection: &MarketSelection,
    operation_root: &Path,
) -> Result<PathBuf, OperationError> {
    if selection.repository_url != OFFICIAL_REPOSITORY_URL
        || !valid_commit_sha(&selection.commit_sha)
        || !valid_market_subdirectory(&selection.subdirectory)
    {
        return Err(OperationError::market_source_changed());
    }
    let client = market_client().map_err(|_| OperationError::market_protocol_error())?;
    let tree = fetch_tree(&client, endpoints, &selection.commit_sha)
        .await
        .map_err(market_failure_to_operation)?;
    let prefix = format!("{}/", selection.subdirectory);
    let files = tree
        .tree
        .into_iter()
        .filter(|entry| entry.path.starts_with(&prefix))
        .collect::<Vec<_>>();
    validate_selected_tree(&files, &selection.subdirectory)?;
    let skill_name = selection
        .subdirectory
        .rsplit('/')
        .next()
        .ok_or_else(OperationError::market_source_invalid)?;
    let download_root = operation_root.join("market-download");
    fs::create_dir(&download_root).map_err(|_| OperationError::import_failed())?;
    let skill_root = download_root.join(skill_name);
    fs::create_dir(&skill_root).map_err(|_| OperationError::import_failed())?;
    let regular_files = files
        .into_iter()
        .filter(|entry| entry.kind == "blob")
        .collect::<Vec<_>>();
    validate_market_file_limits(&regular_files, &prefix)?;
    for entry in regular_files {
        let relative = entry
            .path
            .strip_prefix(&prefix)
            .ok_or_else(OperationError::market_source_invalid)?;
        let limit = if is_text_import_path(relative) {
            crate::operations::MAX_IMPORT_TEXT_BYTES
        } else {
            crate::operations::MAX_IMPORT_RESOURCE_BYTES
        };
        let size = entry
            .size
            .ok_or_else(OperationError::market_source_invalid)?;
        let raw_url = raw_file_url(endpoints, &selection.commit_sha, &entry.path)
            .map_err(|_| OperationError::market_protocol_error())?;
        let response = request(&client, raw_url, endpoints)
            .await
            .map_err(market_failure_to_operation)?;
        let body = read_limited(response, limit as usize)
            .await
            .map_err(market_failure_to_operation)?;
        if body.len() as u64 != size {
            return Err(OperationError::market_source_changed());
        }
        let destination = skill_root.join(relative);
        let parent = destination
            .parent()
            .ok_or_else(OperationError::market_source_invalid)?;
        fs::create_dir_all(parent).map_err(|_| OperationError::import_failed())?;
        if fs::symlink_metadata(&destination).is_ok() {
            return Err(OperationError::market_source_invalid());
        }
        fs::write(destination, body).map_err(|_| OperationError::import_failed())?;
    }
    Ok(skill_root)
}

fn validate_market_file_limits(files: &[TreeEntry], prefix: &str) -> Result<(), OperationError> {
    if files.len() > crate::operations::MAX_IMPORT_FILES {
        return Err(OperationError::market_source_invalid());
    }
    let mut total = 0_u64;
    for entry in files {
        let relative = entry
            .path
            .strip_prefix(prefix)
            .ok_or_else(OperationError::market_source_invalid)?;
        let size = entry
            .size
            .ok_or_else(OperationError::market_source_invalid)?;
        let limit = if is_text_import_path(relative) {
            crate::operations::MAX_IMPORT_TEXT_BYTES
        } else {
            crate::operations::MAX_IMPORT_RESOURCE_BYTES
        };
        if size > limit {
            return Err(OperationError::market_source_invalid());
        }
        total = total.saturating_add(size);
        if total > crate::operations::MAX_IMPORT_TOTAL_BYTES {
            return Err(OperationError::market_source_invalid());
        }
    }
    Ok(())
}

fn validate_selected_tree(entries: &[TreeEntry], subdirectory: &str) -> Result<(), OperationError> {
    let prefix = format!("{subdirectory}/");
    let mut paths = HashSet::new();
    let mut has_manifest = false;
    for entry in entries {
        if !valid_repository_path(&entry.path) || !paths.insert(entry.path.clone()) {
            return Err(OperationError::market_source_invalid());
        }
        let relative = entry
            .path
            .strip_prefix(&prefix)
            .ok_or_else(OperationError::market_source_invalid)?;
        if relative == "SKILL.md" {
            has_manifest = entry.kind == "blob" && entry.mode == "100644";
        }
        match (entry.kind.as_str(), entry.mode.as_str()) {
            ("tree", "040000") | ("blob", "100644") | ("blob", "100755") => {}
            _ => return Err(OperationError::market_source_invalid()),
        }
    }
    if !has_manifest {
        return Err(OperationError::market_item_unavailable());
    }
    Ok(())
}

fn is_text_import_path(path: &str) -> bool {
    path == "SKILL.md"
        || path.ends_with(".md")
        || path.ends_with(".json")
        || path.ends_with(".yaml")
        || path.ends_with(".yml")
        || path.ends_with(".txt")
}

fn raw_file_url(
    endpoints: &MarketEndpoints,
    commit_sha: &str,
    path: &str,
) -> Result<Url, MarketFailure> {
    let mut segments = vec![OFFICIAL_OWNER, OFFICIAL_REPOSITORY, commit_sha];
    segments.extend(path.split('/'));
    endpoint_url(&endpoints.raw_base, &segments)
}

fn market_client() -> Result<Client, MarketFailure> {
    Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(REQUEST_TIMEOUT)
        .user_agent("Codex-O/1.0")
        .build()
        .map_err(|_| MarketFailure::Protocol)
}

async fn fetch_json<T: for<'de> Deserialize<'de>>(
    client: &Client,
    url: Url,
    endpoints: &MarketEndpoints,
    limit: usize,
) -> Result<T, MarketFailure> {
    let response = request(client, url, endpoints).await?;
    let body = read_limited(response, limit).await?;
    serde_json::from_slice(&body).map_err(|_| MarketFailure::Protocol)
}

async fn request(
    client: &Client,
    url: Url,
    endpoints: &MarketEndpoints,
) -> Result<Response, MarketFailure> {
    if !endpoints.allows(&url) {
        return Err(MarketFailure::Protocol);
    }
    let response = client.get(url).send().await.map_err(map_request_error)?;
    match response.status() {
        status if status.is_success() => Ok(response),
        StatusCode::NOT_FOUND | StatusCode::UNAUTHORIZED => Err(MarketFailure::NotFound),
        StatusCode::TOO_MANY_REQUESTS | StatusCode::FORBIDDEN => Err(MarketFailure::RateLimited),
        _ => Err(MarketFailure::Protocol),
    }
}

async fn read_limited(mut response: Response, limit: usize) -> Result<Vec<u8>, MarketFailure> {
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        return Err(MarketFailure::ResponseTooLarge);
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(map_request_error)? {
        if body.len().saturating_add(chunk.len()) > limit {
            return Err(MarketFailure::ResponseTooLarge);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn endpoint_url(base: &Url, segments: &[&str]) -> Result<Url, MarketFailure> {
    let mut url = base.clone();
    {
        let mut path = url
            .path_segments_mut()
            .map_err(|_| MarketFailure::Protocol)?;
        path.clear();
        path.extend(segments.iter().copied());
    }
    Ok(url)
}

fn map_request_error(error: reqwest::Error) -> MarketFailure {
    if error.is_timeout() {
        MarketFailure::Timeout
    } else {
        MarketFailure::Offline
    }
}

fn market_failure_to_operation(error: MarketFailure) -> OperationError {
    match error {
        MarketFailure::Offline => OperationError::market_offline(),
        MarketFailure::Timeout => OperationError::market_timeout(),
        MarketFailure::RateLimited => OperationError::market_rate_limited(),
        MarketFailure::ResponseTooLarge => OperationError::market_source_invalid(),
        MarketFailure::NotFound => OperationError::market_item_unavailable(),
        MarketFailure::CacheMissing
        | MarketFailure::StorageUnavailable
        | MarketFailure::Protocol
        | MarketFailure::InvalidIndex
        | MarketFailure::TruncatedTree => OperationError::market_protocol_error(),
    }
}

fn valid_commit_sha(value: &str) -> bool {
    value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn unavailable_catalog(issue: MarketIssue) -> MarketCatalog {
    MarketCatalog {
        status: MarketStatus::Unavailable,
        provider_name: None,
        commit_sha: None,
        synced_at_ms: None,
        items: Vec::new(),
        issue: Some(issue),
    }
}

fn lock_unpoisoned<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or_default()
}

fn origin_of(url: &Url) -> String {
    url.origin().ascii_serialization()
}

fn is_loopback_host(host: &str) -> bool {
    matches!(host, "127.0.0.1" | "::1" | "localhost")
}

#[tauri::command]
pub fn get_market_catalog(state: State<'_, Arc<MarketService>>) -> MarketCatalog {
    state.catalog()
}

#[tauri::command]
pub async fn refresh_market_catalog(
    state: State<'_, Arc<MarketService>>,
) -> Result<MarketCatalog, OperationError> {
    Ok(state.refresh().await)
}

#[tauri::command]
pub async fn plan_market_import(
    state: State<'_, Arc<MarketService>>,
    market_item_id: String,
) -> Result<PlannedImport, OperationError> {
    state.plan_import(&market_item_id).await
}

#[cfg(test)]
mod tests;
