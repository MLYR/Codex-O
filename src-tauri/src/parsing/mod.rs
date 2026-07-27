//! Deterministic parsing for skill directories returned by the provider registry.

use std::{
    collections::BTreeMap,
    fs,
    io::{self, Read},
    path::{Component, Path},
};

use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use serde::Serialize;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::providers::DiscoveredSkill;

const SKILL_MARKDOWN_FILE: &str = "SKILL.md";
const OPENAI_MANIFEST_FILE: &str = "agents/openai.yaml";
const RESOURCE_DIRECTORIES: [&str; 3] = ["resources", "scripts", "references"];
const MAX_TEXT_BYTES: u64 = 1024 * 1024;
const MAX_RESOURCE_BYTES: u64 = 16 * 1024 * 1024;
pub const PARSER_VERSION: &str = "m1-s2-v1";

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ParseDiagnosticCode {
    EntryUnreadable,
    InputTooLarge,
    InvalidUtf8,
    InvalidFrontmatter,
    InvalidYaml,
    InvalidMarkdown,
    InvalidPath,
    SymlinkDenied,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ParseDiagnostic {
    pub code: ParseDiagnosticCode,
    pub relative_path: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SkillFrontmatter {
    pub name: Option<String>,
    pub description: Option<String>,
    pub extensions: Map<String, Value>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct MarkdownHeading {
    pub level: u8,
    pub text: String,
    pub line_start: usize,
    pub line_end: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct OpenAiManifest {
    pub display_name: Option<String>,
    pub short_description: Option<String>,
    pub default_prompt: Option<String>,
    pub extensions: Map<String, Value>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ResourceEntry {
    pub relative_path: String,
    pub size_bytes: u64,
    pub content_hash: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ArtifactSnapshot {
    pub skill_id: String,
    pub content_hash: String,
    pub parser_version: &'static str,
    pub frontmatter: SkillFrontmatter,
    pub headings: Vec<MarkdownHeading>,
    pub openai_manifest: Option<OpenAiManifest>,
    pub resources: Vec<ResourceEntry>,
    pub diagnostics: Vec<ParseDiagnostic>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ParseResult {
    pub skill_id: String,
    pub snapshot: Option<ArtifactSnapshot>,
    pub diagnostics: Vec<ParseDiagnostic>,
}

pub fn parse_skills(skills: &[DiscoveredSkill]) -> Vec<ParseResult> {
    skills.iter().map(parse_skill).collect()
}

pub fn parse_skill(skill: &DiscoveredSkill) -> ParseResult {
    let mut diagnostics = Vec::new();
    let skill_directory = skill.skill_directory();
    let skill_markdown = skill_directory.join(SKILL_MARKDOWN_FILE);
    let Some(markdown) = read_text_file(&skill_markdown, SKILL_MARKDOWN_FILE, &mut diagnostics)
    else {
        return ParseResult {
            skill_id: skill.id.clone(),
            snapshot: None,
            diagnostics,
        };
    };

    let (frontmatter, markdown_offset) = parse_frontmatter(&markdown, &mut diagnostics);
    let headings = parse_headings(
        &markdown[markdown_offset..],
        markdown[..markdown_offset]
            .bytes()
            .filter(|byte| *byte == b'\n')
            .count(),
        &mut diagnostics,
    );
    let openai_manifest = parse_openai_manifest(skill_directory, &mut diagnostics);
    let resources = collect_resources(skill_directory, &mut diagnostics);
    let content_hash = snapshot_hash(
        skill_directory,
        &resources,
        regular_file_exists(&skill_directory.join(OPENAI_MANIFEST_FILE)),
        &mut diagnostics,
    );

    let snapshot = ArtifactSnapshot {
        skill_id: skill.id.clone(),
        content_hash,
        parser_version: PARSER_VERSION,
        frontmatter,
        headings,
        openai_manifest,
        resources,
        diagnostics: diagnostics.clone(),
    };

    ParseResult {
        skill_id: skill.id.clone(),
        snapshot: Some(snapshot),
        diagnostics,
    }
}

fn read_text_file(
    path: &Path,
    relative_path: &str,
    diagnostics: &mut Vec<ParseDiagnostic>,
) -> Option<String> {
    let metadata = match regular_file_metadata(path, relative_path, diagnostics) {
        Some(metadata) => metadata,
        None => return None,
    };
    if metadata.len() > MAX_TEXT_BYTES {
        diagnose(
            diagnostics,
            ParseDiagnosticCode::InputTooLarge,
            relative_path,
        );
        return None;
    }

    match fs::read_to_string(path) {
        Ok(value) => Some(value),
        Err(error) if error.kind() == io::ErrorKind::InvalidData => {
            diagnose(diagnostics, ParseDiagnosticCode::InvalidUtf8, relative_path);
            None
        }
        Err(_) => {
            diagnose(
                diagnostics,
                ParseDiagnosticCode::EntryUnreadable,
                relative_path,
            );
            None
        }
    }
}

fn parse_frontmatter(
    markdown: &str,
    diagnostics: &mut Vec<ParseDiagnostic>,
) -> (SkillFrontmatter, usize) {
    let empty = SkillFrontmatter {
        name: None,
        description: None,
        extensions: Map::new(),
    };
    if !markdown.starts_with("---\n") {
        return (empty, 0);
    }

    let Some(end) = markdown[4..].find("\n---\n") else {
        diagnose(
            diagnostics,
            ParseDiagnosticCode::InvalidFrontmatter,
            SKILL_MARKDOWN_FILE,
        );
        return (empty, 0);
    };
    let yaml_end = 4 + end;
    let body_offset = yaml_end + "\n---\n".len();
    let yaml: BTreeMap<String, serde_yaml::Value> =
        match serde_yaml::from_str(&markdown[4..yaml_end]) {
            Ok(value) => value,
            Err(_) => {
                diagnose(
                    diagnostics,
                    ParseDiagnosticCode::InvalidYaml,
                    SKILL_MARKDOWN_FILE,
                );
                return (empty, body_offset);
            }
        };
    let mut extensions = Map::new();
    let mut name = None;
    let mut description = None;

    for (key, value) in yaml {
        let value = match serde_json::to_value(value) {
            Ok(value) => value,
            Err(_) => {
                diagnose(
                    diagnostics,
                    ParseDiagnosticCode::InvalidYaml,
                    SKILL_MARKDOWN_FILE,
                );
                continue;
            }
        };
        match key.as_str() {
            "name" => name = value.as_str().map(str::to_owned),
            "description" => description = value.as_str().map(str::to_owned),
            _ => {
                extensions.insert(key, value);
            }
        }
    }

    (
        SkillFrontmatter {
            name,
            description,
            extensions,
        },
        body_offset,
    )
}

fn parse_headings(
    markdown: &str,
    leading_line_count: usize,
    diagnostics: &mut Vec<ParseDiagnostic>,
) -> Vec<MarkdownHeading> {
    let mut headings = Vec::new();
    let mut current: Option<(u8, usize, usize, String)> = None;

    for (event, range) in Parser::new_ext(markdown, Options::all()).into_offset_iter() {
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                current = Some((heading_level(level), range.start, range.end, String::new()));
            }
            Event::Text(text) | Event::Code(text) if current.is_some() => {
                current.as_mut().unwrap().3.push_str(&text);
            }
            Event::End(TagEnd::Heading(_)) => {
                if let Some((level, start, end, text)) = current.take() {
                    headings.push(MarkdownHeading {
                        level,
                        text,
                        line_start: leading_line_count + line_number(markdown, start),
                        line_end: leading_line_count + line_number(markdown, end),
                    });
                }
            }
            _ => {}
        }
    }

    if current.is_some() {
        diagnose(
            diagnostics,
            ParseDiagnosticCode::InvalidMarkdown,
            SKILL_MARKDOWN_FILE,
        );
    }
    headings
}

fn parse_openai_manifest(
    skill_directory: &Path,
    diagnostics: &mut Vec<ParseDiagnostic>,
) -> Option<OpenAiManifest> {
    let path = skill_directory.join(OPENAI_MANIFEST_FILE);
    match fs::symlink_metadata(&path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return None,
        Err(_) => {
            diagnose(
                diagnostics,
                ParseDiagnosticCode::EntryUnreadable,
                OPENAI_MANIFEST_FILE,
            );
            return None;
        }
        Ok(_) => {}
    }
    let content = read_text_file(&path, OPENAI_MANIFEST_FILE, diagnostics)?;
    let yaml: BTreeMap<String, serde_yaml::Value> = match serde_yaml::from_str(&content) {
        Ok(value) => value,
        Err(_) => {
            diagnose(
                diagnostics,
                ParseDiagnosticCode::InvalidYaml,
                OPENAI_MANIFEST_FILE,
            );
            return None;
        }
    };
    let mut extensions = Map::new();
    let mut display_name = None;
    let mut short_description = None;
    let mut default_prompt = None;

    for (key, value) in yaml {
        let value = match serde_json::to_value(value) {
            Ok(value) => value,
            Err(_) => {
                diagnose(
                    diagnostics,
                    ParseDiagnosticCode::InvalidYaml,
                    OPENAI_MANIFEST_FILE,
                );
                continue;
            }
        };
        match key.as_str() {
            "display_name" => display_name = value.as_str().map(str::to_owned),
            "short_description" => short_description = value.as_str().map(str::to_owned),
            "default_prompt" => default_prompt = value.as_str().map(str::to_owned),
            _ => {
                extensions.insert(key, value);
            }
        }
    }

    Some(OpenAiManifest {
        display_name,
        short_description,
        default_prompt,
        extensions,
    })
}

fn collect_resources(
    skill_directory: &Path,
    diagnostics: &mut Vec<ParseDiagnostic>,
) -> Vec<ResourceEntry> {
    let mut resources = Vec::new();
    for directory in RESOURCE_DIRECTORIES {
        let root = skill_directory.join(directory);
        match fs::symlink_metadata(&root) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(_) => {
                diagnose(diagnostics, ParseDiagnosticCode::EntryUnreadable, directory);
                continue;
            }
            Ok(_) => {}
        }
        collect_resource_directory(skill_directory, &root, diagnostics, &mut resources);
    }
    resources.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    resources
}

fn collect_resource_directory(
    skill_directory: &Path,
    directory: &Path,
    diagnostics: &mut Vec<ParseDiagnostic>,
    resources: &mut Vec<ResourceEntry>,
) {
    let relative_directory = skill_relative_path(skill_directory, directory).unwrap_or_default();
    let metadata = match fs::symlink_metadata(directory) {
        Ok(metadata) => metadata,
        Err(_) => {
            diagnose(
                diagnostics,
                ParseDiagnosticCode::EntryUnreadable,
                &relative_directory,
            );
            return;
        }
    };
    if metadata.file_type().is_symlink() {
        diagnose(
            diagnostics,
            ParseDiagnosticCode::SymlinkDenied,
            &relative_directory,
        );
        return;
    }
    if !metadata.is_dir() {
        diagnose(
            diagnostics,
            ParseDiagnosticCode::InvalidPath,
            &relative_directory,
        );
        return;
    }

    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(_) => {
            diagnose(
                diagnostics,
                ParseDiagnosticCode::EntryUnreadable,
                &relative_directory,
            );
            return;
        }
    };
    let mut paths = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    paths.sort();

    for path in paths {
        let Some(relative_path) = skill_relative_path(skill_directory, &path) else {
            diagnose(
                diagnostics,
                ParseDiagnosticCode::InvalidPath,
                &relative_directory,
            );
            continue;
        };
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(_) => {
                diagnose(
                    diagnostics,
                    ParseDiagnosticCode::EntryUnreadable,
                    &relative_path,
                );
                continue;
            }
        };
        if metadata.file_type().is_symlink() {
            diagnose(
                diagnostics,
                ParseDiagnosticCode::SymlinkDenied,
                &relative_path,
            );
        } else if metadata.is_dir() {
            collect_resource_directory(skill_directory, &path, diagnostics, resources);
        } else if metadata.is_file() {
            if metadata.len() > MAX_RESOURCE_BYTES {
                diagnose(
                    diagnostics,
                    ParseDiagnosticCode::InputTooLarge,
                    &relative_path,
                );
            }
            match file_hash(&path) {
                Ok(content_hash) => resources.push(ResourceEntry {
                    relative_path,
                    size_bytes: metadata.len(),
                    content_hash,
                }),
                Err(_) => diagnose(
                    diagnostics,
                    ParseDiagnosticCode::EntryUnreadable,
                    &relative_path,
                ),
            }
        }
    }
}

fn snapshot_hash(
    skill_directory: &Path,
    resources: &[ResourceEntry],
    has_openai_manifest: bool,
    diagnostics: &mut Vec<ParseDiagnostic>,
) -> String {
    let mut inputs = vec![(
        SKILL_MARKDOWN_FILE.to_owned(),
        skill_directory.join(SKILL_MARKDOWN_FILE),
    )];
    if has_openai_manifest {
        inputs.push((
            OPENAI_MANIFEST_FILE.to_owned(),
            skill_directory.join(OPENAI_MANIFEST_FILE),
        ));
    }
    for resource in resources {
        inputs.push((
            resource.relative_path.clone(),
            skill_directory.join(&resource.relative_path),
        ));
    }
    inputs.sort_by(|left, right| left.0.cmp(&right.0));

    let mut digest = Sha256::new();
    for (relative_path, path) in inputs {
        match file_hash_bytes(&path) {
            Ok(file_digest) => {
                digest.update((relative_path.len() as u64).to_be_bytes());
                digest.update(relative_path.as_bytes());
                digest.update(file_digest);
            }
            Err(_) => diagnose(
                diagnostics,
                ParseDiagnosticCode::EntryUnreadable,
                &relative_path,
            ),
        }
    }
    format!("{:x}", digest.finalize())
}

fn regular_file_metadata(
    path: &Path,
    relative_path: &str,
    diagnostics: &mut Vec<ParseDiagnostic>,
) -> Option<fs::Metadata> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            diagnose(
                diagnostics,
                ParseDiagnosticCode::SymlinkDenied,
                relative_path,
            );
            None
        }
        Ok(metadata) if metadata.is_file() => Some(metadata),
        Ok(_) => {
            diagnose(diagnostics, ParseDiagnosticCode::InvalidPath, relative_path);
            None
        }
        Err(_) => {
            diagnose(
                diagnostics,
                ParseDiagnosticCode::EntryUnreadable,
                relative_path,
            );
            None
        }
    }
}

fn regular_file_exists(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .map(|metadata| metadata.is_file() && !metadata.file_type().is_symlink())
        .unwrap_or(false)
}

fn skill_relative_path(skill_directory: &Path, path: &Path) -> Option<String> {
    let relative = path.strip_prefix(skill_directory).ok()?;
    let mut components = Vec::new();
    for component in relative.components() {
        match component {
            Component::Normal(value) => components.push(value.to_str()?.to_owned()),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    Some(components.join("/"))
}

fn file_hash(path: &Path) -> io::Result<String> {
    Ok(hex_digest(file_hash_bytes(path)?))
}

fn file_hash_bytes(path: &Path) -> io::Result<[u8; 32]> {
    let mut file = fs::File::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 8192];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(digest.finalize().into())
}

fn hex_digest(digest: [u8; 32]) -> String {
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn heading_level(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

fn line_number(markdown: &str, byte_offset: usize) -> usize {
    markdown[..byte_offset.min(markdown.len())]
        .bytes()
        .filter(|byte| *byte == b'\n')
        .count()
        + 1
}

fn diagnose(
    diagnostics: &mut Vec<ParseDiagnostic>,
    code: ParseDiagnosticCode,
    relative_path: &str,
) {
    diagnostics.push(ParseDiagnostic {
        code,
        relative_path: relative_path.to_owned(),
    });
}

#[cfg(test)]
mod tests;
