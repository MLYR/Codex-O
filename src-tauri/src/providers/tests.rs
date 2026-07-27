use std::{
    fs,
    path::{Path, PathBuf},
};

#[cfg(unix)]
use std::os::unix::fs::symlink;

use tempfile::TempDir;

use super::{
    AdditionalRoot, DiscoveryWarningCode, ProviderDiagnosticCode, ProviderKind, ProviderRegistry,
    ProviderRoots,
};

#[test]
fn provider_roots_are_injected_without_reading_the_real_home() {
    let fixture = ProviderFixture::new();
    fixture.write_skill(&fixture.user_root(), "user-skill");

    let discovery = fixture.registry().discover_all();

    assert_eq!(discovery.skills.len(), 1);
    assert_eq!(discovery.skills[0].provider_kind, ProviderKind::UserGlobal);
    assert_eq!(discovery.skills[0].relative_path, "user-skill");
    assert!(!discovery.skills[0]
        .relative_path
        .contains(fixture.root().to_str().unwrap()));
}

#[test]
fn missing_roots_return_an_empty_discovery() {
    let fixture = ProviderFixture::new();

    let discovery = fixture.registry().discover_all();

    assert!(discovery.skills.is_empty());
    assert!(discovery.warnings.is_empty());
    assert_eq!(
        discovery
            .providers
            .iter()
            .map(|provider| provider.kind)
            .collect::<Vec<_>>(),
        vec![
            ProviderKind::UserGlobal,
            ProviderKind::Repo,
            ProviderKind::LegacyUser,
            ProviderKind::System,
        ]
    );
}

#[test]
fn user_repo_and_legacy_skills_are_discovered_separately() {
    let fixture = ProviderFixture::new();
    fixture.write_skill(&fixture.user_root(), "same-name");
    fixture.write_skill(&fixture.repo_root(), "same-name");
    fixture.write_skill(&fixture.legacy_root(), "legacy");

    let discovery = fixture.registry().discover_all();

    assert_eq!(discovery.skills.len(), 3);
    assert_eq!(
        discovery
            .skills
            .iter()
            .map(|skill| (&skill.provider_id, &skill.relative_path))
            .collect::<Vec<_>>(),
        vec![
            (&"user_global".to_owned(), &"same-name".to_owned()),
            (&"repo".to_owned(), &"same-name".to_owned()),
            (&"legacy_user".to_owned(), &"legacy".to_owned()),
        ]
    );
}

#[test]
fn discovery_stops_after_recognizing_a_skill_directory() {
    let fixture = ProviderFixture::new();
    fixture.write_skill(&fixture.user_root(), "parent");
    fixture.write_skill(&fixture.user_root().join("parent"), "nested");

    let discovery = fixture.registry().discover_all();

    assert_eq!(
        discovery
            .skills
            .iter()
            .map(|skill| skill.relative_path.as_str())
            .collect::<Vec<_>>(),
        vec!["parent"]
    );
}

#[test]
fn read_only_provider_capabilities_do_not_allow_writes() {
    let fixture = ProviderFixture::new();
    let discovery = fixture.registry().discover_all();
    let repo = discovery
        .providers
        .iter()
        .find(|provider| provider.id == "repo")
        .unwrap();

    assert_eq!(repo.kind, ProviderKind::Repo);
    assert!(!repo.capabilities.can_import);
    assert!(!repo.capabilities.can_quarantine);
}

#[test]
fn user_global_capabilities_only_enable_managed_user_operations() {
    let fixture = ProviderFixture::new();
    let discovery = fixture.registry().discover_all();
    let capabilities = discovery
        .providers
        .iter()
        .find(|provider| provider.id == "user_global")
        .unwrap()
        .capabilities;

    assert!(capabilities.can_read);
    assert!(capabilities.can_import);
    assert!(capabilities.can_quarantine);
    assert!(capabilities.can_restore);
    assert!(!capabilities.can_update);
    assert!(!capabilities.can_delete);
}

#[test]
fn plugin_and_bundled_cache_entries_are_classified_from_layout() {
    let fixture = ProviderFixture::new();
    fixture.write_cache_skill("third-party", "plugin-one", "1.0.0", "plugin-skill");
    fixture.write_cache_skill("openai-bundled", "bundle-one", "2.0.0", "bundled-skill");

    let discovery = fixture.registry().discover_all();

    assert_eq!(
        discovery
            .skills
            .iter()
            .map(|skill| (skill.provider_kind, skill.relative_path.as_str()))
            .collect::<Vec<_>>(),
        vec![
            (ProviderKind::Bundled, "bundled-skill"),
            (ProviderKind::Plugin, "plugin-skill"),
        ]
    );
}

#[test]
fn malformed_cache_entry_warns_without_blocking_valid_entries() {
    let fixture = ProviderFixture::new();
    fixture.write_cache_skill("third-party", "valid", "1.0.0", "found");
    fs::create_dir_all(
        fixture
            .cache_root()
            .join("third-party")
            .join("broken")
            .join("1.0.0")
            .join("skills"),
    )
    .unwrap();

    let discovery = fixture.registry().discover_all();

    assert_eq!(
        discovery
            .skills
            .iter()
            .map(|skill| skill.relative_path.as_str())
            .collect::<Vec<_>>(),
        vec!["found"]
    );
    assert!(discovery
        .warnings
        .iter()
        .any(|warning| warning.code == DiscoveryWarningCode::UnsupportedCacheLayout));
}

#[test]
#[cfg(unix)]
fn symlinked_cache_layout_is_rejected_without_following_it() {
    let fixture = ProviderFixture::new();
    let outside_manifest_directory = fixture.root().join("outside-manifest");
    fs::create_dir_all(&outside_manifest_directory).unwrap();
    fs::write(outside_manifest_directory.join("plugin.json"), b"{}").unwrap();
    let version_root = fixture
        .cache_root()
        .join("third-party")
        .join("linked")
        .join("1.0.0");
    fs::create_dir_all(&version_root).unwrap();
    fs::create_dir_all(version_root.join("skills")).unwrap();
    symlink(
        &outside_manifest_directory,
        version_root.join(".codex-plugin"),
    )
    .unwrap();

    let discovery = fixture.registry().discover_all();

    assert!(discovery.skills.is_empty());
    assert!(discovery
        .warnings
        .iter()
        .any(|warning| warning.code == DiscoveryWarningCode::SymlinkDenied));
}

#[test]
#[cfg(unix)]
fn symlinked_skill_directory_is_rejected_without_following_it() {
    let fixture = ProviderFixture::new();
    let outside = fixture.root().join("outside");
    fixture.write_skill(&outside, "escaped");
    fs::create_dir_all(fixture.user_root()).unwrap();
    symlink(&outside, fixture.user_root().join("link")).unwrap();

    let discovery = fixture.registry().discover_all();

    assert!(discovery.skills.is_empty());
    assert!(discovery
        .warnings
        .iter()
        .any(|warning| warning.code == DiscoveryWarningCode::SymlinkDenied));
}

#[test]
#[cfg(unix)]
fn symlinked_provider_root_is_rejected_without_following_it() {
    let fixture = ProviderFixture::new();
    let outside = fixture.root().join("outside");
    fixture.write_skill(&outside, "escaped");
    fs::create_dir_all(fixture.home().join(".agents")).unwrap();
    symlink(&outside, fixture.user_root()).unwrap();

    let discovery = fixture.registry().discover_all();

    assert!(discovery.skills.is_empty());
    assert!(discovery
        .warnings
        .iter()
        .any(|warning| warning.code == DiscoveryWarningCode::SymlinkDenied));
}

#[test]
fn invalid_skill_marker_warns_without_blocking_other_skills() {
    let fixture = ProviderFixture::new();
    fixture.write_skill(&fixture.user_root(), "valid");
    fs::create_dir_all(fixture.user_root().join("invalid").join("SKILL.md")).unwrap();

    let discovery = fixture.registry().discover_all();

    assert_eq!(
        discovery
            .skills
            .iter()
            .map(|skill| skill.relative_path.as_str())
            .collect::<Vec<_>>(),
        vec!["valid"]
    );
    assert!(discovery
        .warnings
        .iter()
        .any(|warning| warning.code == DiscoveryWarningCode::InvalidSkillMarker));
}

#[test]
fn system_provider_is_explicitly_unavailable_until_a_stable_root_exists() {
    let fixture = ProviderFixture::new();

    let discovery = fixture.registry().discover_all();

    assert!(discovery.diagnostics.iter().any(|diagnostic| {
        diagnostic.kind == ProviderKind::System
            && diagnostic.code == ProviderDiagnosticCode::Unavailable
    }));
}

#[test]
fn skill_id_is_stable_and_separates_same_relative_paths_by_provider() {
    let fixture = ProviderFixture::new();
    fixture.write_skill(&fixture.user_root(), "same-name");
    fixture.write_skill(&fixture.repo_root(), "same-name");

    let first = fixture.registry().discover_all();
    let second = fixture.registry().discover_all();

    assert_eq!(first.skills[0].id, second.skills[0].id);
    assert_ne!(first.skills[0].id, first.skills[1].id);
    assert!(first.skills[0].id.contains("user_global"));
    assert!(first.skills[1].id.contains("repo"));
}

#[test]
fn discovery_dto_does_not_serialize_the_internal_skill_directory() {
    let fixture = ProviderFixture::new();
    fixture.write_skill(&fixture.user_root(), "private-root");

    let discovery = fixture.registry().discover_all();
    let serialized = serde_json::to_string(&discovery.skills[0]).unwrap();

    assert!(!serialized.contains(fixture.root().to_str().unwrap()));
    assert!(serialized.contains("\"id\""));
}

#[test]
fn additional_root_is_read_only_and_scanned_as_a_distinct_provider() {
    let fixture = ProviderFixture::new();
    let additional = fixture.root().join("additional");
    fixture.write_skill(&additional, "shared");
    let root = AdditionalRoot::new("team", additional).unwrap();
    let roots = ProviderRoots::new(
        fixture.home_directory.clone(),
        fixture.repository_directory.clone(),
        fixture.cache_directory.clone(),
    )
    .with_additional_roots(vec![root]);

    let discovery = ProviderRegistry::with_roots(roots).discover_all();
    let provider = discovery
        .providers
        .iter()
        .find(|provider| provider.id == "additional_root:team")
        .unwrap();

    assert_eq!(provider.kind, ProviderKind::AdditionalRoot);
    assert!(!provider.capabilities.can_import);
    assert_eq!(discovery.skills[0].provider_id, "additional_root:team");
}

#[test]
fn additional_root_rejects_empty_path_like_and_oversized_ids() {
    let directory = PathBuf::from("fixture");

    assert!(AdditionalRoot::new("", directory.clone()).is_none());
    assert!(AdditionalRoot::new("../outside", directory.clone()).is_none());
    assert!(AdditionalRoot::new("a".repeat(65), directory.clone()).is_none());
    assert!(AdditionalRoot::new("team_01", directory).is_some());
}

#[test]
fn unavailable_additional_root_does_not_block_other_provider_scans() {
    let fixture = ProviderFixture::new();
    fixture.write_skill(&fixture.user_root(), "available");
    let root = AdditionalRoot::new("missing", fixture.root().join("missing")).unwrap();
    let roots = ProviderRoots::new(
        fixture.home_directory.clone(),
        fixture.repository_directory.clone(),
        fixture.cache_directory.clone(),
    )
    .with_additional_roots(vec![root]);

    let discovery = ProviderRegistry::with_roots(roots).discover_all();

    assert_eq!(discovery.skills.len(), 1);
    assert!(discovery
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.provider_id == "additional_root:missing"));
}

#[test]
fn stable_skill_id_keeps_provider_and_path_boundaries_unambiguous() {
    let fixture = ProviderFixture::new();
    fixture.write_skill(&fixture.user_root(), "x");
    let root = AdditionalRoot::new("user_globalx", fixture.root().join("additional")).unwrap();
    fixture.write_skill(&fixture.root().join("additional"), "x");
    let roots = ProviderRoots::new(
        fixture.home_directory.clone(),
        fixture.repository_directory.clone(),
        fixture.cache_directory.clone(),
    )
    .with_additional_roots(vec![root]);

    let discovery = ProviderRegistry::with_roots(roots).discover_all();

    assert_ne!(discovery.skills[0].id, discovery.skills[1].id);
}

struct ProviderFixture {
    temporary_directory: TempDir,
    home_directory: PathBuf,
    repository_directory: PathBuf,
    cache_directory: PathBuf,
}

impl ProviderFixture {
    fn new() -> Self {
        let temporary_directory = tempfile::tempdir().unwrap();
        let root = temporary_directory.path().to_path_buf();

        Self {
            temporary_directory,
            home_directory: root.join("home"),
            repository_directory: root.join("repository"),
            cache_directory: root.join("cache"),
        }
    }

    fn root(&self) -> &Path {
        self.temporary_directory.path()
    }

    fn home(&self) -> &Path {
        &self.home_directory
    }

    fn user_root(&self) -> PathBuf {
        self.home_directory.join(".agents/skills")
    }

    fn repo_root(&self) -> PathBuf {
        self.repository_directory.join(".agents/skills")
    }

    fn legacy_root(&self) -> PathBuf {
        self.home_directory.join(".codex/skills")
    }

    fn cache_root(&self) -> PathBuf {
        self.cache_directory.clone()
    }

    fn registry(&self) -> ProviderRegistry {
        ProviderRegistry::with_roots(ProviderRoots::new(
            self.home_directory.clone(),
            self.repository_directory.clone(),
            self.cache_directory.clone(),
        ))
    }

    fn write_skill(&self, root: &Path, relative_path: &str) {
        let skill_directory = root.join(relative_path);
        fs::create_dir_all(&skill_directory).unwrap();
        fs::write(skill_directory.join("SKILL.md"), b"fixture").unwrap();
    }

    fn write_cache_skill(&self, channel: &str, plugin: &str, version: &str, skill: &str) {
        let version_root = self
            .cache_directory
            .join(channel)
            .join(plugin)
            .join(version);
        fs::create_dir_all(version_root.join(".codex-plugin")).unwrap();
        fs::write(version_root.join(".codex-plugin/plugin.json"), b"{}").unwrap();
        self.write_skill(&version_root.join("skills"), skill);
    }
}
