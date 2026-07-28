use std::{
    fs,
    path::{Path, PathBuf},
    time::Instant,
};

use tempfile::TempDir;

use crate::{
    db::{self, AppDatabase},
    providers::{AdditionalRoot, ProviderRoots},
};

use super::{
    AnalysisStatus, ProviderAvailability, SkillCatalog, SkillListQuery, SkillScope, SkillSort,
    SkillValidity,
};

fn catalog_fixture() -> (TempDir, SkillCatalog) {
    let temporary = TempDir::new().expect("temporary fixture");
    let roots = ProviderRoots::new(
        temporary.path().join("home"),
        temporary.path().join("repository"),
        temporary.path().join("plugin-cache"),
    )
    .with_additional_roots(vec![AdditionalRoot::new(
        "extra",
        temporary.path().join("extra"),
    )
    .expect("valid extra root")]);
    (temporary, SkillCatalog::new(roots))
}

fn indexed_catalog_fixture() -> (TempDir, ProviderRoots, PathBuf) {
    let temporary = TempDir::new().expect("temporary fixture");
    let roots = ProviderRoots::new(
        temporary.path().join("home"),
        temporary.path().join("repository"),
        temporary.path().join("plugin-cache"),
    )
    .with_additional_roots(vec![AdditionalRoot::new(
        "extra",
        temporary.path().join("extra"),
    )
    .expect("valid extra root")]);
    let database_path = temporary.path().join("catalog.db");

    assert!(matches!(
        db::initialize(database_path.clone()),
        AppDatabase::Ready(_)
    ));
    (temporary, roots, database_path)
}

#[test]
fn analysis_metadata_does_not_trigger_a_scan_without_an_index() {
    let (_temporary, catalog) = catalog_fixture();

    let error = catalog.analysis_metadata("unknown").unwrap_err();

    assert_eq!(error.code, "catalog_unavailable");
    assert!(catalog.load_catalog().is_none());
}

#[test]
fn indexed_analysis_material_revalidates_source_by_stable_id() {
    let (temporary, roots, database_path) = indexed_catalog_fixture();
    let directory = write_skill(
        &temporary.path().join("home/.agents/skills"),
        "analysis",
        "# Overview\nPersisted analysis source. See references/guide.md.",
    );
    fs::create_dir_all(directory.join("references")).unwrap();
    fs::write(
        directory.join("references/guide.md"),
        "# Guide\nReferenced evidence",
    )
    .unwrap();
    let first = SkillCatalog::with_index_path(roots.clone(), database_path.clone());
    let skill_id = first.scan_skills().skills[0].id.clone();
    let restarted = SkillCatalog::with_index_path(roots, database_path);

    let material = restarted.analysis_material(&skill_id).unwrap();

    assert!(material
        .metadata
        .snapshot_id
        .starts_with(&format!("snapshot:{skill_id}:")));
    assert!(material
        .sources
        .iter()
        .any(|source| source.relative_path == "SKILL.md"
            && source.content.contains("Persisted analysis source")));
    assert!(material
        .sources
        .iter()
        .any(|source| source.relative_path == "references/guide.md"
            && source.content.contains("Referenced evidence")));
    assert_eq!(
        material.snapshot.content_hash,
        material.metadata.content_hash
    );
}

#[test]
fn analysis_material_does_not_read_unreferenced_references() {
    let (temporary, catalog) = catalog_fixture();
    let directory = write_skill(
        &user_root(&temporary),
        "analysis",
        "# Overview\nOnly references/selected.md is used.",
    );
    fs::create_dir_all(directory.join("references")).unwrap();
    fs::write(
        directory.join("references/selected.md"),
        "selected evidence",
    )
    .unwrap();
    fs::write(
        directory.join("references/unselected.md"),
        "unselected evidence",
    )
    .unwrap();
    let skill_id = catalog.scan_skills().skills[0].id.clone();

    let material = catalog.analysis_material(&skill_id).unwrap();

    assert!(material
        .sources
        .iter()
        .any(|source| source.relative_path == "references/selected.md"));
    assert!(!material
        .sources
        .iter()
        .any(|source| source.relative_path == "references/unselected.md"));
}

fn write_skill(root: &Path, relative: &str, source: &str) -> PathBuf {
    let directory = root.join(relative);
    fs::create_dir_all(&directory).expect("skill directory");
    fs::write(directory.join("SKILL.md"), source).expect("skill source");
    directory
}

fn user_root(temporary: &TempDir) -> PathBuf {
    temporary.path().join("home/.agents/skills")
}

fn repository_root(temporary: &TempDir) -> PathBuf {
    temporary.path().join("repository/.agents/skills")
}

fn extra_root(temporary: &TempDir) -> PathBuf {
    temporary.path().join("extra")
}

fn write_plugin_skill(temporary: &TempDir) {
    let version_root = temporary.path().join("plugin-cache/personal/example/1.0.0");
    fs::create_dir_all(version_root.join(".codex-plugin")).expect("plugin manifest directory");
    fs::write(version_root.join(".codex-plugin/plugin.json"), "{}").expect("plugin manifest");
    write_skill(
        &version_root.join("skills"),
        "cached-skill",
        "# Cached plugin Skill",
    );
}

#[test]
fn provider_views_are_safe_and_include_unavailable_sources() {
    let (temporary, catalog) = catalog_fixture();
    write_skill(&user_root(&temporary), "review", "# Review");

    let providers = catalog.list_providers();
    let encoded = serde_json::to_string(&providers).expect("provider JSON");

    assert!(providers
        .providers
        .iter()
        .any(|provider| provider.id == "user_global"));
    assert!(providers.providers.iter().any(|provider| {
        provider.id == "system" && provider.availability == ProviderAvailability::Unavailable
    }));
    assert!(!encoded.contains(temporary.path().to_str().expect("UTF-8 fixture path")));
}

#[test]
fn same_relative_path_in_multiple_providers_remains_distinct() {
    let (temporary, catalog) = catalog_fixture();
    write_skill(&user_root(&temporary), "review", "# User");
    write_skill(&repository_root(&temporary), "review", "# Repo");

    let skills = catalog.list_skills(SkillListQuery::default()).skills;

    assert_eq!(skills.len(), 2);
    assert_ne!(skills[0].id, skills[1].id);
    assert_ne!(skills[0].provider.id, skills[1].provider.id);
}

#[test]
fn list_dto_never_contains_markdown_or_absolute_paths() {
    let (temporary, catalog) = catalog_fixture();
    write_skill(
        &user_root(&temporary),
        "private",
        "---\nname: Private\ndescription: safe\n---\n# Visible\nsecret-body-marker",
    );

    let encoded =
        serde_json::to_string(&catalog.list_skills(SkillListQuery::default())).expect("list JSON");

    assert!(!encoded.contains("secret-body-marker"));
    assert!(!encoded.contains(temporary.path().to_str().expect("UTF-8 fixture path")));
}

#[test]
fn search_matches_name_description_and_headings() {
    let (temporary, catalog) = catalog_fixture();
    write_skill(
        &user_root(&temporary),
        "first",
        "---\nname: Named\ndescription: searchable description\n---\n# Heading token",
    );

    for query in ["named", "description", "heading token"] {
        assert_eq!(
            catalog
                .list_skills(SkillListQuery {
                    query: Some(query.to_owned()),
                    ..Default::default()
                })
                .skills
                .len(),
            1
        );
    }
}

#[test]
fn provider_scope_and_validity_filters_are_applied() {
    let (temporary, catalog) = catalog_fixture();
    write_skill(&user_root(&temporary), "valid", "# Valid");
    let invalid = write_skill(&repository_root(&temporary), "invalid", "# Bad");
    fs::write(invalid.join("SKILL.md"), [0xff_u8]).expect("invalid UTF-8");

    assert_eq!(
        catalog
            .list_skills(SkillListQuery {
                provider_id: Some("user_global".to_owned()),
                ..Default::default()
            })
            .skills
            .len(),
        1
    );
    assert_eq!(
        catalog
            .list_skills(SkillListQuery {
                scope: Some(SkillScope::Repository),
                validity: Some(SkillValidity::NeedsAttention),
                ..Default::default()
            })
            .skills
            .len(),
        1
    );
}

#[test]
fn name_sort_is_deterministic() {
    let (temporary, catalog) = catalog_fixture();
    write_skill(&user_root(&temporary), "z", "---\nname: Zebra\n---\n# Z");
    write_skill(&user_root(&temporary), "a", "---\nname: Alpha\n---\n# A");

    let names = catalog
        .list_skills(SkillListQuery {
            sort: SkillSort::Name,
            ..Default::default()
        })
        .skills
        .into_iter()
        .map(|skill| skill.display_name)
        .collect::<Vec<_>>();

    assert_eq!(names, vec!["Alpha", "Zebra"]);
}

#[test]
fn size_sort_includes_discovered_resources() {
    let (temporary, catalog) = catalog_fixture();
    let large = write_skill(&user_root(&temporary), "large", "# L");
    fs::create_dir_all(large.join("resources")).expect("resources");
    fs::write(large.join("resources/data.txt"), "x".repeat(4096)).expect("resource");
    write_skill(&user_root(&temporary), "small", "# S");

    let skills = catalog
        .list_skills(SkillListQuery {
            sort: SkillSort::Size,
            ..Default::default()
        })
        .skills;

    assert_eq!(skills[0].display_name, "large");
    assert!(skills[0].size_bytes > skills[1].size_bytes);
}

#[test]
fn detail_excludes_source_by_default() {
    let (temporary, catalog) = catalog_fixture();
    write_skill(&user_root(&temporary), "review", "# Review\nprivate source");
    let summary = catalog
        .list_skills(SkillListQuery::default())
        .skills
        .remove(0);

    let detail = catalog
        .get_skill_detail(&summary.id, false)
        .expect("detail result");

    assert_eq!(detail.source, None);
    assert!(!serde_json::to_string(&detail)
        .expect("detail JSON")
        .contains("private source"));
}

#[test]
fn detail_returns_source_only_for_a_known_stable_id() {
    let (temporary, catalog) = catalog_fixture();
    write_skill(&user_root(&temporary), "review", "# Review\nprivate source");
    let summary = catalog
        .list_skills(SkillListQuery::default())
        .skills
        .remove(0);

    let detail = catalog
        .get_skill_detail(&summary.id, true)
        .expect("detail source");

    assert_eq!(detail.source.as_deref(), Some("# Review\nprivate source"));
}

#[test]
fn unknown_skill_id_is_rejected_without_reading_a_path() {
    let (temporary, catalog) = catalog_fixture();
    let outside = temporary.path().join("outside.md");
    fs::write(&outside, "outside marker").expect("outside file");

    let error = catalog
        .get_skill_detail(outside.to_str().expect("UTF-8 fixture path"), true)
        .expect_err("arbitrary paths are not skill identifiers");

    assert_eq!(error.code, "skill_not_found");
    assert!(!error.message.contains("outside"));
}

#[test]
fn oversized_source_returns_a_safe_diagnostic() {
    let (temporary, catalog) = catalog_fixture();
    write_skill(
        &user_root(&temporary),
        "large",
        &format!("# Large\n{}", "x".repeat(1024 * 1024)),
    );
    let id = catalog
        .list_skills(SkillListQuery::default())
        .skills
        .remove(0)
        .id;

    let detail = catalog.get_skill_detail(&id, true).expect("safe detail");

    assert_eq!(detail.source, None);
    assert!(detail
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.code == "input_too_large"));
}

#[test]
fn detail_exposes_resource_tree_without_absolute_paths() {
    let (temporary, catalog) = catalog_fixture();
    let directory = write_skill(&extra_root(&temporary), "asset", "# Asset");
    fs::create_dir_all(directory.join("references/nested")).expect("references");
    fs::write(directory.join("references/nested/info.txt"), "content").expect("reference");
    let id = catalog
        .list_skills(SkillListQuery::default())
        .skills
        .remove(0)
        .id;

    let detail = catalog.get_skill_detail(&id, false).expect("detail");
    let encoded = serde_json::to_string(&detail).expect("detail JSON");

    assert_eq!(
        detail.resources[0].relative_path,
        "references/nested/info.txt"
    );
    assert!(!encoded.contains(temporary.path().to_str().expect("UTF-8 fixture path")));
}

#[test]
fn malformed_skill_does_not_block_other_skills() {
    let (temporary, catalog) = catalog_fixture();
    write_skill(&user_root(&temporary), "good", "# Good");
    let invalid = write_skill(&extra_root(&temporary), "bad", "# Bad");
    fs::write(invalid.join("SKILL.md"), [0xff_u8]).expect("invalid UTF-8");

    let skills = catalog.list_skills(SkillListQuery::default()).skills;

    assert_eq!(skills.len(), 2);
    assert!(skills.iter().any(|skill| skill.display_name == "good"));
    assert!(skills
        .iter()
        .any(|skill| skill.validity == SkillValidity::NeedsAttention));
}

#[test]
fn summaries_always_report_unconfigured_analysis() {
    let (temporary, catalog) = catalog_fixture();
    write_skill(&user_root(&temporary), "skill", "# Skill");

    let summary = catalog
        .list_skills(SkillListQuery::default())
        .skills
        .remove(0);

    assert_eq!(summary.analysis_status, AnalysisStatus::NotConfigured);
}

#[test]
fn indexed_catalog_recovers_list_and_detail_metadata_after_restart() {
    let (temporary, roots, database_path) = indexed_catalog_fixture();
    write_skill(
        &user_root(&temporary),
        "review",
        "---\nname: Persisted review\ndescription: metadata only\n---\n# Safe heading\nprivate source",
    );
    let catalog = SkillCatalog::with_index_path(roots.clone(), database_path.clone());

    let scanned = catalog.scan_skills();
    let id = scanned.skills[0].id.clone();
    drop(catalog);
    fs::remove_dir_all(user_root(&temporary)).expect("remove live source root");

    let restarted = SkillCatalog::with_index_path(roots, database_path);
    let cached = restarted.load_catalog().expect("persisted catalog");
    let detail = restarted
        .get_skill_detail(&id, false)
        .expect("persisted detail metadata");
    let encoded = serde_json::to_string(&cached).expect("persisted list JSON");

    assert_eq!(cached.skills[0].display_name, "Persisted review");
    assert_eq!(detail.headings[0].text, "Safe heading");
    assert!(!encoded.contains("private source"));
    assert!(!encoded.contains(temporary.path().to_str().expect("UTF-8 fixture path")));
}

#[test]
fn first_run_without_an_index_returns_none_before_the_initial_scan() {
    let (_temporary, roots, database_path) = indexed_catalog_fixture();
    let catalog = SkillCatalog::with_index_path(roots, database_path);

    assert_eq!(catalog.load_catalog(), None);
}

#[test]
fn catalog_rejects_a_second_scan_while_one_is_running() {
    let (_temporary, catalog) = catalog_fixture();

    catalog.begin_scan().expect("first scan lock");
    let error = catalog.begin_scan().expect_err("second scan rejected");
    catalog.finish_scan();

    assert_eq!(error.code, "scan_in_progress");
    assert!(catalog.begin_scan().is_ok());
    catalog.finish_scan();
}

#[test]
fn plugin_cache_scanning_is_disabled_by_default_and_persists_when_enabled() {
    let (temporary, roots, _database_path) = indexed_catalog_fixture();
    let preferences_path = temporary.path().join("scan-preferences.json");
    write_plugin_skill(&temporary);
    let catalog = SkillCatalog::new(roots.clone()).with_preferences_path(preferences_path.clone());

    assert!(!catalog.scan_preferences().include_plugin_cache);
    assert!(catalog.scan_skills().skills.is_empty());
    let before_setting_change = catalog.load_catalog();

    let enabled_preferences = catalog
        .update_scan_preferences(true, true)
        .expect("enable plugin scanning");
    assert_eq!(catalog.load_catalog(), before_setting_change);
    let enabled = catalog.scan_skills();
    assert!(enabled_preferences.include_plugin_cache);
    assert_eq!(enabled.skills.len(), 1);
    assert_eq!(
        enabled.skills[0].provider.kind,
        crate::providers::ProviderKind::Plugin
    );
    drop(catalog);

    let restarted = SkillCatalog::new(roots).with_preferences_path(preferences_path.clone());
    let stored = fs::read_to_string(preferences_path).expect("stored preference");

    assert!(restarted.scan_preferences().include_plugin_cache);
    assert_eq!(restarted.scan_skills().skills.len(), 1);
    assert_eq!(
        stored,
        r#"{"include_plugin_cache":true,"include_bundled_cache":true,"initial_scan_notice_seen":false}"#
    );
    assert!(!stored.contains(temporary.path().to_str().expect("UTF-8 fixture path")));
}

#[test]
fn persisted_detail_reads_source_only_after_the_user_requests_it() {
    let (temporary, roots, database_path) = indexed_catalog_fixture();
    write_skill(
        &user_root(&temporary),
        "review",
        "---\nname: Persisted review\n---\n# Heading\nsource marker",
    );
    let catalog = SkillCatalog::with_index_path(roots.clone(), database_path.clone());
    let id = catalog.scan_skills().skills[0].id.clone();
    drop(catalog);

    let restarted = SkillCatalog::with_index_path(roots, database_path);
    let detail = restarted
        .get_skill_detail(&id, true)
        .expect("on-demand source");

    assert_eq!(
        detail.source.as_deref(),
        Some("---\nname: Persisted review\n---\n# Heading\nsource marker")
    );
}

#[test]
fn rescanning_replaces_the_live_index_without_deleting_snapshot_history() {
    let (temporary, roots, database_path) = indexed_catalog_fixture();
    let skill_directory = write_skill(
        &user_root(&temporary),
        "review",
        "---\nname: First\n---\n# First",
    );
    let catalog = SkillCatalog::with_index_path(roots.clone(), database_path.clone());

    catalog.scan_skills();
    fs::write(
        skill_directory.join("SKILL.md"),
        "---\nname: Second\n---\n# Second",
    )
    .expect("updated source");
    catalog.scan_skills();
    drop(catalog);

    let restarted = SkillCatalog::with_index_path(roots, database_path.clone());
    let cached = restarted.load_catalog().expect("reloaded catalog");
    let connection = rusqlite::Connection::open(database_path).expect("index database");
    let snapshots: i64 = connection
        .query_row("SELECT COUNT(*) FROM artifact_snapshots", [], |row| {
            row.get(0)
        })
        .expect("snapshot count");

    assert_eq!(cached.skills[0].display_name, "Second");
    assert_eq!(snapshots, 2);
}

#[test]
fn list_and_detail_reuse_the_last_explicit_catalog_scan() {
    let (temporary, catalog) = catalog_fixture();
    write_skill(
        &user_root(&temporary),
        "first",
        "---\nname: First version\n---\n# First",
    );

    let first = catalog
        .list_skills(SkillListQuery::default())
        .skills
        .remove(0);
    fs::write(
        user_root(&temporary).join("first/SKILL.md"),
        "---\nname: Updated version\n---\n# Updated",
    )
    .expect("updated fixture source");
    write_skill(&user_root(&temporary), "second", "# Second");

    assert_eq!(
        catalog.list_skills(SkillListQuery::default()).skills.len(),
        1,
        "list queries reuse the previous scan rather than traversing the file system",
    );
    assert_eq!(
        catalog
            .get_skill_detail(&first.id, false)
            .expect("cached detail")
            .summary
            .display_name,
        "First version",
    );

    catalog.scan_skills();
    let refreshed = catalog.list_skills(SkillListQuery::default()).skills;

    assert_eq!(refreshed.len(), 2);
    assert!(refreshed
        .iter()
        .any(|skill| skill.display_name == "Updated version"));
}

#[test]
fn one_hundred_tempfile_skills_list_in_under_three_seconds() {
    let (temporary, catalog) = catalog_fixture();
    for index in 0..100 {
        write_skill(
            &user_root(&temporary),
            &format!("skill-{index:03}"),
            &format!("---\nname: Skill {index}\n---\n# Heading {index}"),
        );
    }

    let started = Instant::now();
    let skills = catalog.list_skills(SkillListQuery::default()).skills;

    assert_eq!(skills.len(), 100);
    assert!(started.elapsed().as_secs_f32() < 3.0);
}
