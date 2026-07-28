use std::{
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{AiProviderIdentity, SkillPassport, PROMPT_VERSION, SCHEMA_VERSION};

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AnalysisRecordStatus {
    Ready,
    Stale,
    Failed,
    Degraded,
}

impl AnalysisRecordStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Stale => "stale",
            Self::Failed => "failed",
            Self::Degraded => "degraded",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "ready" => Some(Self::Ready),
            "stale" => Some(Self::Stale),
            "failed" => Some(Self::Failed),
            "degraded" => Some(Self::Degraded),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnalysisRecord {
    pub snapshot_id: String,
    pub analysis_key: String,
    pub status: AnalysisRecordStatus,
    pub passport: Option<SkillPassport>,
    pub provider: String,
    pub model: String,
    pub prompt_version: String,
    pub schema_version: String,
    pub language: String,
    pub created_at: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnalysisCacheErrorCode {
    DatabaseUnavailable,
    InvalidRecord,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AnalysisCacheError {
    pub code: AnalysisCacheErrorCode,
}

pub trait AnalysisCache: Send + Sync {
    fn load(&self, analysis_key: &str) -> Result<Option<AnalysisRecord>, AnalysisCacheError>;
    fn latest_for_snapshot(
        &self,
        snapshot_id: &str,
    ) -> Result<Option<AnalysisRecord>, AnalysisCacheError>;
    fn mark_stale(
        &self,
        snapshot_id: &str,
        current_analysis_key: &str,
    ) -> Result<(), AnalysisCacheError>;
    fn save(&self, record: &AnalysisRecord) -> Result<(), AnalysisCacheError>;
}

#[derive(Clone, Debug)]
pub struct SqliteAnalysisCache {
    database_path: PathBuf,
}

impl SqliteAnalysisCache {
    pub fn new(database_path: PathBuf) -> Self {
        Self { database_path }
    }

    fn open(&self) -> Result<Connection, AnalysisCacheError> {
        let connection =
            Connection::open(&self.database_path).map_err(|_| database_unavailable())?;
        connection
            .execute_batch(
                "
                PRAGMA foreign_keys = ON;
                PRAGMA busy_timeout = 5000;
                ",
            )
            .map_err(|_| database_unavailable())?;
        Ok(connection)
    }
}

impl AnalysisCache for SqliteAnalysisCache {
    fn load(&self, analysis_key: &str) -> Result<Option<AnalysisRecord>, AnalysisCacheError> {
        let connection = self.open()?;
        load_record(
            &connection,
            "
                SELECT snapshot_id, analysis_key, status, passport_json, provider, model,
                       prompt_version, schema_version, language, created_at
                FROM skill_analyses
                WHERE analysis_key = ?1
            ",
            analysis_key,
        )
    }

    fn latest_for_snapshot(
        &self,
        snapshot_id: &str,
    ) -> Result<Option<AnalysisRecord>, AnalysisCacheError> {
        let connection = self.open()?;
        load_record(
            &connection,
            "
                SELECT snapshot_id, analysis_key, status, passport_json, provider, model,
                       prompt_version, schema_version, language, created_at
                FROM skill_analyses
                WHERE snapshot_id = ?1
                  AND passport_json IS NOT NULL
                ORDER BY created_at DESC, analysis_key DESC
                LIMIT 1
            ",
            snapshot_id,
        )
    }

    fn mark_stale(
        &self,
        snapshot_id: &str,
        current_analysis_key: &str,
    ) -> Result<(), AnalysisCacheError> {
        let connection = self.open()?;
        connection
            .execute(
                "
                UPDATE skill_analyses
                SET status = 'stale'
                WHERE snapshot_id = ?1
                  AND analysis_key <> ?2
                  AND status IN ('ready', 'degraded')
                ",
                params![snapshot_id, current_analysis_key],
            )
            .map_err(|_| database_unavailable())?;
        Ok(())
    }

    fn save(&self, record: &AnalysisRecord) -> Result<(), AnalysisCacheError> {
        let mut connection = self.open()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| database_unavailable())?;
        if matches!(
            record.status,
            AnalysisRecordStatus::Ready | AnalysisRecordStatus::Degraded
        ) {
            transaction
                .execute(
                    "
                    UPDATE skill_analyses
                    SET status = 'stale'
                    WHERE snapshot_id = ?1
                      AND analysis_key <> ?2
                      AND status IN ('ready', 'degraded')
                    ",
                    params![record.snapshot_id, record.analysis_key],
                )
                .map_err(|_| database_unavailable())?;
        }
        let passport_json = record
            .passport
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|_| invalid_record())?;
        let evidence_json = record
            .passport
            .as_ref()
            .map(|passport| serde_json::to_string(&passport.evidence_refs))
            .transpose()
            .map_err(|_| invalid_record())?;
        transaction
            .execute(
                "
                INSERT INTO skill_analyses (
                    id, snapshot_id, analysis_key, status, passport_json, evidence_json,
                    provider, model, prompt_version, schema_version, language, created_at
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
                ON CONFLICT(analysis_key) DO NOTHING
                ",
                params![
                    format!("analysis:{}", record.analysis_key),
                    record.snapshot_id,
                    record.analysis_key,
                    record.status.as_str(),
                    passport_json,
                    evidence_json,
                    record.provider,
                    record.model,
                    record.prompt_version,
                    record.schema_version,
                    record.language,
                    record.created_at,
                ],
            )
            .map_err(|_| database_unavailable())?;
        transaction.commit().map_err(|_| database_unavailable())
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct UnavailableAnalysisCache;

impl AnalysisCache for UnavailableAnalysisCache {
    fn load(&self, _analysis_key: &str) -> Result<Option<AnalysisRecord>, AnalysisCacheError> {
        Err(database_unavailable())
    }

    fn latest_for_snapshot(
        &self,
        _snapshot_id: &str,
    ) -> Result<Option<AnalysisRecord>, AnalysisCacheError> {
        Err(database_unavailable())
    }

    fn mark_stale(
        &self,
        _snapshot_id: &str,
        _current_analysis_key: &str,
    ) -> Result<(), AnalysisCacheError> {
        Err(database_unavailable())
    }

    fn save(&self, _record: &AnalysisRecord) -> Result<(), AnalysisCacheError> {
        Err(database_unavailable())
    }
}

pub fn analysis_key(
    content_hash: &str,
    parser_version: &str,
    identity: &AiProviderIdentity,
) -> String {
    let mut hasher = Sha256::new();
    for field in [
        content_hash,
        parser_version,
        PROMPT_VERSION,
        SCHEMA_VERSION,
        identity.provider.as_str(),
        identity.model.as_str(),
        identity.language.as_str(),
    ] {
        hasher.update((field.len() as u64).to_le_bytes());
        hasher.update(field.as_bytes());
    }
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub(crate) fn new_record(
    snapshot_id: String,
    analysis_key: String,
    status: AnalysisRecordStatus,
    passport: Option<SkillPassport>,
    identity: &AiProviderIdentity,
) -> AnalysisRecord {
    AnalysisRecord {
        snapshot_id,
        analysis_key,
        status,
        passport,
        provider: identity.provider.clone(),
        model: identity.model.clone(),
        prompt_version: PROMPT_VERSION.to_owned(),
        schema_version: SCHEMA_VERSION.to_owned(),
        language: identity.language.clone(),
        created_at: now_ms(),
    }
}

fn load_record(
    connection: &Connection,
    sql: &str,
    parameter: &str,
) -> Result<Option<AnalysisRecord>, AnalysisCacheError> {
    let stored = connection
        .query_row(sql, params![parameter], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, String>(8)?,
                row.get::<_, i64>(9)?,
            ))
        })
        .optional()
        .map_err(|_| database_unavailable())?;
    let Some((
        snapshot_id,
        analysis_key,
        status,
        passport_json,
        provider,
        model,
        prompt_version,
        schema_version,
        language,
        created_at,
    )) = stored
    else {
        return Ok(None);
    };
    let status = AnalysisRecordStatus::parse(&status).ok_or_else(invalid_record)?;
    let passport = passport_json
        .map(|value| serde_json::from_str::<SkillPassport>(&value))
        .transpose()
        .map_err(|_| invalid_record())?;
    Ok(Some(AnalysisRecord {
        snapshot_id,
        analysis_key,
        status,
        passport,
        provider: provider.unwrap_or_default(),
        model: model.unwrap_or_default(),
        prompt_version,
        schema_version,
        language,
        created_at,
    }))
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .unwrap_or_default()
}

const fn database_unavailable() -> AnalysisCacheError {
    AnalysisCacheError {
        code: AnalysisCacheErrorCode::DatabaseUnavailable,
    }
}

const fn invalid_record() -> AnalysisCacheError {
    AnalysisCacheError {
        code: AnalysisCacheErrorCode::InvalidRecord,
    }
}

#[cfg(test)]
mod tests {
    use rusqlite::Connection;
    use tempfile::TempDir;

    use crate::{
        analysis::{
            analysis_key, cache::new_record, AiProviderIdentity, AnalysisCache,
            AnalysisRecordStatus, Confidence, SkillPassport, SqliteAnalysisCache,
        },
        db::{self, AppDatabase},
    };

    fn identity(model: &str) -> AiProviderIdentity {
        AiProviderIdentity {
            provider: "provider".to_owned(),
            model: model.to_owned(),
            language: "en".to_owned(),
        }
    }

    fn passport(summary: &str) -> SkillPassport {
        SkillPassport {
            summary: summary.to_owned(),
            capabilities: vec!["Review".to_owned()],
            trigger_examples: vec!["Review this".to_owned()],
            suitable_when: vec!["Review is needed".to_owned()],
            avoid_when: vec!["No source".to_owned()],
            workflow: vec!["Read facts".to_owned()],
            prerequisites: vec!["Parsed Skill".to_owned()],
            resources: Vec::new(),
            side_effects: vec!["No writes".to_owned()],
            risks: Vec::new(),
            related_hints: vec!["Compare results".to_owned()],
            confidence: Confidence::High,
            evidence_refs: Vec::new(),
            uncertainties: vec!["Runtime is not executed".to_owned()],
        }
    }

    fn cache_fixture() -> (TempDir, SqliteAnalysisCache) {
        let temporary = TempDir::new().unwrap();
        let path = temporary.path().join("data.db");
        assert!(matches!(
            db::initialize(path.clone()),
            AppDatabase::Ready(_)
        ));
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "
                PRAGMA foreign_keys = ON;
                INSERT INTO providers (
                    id, kind, root_path, display_name, read_only, capabilities_json, last_scan_at
                ) VALUES ('provider', 'user', 'managed', 'Provider', 1, '{}', 0);
                INSERT INTO skills (
                    id, provider_id, relative_path, display_name, scope, lifecycle_state,
                    latest_snapshot_id, first_seen_at, last_seen_at
                ) VALUES ('skill', 'provider', 'skill', 'Skill', 'user', 'active', NULL, 0, 0);
                INSERT INTO artifact_snapshots (
                    id, skill_id, content_hash, parser_version, manifest_json, resources_json,
                    diagnostics_json, created_at
                ) VALUES ('snapshot', 'skill', 'content', 'parser', '{}', '[]', '[]', 0);
                UPDATE skills SET latest_snapshot_id = 'snapshot' WHERE id = 'skill';
                ",
            )
            .unwrap();
        (temporary, SqliteAnalysisCache::new(path))
    }

    #[test]
    fn analysis_keys_are_deterministic_and_fixed_length() {
        let key = analysis_key("content", "parser", &identity("model"));

        assert_eq!(key, analysis_key("content", "parser", &identity("model")));
        assert_eq!(key.len(), 64);
    }

    #[test]
    fn model_changes_produce_a_new_analysis_key() {
        assert_ne!(
            analysis_key("content", "parser", &identity("model-a")),
            analysis_key("content", "parser", &identity("model-b"))
        );
    }

    #[test]
    fn field_boundaries_cannot_collide_in_analysis_keys() {
        let left = AiProviderIdentity {
            provider: "ab".to_owned(),
            model: "c".to_owned(),
            language: "en".to_owned(),
        };
        let right = AiProviderIdentity {
            provider: "a".to_owned(),
            model: "bc".to_owned(),
            language: "en".to_owned(),
        };

        assert_ne!(
            analysis_key("content", "parser", &left),
            analysis_key("content", "parser", &right)
        );
    }

    #[test]
    fn ready_records_round_trip_without_raw_provider_output() {
        let (_temporary, cache) = cache_fixture();
        let key = analysis_key("content", "parser", &identity("model"));
        let record = new_record(
            "snapshot".to_owned(),
            key.clone(),
            AnalysisRecordStatus::Ready,
            Some(passport("safe")),
            &identity("model"),
        );
        cache.save(&record).unwrap();

        assert_eq!(cache.load(&key).unwrap(), Some(record));
    }

    #[test]
    fn new_identity_marks_previous_success_as_stale() {
        let (_temporary, cache) = cache_fixture();
        let first_key = analysis_key("content", "parser", &identity("model-a"));
        let second_key = analysis_key("content", "parser", &identity("model-b"));
        cache
            .save(&new_record(
                "snapshot".to_owned(),
                first_key.clone(),
                AnalysisRecordStatus::Ready,
                Some(passport("first")),
                &identity("model-a"),
            ))
            .unwrap();
        cache
            .save(&new_record(
                "snapshot".to_owned(),
                second_key,
                AnalysisRecordStatus::Ready,
                Some(passport("second")),
                &identity("model-b"),
            ))
            .unwrap();

        assert_eq!(
            cache.load(&first_key).unwrap().unwrap().status,
            AnalysisRecordStatus::Stale
        );
    }

    #[test]
    fn the_same_analysis_key_is_not_silently_overwritten() {
        let (_temporary, cache) = cache_fixture();
        let key = analysis_key("content", "parser", &identity("model"));
        cache
            .save(&new_record(
                "snapshot".to_owned(),
                key.clone(),
                AnalysisRecordStatus::Ready,
                Some(passport("first")),
                &identity("model"),
            ))
            .unwrap();
        cache
            .save(&new_record(
                "snapshot".to_owned(),
                key.clone(),
                AnalysisRecordStatus::Ready,
                Some(passport("second")),
                &identity("model"),
            ))
            .unwrap();

        assert_eq!(
            cache.load(&key).unwrap().unwrap().passport.unwrap().summary,
            "first"
        );
    }

    #[test]
    fn invalid_stored_passport_is_reported_without_exposing_it() {
        let (temporary, cache) = cache_fixture();
        let connection = Connection::open(temporary.path().join("data.db")).unwrap();
        connection
            .execute(
                "
                INSERT INTO skill_analyses (
                    id, snapshot_id, analysis_key, status, passport_json, evidence_json,
                    provider, model, prompt_version, schema_version, language, created_at
                ) VALUES ('id', 'snapshot', 'bad', 'ready', 'not-json', '[]',
                          'provider', 'model', 'prompt', 'schema', 'en', 0)
                ",
                [],
            )
            .unwrap();
        let error = cache.load("bad").unwrap_err();

        assert_eq!(
            error.code,
            crate::analysis::AnalysisCacheErrorCode::InvalidRecord
        );
        assert!(!format!("{error:?}").contains("not-json"));
    }
}
