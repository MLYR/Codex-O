use schemars::{schema_for, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::AnalysisContext;

const MAX_SUMMARY_CHARS: usize = 500;
const MAX_ITEM_CHARS: usize = 300;
const MAX_ITEMS: usize = 32;
const MAX_EVIDENCE_REFS: usize = 64;

#[derive(Clone, Copy, Debug, Deserialize, JsonSchema, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    High,
    Medium,
    Low,
}

#[derive(Clone, Copy, Debug, Deserialize, JsonSchema, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RiskSeverity {
    Low,
    Medium,
    High,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ResourceSummary {
    pub relative_path: String,
    pub kind: String,
    pub summary: String,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct RiskItem {
    pub category: String,
    pub severity: RiskSeverity,
    pub description: String,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct EvidenceRef {
    pub section_id: String,
    pub relative_path: String,
    pub line_start: usize,
    pub line_end: usize,
}

#[derive(Clone, Debug, Deserialize, JsonSchema, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SkillPassport {
    pub summary: String,
    pub capabilities: Vec<String>,
    pub trigger_examples: Vec<String>,
    pub suitable_when: Vec<String>,
    pub avoid_when: Vec<String>,
    pub workflow: Vec<String>,
    pub prerequisites: Vec<String>,
    pub resources: Vec<ResourceSummary>,
    pub side_effects: Vec<String>,
    pub risks: Vec<RiskItem>,
    pub related_hints: Vec<String>,
    pub confidence: Confidence,
    pub evidence_refs: Vec<EvidenceRef>,
    pub uncertainties: Vec<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AnalysisOutcomeStatus {
    Ready,
    Degraded,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct ValidatedPassport {
    pub status: AnalysisOutcomeStatus,
    pub passport: SkillPassport,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PassportValidationErrorCode {
    InvalidJson,
    InvalidLength,
    InvalidEvidence,
    InvalidRelativePath,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PassportValidationError {
    pub code: PassportValidationErrorCode,
}

pub fn skill_passport_schema() -> Value {
    serde_json::to_value(schema_for!(SkillPassport)).expect("SkillPassport schema serializes")
}

pub fn validate_passport(
    raw: &str,
    context: &AnalysisContext,
) -> Result<ValidatedPassport, PassportValidationError> {
    let passport =
        serde_json::from_str::<SkillPassport>(raw).map_err(|_| PassportValidationError {
            code: PassportValidationErrorCode::InvalidJson,
        })?;
    validate_lengths(&passport)?;
    validate_evidence(&passport, context)?;
    validate_resource_paths(&passport)?;
    let status = if passport.evidence_refs.is_empty() || passport.uncertainties.is_empty() {
        AnalysisOutcomeStatus::Degraded
    } else {
        AnalysisOutcomeStatus::Ready
    };
    Ok(ValidatedPassport { status, passport })
}

fn validate_lengths(passport: &SkillPassport) -> Result<(), PassportValidationError> {
    if passport.summary.is_empty() || passport.summary.chars().count() > MAX_SUMMARY_CHARS {
        return Err(invalid_length());
    }
    for values in [
        &passport.capabilities,
        &passport.trigger_examples,
        &passport.suitable_when,
        &passport.avoid_when,
        &passport.workflow,
        &passport.prerequisites,
        &passport.side_effects,
        &passport.related_hints,
        &passport.uncertainties,
    ] {
        validate_string_array(values)?;
    }
    if passport.resources.len() > MAX_ITEMS
        || passport.risks.len() > MAX_ITEMS
        || passport.evidence_refs.len() > MAX_EVIDENCE_REFS
    {
        return Err(invalid_length());
    }
    for resource in &passport.resources {
        validate_text(&resource.kind)?;
        validate_text(&resource.summary)?;
    }
    for risk in &passport.risks {
        validate_text(&risk.category)?;
        validate_text(&risk.description)?;
    }
    Ok(())
}

fn validate_string_array(values: &[String]) -> Result<(), PassportValidationError> {
    if values.len() > MAX_ITEMS {
        return Err(invalid_length());
    }
    for value in values {
        validate_text(value)?;
    }
    Ok(())
}

fn validate_text(value: &str) -> Result<(), PassportValidationError> {
    if value.is_empty() || value.chars().count() > MAX_ITEM_CHARS {
        Err(invalid_length())
    } else {
        Ok(())
    }
}

fn validate_evidence(
    passport: &SkillPassport,
    context: &AnalysisContext,
) -> Result<(), PassportValidationError> {
    for evidence in &passport.evidence_refs {
        let valid = context.sections.iter().any(|section| {
            section.id == evidence.section_id
                && section.relative_path == evidence.relative_path
                && evidence.line_start >= section.line_start
                && evidence.line_end <= section.line_end
                && evidence.line_start <= evidence.line_end
        });
        if !valid {
            return Err(PassportValidationError {
                code: PassportValidationErrorCode::InvalidEvidence,
            });
        }
    }
    Ok(())
}

fn validate_resource_paths(passport: &SkillPassport) -> Result<(), PassportValidationError> {
    if passport
        .resources
        .iter()
        .any(|resource| !is_safe_relative_path(&resource.relative_path))
    {
        Err(PassportValidationError {
            code: PassportValidationErrorCode::InvalidRelativePath,
        })
    } else {
        Ok(())
    }
}

fn is_safe_relative_path(value: &str) -> bool {
    !value.is_empty()
        && !value.starts_with('/')
        && !value.contains('\\')
        && value
            .split('/')
            .all(|part| !matches!(part, "" | "." | ".."))
}

const fn invalid_length() -> PassportValidationError {
    PassportValidationError {
        code: PassportValidationErrorCode::InvalidLength,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::analysis::{
        AnalysisContext, AnalysisOutcomeStatus, AnalysisSection, AnalysisSectionKind,
    };

    use super::{
        skill_passport_schema, validate_passport, PassportValidationErrorCode, SkillPassport,
    };

    fn context() -> AnalysisContext {
        AnalysisContext {
            skill_id: "skill".to_owned(),
            content_hash: "hash".to_owned(),
            parser_version: "parser".to_owned(),
            sections: vec![AnalysisSection {
                id: "section-1".to_owned(),
                kind: AnalysisSectionKind::Overview,
                relative_path: "SKILL.md".to_owned(),
                line_start: 10,
                line_end: 20,
                title: "Overview".to_owned(),
                content: "evidence".to_owned(),
            }],
            omitted_sections: Vec::new(),
            used_chars: 8,
            budget_chars: 16_000,
        }
    }

    fn passport_value() -> serde_json::Value {
        json!({
            "summary": "A safe summary.",
            "capabilities": ["Review code"],
            "triggerExamples": ["Review this patch"],
            "suitableWhen": ["A patch needs review"],
            "avoidWhen": ["No source is available"],
            "workflow": ["Read deterministic facts"],
            "prerequisites": ["A parsed Skill"],
            "resources": [{
                "relativePath": "references/guide.md",
                "kind": "reference",
                "summary": "Review guidance"
            }],
            "sideEffects": ["No writes"],
            "risks": [{
                "category": "privacy",
                "severity": "low",
                "description": "Selected text leaves the device only when configured"
            }],
            "relatedHints": ["Compare with another reviewer"],
            "confidence": "high",
            "evidenceRefs": [{
                "sectionId": "section-1",
                "relativePath": "SKILL.md",
                "lineStart": 10,
                "lineEnd": 12
            }],
            "uncertainties": ["Runtime tools are not executed"]
        })
    }

    #[test]
    fn complete_passports_are_ready() {
        let result = validate_passport(&passport_value().to_string(), &context()).unwrap();

        assert_eq!(result.status, AnalysisOutcomeStatus::Ready);
    }

    #[test]
    fn missing_required_fields_are_rejected() {
        let mut value = passport_value();
        value.as_object_mut().unwrap().remove("uncertainties");
        let error = validate_passport(&value.to_string(), &context()).unwrap_err();

        assert_eq!(error.code, PassportValidationErrorCode::InvalidJson);
    }

    #[test]
    fn unknown_fields_are_rejected() {
        let mut value = passport_value();
        value["inventedField"] = json!(true);
        let error = validate_passport(&value.to_string(), &context()).unwrap_err();

        assert_eq!(error.code, PassportValidationErrorCode::InvalidJson);
    }

    #[test]
    fn invalid_confidence_values_are_rejected() {
        let mut value = passport_value();
        value["confidence"] = json!("certain");
        let error = validate_passport(&value.to_string(), &context()).unwrap_err();

        assert_eq!(error.code, PassportValidationErrorCode::InvalidJson);
    }

    #[test]
    fn evidence_must_match_a_sent_section_and_line_range() {
        let mut value = passport_value();
        value["evidenceRefs"][0]["lineEnd"] = json!(21);
        let error = validate_passport(&value.to_string(), &context()).unwrap_err();

        assert_eq!(error.code, PassportValidationErrorCode::InvalidEvidence);
    }

    #[test]
    fn empty_evidence_is_degraded_instead_of_ready() {
        let mut value = passport_value();
        value["evidenceRefs"] = json!([]);
        let result = validate_passport(&value.to_string(), &context()).unwrap();

        assert_eq!(result.status, AnalysisOutcomeStatus::Degraded);
    }

    #[test]
    fn absent_uncertainty_content_is_degraded() {
        let mut value = passport_value();
        value["uncertainties"] = json!([]);
        let result = validate_passport(&value.to_string(), &context()).unwrap();

        assert_eq!(result.status, AnalysisOutcomeStatus::Degraded);
    }

    #[test]
    fn oversized_summary_is_rejected() {
        let mut value = passport_value();
        value["summary"] = json!("x".repeat(501));
        let error = validate_passport(&value.to_string(), &context()).unwrap_err();

        assert_eq!(error.code, PassportValidationErrorCode::InvalidLength);
    }

    #[test]
    fn unsafe_resource_paths_are_rejected() {
        let mut value = passport_value();
        value["resources"][0]["relativePath"] = json!("/Users/private/file");
        let error = validate_passport(&value.to_string(), &context()).unwrap_err();

        assert_eq!(error.code, PassportValidationErrorCode::InvalidRelativePath);
    }

    #[test]
    fn generated_schema_requires_all_passport_fields_and_forbids_extras() {
        let schema = skill_passport_schema();
        let encoded = schema.to_string();

        assert!(encoded.contains("\"additionalProperties\":false"));
        assert!(encoded.contains("\"uncertainties\""));
        assert!(encoded.contains("\"evidenceRefs\""));
    }

    #[test]
    fn validated_passport_serialization_contains_no_unmodeled_fields() {
        let result = validate_passport(&passport_value().to_string(), &context()).unwrap();
        let encoded = serde_json::to_string(&result).unwrap();
        let reparsed = serde_json::from_str::<SkillPassport>(
            serde_json::to_value(&result.passport)
                .unwrap()
                .to_string()
                .as_str(),
        )
        .unwrap();

        assert_eq!(reparsed, result.passport);
        assert!(!encoded.contains("inventedField"));
    }
}
