use std::{path::Path, sync::OnceLock};

use regex::Regex;
use serde::{Deserialize, Serialize};

use super::AnalysisContext;

const API_KEY_MARKER: &str = "[REDACTED:API_KEY]";
const AUTHORIZATION_MARKER: &str = "[REDACTED:AUTHORIZATION]";
const PRIVATE_KEY_MARKER: &str = "[REDACTED:PRIVATE_KEY]";
const SECRET_FIELD_MARKER: &str = "[REDACTED:SECRET_FIELD]";
const HOME_PATH_MARKER: &str = "[REDACTED:HOME_PATH]";

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
pub struct RedactionCounts {
    pub api_keys: u32,
    pub authorization_headers: u32,
    pub private_keys: u32,
    pub secret_fields: u32,
    pub home_paths: u32,
}

pub fn redact_context(
    mut context: AnalysisContext,
    home_directory: Option<&Path>,
) -> (AnalysisContext, RedactionCounts) {
    let mut counts = RedactionCounts::default();
    for section in &mut context.sections {
        section.content = redact_text(&section.content, home_directory, &mut counts);
    }
    (context, counts)
}

fn redact_text(value: &str, home_directory: Option<&Path>, counts: &mut RedactionCounts) -> String {
    let value = replace_regex(
        private_key_pattern(),
        value,
        PRIVATE_KEY_MARKER,
        &mut counts.private_keys,
    );
    let value = replace_regex(
        authorization_pattern(),
        &value,
        AUTHORIZATION_MARKER,
        &mut counts.authorization_headers,
    );
    let value = replace_regex(
        secret_field_pattern(),
        &value,
        SECRET_FIELD_MARKER,
        &mut counts.secret_fields,
    );
    let mut value = replace_regex(
        api_key_pattern(),
        &value,
        API_KEY_MARKER,
        &mut counts.api_keys,
    );
    if let Some(home) = home_directory.and_then(Path::to_str) {
        if !home.is_empty() {
            counts.home_paths += value.matches(home).count() as u32;
            value = value.replace(home, HOME_PATH_MARKER);
        }
    }
    value
}

fn replace_regex(pattern: &Regex, value: &str, marker: &str, count: &mut u32) -> String {
    *count += pattern.find_iter(value).count() as u32;
    pattern.replace_all(value, marker).into_owned()
}

fn api_key_pattern() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| {
        Regex::new(r"(?i)\b(?:sk|rk|pk|api|xox)[-_][a-z0-9_-]{16,}\b")
            .expect("constant API key regex")
    })
}

fn authorization_pattern() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| {
        Regex::new(r"(?i)authorization\s*:\s*[^\r\n]+").expect("constant Authorization regex")
    })
}

fn private_key_pattern() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| {
        Regex::new(r"(?s)-----BEGIN [^-\r\n]*PRIVATE KEY-----.*?-----END [^-\r\n]*PRIVATE KEY-----")
            .expect("constant private key regex")
    })
}

fn secret_field_pattern() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| {
        Regex::new(
            r#"(?i)\b(?:api[_-]?key|token|secret|password)\b["']?\s*[:=]\s*["']?[^\s"',;}]+"#,
        )
        .expect("constant secret field regex")
    })
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::redact_context;
    use crate::analysis::{AnalysisContext, AnalysisSection, AnalysisSectionKind, OmittedSection};

    fn context_with(content: &str) -> AnalysisContext {
        AnalysisContext {
            skill_id: "skill".to_owned(),
            content_hash: "hash".to_owned(),
            parser_version: "parser".to_owned(),
            sections: vec![AnalysisSection {
                id: "section".to_owned(),
                kind: AnalysisSectionKind::Overview,
                relative_path: "SKILL.md".to_owned(),
                line_start: 1,
                line_end: 1,
                title: "Overview".to_owned(),
                content: content.to_owned(),
            }],
            omitted_sections: Vec::<OmittedSection>::new(),
            used_chars: content.chars().count(),
            budget_chars: 16_000,
        }
    }

    #[test]
    fn obvious_api_keys_are_replaced_and_counted() {
        let (context, counts) = redact_context(context_with("use sk-abcdefghijklmnopqrst"), None);

        assert_eq!(counts.api_keys, 1);
        assert!(context.sections[0].content.contains("[REDACTED:API_KEY]"));
        assert!(!context.sections[0].content.contains("abcdefghijkl"));
    }

    #[test]
    fn authorization_headers_are_replaced_and_counted() {
        let (context, counts) =
            redact_context(context_with("Authorization: Bearer sensitive-value"), None);

        assert_eq!(counts.authorization_headers, 1);
        assert!(!context.sections[0].content.contains("sensitive-value"));
    }

    #[test]
    fn private_key_blocks_are_replaced_and_counted() {
        let source = "-----BEGIN PRIVATE KEY-----\nsecret\n-----END PRIVATE KEY-----";
        let (context, counts) = redact_context(context_with(source), None);

        assert_eq!(counts.private_keys, 1);
        assert!(!context.sections[0].content.contains("secret"));
    }

    #[test]
    fn named_secret_fields_are_replaced_and_counted() {
        let (context, counts) = redact_context(context_with("token=fixture-sensitive"), None);

        assert_eq!(counts.secret_fields, 1);
        assert!(!context.sections[0].content.contains("fixture-sensitive"));
    }

    #[test]
    fn json_secret_fields_are_replaced_and_counted() {
        let (context, counts) = redact_context(
            context_with(r#"{"api_key":"fixture-sensitive","safe":"visible"}"#),
            None,
        );

        assert_eq!(counts.secret_fields, 1);
        assert!(!context.sections[0].content.contains("fixture-sensitive"));
        assert!(context.sections[0].content.contains("\"safe\":\"visible\""));
    }

    #[test]
    fn injected_home_paths_are_replaced_and_counted() {
        let (context, counts) = redact_context(
            context_with("read /Users/private-user/.config/file"),
            Some(Path::new("/Users/private-user")),
        );

        assert_eq!(counts.home_paths, 1);
        assert!(!context.sections[0].content.contains("/Users/private-user"));
    }

    #[test]
    fn redacted_context_serialization_contains_counts_but_no_original_values() {
        let (context, counts) = redact_context(
            context_with("password='fixture-sensitive'"),
            Some(Path::new("/Users/private-user")),
        );
        let encoded = serde_json::to_string(&(context, counts)).unwrap();

        assert!(!encoded.contains("fixture-sensitive"));
        assert!(!encoded.contains("/Users/private-user"));
        assert!(encoded.contains("secret_fields"));
    }
}
