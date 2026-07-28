use std::path::{Component, Path};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::parsing::ArtifactSnapshot;

use super::DEFAULT_CONTEXT_BUDGET_CHARS;

const MIN_CONTEXT_BUDGET_CHARS: usize = 256;
const SKILL_MARKDOWN_PATH: &str = "SKILL.md";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnalysisSource {
    pub relative_path: String,
    pub content: String,
}

impl AnalysisSource {
    pub fn new(
        relative_path: impl Into<String>,
        content: impl Into<String>,
    ) -> Result<Self, ContextBuildError> {
        let relative_path = relative_path.into();
        if !is_safe_relative_path(&relative_path) {
            return Err(ContextBuildError {
                code: ContextBuildErrorCode::InvalidRelativePath,
            });
        }
        Ok(Self {
            relative_path,
            content: content.into(),
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContextBuildErrorCode {
    BudgetTooSmall,
    InvalidRelativePath,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ContextBuildError {
    pub code: ContextBuildErrorCode,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AnalysisSectionKind {
    Manifest,
    Overview,
    Prerequisite,
    Reference,
    ScriptSummary,
    ResourceManifest,
    Diagnostic,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct AnalysisSection {
    pub id: String,
    pub kind: AnalysisSectionKind,
    pub relative_path: String,
    pub line_start: usize,
    pub line_end: usize,
    pub title: String,
    pub content: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct OmittedSection {
    pub relative_path: String,
    pub line_start: usize,
    pub line_end: usize,
    pub title: String,
    pub reason: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct AnalysisContext {
    pub skill_id: String,
    pub content_hash: String,
    pub parser_version: String,
    pub sections: Vec<AnalysisSection>,
    pub omitted_sections: Vec<OmittedSection>,
    pub used_chars: usize,
    pub budget_chars: usize,
}

#[derive(Clone, Copy, Debug)]
pub struct AnalysisContextBuilder {
    budget_chars: usize,
}

impl AnalysisContextBuilder {
    pub const fn new(budget_chars: usize) -> Self {
        Self { budget_chars }
    }

    pub fn build(
        &self,
        snapshot: &ArtifactSnapshot,
        sources: &[AnalysisSource],
    ) -> Result<AnalysisContext, ContextBuildError> {
        if self.budget_chars < MIN_CONTEXT_BUDGET_CHARS {
            return Err(ContextBuildError {
                code: ContextBuildErrorCode::BudgetTooSmall,
            });
        }
        if sources
            .iter()
            .any(|source| !is_safe_relative_path(&source.relative_path))
        {
            return Err(ContextBuildError {
                code: ContextBuildErrorCode::InvalidRelativePath,
            });
        }

        let mut candidates = Vec::new();
        candidates.push(manifest_candidate(snapshot));
        let skill_source = sources
            .iter()
            .find(|source| source.relative_path == SKILL_MARKDOWN_PATH);
        if let Some(source) = skill_source {
            candidates.extend(markdown_candidates(snapshot, source));
        }
        candidates.extend(reference_candidates(sources, skill_source));
        candidates.extend(resource_candidates(snapshot));
        candidates.extend(diagnostic_candidates(snapshot));
        candidates.sort_by(|left, right| {
            left.priority
                .cmp(&right.priority)
                .then_with(|| left.relative_path.cmp(&right.relative_path))
                .then_with(|| left.line_start.cmp(&right.line_start))
        });

        let mut used_chars = 0;
        let mut sections = Vec::new();
        let mut omitted_sections = Vec::new();
        for candidate in candidates {
            if !candidate.selected {
                omitted_sections.push(candidate.omitted("low_priority"));
                continue;
            }
            if used_chars + candidate.content.chars().count() > self.budget_chars {
                omitted_sections.push(candidate.omitted("budget_exceeded"));
                continue;
            }
            used_chars += candidate.content.chars().count();
            sections.push(candidate.into_section());
        }

        Ok(AnalysisContext {
            skill_id: snapshot.skill_id.clone(),
            content_hash: snapshot.content_hash.clone(),
            parser_version: snapshot.parser_version.to_owned(),
            sections,
            omitted_sections,
            used_chars,
            budget_chars: self.budget_chars,
        })
    }
}

impl Default for AnalysisContextBuilder {
    fn default() -> Self {
        Self::new(DEFAULT_CONTEXT_BUDGET_CHARS)
    }
}

#[derive(Clone, Debug)]
struct Candidate {
    priority: u8,
    selected: bool,
    kind: AnalysisSectionKind,
    relative_path: String,
    line_start: usize,
    line_end: usize,
    title: String,
    content: String,
}

impl Candidate {
    fn into_section(self) -> AnalysisSection {
        let id = section_id(
            &self.relative_path,
            self.line_start,
            self.line_end,
            self.kind,
        );
        AnalysisSection {
            id,
            kind: self.kind,
            relative_path: self.relative_path,
            line_start: self.line_start,
            line_end: self.line_end,
            title: self.title,
            content: self.content,
        }
    }

    fn omitted(&self, reason: &'static str) -> OmittedSection {
        OmittedSection {
            relative_path: self.relative_path.clone(),
            line_start: self.line_start,
            line_end: self.line_end,
            title: self.title.clone(),
            reason: reason.to_owned(),
        }
    }
}

fn manifest_candidate(snapshot: &ArtifactSnapshot) -> Candidate {
    let content = serde_json::json!({
        "frontmatter": snapshot.frontmatter,
        "openai_manifest": snapshot.openai_manifest,
    })
    .to_string();
    Candidate {
        priority: 0,
        selected: true,
        kind: AnalysisSectionKind::Manifest,
        relative_path: SKILL_MARKDOWN_PATH.to_owned(),
        line_start: 1,
        line_end: 1,
        title: "Manifest".to_owned(),
        content,
    }
}

fn markdown_candidates(snapshot: &ArtifactSnapshot, source: &AnalysisSource) -> Vec<Candidate> {
    let lines = source.content.lines().collect::<Vec<_>>();
    snapshot
        .headings
        .iter()
        .enumerate()
        .map(|(index, heading)| {
            let line_end = snapshot
                .headings
                .get(index + 1)
                .map(|next| next.line_start.saturating_sub(1))
                .unwrap_or(lines.len())
                .max(heading.line_end);
            let content = lines
                .get(heading.line_start.saturating_sub(1)..line_end.min(lines.len()))
                .unwrap_or_default()
                .join("\n");
            let (priority, kind, selected) = heading_classification(&heading.text);
            Candidate {
                priority,
                selected,
                kind,
                relative_path: source.relative_path.clone(),
                line_start: heading.line_start,
                line_end,
                title: heading.text.clone(),
                content,
            }
        })
        .collect()
}

fn heading_classification(title: &str) -> (u8, AnalysisSectionKind, bool) {
    let normalized = title.to_ascii_lowercase();
    if [
        "overview",
        "about",
        "usage",
        "workflow",
        "when to use",
        "purpose",
        "trigger",
        "用途",
        "流程",
        "何时使用",
        "触发",
    ]
    .iter()
    .any(|keyword| normalized.contains(keyword))
    {
        (1, AnalysisSectionKind::Overview, true)
    } else if [
        "prerequisite",
        "configuration",
        "setup",
        "requirement",
        "dependency",
        "前置",
        "配置",
        "依赖",
    ]
    .iter()
    .any(|keyword| normalized.contains(keyword))
    {
        (2, AnalysisSectionKind::Prerequisite, true)
    } else {
        (7, AnalysisSectionKind::Overview, false)
    }
}

fn reference_candidates(
    sources: &[AnalysisSource],
    skill_source: Option<&AnalysisSource>,
) -> Vec<Candidate> {
    let skill_text = skill_source
        .map(|source| source.content.as_str())
        .unwrap_or_default();
    sources
        .iter()
        .filter(|source| source.relative_path.starts_with("references/"))
        .map(|source| {
            let file_name = source
                .relative_path
                .rsplit('/')
                .next()
                .unwrap_or(source.relative_path.as_str());
            let selected =
                skill_text.contains(&source.relative_path) || skill_text.contains(file_name);
            Candidate {
                priority: 3,
                selected,
                kind: AnalysisSectionKind::Reference,
                relative_path: source.relative_path.clone(),
                line_start: 1,
                line_end: source.content.lines().count().max(1),
                title: file_name.to_owned(),
                content: source.content.clone(),
            }
        })
        .collect()
}

fn resource_candidates(snapshot: &ArtifactSnapshot) -> Vec<Candidate> {
    let (scripts, resources): (Vec<_>, Vec<_>) = snapshot
        .resources
        .iter()
        .partition(|resource| resource.relative_path.starts_with("scripts/"));
    let mut candidates = Vec::new();
    if !scripts.is_empty() {
        candidates.push(Candidate {
            priority: 4,
            selected: true,
            kind: AnalysisSectionKind::ScriptSummary,
            relative_path: "scripts".to_owned(),
            line_start: 1,
            line_end: scripts.len(),
            title: "Script entries".to_owned(),
            content: serde_json::to_string(&scripts).unwrap_or_else(|_| "[]".to_owned()),
        });
    }
    if !resources.is_empty() {
        candidates.push(Candidate {
            priority: 5,
            selected: true,
            kind: AnalysisSectionKind::ResourceManifest,
            relative_path: "resources".to_owned(),
            line_start: 1,
            line_end: resources.len(),
            title: "Resource manifest".to_owned(),
            content: serde_json::to_string(&resources).unwrap_or_else(|_| "[]".to_owned()),
        });
    }
    candidates
}

fn diagnostic_candidates(snapshot: &ArtifactSnapshot) -> Vec<Candidate> {
    if snapshot.diagnostics.is_empty() {
        return Vec::new();
    }
    vec![Candidate {
        priority: 6,
        selected: true,
        kind: AnalysisSectionKind::Diagnostic,
        relative_path: "diagnostics".to_owned(),
        line_start: 1,
        line_end: snapshot.diagnostics.len(),
        title: "Static diagnostics".to_owned(),
        content: serde_json::to_string(&snapshot.diagnostics).unwrap_or_else(|_| "[]".to_owned()),
    }]
}

fn section_id(
    relative_path: &str,
    line_start: usize,
    line_end: usize,
    kind: AnalysisSectionKind,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(relative_path.as_bytes());
    hasher.update(line_start.to_le_bytes());
    hasher.update(line_end.to_le_bytes());
    hasher.update([kind as u8]);
    let digest = hasher.finalize();
    format!("section-{}", hex_prefix(&digest, 12))
}

fn hex_prefix(bytes: &[u8], length: usize) -> String {
    bytes
        .iter()
        .take(length / 2)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn is_safe_relative_path(value: &str) -> bool {
    if value.is_empty() || value.contains('\\') {
        return false;
    }
    let path = Path::new(value);
    !path.is_absolute()
        && path.components().all(|component| {
            matches!(component, Component::Normal(_))
                && component
                    .as_os_str()
                    .to_str()
                    .is_some_and(|part| part != ".")
        })
}

#[cfg(test)]
mod tests {
    use serde_json::Map;

    use crate::parsing::{
        ArtifactSnapshot, MarkdownHeading, OpenAiManifest, ResourceEntry, SkillFrontmatter,
    };

    use super::{
        AnalysisContextBuilder, AnalysisSectionKind, AnalysisSource, ContextBuildErrorCode,
    };

    fn snapshot() -> ArtifactSnapshot {
        ArtifactSnapshot {
            skill_id: "skill-1".to_owned(),
            content_hash: "hash".to_owned(),
            parser_version: "parser-v1",
            frontmatter: SkillFrontmatter {
                name: Some("Example".to_owned()),
                description: Some("Example description".to_owned()),
                extensions: Map::new(),
            },
            headings: vec![
                MarkdownHeading {
                    level: 1,
                    text: "Overview".to_owned(),
                    line_start: 1,
                    line_end: 1,
                },
                MarkdownHeading {
                    level: 2,
                    text: "Setup".to_owned(),
                    line_start: 4,
                    line_end: 4,
                },
                MarkdownHeading {
                    level: 2,
                    text: "Appendix".to_owned(),
                    line_start: 7,
                    line_end: 7,
                },
            ],
            openai_manifest: Some(OpenAiManifest {
                display_name: Some("Example".to_owned()),
                short_description: None,
                default_prompt: None,
                extensions: Map::new(),
            }),
            resources: vec![
                ResourceEntry {
                    relative_path: "scripts/run.sh".to_owned(),
                    size_bytes: 10,
                    content_hash: "script-hash".to_owned(),
                },
                ResourceEntry {
                    relative_path: "assets/icon.png".to_owned(),
                    size_bytes: 20,
                    content_hash: "asset-hash".to_owned(),
                },
            ],
            diagnostics: Vec::new(),
        }
    }

    fn skill_source() -> AnalysisSource {
        AnalysisSource::new(
            "SKILL.md",
            "# Overview\nUseful overview.\nSee references/guide.md.\n## Setup\nInstall it.\nDone.\n## Appendix\nLow priority.",
        )
        .unwrap()
    }

    #[test]
    fn builder_prioritizes_manifest_overview_and_prerequisites() {
        let context = AnalysisContextBuilder::default()
            .build(&snapshot(), &[skill_source()])
            .unwrap();
        let kinds = context
            .sections
            .iter()
            .map(|section| section.kind)
            .collect::<Vec<_>>();

        assert_eq!(kinds[0], AnalysisSectionKind::Manifest);
        assert!(kinds.contains(&AnalysisSectionKind::Overview));
        assert!(kinds.contains(&AnalysisSectionKind::Prerequisite));
    }

    #[test]
    fn low_priority_markdown_is_recorded_as_omitted() {
        let context = AnalysisContextBuilder::default()
            .build(&snapshot(), &[skill_source()])
            .unwrap();

        assert!(context
            .omitted_sections
            .iter()
            .any(|section| section.title == "Appendix" && section.reason == "low_priority"));
    }

    #[test]
    fn referenced_documents_are_selected_by_structure() {
        let context = AnalysisContextBuilder::default()
            .build(
                &snapshot(),
                &[
                    skill_source(),
                    AnalysisSource::new("references/guide.md", "# Guide\nEvidence").unwrap(),
                ],
            )
            .unwrap();

        assert!(context.sections.iter().any(|section| {
            section.kind == AnalysisSectionKind::Reference
                && section.relative_path == "references/guide.md"
        }));
    }

    #[test]
    fn unreferenced_documents_are_not_sent() {
        let context = AnalysisContextBuilder::default()
            .build(
                &snapshot(),
                &[
                    skill_source(),
                    AnalysisSource::new("references/unused.md", "not selected").unwrap(),
                ],
            )
            .unwrap();

        assert!(context
            .omitted_sections
            .iter()
            .any(|section| section.relative_path == "references/unused.md"));
    }

    #[test]
    fn budget_omits_whole_sections_without_prefix_truncation() {
        let context = AnalysisContextBuilder::new(300)
            .build(&snapshot(), &[skill_source()])
            .unwrap();

        assert!(context.used_chars <= context.budget_chars);
        assert!(context
            .omitted_sections
            .iter()
            .any(|section| section.reason == "budget_exceeded"));
        assert!(context
            .sections
            .iter()
            .all(|section| !section.content.ends_with("Low prior")));
    }

    #[test]
    fn script_content_is_not_sent_but_entry_metadata_is_available() {
        let context = AnalysisContextBuilder::default()
            .build(&snapshot(), &[skill_source()])
            .unwrap();
        let script = context
            .sections
            .iter()
            .find(|section| section.kind == AnalysisSectionKind::ScriptSummary)
            .unwrap();

        assert!(script.content.contains("scripts/run.sh"));
        assert!(!script.content.contains("shell source"));
    }

    #[test]
    fn section_ids_are_stable_for_identical_inputs() {
        let builder = AnalysisContextBuilder::default();
        let left = builder.build(&snapshot(), &[skill_source()]).unwrap();
        let right = builder.build(&snapshot(), &[skill_source()]).unwrap();

        assert_eq!(left.sections, right.sections);
    }

    #[test]
    fn unsafe_or_absolute_source_paths_are_rejected() {
        for path in ["/private/skill.md", "../secret.md", "references\\secret.md"] {
            let error = AnalysisSource::new(path, "content").unwrap_err();
            assert_eq!(error.code, ContextBuildErrorCode::InvalidRelativePath);
        }
    }

    #[test]
    fn tiny_context_budgets_are_rejected() {
        let error = AnalysisContextBuilder::new(32)
            .build(&snapshot(), &[skill_source()])
            .unwrap_err();

        assert_eq!(error.code, ContextBuildErrorCode::BudgetTooSmall);
    }
}
