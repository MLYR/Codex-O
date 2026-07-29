//! Read-only catalog views built from controlled provider discovery results.

mod index;
mod preferences;

use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, RwLock,
    },
    time::{Instant, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use tauri::State;

use crate::{
    observability::{
        DiagnosticDomain, DiagnosticErrorCode, DiagnosticEventCode, DiagnosticLevel,
        DiagnosticRecord, DiagnosticRecoveryCode, DiagnosticResult, DiagnosticService,
    },
    parsing::{
        parse_skill, read_skill_source, ArtifactSnapshot, MarkdownHeading, ParseDiagnostic,
        ParseDiagnosticCode, ResourceEntry, PARSER_VERSION,
    },
    providers::{
        DiscoveredSkill, DiscoveryWarning, ProviderCapabilities, ProviderDescriptor,
        ProviderDiagnostic, ProviderKind, ProviderRegistry, ProviderRoots,
    },
};

use index::CatalogIndex;
use preferences::ScanPreferencesStore;

pub use preferences::ScanPreferences;

const SKILL_MARKDOWN_FILE: &str = "SKILL.md";
const MAX_ANALYSIS_REFERENCE_BYTES: u64 = 1024 * 1024;

#[derive(Clone, Debug)]
pub struct SkillCatalog {
    roots: Arc<RwLock<ProviderRoots>>,
    // The cache is only replaced by an explicit scan, so list interactions never rescan disk.
    cache: Arc<RwLock<Option<CatalogSnapshot>>>,
    index: Option<CatalogIndex>,
    preferences: Arc<ScanPreferencesStore>,
    scan_in_progress: Arc<AtomicBool>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ImportStagedSkill {
    pub content_hash: String,
    pub name: Option<String>,
    pub description: Option<String>,
}

impl SkillCatalog {
    pub fn new(roots: ProviderRoots) -> Self {
        let preferences = ScanPreferences {
            include_plugin_cache: roots.include_plugin_cache,
            include_bundled_cache: roots.include_bundled_cache,
            initial_scan_notice_seen: true,
        };
        Self {
            roots: Arc::new(RwLock::new(roots)),
            cache: Arc::new(RwLock::new(None)),
            index: None,
            preferences: Arc::new(ScanPreferencesStore::in_memory(preferences)),
            scan_in_progress: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn with_index_path(roots: ProviderRoots, database_path: PathBuf) -> Self {
        let preferences = ScanPreferences {
            include_plugin_cache: roots.include_plugin_cache,
            include_bundled_cache: roots.include_bundled_cache,
            initial_scan_notice_seen: true,
        };
        Self {
            roots: Arc::new(RwLock::new(roots)),
            cache: Arc::new(RwLock::new(None)),
            index: Some(CatalogIndex::new(database_path)),
            preferences: Arc::new(ScanPreferencesStore::in_memory(preferences)),
            scan_in_progress: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn with_preferences_path(mut self, path: PathBuf) -> Self {
        self.preferences = Arc::new(ScanPreferencesStore::at_path(path));
        self
    }

    pub fn scan_preferences(&self) -> ScanPreferences {
        self.preferences.get()
    }

    pub fn update_scan_preferences(
        &self,
        include_plugin_cache: bool,
        include_bundled_cache: bool,
    ) -> Result<ScanPreferences, CatalogError> {
        let current = self.scan_preferences();
        self.preferences
            .set(ScanPreferences {
                include_plugin_cache,
                include_bundled_cache,
                initial_scan_notice_seen: current.initial_scan_notice_seen,
            })
            .map_err(|_| CatalogError::settings_unavailable())?;
        Ok(self.scan_preferences())
    }

    pub fn acknowledge_initial_scan_notice(&self) -> Result<ScanPreferences, CatalogError> {
        let mut current = self.scan_preferences();
        current.initial_scan_notice_seen = true;
        self.preferences
            .set(current)
            .map_err(|_| CatalogError::settings_unavailable())?;
        Ok(current)
    }

    fn begin_scan(&self) -> Result<(), CatalogError> {
        self.scan_in_progress
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| ())
            .map_err(|_| CatalogError::scan_in_progress())
    }

    fn finish_scan(&self) {
        self.scan_in_progress.store(false, Ordering::Release);
    }

    pub fn list_providers(&self) -> ProviderList {
        let scan = self.cached_scan();
        ProviderList {
            providers: scan.providers,
            diagnostics: scan.diagnostics,
        }
    }

    pub fn scan_skills(&self) -> CatalogScan {
        let mut scan = self.refresh_scan();
        if self
            .index
            .as_ref()
            .is_some_and(|index| index.save(&scan).is_err())
        {
            scan.diagnostics.push(CatalogDiagnostic {
                code: "index_unavailable".to_owned(),
                provider_id: None,
                relative_path: None,
            });
        }
        let mut cache = self
            .cache
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *cache = Some(scan.clone());
        scan.to_catalog_scan()
    }

    pub fn load_catalog(&self) -> Option<CatalogScan> {
        self.cached_snapshot_from_memory_or_index()
            .map(|snapshot| snapshot.to_catalog_scan())
    }

    pub fn list_skills(&self, query: SkillListQuery) -> SkillList {
        let mut scan = self.cached_scan();
        scan.skills.retain(|skill| query.matches(skill));
        scan.skills
            .sort_by(|left, right| query.compare(left, right));
        SkillList {
            skills: scan.skills,
            diagnostics: scan.diagnostics,
        }
    }

    pub fn get_skill_detail(
        &self,
        skill_id: &str,
        include_source: bool,
    ) -> Result<SkillDetail, CatalogError> {
        let entry = self
            .cached_snapshot()
            .entries
            .into_iter()
            .find(|entry| entry.summary.id == skill_id)
            .ok_or_else(CatalogError::skill_not_found)?;
        let mut diagnostics = entry.summary.diagnostics.clone();
        let mut source = None;

        if include_source {
            let source_skill = entry.skill.or_else(|| self.discover_skill_by_id(skill_id));
            match source_skill.map(|skill| read_skill_source(&skill)) {
                Some(Ok(value)) => source = Some(value),
                Some(Err(diagnostic)) => diagnostics.push(parse_diagnostic(&diagnostic)),
                None => diagnostics.push(CatalogDiagnostic {
                    code: "skill_not_found".to_owned(),
                    provider_id: Some(entry.summary.provider.id.clone()),
                    relative_path: None,
                }),
            }
        }

        Ok(SkillDetail {
            summary: entry.summary,
            headings: entry.headings,
            resources: entry.resources,
            diagnostics,
            source,
        })
    }

    pub(crate) fn analysis_metadata(
        &self,
        skill_id: &str,
    ) -> Result<SkillAnalysisMetadata, CatalogError> {
        let snapshot = self
            .cached_snapshot_from_memory_or_index()
            .ok_or_else(CatalogError::catalog_unavailable)?;
        let entry = snapshot
            .entries
            .into_iter()
            .find(|entry| entry.summary.id == skill_id)
            .ok_or_else(CatalogError::skill_not_found)?;
        Ok(SkillAnalysisMetadata {
            snapshot_id: format!("snapshot:{}:{}", entry.summary.id, entry.content_hash),
            content_hash: entry.content_hash,
            parser_version: entry.parser_version,
        })
    }

    pub(crate) fn analysis_material(
        &self,
        skill_id: &str,
    ) -> Result<SkillAnalysisMaterial, CatalogError> {
        let snapshot = self
            .cached_snapshot_from_memory_or_index()
            .ok_or_else(CatalogError::catalog_unavailable)?;
        let entry = snapshot
            .entries
            .into_iter()
            .find(|entry| entry.summary.id == skill_id)
            .ok_or_else(CatalogError::skill_not_found)?;
        let source_skill = entry.skill.or_else(|| self.discover_skill_by_id(skill_id));
        let source_skill = source_skill.ok_or_else(CatalogError::analysis_unavailable)?;
        let source =
            read_skill_source(&source_skill).map_err(|_| CatalogError::analysis_unavailable())?;
        let parsed = entry.snapshot.unwrap_or_else(|| {
            parse_skill(&source_skill)
                .snapshot
                .unwrap_or_else(|| ArtifactSnapshot {
                    skill_id: entry.summary.id.clone(),
                    content_hash: entry.content_hash.clone(),
                    parser_version: PARSER_VERSION,
                    frontmatter: crate::parsing::SkillFrontmatter {
                        name: Some(entry.summary.display_name.clone()),
                        description: entry.summary.description.clone(),
                        extensions: serde_json::Map::new(),
                    },
                    headings: entry.headings.clone(),
                    openai_manifest: None,
                    resources: entry.resources.clone(),
                    diagnostics: Vec::new(),
                })
        });
        let mut sources = vec![SkillAnalysisSource {
            relative_path: SKILL_MARKDOWN_FILE.to_owned(),
            content: source.clone(),
        }];
        sources.extend(referenced_analysis_sources(
            &source_skill,
            &parsed.resources,
            &source,
        ));
        Ok(SkillAnalysisMaterial {
            metadata: SkillAnalysisMetadata {
                snapshot_id: format!("snapshot:{}:{}", parsed.skill_id, parsed.content_hash),
                content_hash: parsed.content_hash.clone(),
                parser_version: parsed.parser_version.to_owned(),
            },
            snapshot: parsed,
            sources,
        })
    }

    pub(crate) fn current_content_hash(&self, skill_id: &str) -> Result<String, CatalogError> {
        let skill = self
            .discover_skill_by_id(skill_id)
            .ok_or_else(CatalogError::skill_not_found)?;
        parse_skill(&skill)
            .snapshot
            .map(|snapshot| snapshot.content_hash)
            .ok_or_else(CatalogError::analysis_unavailable)
    }

    pub(crate) fn managed_user_root(&self) -> PathBuf {
        // Write targets are derived inside Rust from the managed provider root.
        self.roots_snapshot().home_directory.join(".agents/skills")
    }

    pub(crate) fn validate_import_staging(
        &self,
        staging_home: PathBuf,
        expected_relative_path: &str,
    ) -> Result<ImportStagedSkill, CatalogError> {
        // Reuse provider discovery and deterministic parsing against an isolated staging home.
        let roots = ProviderRoots::new(staging_home, PathBuf::new(), PathBuf::new())
            .with_plugin_cache_enabled(false)
            .with_bundled_cache_enabled(false);
        let discovered = ProviderRegistry::with_roots(roots).discover_all();
        let Some(skill) = discovered.skills.into_iter().find(|skill| {
            skill.provider_id == "user_global" && skill.relative_path == expected_relative_path
        }) else {
            return Err(CatalogError::import_source_invalid());
        };
        let parsed = parse_skill(&skill);
        let Some(snapshot) = parsed.snapshot else {
            return Err(CatalogError::import_source_invalid());
        };

        if !parsed.diagnostics.is_empty() || !snapshot.diagnostics.is_empty() {
            return Err(CatalogError::import_source_invalid());
        }

        Ok(ImportStagedSkill {
            content_hash: snapshot.content_hash,
            name: snapshot.frontmatter.name,
            description: snapshot.frontmatter.description,
        })
    }

    pub(crate) fn managed_skill_id(&self, relative_path: &str) -> Option<String> {
        self.cached_snapshot_from_memory_or_index()?
            .entries
            .into_iter()
            .find_map(|entry| {
                entry.skill.and_then(|skill| {
                    (skill.provider_id == "user_global" && skill.relative_path == relative_path)
                        .then_some(skill.id)
                })
            })
    }

    fn cached_scan(&self) -> CatalogScan {
        self.cached_snapshot().to_catalog_scan()
    }

    fn cached_snapshot(&self) -> CatalogSnapshot {
        if let Some(snapshot) = self.cached_snapshot_from_memory_or_index() {
            return snapshot;
        }

        self.scan_skills();
        self.cache
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .cloned()
            .expect("scan_skills populates the catalog cache")
    }

    fn cached_snapshot_from_memory_or_index(&self) -> Option<CatalogSnapshot> {
        {
            let cache = self
                .cache
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(snapshot) = cache.as_ref() {
                return Some(snapshot.clone());
            }
        }

        let snapshot = self
            .index
            .as_ref()?
            .load()
            .ok()
            .flatten()?
            .filtered_for(self.scan_preferences());
        let mut cache = self
            .cache
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *cache = Some(snapshot.clone());
        Some(snapshot)
    }

    fn discover_skill_by_id(&self, skill_id: &str) -> Option<DiscoveredSkill> {
        ProviderRegistry::with_roots(self.roots_for_scan())
            .discover_all()
            .skills
            .into_iter()
            .find(|skill| skill.id == skill_id)
    }

    fn refresh_scan(&self) -> CatalogSnapshot {
        let discovered = ProviderRegistry::with_roots(self.roots_for_scan()).discover_all();
        let providers = provider_views(&discovered.providers, &discovered.diagnostics);
        let provider_map = providers
            .iter()
            .map(|provider| (provider.id.as_str(), provider))
            .collect::<HashMap<_, _>>();
        let entries = discovered
            .skills
            .iter()
            .map(|skill| {
                let result = parse_skill(skill);
                let provider = provider_map
                    .get(skill.provider_id.as_str())
                    .expect("discovered skills always have a provider descriptor");
                let summary = skill_summary(
                    skill,
                    result.snapshot.as_ref(),
                    &result.diagnostics,
                    provider,
                );
                CatalogEntry {
                    skill: Some(skill.clone()),
                    content_hash: result
                        .snapshot
                        .as_ref()
                        .map(|snapshot| snapshot.content_hash.clone())
                        .unwrap_or_else(|| format!("unparsed:{}", skill.id)),
                    parser_version: result
                        .snapshot
                        .as_ref()
                        .map(|snapshot| snapshot.parser_version.to_owned())
                        .unwrap_or_else(|| PARSER_VERSION.to_owned()),
                    headings: result
                        .snapshot
                        .as_ref()
                        .map(|snapshot| snapshot.headings.clone())
                        .unwrap_or_default(),
                    resources: result
                        .snapshot
                        .as_ref()
                        .map(|snapshot| snapshot.resources.clone())
                        .unwrap_or_default(),
                    snapshot: result.snapshot.clone(),
                    summary,
                }
            })
            .collect();

        CatalogSnapshot {
            providers,
            entries,
            diagnostics: discovery_diagnostics(&discovered.warnings, &discovered.diagnostics),
        }
    }

    fn roots_for_scan(&self) -> ProviderRoots {
        self.roots
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
            .with_plugin_cache_enabled(self.scan_preferences().include_plugin_cache)
            .with_bundled_cache_enabled(self.scan_preferences().include_bundled_cache)
    }

    pub(crate) fn roots_snapshot(&self) -> ProviderRoots {
        self.roots
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub(crate) fn set_additional_roots(&self, roots: Vec<crate::providers::AdditionalRoot>) {
        self.roots
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .additional_roots = roots;
    }
}

#[derive(Clone, Debug)]
struct CatalogSnapshot {
    providers: Vec<ProviderView>,
    entries: Vec<CatalogEntry>,
    diagnostics: Vec<CatalogDiagnostic>,
}

impl CatalogSnapshot {
    fn to_catalog_scan(&self) -> CatalogScan {
        CatalogScan {
            providers: self.providers.clone(),
            skills: self
                .entries
                .iter()
                .map(|entry| entry.summary.clone())
                .collect(),
            diagnostics: self.diagnostics.clone(),
        }
    }

    fn filtered_for(mut self, preferences: ScanPreferences) -> Self {
        if !preferences.include_plugin_cache || !preferences.include_bundled_cache {
            self.providers.retain(|provider| {
                (preferences.include_plugin_cache || provider.kind != ProviderKind::Plugin)
                    && (preferences.include_bundled_cache || provider.kind != ProviderKind::Bundled)
            });
            self.entries.retain(|entry| {
                (preferences.include_plugin_cache
                    || entry.summary.provider.kind != ProviderKind::Plugin)
                    && (preferences.include_bundled_cache
                        || entry.summary.provider.kind != ProviderKind::Bundled)
            });
        }
        self
    }
}

#[derive(Clone, Debug)]
struct CatalogEntry {
    skill: Option<DiscoveredSkill>,
    content_hash: String,
    parser_version: String,
    headings: Vec<MarkdownHeading>,
    resources: Vec<ResourceEntry>,
    snapshot: Option<ArtifactSnapshot>,
    summary: SkillSummary,
}

#[derive(Clone, Debug)]
pub(crate) struct SkillAnalysisMetadata {
    pub snapshot_id: String,
    pub content_hash: String,
    pub parser_version: String,
}

#[derive(Clone, Debug)]
pub(crate) struct SkillAnalysisMaterial {
    pub metadata: SkillAnalysisMetadata,
    pub snapshot: ArtifactSnapshot,
    pub sources: Vec<SkillAnalysisSource>,
}

#[derive(Clone, Debug)]
pub(crate) struct SkillAnalysisSource {
    pub relative_path: String,
    pub content: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct ProviderList {
    pub providers: Vec<ProviderView>,
    pub diagnostics: Vec<CatalogDiagnostic>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct ProviderView {
    pub id: String,
    pub kind: ProviderKind,
    pub display_name: String,
    pub capabilities: ProviderCapabilities,
    pub availability: ProviderAvailability,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderAvailability {
    Available,
    Unavailable,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct CatalogDiagnostic {
    pub code: String,
    pub provider_id: Option<String>,
    pub relative_path: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct CatalogScan {
    pub providers: Vec<ProviderView>,
    pub skills: Vec<SkillSummary>,
    pub diagnostics: Vec<CatalogDiagnostic>,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct SkillList {
    pub skills: Vec<SkillSummary>,
    pub diagnostics: Vec<CatalogDiagnostic>,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct SkillSummary {
    pub id: String,
    pub display_name: String,
    pub description: Option<String>,
    pub provider: ProviderView,
    pub scope: SkillScope,
    pub validity: SkillValidity,
    pub analysis_status: AnalysisStatus,
    pub size_bytes: u64,
    pub updated_at_ms: Option<u64>,
    pub diagnostics: Vec<CatalogDiagnostic>,
    #[serde(skip_serializing, default)]
    search_headings: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillScope {
    User,
    Repository,
    LegacyUser,
    System,
    Plugin,
    Bundled,
    Additional,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillValidity {
    Valid,
    NeedsAttention,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AnalysisStatus {
    NotConfigured,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct SkillDetail {
    pub summary: SkillSummary,
    pub headings: Vec<MarkdownHeading>,
    pub resources: Vec<ResourceEntry>,
    pub diagnostics: Vec<CatalogDiagnostic>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillSort {
    #[default]
    Name,
    Updated,
    Size,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillListQuery {
    pub query: Option<String>,
    pub provider_id: Option<String>,
    pub scope: Option<SkillScope>,
    pub validity: Option<SkillValidity>,
    #[serde(default)]
    pub sort: SkillSort,
}

impl SkillListQuery {
    fn matches(&self, skill: &SkillSummary) -> bool {
        let query = self.query.as_deref().unwrap_or("").trim().to_lowercase();
        (query.is_empty()
            || skill.display_name.to_lowercase().contains(&query)
            || skill
                .description
                .as_deref()
                .unwrap_or("")
                .to_lowercase()
                .contains(&query)
            || skill
                .search_headings
                .iter()
                .any(|heading| heading.to_lowercase().contains(&query)))
            && self
                .provider_id
                .as_deref()
                .is_none_or(|provider_id| provider_id == skill.provider.id)
            && self.scope.is_none_or(|scope| scope == skill.scope)
            && self
                .validity
                .is_none_or(|validity| validity == skill.validity)
    }

    fn compare(&self, left: &SkillSummary, right: &SkillSummary) -> std::cmp::Ordering {
        match self.sort {
            SkillSort::Name => left
                .display_name
                .to_lowercase()
                .cmp(&right.display_name.to_lowercase()),
            SkillSort::Updated => right
                .updated_at_ms
                .cmp(&left.updated_at_ms)
                .then_with(|| left.display_name.cmp(&right.display_name)),
            SkillSort::Size => right
                .size_bytes
                .cmp(&left.size_bytes)
                .then_with(|| left.display_name.cmp(&right.display_name)),
        }
        .then_with(|| left.id.cmp(&right.id))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct CatalogError {
    pub code: &'static str,
    pub message: &'static str,
}

impl CatalogError {
    fn skill_not_found() -> Self {
        Self {
            code: "skill_not_found",
            message: "The requested Skill is unavailable.",
        }
    }

    fn settings_unavailable() -> Self {
        Self {
            code: "settings_unavailable",
            message: "The scan preference could not be saved.",
        }
    }

    fn scan_in_progress() -> Self {
        Self {
            code: "scan_in_progress",
            message: "A Skill scan is already in progress.",
        }
    }

    fn catalog_unavailable() -> Self {
        Self {
            code: "catalog_unavailable",
            message: "Scan Skills before requesting analysis.",
        }
    }

    fn analysis_unavailable() -> Self {
        Self {
            code: "analysis_source_unavailable",
            message: "The requested Skill cannot be prepared for analysis.",
        }
    }

    fn import_source_invalid() -> Self {
        Self {
            code: "import_source_invalid",
            message: "The selected Skill cannot be imported safely.",
        }
    }
}

#[tauri::command]
pub fn list_providers(catalog: State<'_, SkillCatalog>) -> ProviderList {
    catalog.list_providers()
}

#[tauri::command]
pub async fn scan_skills(
    catalog: State<'_, SkillCatalog>,
    diagnostics: State<'_, Arc<DiagnosticService>>,
) -> Result<CatalogScan, CatalogError> {
    let started = Instant::now();
    diagnostics.emit(DiagnosticRecord::new(
        DiagnosticLevel::Info,
        DiagnosticDomain::SkillScan,
        DiagnosticEventCode::SkillScanStarted,
        DiagnosticResult::Started,
    ));
    if let Err(error) = catalog.begin_scan() {
        diagnostics.emit(
            DiagnosticRecord::new(
                DiagnosticLevel::Warning,
                DiagnosticDomain::SkillScan,
                DiagnosticEventCode::SkillScanFailed,
                DiagnosticResult::Failed,
            )
            .with_duration(started.elapsed().as_millis() as u64)
            .with_error(
                DiagnosticErrorCode::ScanInProgress,
                true,
                DiagnosticRecoveryCode::Retry,
            ),
        );
        return Err(error);
    }
    let scan_catalog = catalog.inner().clone();
    let result = tauri::async_runtime::spawn_blocking(move || scan_catalog.scan_skills())
        .await
        .map_err(|_| CatalogError {
            code: "scan_failed",
            message: "The Skill scan did not complete.",
        });
    catalog.finish_scan();
    match &result {
        Ok(scan) => {
            diagnostics.emit(
                DiagnosticRecord::new(
                    DiagnosticLevel::Info,
                    DiagnosticDomain::SkillScan,
                    DiagnosticEventCode::SkillScanCompleted,
                    DiagnosticResult::Succeeded,
                )
                .with_duration(started.elapsed().as_millis() as u64)
                .with_counts(Some(scan.skills.len() as u64), None),
            );
        }
        Err(_) => {
            diagnostics.emit(
                DiagnosticRecord::new(
                    DiagnosticLevel::Error,
                    DiagnosticDomain::SkillScan,
                    DiagnosticEventCode::SkillScanFailed,
                    DiagnosticResult::Failed,
                )
                .with_duration(started.elapsed().as_millis() as u64)
                .with_error(
                    DiagnosticErrorCode::ScanFailed,
                    true,
                    DiagnosticRecoveryCode::Rescan,
                ),
            );
        }
    }
    result
}

#[tauri::command]
pub fn load_catalog(
    catalog: State<'_, SkillCatalog>,
    diagnostics: State<'_, Arc<DiagnosticService>>,
) -> Option<CatalogScan> {
    let started = Instant::now();
    let scan = catalog.load_catalog();
    diagnostics.emit(
        DiagnosticRecord::new(
            if scan.is_some() {
                DiagnosticLevel::Info
            } else {
                DiagnosticLevel::Warning
            },
            DiagnosticDomain::Catalog,
            DiagnosticEventCode::CatalogCacheLoaded,
            if scan.is_some() {
                DiagnosticResult::Succeeded
            } else {
                DiagnosticResult::Degraded
            },
        )
        .with_duration(started.elapsed().as_millis() as u64)
        .with_counts(
            scan.as_ref().map(|catalog| catalog.skills.len() as u64),
            None,
        ),
    );
    scan
}

#[tauri::command]
pub fn get_scan_preferences(catalog: State<'_, SkillCatalog>) -> ScanPreferences {
    catalog.scan_preferences()
}

#[tauri::command]
pub fn update_scan_preferences(
    catalog: State<'_, SkillCatalog>,
    include_plugin_cache: bool,
    include_bundled_cache: bool,
) -> Result<ScanPreferences, CatalogError> {
    catalog.update_scan_preferences(include_plugin_cache, include_bundled_cache)
}

#[tauri::command]
pub fn acknowledge_initial_scan_notice(
    catalog: State<'_, SkillCatalog>,
) -> Result<ScanPreferences, CatalogError> {
    catalog.acknowledge_initial_scan_notice()
}

#[tauri::command]
pub fn list_skills(catalog: State<'_, SkillCatalog>, query: SkillListQuery) -> SkillList {
    catalog.list_skills(query)
}

#[tauri::command]
pub fn get_skill_detail(
    catalog: State<'_, SkillCatalog>,
    skill_id: String,
    include_source: Option<bool>,
) -> Result<SkillDetail, CatalogError> {
    catalog.get_skill_detail(&skill_id, include_source.unwrap_or(false))
}

fn provider_views(
    providers: &[ProviderDescriptor],
    diagnostics: &[ProviderDiagnostic],
) -> Vec<ProviderView> {
    let mut result = providers
        .iter()
        .map(|provider| ProviderView {
            id: provider.id.clone(),
            kind: provider.kind,
            display_name: provider.display_name.clone(),
            capabilities: provider.capabilities,
            availability: if diagnostics
                .iter()
                .any(|diagnostic| diagnostic.provider_id == provider.id)
            {
                ProviderAvailability::Unavailable
            } else {
                ProviderAvailability::Available
            },
        })
        .collect::<Vec<_>>();
    result.sort_by(|left, right| left.id.cmp(&right.id));
    result
}

fn discovery_diagnostics(
    warnings: &[DiscoveryWarning],
    diagnostics: &[ProviderDiagnostic],
) -> Vec<CatalogDiagnostic> {
    let mut result = warnings
        .iter()
        .map(|warning| CatalogDiagnostic {
            code: discovery_warning_code(warning).to_owned(),
            provider_id: Some(warning.provider_id.clone()),
            relative_path: warning.relative_path.clone(),
        })
        .collect::<Vec<_>>();
    result.extend(diagnostics.iter().map(|diagnostic| CatalogDiagnostic {
        code: "provider_unavailable".to_owned(),
        provider_id: Some(diagnostic.provider_id.clone()),
        relative_path: None,
    }));
    result
}

fn discovery_warning_code(warning: &DiscoveryWarning) -> &'static str {
    use crate::providers::DiscoveryWarningCode;

    match warning.code {
        DiscoveryWarningCode::EntryUnreadable => "entry_unreadable",
        DiscoveryWarningCode::InvalidRelativePath => "invalid_relative_path",
        DiscoveryWarningCode::InvalidSkillMarker => "invalid_skill_marker",
        DiscoveryWarningCode::RootUnavailable => "root_unavailable",
        DiscoveryWarningCode::SymlinkDenied => "symlink_denied",
        DiscoveryWarningCode::UnsupportedCacheLayout => "unsupported_cache_layout",
    }
}

fn skill_summary(
    skill: &crate::providers::DiscoveredSkill,
    snapshot: Option<&ArtifactSnapshot>,
    parse_diagnostics: &[ParseDiagnostic],
    provider: &ProviderView,
) -> SkillSummary {
    let snapshot_diagnostics = snapshot
        .map(|snapshot| snapshot.diagnostics.as_slice())
        .unwrap_or(parse_diagnostics);
    let diagnostics = snapshot_diagnostics
        .iter()
        .map(parse_diagnostic)
        .collect::<Vec<_>>();
    let display_name = snapshot
        .and_then(|snapshot| snapshot.frontmatter.name.clone())
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| {
            skill
                .relative_path
                .rsplit('/')
                .next()
                .unwrap_or(skill.relative_path.as_str())
                .to_owned()
        });
    let size_bytes = skill_size(skill.skill_directory(), snapshot);

    SkillSummary {
        id: skill.id.clone(),
        display_name,
        description: snapshot.and_then(|snapshot| snapshot.frontmatter.description.clone()),
        provider: provider.clone(),
        scope: scope(skill.provider_kind),
        validity: if snapshot.is_some() && diagnostics.is_empty() {
            SkillValidity::Valid
        } else {
            SkillValidity::NeedsAttention
        },
        analysis_status: AnalysisStatus::NotConfigured,
        size_bytes,
        updated_at_ms: skill_modified_at(skill.skill_directory()),
        diagnostics,
        search_headings: snapshot
            .map(|snapshot| {
                snapshot
                    .headings
                    .iter()
                    .map(|heading| heading.text.clone())
                    .collect()
            })
            .unwrap_or_default(),
    }
}

fn parse_diagnostic(diagnostic: &ParseDiagnostic) -> CatalogDiagnostic {
    CatalogDiagnostic {
        code: parse_diagnostic_code(diagnostic.code.clone()).to_owned(),
        provider_id: None,
        relative_path: Some(diagnostic.relative_path.clone()),
    }
}

fn parse_diagnostic_code(code: ParseDiagnosticCode) -> &'static str {
    match code {
        ParseDiagnosticCode::EntryUnreadable => "entry_unreadable",
        ParseDiagnosticCode::InputTooLarge => "input_too_large",
        ParseDiagnosticCode::InvalidUtf8 => "invalid_utf8",
        ParseDiagnosticCode::InvalidFrontmatter => "invalid_frontmatter",
        ParseDiagnosticCode::InvalidYaml => "invalid_yaml",
        ParseDiagnosticCode::InvalidMarkdown => "invalid_markdown",
        ParseDiagnosticCode::InvalidPath => "invalid_path",
        ParseDiagnosticCode::SymlinkDenied => "symlink_denied",
    }
}

fn scope(kind: ProviderKind) -> SkillScope {
    match kind {
        ProviderKind::UserGlobal => SkillScope::User,
        ProviderKind::Repo => SkillScope::Repository,
        ProviderKind::LegacyUser => SkillScope::LegacyUser,
        ProviderKind::System => SkillScope::System,
        ProviderKind::Plugin => SkillScope::Plugin,
        ProviderKind::Bundled => SkillScope::Bundled,
        ProviderKind::AdditionalRoot => SkillScope::Additional,
    }
}

fn skill_size(skill_directory: &Path, snapshot: Option<&ArtifactSnapshot>) -> u64 {
    let source_size = fs::symlink_metadata(skill_directory.join(SKILL_MARKDOWN_FILE))
        .ok()
        .filter(|metadata| metadata.is_file() && !metadata.file_type().is_symlink())
        .map(|metadata| metadata.len())
        .unwrap_or_default();
    source_size
        + snapshot
            .map(|snapshot| {
                snapshot
                    .resources
                    .iter()
                    .map(|resource| resource.size_bytes)
                    .sum::<u64>()
            })
            .unwrap_or_default()
}

fn skill_modified_at(skill_directory: &Path) -> Option<u64> {
    fs::symlink_metadata(skill_directory.join(SKILL_MARKDOWN_FILE))
        .ok()
        .filter(|metadata| metadata.is_file() && !metadata.file_type().is_symlink())
        .and_then(|metadata| metadata.modified().ok())
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis() as u64)
}

fn referenced_analysis_sources(
    skill: &DiscoveredSkill,
    resources: &[ResourceEntry],
    skill_source: &str,
) -> Vec<SkillAnalysisSource> {
    resources
        .iter()
        .filter(|resource| resource.relative_path.starts_with("references/"))
        .filter(|resource| {
            let file_name = resource
                .relative_path
                .rsplit('/')
                .next()
                .unwrap_or(resource.relative_path.as_str());
            skill_source.contains(&resource.relative_path) || skill_source.contains(file_name)
        })
        .filter_map(|resource| {
            let path = skill.skill_directory().join(&resource.relative_path);
            let metadata = fs::symlink_metadata(&path).ok()?;
            if !metadata.is_file()
                || metadata.file_type().is_symlink()
                || metadata.len() > MAX_ANALYSIS_REFERENCE_BYTES
            {
                return None;
            }
            fs::read_to_string(path)
                .ok()
                .map(|content| SkillAnalysisSource {
                    relative_path: resource.relative_path.clone(),
                    content,
                })
        })
        .collect()
}

#[cfg(test)]
mod tests;
