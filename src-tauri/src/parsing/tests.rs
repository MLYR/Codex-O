use std::{fs, path::Path};

#[cfg(unix)]
use std::os::unix::fs::symlink;

use tempfile::TempDir;

use crate::providers::{DiscoveredSkill, ProviderRegistry, ProviderRoots};

use super::{parse_skill, parse_skills, ParseDiagnosticCode, PARSER_VERSION};

#[test]
fn frontmatter_and_markdown_headings_are_parsed_with_source_lines() {
    let fixture = ParsingFixture::new();
    fixture.write_skill(
        "example",
        "---\nname: Example\ndescription: Useful\nlicense: MIT\n---\n# Overview\n\n## Usage\n",
    );

    let snapshot = fixture.parse("example");

    assert_eq!(snapshot.frontmatter.name.as_deref(), Some("Example"));
    assert_eq!(snapshot.frontmatter.description.as_deref(), Some("Useful"));
    assert_eq!(snapshot.frontmatter.extensions["license"], "MIT");
    assert_eq!(snapshot.headings.len(), 2);
    assert_eq!(snapshot.headings[0].line_start, 6);
    assert_eq!(snapshot.headings[1].line_start, 8);
}

#[test]
fn absent_frontmatter_and_openai_file_are_valid() {
    let fixture = ParsingFixture::new();
    fixture.write_skill("plain", "# Plain\n");

    let snapshot = fixture.parse("plain");

    assert_eq!(snapshot.frontmatter.name, None);
    assert_eq!(snapshot.openai_manifest, None);
    assert!(snapshot.diagnostics.is_empty());
}

#[test]
fn malformed_frontmatter_returns_a_snapshot_with_a_diagnostic() {
    let fixture = ParsingFixture::new();
    fixture.write_skill("broken", "---\nname: [\n---\n# Still visible\n");

    let snapshot = fixture.parse("broken");

    assert!(snapshot
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.code == ParseDiagnosticCode::InvalidYaml));
    assert_eq!(snapshot.headings[0].text, "Still visible");
}

#[test]
fn unterminated_frontmatter_is_reported_without_panicking() {
    let fixture = ParsingFixture::new();
    fixture.write_skill("broken", "---\nname: Example\n# Body\n");

    let snapshot = fixture.parse("broken");

    assert!(snapshot
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.code == ParseDiagnosticCode::InvalidFrontmatter));
}

#[test]
fn openai_manifest_is_structured_and_preserves_extensions() {
    let fixture = ParsingFixture::new();
    fixture.write_skill("openai", "# OpenAI\n");
    fixture.write_file(
        "openai/agents/openai.yaml",
        "display_name: Example\nshort_description: Short\ndefault_prompt: Go\nui_color: blue\n",
    );

    let snapshot = fixture.parse("openai");
    let openai = snapshot.openai_manifest.unwrap();

    assert_eq!(openai.display_name.as_deref(), Some("Example"));
    assert_eq!(openai.short_description.as_deref(), Some("Short"));
    assert_eq!(openai.default_prompt.as_deref(), Some("Go"));
    assert_eq!(openai.extensions["ui_color"], "blue");
}

#[test]
fn malformed_openai_manifest_is_diagnostic_and_does_not_block_snapshot() {
    let fixture = ParsingFixture::new();
    fixture.write_skill("bad-openai", "# Content\n");
    fixture.write_file("bad-openai/agents/openai.yaml", "display_name: [");

    let snapshot = fixture.parse("bad-openai");

    assert_eq!(snapshot.openai_manifest, None);
    assert!(snapshot
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.relative_path == "agents/openai.yaml"));
}

#[test]
fn resource_manifest_is_sorted_and_contains_relative_paths_hashes_and_sizes() {
    let fixture = ParsingFixture::new();
    fixture.write_skill("resources", "# Content\n");
    fixture.write_file("resources/scripts/z.sh", "#!/bin/sh\n");
    fixture.write_file("resources/references/a.txt", "reference");
    fixture.write_file("resources/resources/nested/b.txt", "resource");

    let snapshot = fixture.parse("resources");
    let paths = snapshot
        .resources
        .iter()
        .map(|entry| entry.relative_path.as_str())
        .collect::<Vec<_>>();

    assert_eq!(
        paths,
        vec!["references/a.txt", "resources/nested/b.txt", "scripts/z.sh"]
    );
    assert!(snapshot.resources.iter().all(|entry| entry.size_bytes > 0));
    assert!(snapshot
        .resources
        .iter()
        .all(|entry| entry.content_hash.len() == 64));
}

#[test]
fn content_hash_is_deterministic_for_unchanged_files() {
    let fixture = ParsingFixture::new();
    fixture.write_skill("stable", "# Content\n");
    fixture.write_file("stable/references/a.txt", "value");

    let first = fixture.parse("stable");
    let second = fixture.parse("stable");

    assert_eq!(first.content_hash, second.content_hash);
    assert_eq!(first.parser_version, PARSER_VERSION);
}

#[test]
fn content_hash_changes_when_a_manifested_resource_changes() {
    let fixture = ParsingFixture::new();
    fixture.write_skill("changes", "# Content\n");
    fixture.write_file("changes/references/a.txt", "before");
    let before = fixture.parse("changes").content_hash;

    fixture.write_file("changes/references/a.txt", "after");
    let after = fixture.parse("changes").content_hash;

    assert_ne!(before, after);
}

#[test]
fn content_hash_changes_when_an_invalid_openai_manifest_changes() {
    let fixture = ParsingFixture::new();
    fixture.write_skill("changes", "# Content\n");
    fixture.write_file("changes/agents/openai.yaml", "display_name: [");
    let before = fixture.parse("changes").content_hash;

    fixture.write_file("changes/agents/openai.yaml", "display_name: {");
    let after = fixture.parse("changes").content_hash;

    assert_ne!(before, after);
}

#[test]
fn too_large_markdown_is_rejected_before_reading_text() {
    let fixture = ParsingFixture::new();
    fixture.write_file("large/SKILL.md", &"a".repeat(1024 * 1024 + 1));

    let result = parse_skill(&fixture.discovered("large"));

    assert!(result.snapshot.is_none());
    assert!(result
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.code == ParseDiagnosticCode::InputTooLarge));
}

#[test]
fn oversized_resource_is_diagnostic_but_still_participates_in_the_content_hash() {
    let fixture = ParsingFixture::new();
    fixture.write_skill("large-resource", "# Content\n");
    fixture.write_file(
        "large-resource/references/large.txt",
        &"a".repeat(16 * 1024 * 1024 + 1),
    );
    let before = fixture.parse("large-resource");
    fixture.write_file(
        "large-resource/references/large.txt",
        &format!("{}b", "a".repeat(16 * 1024 * 1024)),
    );
    let after = fixture.parse("large-resource");

    assert!(before
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.code == ParseDiagnosticCode::InputTooLarge));
    assert_ne!(before.content_hash, after.content_hash);
}

#[test]
#[cfg(unix)]
fn symlinked_resource_is_not_followed_or_added_to_the_manifest() {
    let fixture = ParsingFixture::new();
    fixture.write_skill("linked", "# Content\n");
    let outside = fixture.root().join("outside.txt");
    fs::write(&outside, "private").unwrap();
    let resource = fixture.skill_root().join("linked/references/outside.txt");
    fs::create_dir_all(resource.parent().unwrap()).unwrap();
    symlink(&outside, resource).unwrap();

    let snapshot = fixture.parse("linked");

    assert!(snapshot.resources.is_empty());
    assert!(snapshot
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.code == ParseDiagnosticCode::SymlinkDenied));
}

#[test]
fn failed_skill_parse_does_not_block_other_skills() {
    let fixture = ParsingFixture::new();
    fixture.write_skill("good", "# Good\n");
    fixture.write_file("bad/SKILL.md", &"a".repeat(1024 * 1024 + 1));
    let skills = vec![fixture.discovered("bad"), fixture.discovered("good")];

    let results = parse_skills(&skills);

    assert!(results[0].snapshot.is_none());
    assert!(results[1].snapshot.is_some());
}

#[test]
fn snapshots_never_include_an_absolute_skill_directory() {
    let fixture = ParsingFixture::new();
    fixture.write_skill("safe", "# Safe\n");

    let serialized = serde_json::to_string(&fixture.parse("safe")).unwrap();

    assert!(!serialized.contains(fixture.root().to_str().unwrap()));
}

struct ParsingFixture {
    temporary_directory: TempDir,
}

impl ParsingFixture {
    fn new() -> Self {
        Self {
            temporary_directory: tempfile::tempdir().unwrap(),
        }
    }

    fn root(&self) -> &Path {
        self.temporary_directory.path()
    }

    fn skill_root(&self) -> std::path::PathBuf {
        self.root().join(".agents/skills")
    }

    fn write_skill(&self, relative_path: &str, content: &str) {
        self.write_file(&format!("{relative_path}/SKILL.md"), content);
    }

    fn write_file(&self, relative_path: &str, content: &str) {
        let path = self.skill_root().join(relative_path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    fn discovered(&self, name: &str) -> DiscoveredSkill {
        let registry = ProviderRegistry::with_roots(ProviderRoots::new(
            self.root().join("home"),
            self.root().to_path_buf(),
            self.root().join("cache"),
        ));
        let discovery = registry.discover_all();
        discovery
            .skills
            .into_iter()
            .find(|skill| skill.relative_path == name)
            .unwrap()
    }

    fn parse(&self, name: &str) -> super::ArtifactSnapshot {
        parse_skill(&self.discovered(name)).snapshot.unwrap()
    }
}
