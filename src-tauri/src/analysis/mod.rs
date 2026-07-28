//! Safe, deterministic inputs and validated outputs for Skill analysis.

use std::sync::Arc;

use tauri::State;

mod cache;
mod context;
mod provider;
pub mod queue;
mod redaction;
mod schema;
mod service;

pub use context::{
    AnalysisContext, AnalysisContextBuilder, AnalysisSection, AnalysisSectionKind, AnalysisSource,
    ContextBuildError, ContextBuildErrorCode, OmittedSection,
};
pub use provider::{
    AiProvider, AiProviderConfig, AiProviderIdentity, AiProviderKind, AnalysisProviderError,
    AnalysisProviderErrorCode, AnalysisRequest, HttpAiProvider, ProviderResponse,
};
pub use queue::{
    analyze_skill, AnalysisEnqueueResult, AnalysisJobStatus, AnalysisProgress,
    AnalysisProgressSink, AnalysisQueue, NoopAnalysisProgressSink, TauriAnalysisProgressSink,
};
pub use redaction::{redact_context, RedactionCounts};
pub use schema::{
    skill_passport_schema, validate_passport, AnalysisOutcomeStatus, Confidence, EvidenceRef,
    PassportValidationError, PassportValidationErrorCode, ResourceSummary, RiskItem, RiskSeverity,
    SkillPassport, ValidatedPassport,
};
pub use service::{
    AnalysisResult, AnalysisRunStatus, AnalysisService, AnalysisServiceError,
    AnalysisServiceErrorCode, AnalysisView, ComparisonRow, ComparisonSkill, EvidenceExcerpt,
    EvidenceLine, EvidenceLink, SentSection, SkillComparison,
};

pub const PROMPT_VERSION: &str = "m1-s4-prompt-v1";
pub const SCHEMA_VERSION: &str = "m1-s4-passport-v1";
pub const DEFAULT_CONTEXT_BUDGET_CHARS: usize = 16_000;
pub const MAX_PROVIDER_RESPONSE_BYTES: usize = 1024 * 1024;
pub use cache::{
    analysis_key, AnalysisCache, AnalysisCacheError, AnalysisCacheErrorCode, AnalysisRecord,
    AnalysisRecordStatus, SqliteAnalysisCache, UnavailableAnalysisCache,
};

#[tauri::command]
pub fn get_skill_analysis(
    service: State<'_, Arc<AnalysisService>>,
    skill_id: String,
) -> Result<AnalysisView, AnalysisServiceError> {
    service.analysis_view(&skill_id)
}

#[tauri::command]
pub fn read_evidence_excerpt(
    service: State<'_, Arc<AnalysisService>>,
    evidence_id: String,
) -> Result<EvidenceExcerpt, AnalysisServiceError> {
    service.read_evidence_excerpt(&evidence_id)
}

#[tauri::command]
pub fn compare_skills(
    service: State<'_, Arc<AnalysisService>>,
    skill_ids: Vec<String>,
) -> Result<SkillComparison, AnalysisServiceError> {
    service.compare_skills(&skill_ids)
}
