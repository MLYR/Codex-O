use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, RwLock},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::catalog::SkillCatalog;

use super::{
    analysis_key, cache::new_record, redact_context, validate_passport, AiProvider, AnalysisCache,
    AnalysisContextBuilder, AnalysisOutcomeStatus, AnalysisRecord, AnalysisRecordStatus,
    AnalysisRequest, RedactionCounts, SkillPassport,
};

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AnalysisRunStatus {
    NotRequested,
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

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct EvidenceLink {
    pub id: String,
    pub relative_path: String,
    pub line_start: usize,
    pub line_end: usize,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct AnalysisView {
    pub skill_id: String,
    pub analysis_key: Option<String>,
    pub status: AnalysisRunStatus,
    pub passport: Option<SkillPassport>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub language: Option<String>,
    pub analyzed_at_ms: Option<i64>,
    pub cache_hit: bool,
    pub stale: bool,
    pub degraded: bool,
    pub redactions: RedactionCounts,
    pub sent_sections: Vec<SentSection>,
    pub evidence: Vec<EvidenceLink>,
    pub diagnostics: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct EvidenceLine {
    pub number: usize,
    pub text: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct EvidenceExcerpt {
    pub evidence_id: String,
    pub relative_path: String,
    pub line_start: usize,
    pub line_end: usize,
    pub lines: Vec<EvidenceLine>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct SkillComparison {
    pub left: ComparisonSkill,
    pub right: ComparisonSkill,
    pub rows: Vec<ComparisonRow>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct ComparisonSkill {
    pub id: String,
    pub display_name: String,
    pub provider: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct ComparisonRow {
    pub key: String,
    pub label: String,
    pub left: Vec<String>,
    pub right: Vec<String>,
    pub different: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AnalysisServiceErrorCode {
    SkillUnavailable,
    ContextUnavailable,
    EvidenceUnavailable,
    EvidenceChanged,
    InvalidComparison,
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
    evidence_registry: RwLock<HashMap<String, EvidenceBinding>>,
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
            evidence_registry: RwLock::new(HashMap::new()),
        }
    }

    pub fn set_provider(&self, provider: Option<Arc<dyn AiProvider>>) {
        *self
            .provider
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = provider;
    }

    pub fn is_configured(&self) -> bool {
        self.provider().is_some()
    }

    pub fn analysis_view(&self, skill_id: &str) -> Result<AnalysisView, AnalysisServiceError> {
        let metadata = self
            .catalog
            .analysis_metadata(skill_id)
            .map_err(|_| skill_unavailable())?;
        let provider = self.provider();
        let current_identity = provider.as_ref().map(|provider| provider.identity());
        let current_key = current_identity.as_ref().map(|identity| {
            analysis_key(&metadata.content_hash, &metadata.parser_version, identity)
        });
        let mut diagnostics = Vec::new();
        let current_record = current_key
            .as_deref()
            .and_then(|key| match self.cache.load(key) {
                Ok(record) => record,
                Err(_) => {
                    diagnostics.push("cache_unavailable".to_owned());
                    None
                }
            });
        let record = if current_record.is_some() {
            current_record
        } else {
            match self.cache.latest_for_snapshot(&metadata.snapshot_id) {
                Ok(record) => record,
                Err(_) => {
                    diagnostics.push("cache_unavailable".to_owned());
                    None
                }
            }
        };

        let (redactions, sent_sections) = if record.is_some() {
            self.prepared_context(skill_id).unwrap_or_else(|_| {
                diagnostics.push("analysis_context_unavailable".to_owned());
                (RedactionCounts::default(), Vec::new())
            })
        } else {
            (RedactionCounts::default(), Vec::new())
        };
        let record_is_stale = record.as_ref().is_some_and(|record| {
            record.status == AnalysisRecordStatus::Stale
                || current_key
                    .as_deref()
                    .is_some_and(|key| key != record.analysis_key)
        });
        let status = record
            .as_ref()
            .map(|record| {
                if record_is_stale {
                    AnalysisRunStatus::Stale
                } else {
                    record_status(record.status)
                }
            })
            .unwrap_or_else(|| {
                if provider.is_some() {
                    AnalysisRunStatus::NotRequested
                } else {
                    AnalysisRunStatus::NotConfigured
                }
            });
        let evidence = record
            .as_ref()
            .and_then(|record| record.passport.as_ref().map(|passport| (record, passport)))
            .map(|(record, passport)| {
                self.register_evidence(
                    skill_id,
                    &metadata.content_hash,
                    &record.analysis_key,
                    passport,
                )
            })
            .unwrap_or_default();

        Ok(AnalysisView {
            skill_id: skill_id.to_owned(),
            analysis_key: record.as_ref().map(|record| record.analysis_key.clone()),
            status,
            passport: record.as_ref().and_then(|record| record.passport.clone()),
            provider: record
                .as_ref()
                .map(|record| record.provider.clone())
                .or_else(|| {
                    current_identity
                        .as_ref()
                        .map(|identity| identity.provider.clone())
                }),
            model: record
                .as_ref()
                .map(|record| record.model.clone())
                .or_else(|| {
                    current_identity
                        .as_ref()
                        .map(|identity| identity.model.clone())
                }),
            language: record
                .as_ref()
                .map(|record| record.language.clone())
                .or_else(|| {
                    current_identity
                        .as_ref()
                        .map(|identity| identity.language.clone())
                }),
            analyzed_at_ms: record.as_ref().map(|record| record.created_at),
            cache_hit: record.is_some(),
            stale: record_is_stale,
            degraded: status == AnalysisRunStatus::Degraded,
            redactions,
            sent_sections,
            evidence,
            diagnostics,
        })
    }

    pub fn read_evidence_excerpt(
        &self,
        evidence_id: &str,
    ) -> Result<EvidenceExcerpt, AnalysisServiceError> {
        let binding = self
            .evidence_registry
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(evidence_id)
            .cloned()
            .ok_or_else(evidence_unavailable)?;
        let current_hash = self
            .catalog
            .current_content_hash(&binding.skill_id)
            .map_err(|_| evidence_changed())?;
        if current_hash != binding.content_hash {
            return Err(evidence_changed());
        }
        let record = self
            .cache
            .load(&binding.analysis_key)
            .map_err(|_| evidence_unavailable())?
            .ok_or_else(evidence_unavailable)?;
        let still_bound = record.passport.as_ref().is_some_and(|passport| {
            passport.evidence_refs.iter().any(|evidence| {
                evidence.relative_path == binding.relative_path
                    && evidence.line_start == binding.line_start
                    && evidence.line_end == binding.line_end
            })
        });
        if !still_bound {
            return Err(evidence_unavailable());
        }
        let material = self
            .catalog
            .analysis_material(&binding.skill_id)
            .map_err(|_| evidence_changed())?;
        let source = material
            .sources
            .into_iter()
            .find(|source| source.relative_path == binding.relative_path)
            .ok_or_else(evidence_changed)?;
        let source_hash = hash_text(&source.content);
        if source_hash != binding.source_hash {
            return Err(evidence_changed());
        }
        let lines = source.content.lines().collect::<Vec<_>>();
        if binding.line_start == 0
            || binding.line_start > binding.line_end
            || binding.line_end > lines.len()
        {
            return Err(evidence_changed());
        }
        Ok(EvidenceExcerpt {
            evidence_id: evidence_id.to_owned(),
            relative_path: binding.relative_path,
            line_start: binding.line_start,
            line_end: binding.line_end,
            lines: lines[(binding.line_start - 1)..binding.line_end]
                .iter()
                .enumerate()
                .map(|(index, line)| EvidenceLine {
                    number: binding.line_start + index,
                    text: (*line).to_owned(),
                })
                .collect(),
        })
    }

    pub fn compare_skills(
        &self,
        skill_ids: &[String],
    ) -> Result<SkillComparison, AnalysisServiceError> {
        if skill_ids.len() != 2 || skill_ids[0] == skill_ids[1] {
            return Err(invalid_comparison());
        }
        let left_detail = self
            .catalog
            .get_skill_detail(&skill_ids[0], false)
            .map_err(|_| skill_unavailable())?;
        let right_detail = self
            .catalog
            .get_skill_detail(&skill_ids[1], false)
            .map_err(|_| skill_unavailable())?;
        let left_analysis = self.analysis_view(&skill_ids[0])?;
        let right_analysis = self.analysis_view(&skill_ids[1])?;
        let left_passport = left_analysis.passport.as_ref();
        let right_passport = right_analysis.passport.as_ref();
        let rows = vec![
            comparison_row(
                "provider",
                "Provider",
                vec![left_detail.summary.provider.display_name.clone()],
                vec![right_detail.summary.provider.display_name.clone()],
            ),
            comparison_row(
                "purpose",
                "用途",
                purpose_values(&left_detail, left_passport),
                purpose_values(&right_detail, right_passport),
            ),
            comparison_row(
                "triggers",
                "触发",
                passport_values(left_passport.map(|passport| &passport.trigger_examples)),
                passport_values(right_passport.map(|passport| &passport.trigger_examples)),
            ),
            comparison_row(
                "suitable",
                "适用",
                passport_values(left_passport.map(|passport| &passport.suitable_when)),
                passport_values(right_passport.map(|passport| &passport.suitable_when)),
            ),
            comparison_row(
                "avoid",
                "不适用",
                passport_values(left_passport.map(|passport| &passport.avoid_when)),
                passport_values(right_passport.map(|passport| &passport.avoid_when)),
            ),
            comparison_row(
                "prerequisites",
                "前置",
                passport_values(left_passport.map(|passport| &passport.prerequisites)),
                passport_values(right_passport.map(|passport| &passport.prerequisites)),
            ),
            comparison_row(
                "workflow",
                "工作流",
                passport_values(left_passport.map(|passport| &passport.workflow)),
                passport_values(right_passport.map(|passport| &passport.workflow)),
            ),
            comparison_row(
                "resources",
                "资源",
                resource_values(&left_detail, left_passport),
                resource_values(&right_detail, right_passport),
            ),
            comparison_row(
                "risks",
                "风险",
                risk_values(left_passport),
                risk_values(right_passport),
            ),
            comparison_row(
                "confidence",
                "置信度",
                confidence_values(left_passport),
                confidence_values(right_passport),
            ),
        ];
        Ok(SkillComparison {
            left: comparison_skill(&left_detail),
            right: comparison_skill(&right_detail),
            rows,
        })
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

    fn prepared_context(
        &self,
        skill_id: &str,
    ) -> Result<(RedactionCounts, Vec<SentSection>), AnalysisServiceError> {
        let material = self
            .catalog
            .analysis_material(skill_id)
            .map_err(|_| context_unavailable())?;
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
        let sent_sections = context
            .sections
            .into_iter()
            .map(|section| SentSection {
                id: section.id,
                relative_path: section.relative_path,
                line_start: section.line_start,
                line_end: section.line_end,
                title: section.title,
            })
            .collect();
        Ok((redactions, sent_sections))
    }

    fn register_evidence(
        &self,
        skill_id: &str,
        content_hash: &str,
        analysis_key: &str,
        passport: &SkillPassport,
    ) -> Vec<EvidenceLink> {
        let material = self.catalog.analysis_material(skill_id).ok();
        let mut registry = self
            .evidence_registry
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // Evidence IDs are short-lived UI handles; cap the registry instead of retaining history.
        if registry.len() >= 512 {
            registry.clear();
        }
        passport
            .evidence_refs
            .iter()
            .filter_map(|evidence| {
                let source_hash = material.as_ref().and_then(|material| {
                    material
                        .sources
                        .iter()
                        .find(|source| source.relative_path == evidence.relative_path)
                        .map(|source| hash_text(&source.content))
                })?;
                let id = evidence_id(
                    skill_id,
                    analysis_key,
                    content_hash,
                    &evidence.relative_path,
                    evidence.line_start,
                    evidence.line_end,
                );
                registry.insert(
                    id.clone(),
                    EvidenceBinding {
                        skill_id: skill_id.to_owned(),
                        analysis_key: analysis_key.to_owned(),
                        content_hash: content_hash.to_owned(),
                        source_hash,
                        relative_path: evidence.relative_path.clone(),
                        line_start: evidence.line_start,
                        line_end: evidence.line_end,
                    },
                );
                Some(EvidenceLink {
                    id,
                    relative_path: evidence.relative_path.clone(),
                    line_start: evidence.line_start,
                    line_end: evidence.line_end,
                })
            })
            .collect()
    }
}

#[derive(Clone)]
struct EvidenceBinding {
    skill_id: String,
    analysis_key: String,
    content_hash: String,
    source_hash: String,
    relative_path: String,
    line_start: usize,
    line_end: usize,
}

fn record_status(status: AnalysisRecordStatus) -> AnalysisRunStatus {
    match status {
        AnalysisRecordStatus::Ready => AnalysisRunStatus::Ready,
        AnalysisRecordStatus::Stale => AnalysisRunStatus::Stale,
        AnalysisRecordStatus::Failed => AnalysisRunStatus::Failed,
        AnalysisRecordStatus::Degraded => AnalysisRunStatus::Degraded,
    }
}

fn comparison_skill(detail: &crate::catalog::SkillDetail) -> ComparisonSkill {
    ComparisonSkill {
        id: detail.summary.id.clone(),
        display_name: detail.summary.display_name.clone(),
        provider: detail.summary.provider.display_name.clone(),
    }
}

fn comparison_row(
    key: &'static str,
    label: &'static str,
    left: Vec<String>,
    right: Vec<String>,
) -> ComparisonRow {
    ComparisonRow {
        key: key.to_owned(),
        label: label.to_owned(),
        different: left != right,
        left,
        right,
    }
}

fn purpose_values(
    detail: &crate::catalog::SkillDetail,
    passport: Option<&SkillPassport>,
) -> Vec<String> {
    passport
        .map(|passport| vec![passport.summary.clone()])
        .or_else(|| {
            detail
                .summary
                .description
                .clone()
                .map(|description| vec![description])
        })
        .unwrap_or_else(information_unavailable)
}

fn passport_values(values: Option<&Vec<String>>) -> Vec<String> {
    values
        .filter(|values| !values.is_empty())
        .cloned()
        .unwrap_or_else(not_analyzed)
}

fn resource_values(
    detail: &crate::catalog::SkillDetail,
    passport: Option<&SkillPassport>,
) -> Vec<String> {
    let analyzed = passport
        .map(|passport| {
            passport
                .resources
                .iter()
                .map(|resource| format!("{} · {}", resource.relative_path, resource.summary))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if !analyzed.is_empty() {
        analyzed
    } else if !detail.resources.is_empty() {
        detail
            .resources
            .iter()
            .map(|resource| resource.relative_path.clone())
            .collect()
    } else if passport.is_some() {
        information_unavailable()
    } else {
        not_analyzed()
    }
}

fn risk_values(passport: Option<&SkillPassport>) -> Vec<String> {
    passport
        .map(|passport| {
            passport
                .risks
                .iter()
                .map(|risk| {
                    format!(
                        "{} · {} · {}",
                        risk_severity_label(risk.severity),
                        risk.category,
                        risk.description
                    )
                })
                .collect::<Vec<_>>()
        })
        .filter(|values| !values.is_empty())
        .unwrap_or_else(|| {
            if passport.is_some() {
                information_unavailable()
            } else {
                not_analyzed()
            }
        })
}

fn confidence_values(passport: Option<&SkillPassport>) -> Vec<String> {
    passport
        .map(|passport| {
            vec![match passport.confidence {
                super::Confidence::High => "高",
                super::Confidence::Medium => "中",
                super::Confidence::Low => "低",
            }
            .to_owned()]
        })
        .unwrap_or_else(not_analyzed)
}

fn risk_severity_label(severity: super::RiskSeverity) -> &'static str {
    match severity {
        super::RiskSeverity::Low => "低",
        super::RiskSeverity::Medium => "中",
        super::RiskSeverity::High => "高",
    }
}

fn not_analyzed() -> Vec<String> {
    vec!["尚未分析".to_owned()]
}

fn information_unavailable() -> Vec<String> {
    vec!["信息不足".to_owned()]
}

fn evidence_id(
    skill_id: &str,
    analysis_key: &str,
    content_hash: &str,
    relative_path: &str,
    line_start: usize,
    line_end: usize,
) -> String {
    let mut hasher = Sha256::new();
    let line_start = line_start.to_string();
    let line_end = line_end.to_string();
    for field in [
        skill_id,
        analysis_key,
        content_hash,
        relative_path,
        line_start.as_str(),
        line_end.as_str(),
    ] {
        hasher.update((field.len() as u64).to_le_bytes());
        hasher.update(field.as_bytes());
    }
    format!(
        "evidence:{}",
        hasher
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    )
}

fn hash_text(content: &str) -> String {
    Sha256::digest(content.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
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
        status: record_status(record.status),
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

const fn evidence_unavailable() -> AnalysisServiceError {
    AnalysisServiceError {
        code: AnalysisServiceErrorCode::EvidenceUnavailable,
        message: "The requested evidence handle is unavailable.",
    }
}

const fn evidence_changed() -> AnalysisServiceError {
    AnalysisServiceError {
        code: AnalysisServiceErrorCode::EvidenceChanged,
        message: "The Skill content changed after this evidence handle was created.",
    }
}

const fn invalid_comparison() -> AnalysisServiceError {
    AnalysisServiceError {
        code: AnalysisServiceErrorCode::InvalidComparison,
        message: "Choose two different Skills to compare.",
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
            AnalysisRequest, AnalysisRunStatus, AnalysisService, AnalysisServiceErrorCode,
            ProviderResponse, UnavailableAnalysisCache,
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

    fn catalog_fixture_two() -> (TempDir, SkillCatalog, Vec<String>) {
        let temporary = TempDir::new().unwrap();
        let skills_root = temporary.path().join("home/.agents/skills");
        for (name, source) in [
            ("alpha", "# Overview\nAlpha workflow"),
            ("beta", "# Overview\nBeta workflow"),
        ] {
            let root = skills_root.join(name);
            fs::create_dir_all(&root).unwrap();
            fs::write(root.join("SKILL.md"), source).unwrap();
            if name == "alpha" {
                fs::create_dir(root.join("scripts")).unwrap();
                fs::write(root.join("scripts/check.sh"), "echo checked").unwrap();
            }
        }
        let catalog = SkillCatalog::new(ProviderRoots::new(
            temporary.path().join("home"),
            temporary.path().join("repository"),
            temporary.path().join("plugin-cache"),
        ));
        let skill_ids = catalog
            .scan_skills()
            .skills
            .into_iter()
            .map(|skill| skill.id)
            .collect();
        (temporary, catalog, skill_ids)
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

    #[test]
    fn cached_analysis_view_exposes_passport_metadata_and_safe_evidence_handles() {
        let provider = Arc::new(FixtureProvider::new("model", FixtureMode::Valid));
        let (_temporary, service, skill_id) = configured_service(
            "# Overview\nEvidence line",
            Arc::new(MemoryCache::default()),
            provider,
        );
        runtime()
            .block_on(service.analyze(&skill_id, false))
            .unwrap();

        let view = service.analysis_view(&skill_id).unwrap();

        assert_eq!(view.status, AnalysisRunStatus::Ready);
        assert!(view.cache_hit);
        assert!(view.passport.is_some());
        assert_eq!(view.evidence.len(), 1);
        assert!(view.evidence[0].id.starts_with("evidence:"));
        assert!(!serde_json::to_string(&view).unwrap().contains("/Users/"));
    }

    #[test]
    fn evidence_excerpt_uses_registered_id_and_preserves_line_numbers() {
        let provider = Arc::new(FixtureProvider::new("model", FixtureMode::Valid));
        let (_temporary, service, skill_id) = configured_service(
            "# Overview\nEvidence line",
            Arc::new(MemoryCache::default()),
            provider,
        );
        runtime()
            .block_on(service.analyze(&skill_id, false))
            .unwrap();
        let view = service.analysis_view(&skill_id).unwrap();

        let excerpt = service.read_evidence_excerpt(&view.evidence[0].id).unwrap();

        assert_eq!(excerpt.relative_path, "SKILL.md");
        assert_eq!(excerpt.lines[0].number, excerpt.line_start);
        assert!(!excerpt.lines[0].text.is_empty());
    }

    #[test]
    fn evidence_excerpt_rejects_unknown_ids_and_changed_content() {
        let provider = Arc::new(FixtureProvider::new("model", FixtureMode::Valid));
        let (temporary, service, skill_id) = configured_service(
            "# Overview\nEvidence line",
            Arc::new(MemoryCache::default()),
            provider,
        );
        runtime()
            .block_on(service.analyze(&skill_id, false))
            .unwrap();
        let view = service.analysis_view(&skill_id).unwrap();
        let unknown = service
            .read_evidence_excerpt("evidence:unknown")
            .unwrap_err();
        fs::write(
            temporary
                .path()
                .join("home/.agents/skills/example/SKILL.md"),
            "# Overview\nChanged evidence",
        )
        .unwrap();
        let changed = service
            .read_evidence_excerpt(&view.evidence[0].id)
            .unwrap_err();

        assert_eq!(unknown.code, AnalysisServiceErrorCode::EvidenceUnavailable);
        assert_eq!(changed.code, AnalysisServiceErrorCode::EvidenceChanged);
    }

    #[test]
    fn comparison_uses_cached_passports_without_calling_the_provider() {
        let (temporary, catalog, skill_ids) = catalog_fixture_two();
        let cache = Arc::new(MemoryCache::default());
        let provider = Arc::new(FixtureProvider::new("model", FixtureMode::Valid));
        let service = AnalysisService::new(catalog, cache, Some(temporary.path().to_path_buf()));
        service.set_provider(Some(Arc::clone(&provider) as Arc<dyn AiProvider>));
        for skill_id in &skill_ids {
            runtime()
                .block_on(service.analyze(skill_id, false))
                .unwrap();
        }
        let calls_before = provider.calls.load(Ordering::SeqCst);

        let comparison = service.compare_skills(&skill_ids).unwrap();

        assert_eq!(provider.calls.load(Ordering::SeqCst), calls_before);
        assert_eq!(comparison.rows.len(), 10);
        assert!(comparison.rows.iter().any(|row| row.different));
    }
}
