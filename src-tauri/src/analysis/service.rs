use std::{
    path::PathBuf,
    sync::{Arc, RwLock},
};

use serde::{Deserialize, Serialize};

use crate::catalog::SkillCatalog;

use super::{
    analysis_key, cache::new_record, redact_context, validate_passport, AiProvider, AnalysisCache,
    AnalysisContextBuilder, AnalysisOutcomeStatus, AnalysisRecord, AnalysisRecordStatus,
    AnalysisRequest, RedactionCounts, SkillPassport,
};

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AnalysisRunStatus {
    NotConfigured,
    Ready,
    Stale,
    Failed,
    Degraded,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct SentSection {
    pub id: String,
    pub relative_path: String,
    pub line_start: usize,
    pub line_end: usize,
    pub title: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct AnalysisResult {
    pub skill_id: String,
    pub analysis_key: Option<String>,
    pub status: AnalysisRunStatus,
    pub passport: Option<SkillPassport>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub cache_hit: bool,
    pub attempts: u8,
    pub redactions: RedactionCounts,
    pub sent_sections: Vec<SentSection>,
    pub diagnostics: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AnalysisServiceErrorCode {
    SkillUnavailable,
    ContextUnavailable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct AnalysisServiceError {
    pub code: AnalysisServiceErrorCode,
    pub message: &'static str,
}

pub struct AnalysisService {
    catalog: SkillCatalog,
    cache: Arc<dyn AnalysisCache>,
    provider: RwLock<Option<Arc<dyn AiProvider>>>,
    context_builder: AnalysisContextBuilder,
    home_directory: Option<PathBuf>,
}

impl AnalysisService {
    pub fn new(
        catalog: SkillCatalog,
        cache: Arc<dyn AnalysisCache>,
        home_directory: Option<PathBuf>,
    ) -> Self {
        Self {
            catalog,
            cache,
            provider: RwLock::new(None),
            context_builder: AnalysisContextBuilder::default(),
            home_directory,
        }
    }

    pub fn set_provider(&self, provider: Option<Arc<dyn AiProvider>>) {
        *self
            .provider
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = provider;
    }

    pub fn job_key(&self, skill_id: &str) -> Result<Option<String>, AnalysisServiceError> {
        let Some(provider) = self.provider() else {
            return Ok(None);
        };
        let metadata = self
            .catalog
            .analysis_metadata(skill_id)
            .map_err(|_| skill_unavailable())?;
        Ok(Some(analysis_key(
            &metadata.content_hash,
            &metadata.parser_version,
            &provider.identity(),
        )))
    }

    pub async fn analyze(
        &self,
        skill_id: &str,
        force: bool,
    ) -> Result<AnalysisResult, AnalysisServiceError> {
        let Some(provider) = self.provider() else {
            return Ok(not_configured(skill_id));
        };
        let material = self
            .catalog
            .analysis_material(skill_id)
            .map_err(|_| skill_unavailable())?;
        let sources = material
            .sources
            .into_iter()
            .map(|source| super::AnalysisSource::new(source.relative_path, source.content))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| context_unavailable())?;
        let context = self
            .context_builder
            .build(&material.snapshot, &sources)
            .map_err(|_| context_unavailable())?;
        let (context, redactions) = redact_context(context, self.home_directory.as_deref());
        let identity = provider.identity();
        let key = analysis_key(
            &material.metadata.content_hash,
            &material.metadata.parser_version,
            &identity,
        );
        let sent_sections = context
            .sections
            .iter()
            .map(|section| SentSection {
                id: section.id.clone(),
                relative_path: section.relative_path.clone(),
                line_start: section.line_start,
                line_end: section.line_end,
                title: section.title.clone(),
            })
            .collect::<Vec<_>>();

        let cached = self.cache.load(&key);
        if !force {
            if let Ok(Some(record)) = &cached {
                if matches!(
                    record.status,
                    AnalysisRecordStatus::Ready | AnalysisRecordStatus::Degraded
                ) && record.passport.is_some()
                {
                    return Ok(result_from_record(
                        skill_id,
                        record,
                        true,
                        redactions,
                        sent_sections,
                        Vec::new(),
                    ));
                }
            }
        }
        let mut diagnostics = Vec::new();
        let cache_available = cached.is_ok();
        if !cache_available {
            diagnostics.push("cache_unavailable".to_owned());
        }
        let fallback = self
            .cache
            .latest_for_snapshot(&material.metadata.snapshot_id)
            .ok()
            .flatten()
            .filter(|record| record.analysis_key != key && record.passport.is_some());
        if self
            .cache
            .mark_stale(&material.metadata.snapshot_id, &key)
            .is_err()
            && !diagnostics.iter().any(|value| value == "cache_unavailable")
        {
            diagnostics.push("cache_unavailable".to_owned());
        }

        let request = AnalysisRequest::new(context, redactions, identity.language.clone());
        let first = provider.analyze(request.clone()).await;
        let (validated, attempts) = match first {
            Ok(response) => match validate_passport(&response.content, &request.context) {
                Ok(validated) => (Some(validated), response.attempts),
                Err(_) => match provider.analyze(request.clone().repair()).await {
                    Ok(repaired) => (
                        validate_passport(&repaired.content, &request.context).ok(),
                        response.attempts.saturating_add(repaired.attempts),
                    ),
                    Err(_) => (None, response.attempts),
                },
            },
            Err(_) => {
                return Ok(provider_failure(
                    fallback,
                    ProviderFailureContext {
                        skill_id,
                        key,
                        snapshot_id: &material.metadata.snapshot_id,
                        identity: &identity,
                        cache: self.cache.as_ref(),
                        redactions,
                        sent_sections,
                        diagnostics,
                    },
                ));
            }
        };
        let Some(validated) = validated else {
            diagnostics.push("schema_invalid".to_owned());
            if let Some(mut fallback) = fallback {
                fallback.status = AnalysisRecordStatus::Stale;
                return Ok(result_from_record(
                    skill_id,
                    &fallback,
                    false,
                    redactions,
                    sent_sections,
                    diagnostics,
                ));
            }
            let record = new_record(
                material.metadata.snapshot_id,
                key.clone(),
                AnalysisRecordStatus::Degraded,
                None,
                &identity,
            );
            let _ = self.cache.save(&record);
            return Ok(AnalysisResult {
                skill_id: skill_id.to_owned(),
                analysis_key: Some(key),
                status: AnalysisRunStatus::Degraded,
                passport: None,
                provider: Some(identity.provider),
                model: Some(identity.model),
                cache_hit: false,
                attempts,
                redactions,
                sent_sections,
                diagnostics,
            });
        };

        let record_status = match validated.status {
            AnalysisOutcomeStatus::Ready => AnalysisRecordStatus::Ready,
            AnalysisOutcomeStatus::Degraded => AnalysisRecordStatus::Degraded,
        };
        let record = new_record(
            material.metadata.snapshot_id,
            key.clone(),
            record_status,
            Some(validated.passport.clone()),
            &identity,
        );
        let cache_saved = self.cache.save(&record).is_ok();
        if !cache_saved
            && !diagnostics
                .iter()
                .any(|diagnostic| diagnostic == "cache_unavailable")
        {
            diagnostics.push("cache_unavailable".to_owned());
        }
        Ok(AnalysisResult {
            skill_id: skill_id.to_owned(),
            analysis_key: Some(key),
            status: if cache_saved && record_status == AnalysisRecordStatus::Ready {
                AnalysisRunStatus::Ready
            } else {
                AnalysisRunStatus::Degraded
            },
            passport: Some(validated.passport),
            provider: Some(identity.provider),
            model: Some(identity.model),
            cache_hit: false,
            attempts,
            redactions,
            sent_sections,
            diagnostics,
        })
    }

    fn provider(&self) -> Option<Arc<dyn AiProvider>> {
        self.provider
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

struct ProviderFailureContext<'a> {
    skill_id: &'a str,
    key: String,
    snapshot_id: &'a str,
    identity: &'a super::AiProviderIdentity,
    cache: &'a dyn AnalysisCache,
    redactions: RedactionCounts,
    sent_sections: Vec<SentSection>,
    diagnostics: Vec<String>,
}

fn provider_failure(
    fallback: Option<AnalysisRecord>,
    mut context: ProviderFailureContext<'_>,
) -> AnalysisResult {
    context.diagnostics.push("provider_unavailable".to_owned());
    if let Some(mut fallback) = fallback {
        fallback.status = AnalysisRecordStatus::Stale;
        return result_from_record(
            context.skill_id,
            &fallback,
            false,
            context.redactions,
            context.sent_sections,
            context.diagnostics,
        );
    }
    let record = new_record(
        context.snapshot_id.to_owned(),
        context.key.clone(),
        AnalysisRecordStatus::Failed,
        None,
        context.identity,
    );
    let _ = context.cache.save(&record);
    AnalysisResult {
        skill_id: context.skill_id.to_owned(),
        analysis_key: Some(context.key),
        status: AnalysisRunStatus::Failed,
        passport: None,
        provider: Some(context.identity.provider.clone()),
        model: Some(context.identity.model.clone()),
        cache_hit: false,
        attempts: 0,
        redactions: context.redactions,
        sent_sections: context.sent_sections,
        diagnostics: context.diagnostics,
    }
}

fn result_from_record(
    skill_id: &str,
    record: &AnalysisRecord,
    cache_hit: bool,
    redactions: RedactionCounts,
    sent_sections: Vec<SentSection>,
    diagnostics: Vec<String>,
) -> AnalysisResult {
    AnalysisResult {
        skill_id: skill_id.to_owned(),
        analysis_key: Some(record.analysis_key.clone()),
        status: match record.status {
            AnalysisRecordStatus::Ready => AnalysisRunStatus::Ready,
            AnalysisRecordStatus::Stale => AnalysisRunStatus::Stale,
            AnalysisRecordStatus::Failed => AnalysisRunStatus::Failed,
            AnalysisRecordStatus::Degraded => AnalysisRunStatus::Degraded,
        },
        passport: record.passport.clone(),
        provider: Some(record.provider.clone()),
        model: Some(record.model.clone()),
        cache_hit,
        attempts: 0,
        redactions,
        sent_sections,
        diagnostics,
    }
}

fn not_configured(skill_id: &str) -> AnalysisResult {
    AnalysisResult {
        skill_id: skill_id.to_owned(),
        analysis_key: None,
        status: AnalysisRunStatus::NotConfigured,
        passport: None,
        provider: None,
        model: None,
        cache_hit: false,
        attempts: 0,
        redactions: RedactionCounts::default(),
        sent_sections: Vec::new(),
        diagnostics: vec!["ai_not_configured".to_owned()],
    }
}

const fn skill_unavailable() -> AnalysisServiceError {
    AnalysisServiceError {
        code: AnalysisServiceErrorCode::SkillUnavailable,
        message: "The requested Skill is unavailable for analysis.",
    }
}

const fn context_unavailable() -> AnalysisServiceError {
    AnalysisServiceError {
        code: AnalysisServiceErrorCode::ContextUnavailable,
        message: "The analysis context could not be prepared.",
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashMap,
        fs,
        path::PathBuf,
        sync::{
            atomic::{AtomicUsize, Ordering},
            Arc, Mutex,
        },
    };

    use async_trait::async_trait;
    use serde_json::json;
    use tempfile::TempDir;

    use crate::{
        analysis::{
            analysis_key, AiProvider, AiProviderIdentity, AnalysisCache, AnalysisCacheError,
            AnalysisProviderError, AnalysisProviderErrorCode, AnalysisRecord, AnalysisRecordStatus,
            AnalysisRequest, AnalysisRunStatus, AnalysisService, ProviderResponse,
            UnavailableAnalysisCache,
        },
        catalog::SkillCatalog,
        providers::ProviderRoots,
    };

    #[derive(Default)]
    struct MemoryCache {
        records: Mutex<HashMap<String, AnalysisRecord>>,
    }

    impl AnalysisCache for MemoryCache {
        fn load(&self, key: &str) -> Result<Option<AnalysisRecord>, AnalysisCacheError> {
            Ok(self.records.lock().unwrap().get(key).cloned())
        }

        fn latest_for_snapshot(
            &self,
            snapshot_id: &str,
        ) -> Result<Option<AnalysisRecord>, AnalysisCacheError> {
            Ok(self
                .records
                .lock()
                .unwrap()
                .values()
                .filter(|record| record.snapshot_id == snapshot_id && record.passport.is_some())
                .max_by_key(|record| record.created_at)
                .cloned())
        }

        fn mark_stale(
            &self,
            snapshot_id: &str,
            current_key: &str,
        ) -> Result<(), AnalysisCacheError> {
            for record in self.records.lock().unwrap().values_mut() {
                if record.snapshot_id == snapshot_id
                    && record.analysis_key != current_key
                    && matches!(
                        record.status,
                        AnalysisRecordStatus::Ready | AnalysisRecordStatus::Degraded
                    )
                {
                    record.status = AnalysisRecordStatus::Stale;
                }
            }
            Ok(())
        }

        fn save(&self, record: &AnalysisRecord) -> Result<(), AnalysisCacheError> {
            self.records
                .lock()
                .unwrap()
                .entry(record.analysis_key.clone())
                .or_insert_with(|| record.clone());
            Ok(())
        }
    }

    enum FixtureMode {
        Valid,
        InvalidThenValid,
        Invalid,
        Failed,
    }

    struct FixtureProvider {
        identity: AiProviderIdentity,
        mode: FixtureMode,
        calls: AtomicUsize,
        requests: Mutex<Vec<AnalysisRequest>>,
    }

    impl FixtureProvider {
        fn new(model: &str, mode: FixtureMode) -> Self {
            Self {
                identity: AiProviderIdentity {
                    provider: "fixture".to_owned(),
                    model: model.to_owned(),
                    language: "en".to_owned(),
                },
                mode,
                calls: AtomicUsize::new(0),
                requests: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl AiProvider for FixtureProvider {
        fn identity(&self) -> AiProviderIdentity {
            self.identity.clone()
        }

        async fn analyze(
            &self,
            request: AnalysisRequest,
        ) -> Result<ProviderResponse, AnalysisProviderError> {
            let call = self.calls.fetch_add(1, Ordering::SeqCst);
            self.requests.lock().unwrap().push(request.clone());
            match self.mode {
                FixtureMode::Failed => Err(AnalysisProviderError {
                    code: AnalysisProviderErrorCode::TransportUnavailable,
                    retryable: true,
                }),
                FixtureMode::Invalid => Ok(ProviderResponse {
                    content: "not-json".to_owned(),
                    attempts: 1,
                }),
                FixtureMode::InvalidThenValid if call == 0 => Ok(ProviderResponse {
                    content: "not-json".to_owned(),
                    attempts: 1,
                }),
                FixtureMode::Valid | FixtureMode::InvalidThenValid => Ok(ProviderResponse {
                    content: valid_passport(&request),
                    attempts: 1,
                }),
            }
        }
    }

    fn valid_passport(request: &AnalysisRequest) -> String {
        let evidence = &request.context.sections[0];
        json!({
            "summary": "Safe summary",
            "capabilities": ["Review"],
            "triggerExamples": ["Review this"],
            "suitableWhen": ["Review is needed"],
            "avoidWhen": ["No source"],
            "workflow": ["Read facts"],
            "prerequisites": ["Parsed Skill"],
            "resources": [],
            "sideEffects": ["No writes"],
            "risks": [],
            "relatedHints": ["Compare results"],
            "confidence": "high",
            "evidenceRefs": [{
                "sectionId": evidence.id,
                "relativePath": evidence.relative_path,
                "lineStart": evidence.line_start,
                "lineEnd": evidence.line_end
            }],
            "uncertainties": ["Runtime is not executed"]
        })
        .to_string()
    }

    fn catalog_fixture(source: &str) -> (TempDir, SkillCatalog, String) {
        let temporary = TempDir::new().unwrap();
        let root = temporary.path().join("home/.agents/skills/example");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("SKILL.md"), source).unwrap();
        let catalog = SkillCatalog::new(ProviderRoots::new(
            temporary.path().join("home"),
            temporary.path().join("repository"),
            temporary.path().join("plugin-cache"),
        ));
        let skill_id = catalog.scan_skills().skills[0].id.clone();
        (temporary, catalog, skill_id)
    }

    fn catalog_fixture_with_reference() -> (TempDir, SkillCatalog, String) {
        let temporary = TempDir::new().unwrap();
        let root = temporary.path().join("home/.agents/skills/example");
        fs::create_dir_all(root.join("references")).unwrap();
        fs::write(
            root.join("SKILL.md"),
            "# Overview\nRead references/guide.md.",
        )
        .unwrap();
        fs::write(
            root.join("references/guide.md"),
            "# Guide\nReference sent marker",
        )
        .unwrap();
        let catalog = SkillCatalog::new(ProviderRoots::new(
            temporary.path().join("home"),
            temporary.path().join("repository"),
            temporary.path().join("plugin-cache"),
        ));
        let skill_id = catalog.scan_skills().skills[0].id.clone();
        (temporary, catalog, skill_id)
    }

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    }

    fn configured_service(
        source: &str,
        cache: Arc<dyn AnalysisCache>,
        provider: Arc<FixtureProvider>,
    ) -> (TempDir, AnalysisService, String) {
        let (temporary, catalog, skill_id) = catalog_fixture(source);
        let service =
            AnalysisService::new(catalog, cache, Some(PathBuf::from("/Users/private-user")));
        service.set_provider(Some(provider));
        (temporary, service, skill_id)
    }

    #[test]
    fn unconfigured_analysis_returns_before_catalog_access() {
        let temporary = TempDir::new().unwrap();
        let catalog = SkillCatalog::new(ProviderRoots::new(
            temporary.path().join("missing-home"),
            temporary.path().join("missing-repository"),
            temporary.path().join("missing-cache"),
        ));
        let service = AnalysisService::new(catalog, Arc::new(MemoryCache::default()), None);
        let result = runtime()
            .block_on(service.analyze("unknown", false))
            .unwrap();

        assert_eq!(result.status, AnalysisRunStatus::NotConfigured);
        assert!(result.sent_sections.is_empty());
    }

    #[test]
    fn service_redacts_before_provider_and_exposes_only_sent_ranges() {
        let provider = Arc::new(FixtureProvider::new("model", FixtureMode::Valid));
        let (_temporary, service, skill_id) = configured_service(
            "# Overview\nAuthorization: Bearer fixture-secret\n/Users/private-user/file",
            Arc::new(MemoryCache::default()),
            Arc::clone(&provider),
        );
        let result = runtime()
            .block_on(service.analyze(&skill_id, false))
            .unwrap();
        let request = &provider.requests.lock().unwrap()[0];
        let encoded_request = serde_json::to_string(request).unwrap();
        let encoded_result = serde_json::to_string(&result).unwrap();

        assert!(!encoded_request.contains("fixture-secret"));
        assert!(!encoded_request.contains("/Users/private-user"));
        assert!(!encoded_result.contains("fixture-secret"));
        assert!(!encoded_result.contains("/Users/private-user"));
        assert_eq!(result.redactions.authorization_headers, 1);
        assert_eq!(result.redactions.home_paths, 1);
    }

    #[test]
    fn service_sends_only_explicitly_referenced_reference_text() {
        let provider = Arc::new(FixtureProvider::new("model", FixtureMode::Valid));
        let (temporary, catalog, skill_id) = catalog_fixture_with_reference();
        let service = AnalysisService::new(
            catalog,
            Arc::new(MemoryCache::default()),
            Some(temporary.path().to_path_buf()),
        );
        service.set_provider(Some(Arc::clone(&provider) as Arc<dyn AiProvider>));

        runtime()
            .block_on(service.analyze(&skill_id, false))
            .unwrap();
        let request = &provider.requests.lock().unwrap()[0];

        assert!(request.context.sections.iter().any(|section| {
            section.relative_path == "references/guide.md"
                && section.content.contains("Reference sent marker")
        }));
    }

    #[test]
    fn ready_cache_hits_skip_the_provider() {
        let cache = Arc::new(MemoryCache::default());
        let provider = Arc::new(FixtureProvider::new("model", FixtureMode::Valid));
        let (_temporary, service, skill_id) =
            configured_service("# Overview\nSafe", cache, Arc::clone(&provider));
        let first = runtime()
            .block_on(service.analyze(&skill_id, false))
            .unwrap();
        let second = runtime()
            .block_on(service.analyze(&skill_id, false))
            .unwrap();

        assert_eq!(first.status, AnalysisRunStatus::Ready);
        assert!(second.cache_hit);
        assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn invalid_schema_is_repaired_only_once() {
        let provider = Arc::new(FixtureProvider::new("model", FixtureMode::InvalidThenValid));
        let (_temporary, service, skill_id) = configured_service(
            "# Overview\nSafe",
            Arc::new(MemoryCache::default()),
            Arc::clone(&provider),
        );
        let result = runtime()
            .block_on(service.analyze(&skill_id, false))
            .unwrap();

        assert_eq!(result.status, AnalysisRunStatus::Ready);
        assert_eq!(provider.calls.load(Ordering::SeqCst), 2);
        assert!(provider.requests.lock().unwrap()[1].repair);
    }

    #[test]
    fn a_second_invalid_schema_degrades_without_raw_output() {
        let provider = Arc::new(FixtureProvider::new("model", FixtureMode::Invalid));
        let (_temporary, service, skill_id) = configured_service(
            "# Overview\nSafe",
            Arc::new(MemoryCache::default()),
            Arc::clone(&provider),
        );
        let result = runtime()
            .block_on(service.analyze(&skill_id, false))
            .unwrap();
        let encoded = serde_json::to_string(&result).unwrap();

        assert_eq!(result.status, AnalysisRunStatus::Degraded);
        assert_eq!(provider.calls.load(Ordering::SeqCst), 2);
        assert!(!encoded.contains("not-json"));
    }

    #[test]
    fn provider_failure_returns_a_stale_previous_passport() {
        let cache = Arc::new(MemoryCache::default());
        let provider_a = Arc::new(FixtureProvider::new("model-a", FixtureMode::Valid));
        let (_temporary, service, skill_id) = configured_service(
            "# Overview\nSafe",
            Arc::clone(&cache) as Arc<dyn AnalysisCache>,
            provider_a,
        );
        let first = runtime()
            .block_on(service.analyze(&skill_id, false))
            .unwrap();
        let provider_b = Arc::new(FixtureProvider::new("model-b", FixtureMode::Failed));
        service.set_provider(Some(provider_b));
        let second = runtime()
            .block_on(service.analyze(&skill_id, false))
            .unwrap();

        assert_eq!(first.status, AnalysisRunStatus::Ready);
        assert_eq!(second.status, AnalysisRunStatus::Stale);
        assert!(second.passport.is_some());
        assert!(second
            .diagnostics
            .iter()
            .any(|value| value == "provider_unavailable"));
    }

    #[test]
    fn cache_failure_preserves_a_valid_passport_as_degraded() {
        let provider = Arc::new(FixtureProvider::new("model", FixtureMode::Valid));
        let (_temporary, service, skill_id) = configured_service(
            "# Overview\nSafe",
            Arc::new(UnavailableAnalysisCache),
            provider,
        );
        let result = runtime()
            .block_on(service.analyze(&skill_id, false))
            .unwrap();

        assert_eq!(result.status, AnalysisRunStatus::Degraded);
        assert!(result.passport.is_some());
        assert!(result
            .diagnostics
            .iter()
            .any(|value| value == "cache_unavailable"));
    }

    #[test]
    fn analysis_key_changes_with_the_provider_model() {
        let (temporary, catalog, skill_id) = catalog_fixture("# Overview\nSafe");
        let service = AnalysisService::new(
            catalog,
            Arc::new(MemoryCache::default()),
            Some(temporary.path().to_path_buf()),
        );
        service.set_provider(Some(Arc::new(FixtureProvider::new(
            "model-a",
            FixtureMode::Valid,
        ))));
        let first = service.job_key(&skill_id).unwrap().unwrap();
        service.set_provider(Some(Arc::new(FixtureProvider::new(
            "model-b",
            FixtureMode::Valid,
        ))));
        let second = service.job_key(&skill_id).unwrap().unwrap();

        assert_ne!(first, second);
        assert_eq!(
            first.len(),
            analysis_key(
                "a",
                "b",
                &AiProviderIdentity {
                    provider: "fixture".to_owned(),
                    model: "model-a".to_owned(),
                    language: "en".to_owned(),
                }
            )
            .len()
        );
    }
}
