use std::{fs, path::PathBuf};

use rusqlite::Connection;
use tempfile::TempDir;

use crate::{
    catalog::SkillCatalog, db, observability::DiagnosticService, providers::ProviderRoots,
};

use super::{
    copy_source_to_staging, inspect_source, valid_relative_path, write_update_recovery_manifest,
    ImportSourceKind, OperationPlanStatus, OperationResultStatus, OperationsService,
    MAX_IMPORT_FILES, MAX_IMPORT_RESOURCE_BYTES,
};

struct Fixture {
    temporary: TempDir,
    home: PathBuf,
    database_path: PathBuf,
    service: OperationsService,
}

impl Fixture {
    fn new() -> Self {
        let temporary = tempfile::tempdir().expect("temporary import fixture");
        let home = temporary.path().join("home");
        let database_path = temporary.path().join("data.db");
        assert!(matches!(
            db::initialize(database_path.clone()),
            db::AppDatabase::Ready(_)
        ));
        let roots = ProviderRoots::new(
            home.clone(),
            temporary.path().join("repository"),
            temporary.path().join("plugin-cache"),
        );
        let catalog = SkillCatalog::with_index_path(roots, database_path.clone());
        let service = OperationsService::new(
            Some(database_path.clone()),
            Some(temporary.path().join("app-local")),
            catalog,
            DiagnosticService::new(None, None),
        );
        Self {
            temporary,
            home,
            database_path,
            service,
        }
    }

    fn source_root(&self, name: &str) -> PathBuf {
        self.temporary.path().join("sources").join(name)
    }

    fn write_valid_skill(&self, name: &str) -> PathBuf {
        let root = self.source_root(name);
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("SKILL.md"), valid_markdown(name)).unwrap();
        root
    }

    fn target(&self, name: &str) -> PathBuf {
        self.home.join(".agents/skills").join(name)
    }

    fn plan_directory(&self, source: PathBuf) -> super::PlannedImport {
        let selection = self
            .service
            .select_source(ImportSourceKind::Directory, source)
            .unwrap();
        self.service.plan_import(&selection.token).unwrap()
    }

    fn execute_plan(&self, plan: &super::PlannedImport) -> super::OperationResult {
        self.service
            .execute_import(&plan.confirmation_token.as_ref().unwrap().token)
            .unwrap()
    }

    fn open_database(&self) -> Connection {
        Connection::open(&self.database_path).unwrap()
    }

    fn import_skill(&self, name: &str) -> super::OperationResult {
        self.execute_plan(&self.plan_directory(self.write_valid_skill(name)))
    }

    fn quarantine_plan(&self, skill_id: &str) -> super::PlannedImport {
        self.service.plan_quarantine(skill_id).unwrap()
    }

    fn execute_quarantine(&self, plan: &super::PlannedImport) -> super::OperationResult {
        self.service
            .execute_managed(&plan.confirmation_token.as_ref().unwrap().token, None)
            .unwrap()
    }

    fn quarantine_path(&self, entry_id: &str) -> PathBuf {
        self.temporary
            .path()
            .join("app-local/quarantine")
            .join(entry_id)
    }
}

fn valid_markdown(name: &str) -> String {
    format!(
        "---\nname: {name}\ndescription: A safe local import fixture.\n---\n# Overview\nFixture body.\n"
    )
}

fn recovery_manifest(
    fixture: &Fixture,
    skill_id: &str,
    target_name: &str,
    operation_id: &str,
    new_hash: &str,
    phase: super::UpdateRecoveryPhase,
) -> (PathBuf, PathBuf) {
    let backup_root = fixture
        .temporary
        .path()
        .join("app-local/update-backups")
        .join(operation_id);
    let backup_skill = backup_root.join(target_name);
    fs::create_dir_all(&backup_root).unwrap();
    let current =
        inspect_source(ImportSourceKind::Directory, &fixture.target(target_name)).unwrap();
    copy_source_to_staging(&current, &backup_skill).unwrap();
    let receipt = fixture.service.install_receipt(skill_id).unwrap();
    let manifest = super::UpdateRecoveryManifest {
        version: super::UPDATE_RECOVERY_MANIFEST_VERSION,
        operation_id: operation_id.to_owned(),
        skill_id: skill_id.to_owned(),
        target_name: target_name.to_owned(),
        old_receipt: super::UpdateRecoveryReceipt::from_old(&receipt),
        old_hash: current.source_hash,
        new_receipt: super::UpdateRecoveryReceipt {
            source_type: "github".to_owned(),
            source_url: Some("https://github.com/openai/codex".to_owned()),
            repo_ref: Some("main".to_owned()),
            commit_sha: Some("2222222222222222222222222222222222222222".to_owned()),
            subdirectory: Some(format!("skills/{target_name}")),
            installed_hash: new_hash.to_owned(),
            installed_at: None,
            managed_by: "codex-o".to_owned(),
        },
        new_hash: new_hash.to_owned(),
        phase,
        backup_relative_path: target_name.to_owned(),
    };
    write_update_recovery_manifest(&backup_root, &manifest).unwrap();
    (backup_root, backup_skill)
}

fn recovery_service(fixture: &Fixture) -> OperationsService {
    OperationsService::new(
        Some(fixture.database_path.clone()),
        Some(fixture.temporary.path().join("app-local")),
        fixture.service.catalog.clone(),
        DiagnosticService::new(None, None),
    )
}

#[test]
fn startup_recovery_restores_after_old_directory_was_moved() {
    let fixture = Fixture::new();
    let result = fixture.import_skill("recover-moved");
    let operation_id = "a".repeat(64);
    let (backup_root, _) = recovery_manifest(
        &fixture,
        &result.skill_id,
        "recover-moved",
        &operation_id,
        &"b".repeat(64),
        super::UpdateRecoveryPhase::OldMoved,
    );
    let previous = fixture
        .target("recover-moved")
        .parent()
        .unwrap()
        .join(format!(
            ".codex-o-update-{operation_id}/previous/recover-moved"
        ));
    fs::create_dir_all(previous.parent().unwrap()).unwrap();
    fs::rename(fixture.target("recover-moved"), &previous).unwrap();

    let _service = recovery_service(&fixture);

    assert!(fixture.target("recover-moved").is_dir());
    assert!(!backup_root.exists());
    assert!(!previous.ancestors().nth(2).unwrap().exists());
}

#[test]
fn startup_recovery_cleans_prepared_backup_without_touching_active_skill() {
    let fixture = Fixture::new();
    let result = fixture.import_skill("recover-prepared");
    let operation_id = "f".repeat(64);
    let (backup_root, _) = recovery_manifest(
        &fixture,
        &result.skill_id,
        "recover-prepared",
        &operation_id,
        &"a".repeat(64),
        super::UpdateRecoveryPhase::Prepared,
    );

    let _service = recovery_service(&fixture);

    assert!(fixture.target("recover-prepared").is_dir());
    assert!(!backup_root.exists());
}

#[test]
fn startup_recovery_restores_new_files_when_receipt_is_old() {
    let fixture = Fixture::new();
    let result = fixture.import_skill("recover-receipt");
    let operation_id = "b".repeat(64);
    let new_content = format!("{}\nnew content", valid_markdown("recover-receipt"));
    let new_source = fixture.temporary.path().join("recover-receipt-new");
    fs::create_dir_all(&new_source).unwrap();
    fs::write(new_source.join("SKILL.md"), &new_content).unwrap();
    let new_hash = inspect_source(ImportSourceKind::Directory, &new_source)
        .unwrap()
        .source_hash;
    let (backup_root, backup_skill) = recovery_manifest(
        &fixture,
        &result.skill_id,
        "recover-receipt",
        &operation_id,
        &new_hash,
        super::UpdateRecoveryPhase::ReplacementInstalled,
    );
    let previous = fixture
        .target("recover-receipt")
        .parent()
        .unwrap()
        .join(format!(
            ".codex-o-update-{operation_id}/previous/recover-receipt"
        ));
    fs::create_dir_all(previous.parent().unwrap()).unwrap();
    let old = inspect_source(ImportSourceKind::Directory, &backup_skill).unwrap();
    copy_source_to_staging(&old, &previous).unwrap();
    fs::write(
        fixture.target("recover-receipt").join("SKILL.md"),
        new_content,
    )
    .unwrap();

    let _service = recovery_service(&fixture);

    assert!(
        fs::read_to_string(fixture.target("recover-receipt").join("SKILL.md"))
            .unwrap()
            .contains("Fixture body.")
    );
    assert!(!backup_root.exists());
}

#[test]
fn startup_recovery_cleans_completed_new_files_and_receipt() {
    let fixture = Fixture::new();
    let result = fixture.import_skill("recover-complete");
    let operation_id = "c".repeat(64);
    let new_content = format!("{}\nnew content", valid_markdown("recover-complete"));
    let new_source = fixture.temporary.path().join("recover-complete-new");
    fs::create_dir_all(&new_source).unwrap();
    fs::write(new_source.join("SKILL.md"), &new_content).unwrap();
    let new_hash = inspect_source(ImportSourceKind::Directory, &new_source)
        .unwrap()
        .source_hash;
    let (backup_root, _) = recovery_manifest(
        &fixture,
        &result.skill_id,
        "recover-complete",
        &operation_id,
        &new_hash,
        super::UpdateRecoveryPhase::ReceiptPersisted,
    );
    fs::write(
        fixture.target("recover-complete").join("SKILL.md"),
        new_content,
    )
    .unwrap();
    let receipt_hash = inspect_source(
        ImportSourceKind::Directory,
        &fixture.target("recover-complete"),
    )
    .unwrap()
    .source_hash;
    Connection::open(&fixture.database_path)
        .unwrap()
        .execute(
            "UPDATE install_receipts SET source_type = 'github', source_url = 'https://github.com/openai/codex', repo_ref = 'main', commit_sha = '2222222222222222222222222222222222222222', subdirectory = 'skills/recover-complete', installed_hash = ?1 WHERE skill_id = ?2",
            rusqlite::params![receipt_hash, result.skill_id],
        )
        .unwrap();
    let _service = recovery_service(&fixture);

    assert!(
        fs::read_to_string(fixture.target("recover-complete").join("SKILL.md"))
            .unwrap()
            .contains("new content")
    );
    assert!(!backup_root.exists());
}

#[test]
fn startup_recovery_preserves_backup_for_user_modified_target() {
    let fixture = Fixture::new();
    let result = fixture.import_skill("recover-ambiguous");
    let operation_id = "d".repeat(64);
    let (backup_root, _) = recovery_manifest(
        &fixture,
        &result.skill_id,
        "recover-ambiguous",
        &operation_id,
        &"e".repeat(64),
        super::UpdateRecoveryPhase::ReplacementInstalled,
    );
    fs::write(
        fixture.target("recover-ambiguous").join("SKILL.md"),
        "user modified after interruption",
    )
    .unwrap();

    let _service = recovery_service(&fixture);

    assert_eq!(
        fs::read_to_string(fixture.target("recover-ambiguous").join("SKILL.md")).unwrap(),
        "user modified after interruption"
    );
    assert!(backup_root.join("recover-ambiguous/SKILL.md").is_file());
}

#[test]
fn startup_recovery_is_idempotent_after_successful_restore() {
    let fixture = Fixture::new();
    let result = fixture.import_skill("recover-idempotent");
    let operation_id = "e".repeat(64);
    let (backup_root, _) = recovery_manifest(
        &fixture,
        &result.skill_id,
        "recover-idempotent",
        &operation_id,
        &"f".repeat(64),
        super::UpdateRecoveryPhase::OldMoved,
    );
    let previous = fixture
        .target("recover-idempotent")
        .parent()
        .unwrap()
        .join(format!(
            ".codex-o-update-{operation_id}/previous/recover-idempotent"
        ));
    fs::create_dir_all(previous.parent().unwrap()).unwrap();
    fs::rename(fixture.target("recover-idempotent"), &previous).unwrap();
    let _first = recovery_service(&fixture);
    let _second = recovery_service(&fixture);

    assert!(fixture.target("recover-idempotent").is_dir());
    assert!(!backup_root.exists());
}

#[test]
fn startup_recovery_rejects_invalid_manifest_without_deleting_backup() {
    let fixture = Fixture::new();
    let backup_root = fixture
        .temporary
        .path()
        .join("app-local/update-backups/not-an-operation");
    fs::create_dir_all(&backup_root).unwrap();
    fs::write(backup_root.join("manifest.json"), b"{}").unwrap();

    let _service = recovery_service(&fixture);

    assert!(backup_root.exists());
    assert!(backup_root.join("manifest.json").is_file());
}

#[cfg(unix)]
#[test]
fn startup_recovery_rejects_symlink_backup_entry() {
    use std::os::unix::fs::symlink;

    let fixture = Fixture::new();
    let target = fixture.temporary.path().join("outside-backup");
    fs::create_dir_all(&target).unwrap();
    let link = fixture
        .temporary
        .path()
        .join("app-local/update-backups/".to_owned() + &"a".repeat(64));
    fs::create_dir_all(link.parent().unwrap()).unwrap();
    symlink(&target, &link).unwrap();

    let _service = recovery_service(&fixture);

    assert!(fs::symlink_metadata(link).unwrap().file_type().is_symlink());
}

#[test]
fn directory_import_succeeds_through_plan_and_execute() {
    let fixture = Fixture::new();
    let source = fixture.write_valid_skill("directory-import");
    let plan = fixture.plan_directory(source);

    let result = fixture.execute_plan(&plan);

    assert_eq!(plan.plan.status, OperationPlanStatus::Ready);
    assert_eq!(result.status, OperationResultStatus::Succeeded);
    assert!(fixture
        .target("directory-import")
        .join("SKILL.md")
        .is_file());
}

#[test]
fn selected_skill_markdown_file_imports_its_parent_directory() {
    let fixture = Fixture::new();
    let source = fixture.write_valid_skill("file-import");
    let selection = fixture
        .service
        .select_source(ImportSourceKind::File, source.join("SKILL.md"))
        .unwrap();
    let plan = fixture.service.plan_import(&selection.token).unwrap();

    let result = fixture.execute_plan(&plan);

    assert_eq!(result.status, OperationResultStatus::Succeeded);
    assert!(fixture.target("file-import").is_dir());
}

#[test]
fn successful_import_persists_complete_install_receipt() {
    let fixture = Fixture::new();
    let plan = fixture.plan_directory(fixture.write_valid_skill("receipt"));
    let result = fixture.execute_plan(&plan);
    let connection = fixture.open_database();
    let receipt: (String, String, String, i64, String) = connection
        .query_row(
            "SELECT skill_id, source_type, installed_hash, installed_at, managed_by FROM install_receipts",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
        )
        .unwrap();

    assert_eq!(receipt.0, result.skill_id);
    assert_eq!(receipt.1, "local");
    assert_eq!(receipt.2, result.installed_hash);
    assert!(receipt.3 > 0);
    assert_eq!(receipt.4, "codex-o");
}

#[test]
fn successful_import_persists_management_operation_result() {
    let fixture = Fixture::new();
    let plan = fixture.plan_directory(fixture.write_valid_skill("operation-record"));
    let result = fixture.execute_plan(&plan);
    let connection = fixture.open_database();
    let operation: (String, String, String, String, String, i64) = connection
        .query_row(
            "SELECT id, skill_id, operation, status, result_json, completed_at FROM management_operations",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?)),
        )
        .unwrap();

    assert_eq!(operation.0, result.operation_id);
    assert_eq!(operation.1, result.skill_id);
    assert_eq!(operation.2, "skill_import");
    assert_eq!(operation.3, "succeeded");
    assert_eq!(operation.4, "\"succeeded\"");
    assert!(operation.5 > 0);
}

#[test]
fn successful_import_refreshes_catalog_with_managed_skill() {
    let fixture = Fixture::new();
    let plan = fixture.plan_directory(fixture.write_valid_skill("catalog-refresh"));
    let result = fixture.execute_plan(&plan);

    assert_eq!(
        fixture.service.catalog.managed_skill_id("catalog-refresh"),
        Some(result.skill_id)
    );
}

#[test]
fn existing_target_produces_conflict_plan_without_confirmation() {
    let fixture = Fixture::new();
    let source = fixture.write_valid_skill("conflict");
    fs::create_dir_all(fixture.target("conflict")).unwrap();

    let plan = fixture.plan_directory(source);

    assert_eq!(plan.plan.status, OperationPlanStatus::Conflict);
    assert!(plan.confirmation_token.is_none());
}

#[test]
fn target_created_after_confirmation_blocks_execution_without_overwrite() {
    let fixture = Fixture::new();
    let plan = fixture.plan_directory(fixture.write_valid_skill("late-conflict"));
    fs::create_dir_all(fixture.target("late-conflict")).unwrap();
    fs::write(fixture.target("late-conflict").join("keep.txt"), "keep").unwrap();

    let error = fixture
        .service
        .execute_import(&plan.confirmation_token.unwrap().token)
        .unwrap_err();

    assert_eq!(error.code, "conflict_detected");
    assert_eq!(
        fs::read_to_string(fixture.target("late-conflict").join("keep.txt")).unwrap(),
        "keep"
    );
}

#[test]
fn read_only_provider_permission_rejects_before_writing() {
    let mut fixture = Fixture::new();
    fixture.service.import_allowed = false;
    let plan = fixture.plan_directory(fixture.write_valid_skill("read-only"));

    let error = fixture
        .service
        .execute_import(&plan.confirmation_token.unwrap().token)
        .unwrap_err();

    assert_eq!(error.code, "provider_read_only");
    assert!(!fixture.target("read-only").exists());
}

#[cfg(unix)]
#[test]
fn symlinked_resource_is_rejected_without_following_target() {
    use std::os::unix::fs::symlink;

    let fixture = Fixture::new();
    let source = fixture.write_valid_skill("symlinked");
    fs::create_dir_all(source.join("resources")).unwrap();
    let outside = fixture.temporary.path().join("outside.txt");
    fs::write(&outside, "outside").unwrap();
    symlink(&outside, source.join("resources/link.txt")).unwrap();
    let selection = fixture
        .service
        .select_source(ImportSourceKind::Directory, source)
        .unwrap();

    let error = fixture.service.plan_import(&selection.token).unwrap_err();

    assert_eq!(error.code, "import_source_invalid");
    assert_eq!(fs::read_to_string(outside).unwrap(), "outside");
}

#[test]
fn parent_path_components_are_rejected() {
    assert!(!valid_relative_path("../escape"));
    assert!(!valid_relative_path("resources/../../escape"));
}

#[test]
fn unknown_top_level_file_is_rejected() {
    let fixture = Fixture::new();
    let source = fixture.write_valid_skill("unknown-layout");
    fs::write(source.join("unexpected.txt"), "unexpected").unwrap();

    let error = inspect_source(ImportSourceKind::Directory, &source).unwrap_err();

    assert_eq!(error.code, "import_source_invalid");
}

#[test]
fn oversized_resource_is_rejected_before_copy() {
    let fixture = Fixture::new();
    let source = fixture.write_valid_skill("oversized");
    fs::create_dir_all(source.join("resources")).unwrap();
    let resource = fs::File::create(source.join("resources/large.bin")).unwrap();
    resource.set_len(MAX_IMPORT_RESOURCE_BYTES + 1).unwrap();

    let error = inspect_source(ImportSourceKind::Directory, &source).unwrap_err();

    assert_eq!(error.code, "import_source_invalid");
}

#[test]
fn total_import_size_limit_is_enforced() {
    let fixture = Fixture::new();
    let source = fixture.write_valid_skill("total-size");
    fs::create_dir_all(source.join("resources")).unwrap();
    for index in 0..3 {
        let resource = fs::File::create(source.join(format!("resources/{index}.bin"))).unwrap();
        resource.set_len(12 * 1024 * 1024).unwrap();
    }

    let error = inspect_source(ImportSourceKind::Directory, &source).unwrap_err();

    assert_eq!(error.code, "import_source_invalid");
}

#[test]
fn file_count_limit_is_enforced() {
    let fixture = Fixture::new();
    let source = fixture.write_valid_skill("file-count");
    fs::create_dir_all(source.join("resources")).unwrap();
    for index in 0..MAX_IMPORT_FILES {
        fs::write(source.join(format!("resources/{index}.txt")), "x").unwrap();
    }

    let error = inspect_source(ImportSourceKind::Directory, &source).unwrap_err();

    assert_eq!(error.code, "import_source_invalid");
}

#[test]
fn invalid_utf8_skill_fails_in_staging_and_leaves_no_target() {
    let fixture = Fixture::new();
    let source = fixture.source_root("invalid-utf8");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("SKILL.md"), [0xff, 0xfe]).unwrap();
    let plan = fixture.plan_directory(source);

    let error = fixture
        .service
        .execute_import(&plan.confirmation_token.unwrap().token)
        .unwrap_err();

    assert_eq!(error.code, "import_source_invalid");
    assert!(!fixture.target("invalid-utf8").exists());
    assert!(staging_directories(&fixture).is_empty());
}

#[test]
fn malformed_frontmatter_fails_in_staging() {
    let fixture = Fixture::new();
    let source = fixture.source_root("bad-frontmatter");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("SKILL.md"), "---\nname: [\n---\n# Broken").unwrap();
    let plan = fixture.plan_directory(source);

    let error = fixture
        .service
        .execute_import(&plan.confirmation_token.unwrap().token)
        .unwrap_err();

    assert_eq!(error.code, "import_source_invalid");
    assert!(!fixture.target("bad-frontmatter").exists());
}

#[test]
fn missing_description_is_rejected() {
    let fixture = Fixture::new();
    let source = fixture.source_root("missing-description");
    fs::create_dir_all(&source).unwrap();
    fs::write(
        source.join("SKILL.md"),
        "---\nname: missing-description\n---\n# Overview\n",
    )
    .unwrap();
    let plan = fixture.plan_directory(source);

    let error = fixture
        .service
        .execute_import(&plan.confirmation_token.unwrap().token)
        .unwrap_err();

    assert_eq!(error.code, "import_source_invalid");
}

#[test]
fn declared_name_must_match_safe_target_directory() {
    let fixture = Fixture::new();
    let source = fixture.source_root("directory-name");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("SKILL.md"), valid_markdown("different-name")).unwrap();
    let plan = fixture.plan_directory(source);

    let error = fixture
        .service
        .execute_import(&plan.confirmation_token.unwrap().token)
        .unwrap_err();

    assert_eq!(error.code, "import_source_invalid");
}

#[test]
fn expired_selection_token_has_stable_error_and_zero_writes() {
    let fixture = Fixture::new();
    let selection = fixture
        .service
        .select_source(
            ImportSourceKind::Directory,
            fixture.write_valid_skill("expired-selection"),
        )
        .unwrap();
    fixture.service.expire_selection(&selection.token);

    let error = fixture.service.plan_import(&selection.token).unwrap_err();

    assert_eq!(error.code, "selection_token_expired");
    assert!(!fixture.target("expired-selection").exists());
}

#[test]
fn expired_confirmation_token_has_stable_error() {
    let fixture = Fixture::new();
    let plan = fixture.plan_directory(fixture.write_valid_skill("expired-confirmation"));
    let token = plan.confirmation_token.unwrap().token;
    fixture.service.expire_confirmation(&token);

    let error = fixture.service.execute_import(&token).unwrap_err();

    assert_eq!(error.code, "confirmation_token_expired");
    assert!(!fixture.target("expired-confirmation").exists());
}

#[test]
fn confirmation_token_is_one_time_and_replay_is_rejected() {
    let fixture = Fixture::new();
    let plan = fixture.plan_directory(fixture.write_valid_skill("one-time"));
    let token = plan.confirmation_token.as_ref().unwrap().token.clone();
    let _result = fixture.execute_plan(&plan);

    let error = fixture.service.execute_import(&token).unwrap_err();

    assert_eq!(error.code, "confirmation_token_replayed");
}

#[test]
fn source_hash_change_after_confirmation_is_rejected() {
    let fixture = Fixture::new();
    let source = fixture.write_valid_skill("changed-source");
    let plan = fixture.plan_directory(source.clone());
    fs::write(
        source.join("SKILL.md"),
        format!("{}\nchanged", valid_markdown("changed-source")),
    )
    .unwrap();

    let error = fixture
        .service
        .execute_import(&plan.confirmation_token.unwrap().token)
        .unwrap_err();

    assert_eq!(error.code, "source_changed");
    assert!(!fixture.target("changed-source").exists());
}

#[test]
fn unavailable_database_rejects_before_target_write() {
    let fixture = Fixture::new();
    let roots = ProviderRoots::new(
        fixture.temporary.path().join("no-db-home"),
        fixture.temporary.path().join("no-db-repo"),
        fixture.temporary.path().join("no-db-plugin"),
    );
    let catalog = SkillCatalog::new(roots);
    let service = OperationsService::new(
        None,
        Some(fixture.temporary.path().join("no-db-app-local")),
        catalog,
        DiagnosticService::new(None, None),
    );
    let source = fixture.write_valid_skill("no-database");
    let selection = service
        .select_source(ImportSourceKind::Directory, source)
        .unwrap();
    let plan = service.plan_import(&selection.token).unwrap();

    let error = service
        .execute_import(&plan.confirmation_token.unwrap().token)
        .unwrap_err();

    assert_eq!(error.code, "database_unavailable");
    assert!(!fixture
        .temporary
        .path()
        .join("no-db-home/.agents/skills/no-database")
        .exists());
}

#[test]
fn operation_plan_serialization_contains_no_path_or_token() {
    let fixture = Fixture::new();
    let plan = fixture.plan_directory(fixture.write_valid_skill("safe-dto"));

    let serialized = serde_json::to_string(&plan.plan).unwrap();

    assert!(!serialized.contains(fixture.temporary.path().to_str().unwrap()));
    assert!(!serialized.contains(&plan.confirmation_token.unwrap().token));
    assert!(serialized.contains("user_global"));
}

#[test]
fn selection_and_confirmation_tokens_are_not_persisted() {
    let fixture = Fixture::new();
    let selection = fixture
        .service
        .select_source(
            ImportSourceKind::Directory,
            fixture.write_valid_skill("memory-tokens"),
        )
        .unwrap();
    let plan = fixture.service.plan_import(&selection.token).unwrap();
    let confirmation = plan.confirmation_token.as_ref().unwrap().token.clone();
    let _result = fixture.execute_plan(&plan);
    let database_bytes = fs::read(&fixture.database_path).unwrap();

    assert!(!database_bytes
        .windows(selection.token.len())
        .any(|window| window == selection.token.as_bytes()));
    assert!(!database_bytes
        .windows(confirmation.len())
        .any(|window| window == confirmation.as_bytes()));
}

#[test]
fn nested_second_skill_marker_is_rejected() {
    let fixture = Fixture::new();
    let source = fixture.write_valid_skill("multiple-roots");
    fs::create_dir_all(source.join("references/other")).unwrap();
    fs::write(
        source.join("references/other/SKILL.md"),
        valid_markdown("other"),
    )
    .unwrap();

    let error = inspect_source(ImportSourceKind::Directory, &source).unwrap_err();

    assert_eq!(error.code, "import_source_invalid");
}

fn staging_directories(fixture: &Fixture) -> Vec<PathBuf> {
    let root = fixture.home.join(".agents/skills");
    fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            entry
                .file_name()
                .to_str()
                .filter(|name| name.starts_with(".codex-o-import-"))
                .map(|_| entry.path())
        })
        .collect()
}

#[test]
fn managed_skill_quarantine_uses_app_local_directory_and_hides_source() {
    let fixture = Fixture::new();
    let imported = fixture.import_skill("quarantine-rename");
    let plan = fixture.quarantine_plan(&imported.skill_id);

    let result = fixture.execute_quarantine(&plan);

    assert_eq!(result.status, OperationResultStatus::Succeeded);
    assert_eq!(
        result.entry_id.as_deref(),
        Some(result.operation_id.as_str())
    );
    assert!(!fixture.target("quarantine-rename").exists());
    assert!(fixture
        .quarantine_path(&result.operation_id)
        .join("SKILL.md")
        .is_file());
}

#[test]
fn unknown_user_skill_requires_its_display_name_acknowledgement() {
    let fixture = Fixture::new();
    let target = fixture.target("unknown-user");
    fs::create_dir_all(&target).unwrap();
    fs::write(target.join("SKILL.md"), valid_markdown("unknown-user")).unwrap();
    fixture.service.catalog.scan_skills();
    let skill_id = fixture
        .service
        .catalog
        .managed_skill_id("unknown-user")
        .unwrap();
    let plan = fixture.quarantine_plan(&skill_id);

    let error = fixture
        .service
        .execute_managed(&plan.confirmation_token.unwrap().token, Some("wrong"))
        .unwrap_err();

    assert_eq!(error.code, "acknowledgement_required");
    assert!(target.exists());
}

#[test]
fn quarantine_rejects_source_hash_change_after_confirmation() {
    let fixture = Fixture::new();
    let imported = fixture.import_skill("quarantine-changed");
    let plan = fixture.quarantine_plan(&imported.skill_id);
    fs::write(
        fixture.target("quarantine-changed").join("SKILL.md"),
        format!("{}\nchanged", valid_markdown("quarantine-changed")),
    )
    .unwrap();

    let error = fixture
        .service
        .execute_managed(&plan.confirmation_token.unwrap().token, None)
        .unwrap_err();

    assert_eq!(error.code, "source_changed");
    assert!(fixture.target("quarantine-changed").exists());
}

#[test]
fn quarantine_confirmation_expiry_and_replay_are_rejected() {
    let fixture = Fixture::new();
    let imported = fixture.import_skill("quarantine-token");
    let expired = fixture.quarantine_plan(&imported.skill_id);
    let expired_token = expired.confirmation_token.unwrap().token;
    fixture.service.expire_managed_confirmation(&expired_token);
    assert_eq!(
        fixture
            .service
            .execute_managed(&expired_token, None)
            .unwrap_err()
            .code,
        "confirmation_token_expired"
    );

    let plan = fixture.quarantine_plan(&imported.skill_id);
    let token = plan.confirmation_token.as_ref().unwrap().token.clone();
    let _ = fixture.execute_quarantine(&plan);
    assert_eq!(
        fixture
            .service
            .execute_managed(&token, None)
            .unwrap_err()
            .code,
        "confirmation_token_replayed"
    );
}

#[test]
fn copy_fallback_verifies_before_removing_source() {
    let fixture = Fixture::new();
    let imported = fixture.import_skill("quarantine-copy");
    fixture.service.force_copy_fallback();
    let plan = fixture.quarantine_plan(&imported.skill_id);

    let result = fixture.execute_quarantine(&plan);

    assert_eq!(result.status, OperationResultStatus::Succeeded);
    assert!(!fixture.target("quarantine-copy").exists());
    assert!(fixture.quarantine_path(&result.operation_id).exists());
}

#[test]
fn copy_verification_failure_leaves_the_original_skill_untouched() {
    let fixture = Fixture::new();
    let imported = fixture.import_skill("quarantine-copy-hash");
    fixture.service.force_copy_fallback();
    fixture.service.force_copy_verification_failure();
    let plan = fixture.quarantine_plan(&imported.skill_id);
    let token = plan.confirmation_token.as_ref().unwrap().token.clone();

    let error = fixture.service.execute_managed(&token, None).unwrap_err();

    assert_eq!(error.code, "import_failed");
    assert!(fixture
        .target("quarantine-copy-hash")
        .join("SKILL.md")
        .is_file());
    assert!(!fixture.quarantine_path(&plan.plan.id).exists());
    assert!(fixture
        .service
        .list_quarantine_entries()
        .unwrap()
        .is_empty());
}

#[test]
fn source_remove_failure_keeps_both_copies_and_marks_partial() {
    let fixture = Fixture::new();
    let imported = fixture.import_skill("quarantine-partial");
    fixture.service.force_copy_fallback();
    fixture.service.force_remove_failure();
    let plan = fixture.quarantine_plan(&imported.skill_id);

    let result = fixture.execute_quarantine(&plan);
    let entry = fixture.service.list_quarantine_entries().unwrap().remove(0);

    assert_eq!(result.status, OperationResultStatus::Partial);
    assert_eq!(entry.status, "partial");
    assert!(fixture.target("quarantine-partial").exists());
    assert!(fixture.quarantine_path(&result.operation_id).exists());
}

#[test]
fn quarantine_entries_survive_service_restart() {
    let fixture = Fixture::new();
    let imported = fixture.import_skill("quarantine-restart");
    let result = fixture.execute_quarantine(&fixture.quarantine_plan(&imported.skill_id));
    let roots = ProviderRoots::new(
        fixture.home.clone(),
        fixture.temporary.path().join("repository"),
        fixture.temporary.path().join("plugin-cache"),
    );
    let restarted = OperationsService::new(
        Some(fixture.database_path.clone()),
        Some(fixture.temporary.path().join("app-local")),
        SkillCatalog::with_index_path(roots, fixture.database_path.clone()),
        DiagnosticService::new(None, None),
    );

    let entries = restarted.list_quarantine_entries().unwrap();

    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].id, result.operation_id);
}

#[test]
fn restore_moves_verified_quarantine_content_back_to_original_target() {
    let fixture = Fixture::new();
    let imported = fixture.import_skill("restore-success");
    let quarantine = fixture.execute_quarantine(&fixture.quarantine_plan(&imported.skill_id));
    let restore = fixture
        .service
        .plan_restore(&quarantine.operation_id)
        .unwrap();

    let result = fixture
        .service
        .execute_managed(&restore.confirmation_token.unwrap().token, None)
        .unwrap();

    assert_eq!(result.status, OperationResultStatus::Succeeded);
    assert!(fixture.target("restore-success").join("SKILL.md").is_file());
    assert!(!fixture.quarantine_path(&quarantine.operation_id).exists());
}

#[test]
fn restore_conflict_returns_plan_without_confirmation_and_never_overwrites() {
    let fixture = Fixture::new();
    let imported = fixture.import_skill("restore-conflict");
    let quarantine = fixture.execute_quarantine(&fixture.quarantine_plan(&imported.skill_id));
    fs::create_dir_all(fixture.target("restore-conflict")).unwrap();
    fs::write(fixture.target("restore-conflict").join("keep.txt"), "keep").unwrap();

    let plan = fixture
        .service
        .plan_restore(&quarantine.operation_id)
        .unwrap();

    assert_eq!(plan.plan.status, OperationPlanStatus::Conflict);
    assert!(plan.confirmation_token.is_none());
    assert_eq!(
        fs::read_to_string(fixture.target("restore-conflict").join("keep.txt")).unwrap(),
        "keep"
    );
}

#[test]
fn restore_rejects_changed_quarantine_content() {
    let fixture = Fixture::new();
    let imported = fixture.import_skill("restore-changed");
    let quarantine = fixture.execute_quarantine(&fixture.quarantine_plan(&imported.skill_id));
    fs::write(
        fixture
            .quarantine_path(&quarantine.operation_id)
            .join("SKILL.md"),
        format!("{}\nchanged", valid_markdown("restore-changed")),
    )
    .unwrap();

    let error = fixture
        .service
        .plan_restore(&quarantine.operation_id)
        .unwrap_err();

    assert_eq!(error.code, "quarantine_content_changed");
}

#[test]
fn purge_requires_exact_name_acknowledgement() {
    let fixture = Fixture::new();
    let imported = fixture.import_skill("purge-ack");
    let quarantine = fixture.execute_quarantine(&fixture.quarantine_plan(&imported.skill_id));
    let plan = fixture
        .service
        .plan_purge(&quarantine.operation_id)
        .unwrap();

    let error = fixture
        .service
        .execute_managed(&plan.confirmation_token.unwrap().token, Some("wrong"))
        .unwrap_err();

    assert_eq!(error.code, "acknowledgement_required");
    assert!(fixture.quarantine_path(&quarantine.operation_id).exists());
}

#[test]
fn partial_entry_cannot_be_purged() {
    let fixture = Fixture::new();
    let imported = fixture.import_skill("purge-partial");
    fixture.service.force_copy_fallback();
    fixture.service.force_remove_failure();
    let quarantine = fixture.execute_quarantine(&fixture.quarantine_plan(&imported.skill_id));

    let error = fixture
        .service
        .plan_purge(&quarantine.operation_id)
        .unwrap_err();

    assert_eq!(error.code, "quarantine_partial");
}

#[cfg(unix)]
#[test]
fn purge_rejects_symlinked_quarantine_entry() {
    use std::os::unix::fs::symlink;

    let fixture = Fixture::new();
    let imported = fixture.import_skill("purge-symlink");
    let quarantine = fixture.execute_quarantine(&fixture.quarantine_plan(&imported.skill_id));
    let path = fixture.quarantine_path(&quarantine.operation_id);
    fs::remove_dir_all(&path).unwrap();
    symlink(fixture.temporary.path().join("outside"), &path).unwrap();

    let error = fixture
        .service
        .plan_purge(&quarantine.operation_id)
        .unwrap_err();

    assert_eq!(error.code, "quarantine_not_allowed");
}

#[test]
fn purge_rejects_entry_when_original_path_is_active_again() {
    let fixture = Fixture::new();
    let imported = fixture.import_skill("purge-active");
    let quarantine = fixture.execute_quarantine(&fixture.quarantine_plan(&imported.skill_id));
    fs::create_dir_all(fixture.target("purge-active")).unwrap();
    fs::write(
        fixture.target("purge-active").join("SKILL.md"),
        valid_markdown("purge-active"),
    )
    .unwrap();
    let plan = fixture
        .service
        .plan_purge(&quarantine.operation_id)
        .unwrap();

    let error = fixture
        .service
        .execute_managed(
            &plan.confirmation_token.unwrap().token,
            Some("purge-active"),
        )
        .unwrap_err();

    assert_eq!(error.code, "quarantine_not_allowed");
    assert!(fixture.quarantine_path(&quarantine.operation_id).exists());
}

#[test]
fn quarantine_list_dto_hides_hash_and_original_relative_path() {
    let fixture = Fixture::new();
    let imported = fixture.import_skill("quarantine-list-dto");
    let _ = fixture.execute_quarantine(&fixture.quarantine_plan(&imported.skill_id));
    let serialized =
        serde_json::to_string(&fixture.service.list_quarantine_entries().unwrap()).unwrap();

    assert!(!serialized.contains("content_hash"));
    assert!(!serialized.contains("original_relative_path"));
}

#[test]
fn quarantine_plan_lists_each_relative_file_without_an_absolute_path() {
    let fixture = Fixture::new();
    let imported = fixture.import_skill("quarantine-files");
    fs::create_dir_all(fixture.target("quarantine-files").join("resources")).unwrap();
    fs::write(
        fixture
            .target("quarantine-files")
            .join("resources/check.md"),
        "check",
    )
    .unwrap();
    let plan = fixture.quarantine_plan(&imported.skill_id);
    let serialized = serde_json::to_string(&plan.plan).unwrap();

    assert_eq!(
        plan.plan.impact.relative_files,
        vec!["SKILL.md", "resources/check.md"]
    );
    assert!(!serialized.contains(fixture.temporary.path().to_str().unwrap()));
}

#[test]
fn quarantine_database_row_never_contains_fixture_absolute_path() {
    let fixture = Fixture::new();
    let imported = fixture.import_skill("quarantine-private");
    let _ = fixture.execute_quarantine(&fixture.quarantine_plan(&imported.skill_id));
    let bytes = fs::read(&fixture.database_path).unwrap();

    assert!(!bytes
        .windows(fixture.temporary.path().as_os_str().len())
        .any(|window| { window == fixture.temporary.path().as_os_str().as_encoded_bytes() }));
}

#[test]
fn restore_confirmation_expiry_and_replay_are_rejected() {
    let fixture = Fixture::new();
    let imported = fixture.import_skill("restore-token");
    let quarantine = fixture.execute_quarantine(&fixture.quarantine_plan(&imported.skill_id));
    let expired = fixture
        .service
        .plan_restore(&quarantine.operation_id)
        .unwrap();
    let token = expired.confirmation_token.unwrap().token;
    fixture.service.expire_managed_confirmation(&token);
    assert_eq!(
        fixture
            .service
            .execute_managed(&token, None)
            .unwrap_err()
            .code,
        "confirmation_token_expired"
    );

    let plan = fixture
        .service
        .plan_restore(&quarantine.operation_id)
        .unwrap();
    let replay = plan.confirmation_token.as_ref().unwrap().token.clone();
    let _ = fixture.service.execute_managed(&replay, None).unwrap();
    assert_eq!(
        fixture
            .service
            .execute_managed(&replay, None)
            .unwrap_err()
            .code,
        "confirmation_token_replayed"
    );
}

#[test]
fn successful_purge_removes_only_the_recorded_entry() {
    let fixture = Fixture::new();
    let imported = fixture.import_skill("purge-success");
    let quarantine = fixture.execute_quarantine(&fixture.quarantine_plan(&imported.skill_id));
    let plan = fixture
        .service
        .plan_purge(&quarantine.operation_id)
        .unwrap();

    let result = fixture
        .service
        .execute_managed(
            &plan.confirmation_token.unwrap().token,
            Some("purge-success"),
        )
        .unwrap();

    assert_eq!(result.status, OperationResultStatus::Succeeded);
    assert!(fixture
        .service
        .list_quarantine_entries()
        .unwrap()
        .is_empty());
    assert!(!fixture.quarantine_path(&quarantine.operation_id).exists());
}

#[test]
fn rename_fast_path_verifies_destination_and_rolls_back_on_failure() {
    let fixture = Fixture::new();
    let imported = fixture.import_skill("rename-verify");
    fixture.service.force_rename_verification_failure();
    let plan = fixture.quarantine_plan(&imported.skill_id);

    let error = fixture
        .service
        .execute_managed(&plan.confirmation_token.unwrap().token, None)
        .unwrap_err();

    assert_eq!(error.code, "import_failed");
    assert!(fixture.target("rename-verify").is_dir());
    assert!(!fixture.quarantine_path(&plan.plan.id).exists());
    assert!(fixture
        .service
        .list_quarantine_entries()
        .unwrap()
        .is_empty());
}

#[test]
fn restore_database_finalization_failure_rolls_back_to_quarantine() {
    let fixture = Fixture::new();
    let imported = fixture.import_skill("restore-finalize-rollback");
    let quarantine = fixture.execute_quarantine(&fixture.quarantine_plan(&imported.skill_id));
    fixture.service.force_status_update_failures(1);
    let restore = fixture
        .service
        .plan_restore(&quarantine.operation_id)
        .unwrap();

    let error = fixture
        .service
        .execute_managed(&restore.confirmation_token.unwrap().token, None)
        .unwrap_err();

    assert_eq!(error.code, "database_unavailable");
    assert!(fixture.quarantine_path(&quarantine.operation_id).is_dir());
    assert!(!fixture.target("restore-finalize-rollback").exists());
    assert_eq!(
        fixture.service.list_quarantine_entries().unwrap()[0].status,
        "quarantined"
    );
}

#[test]
fn restore_rollback_failure_enters_partial_state() {
    let fixture = Fixture::new();
    let imported = fixture.import_skill("restore-rollback-partial");
    let quarantine = fixture.execute_quarantine(&fixture.quarantine_plan(&imported.skill_id));
    fixture.service.force_status_update_failures(1);
    fixture.service.force_move_failure_after(1);
    let restore = fixture
        .service
        .plan_restore(&quarantine.operation_id)
        .unwrap();

    let error = fixture
        .service
        .execute_managed(&restore.confirmation_token.unwrap().token, None)
        .unwrap_err();

    assert_eq!(error.code, "quarantine_partial");
    assert!(fixture.target("restore-rollback-partial").is_dir());
    let partial_status: String = fixture
        .open_database()
        .query_row(
            "SELECT status FROM quarantine_entries WHERE id = ?1",
            [&quarantine.operation_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(partial_status, "partial");
    assert_eq!(
        fixture.service.list_quarantine_entries().unwrap()[0].status,
        "restored"
    );
}

#[test]
fn persistent_restore_database_failure_converges_the_active_copy() {
    let fixture = Fixture::new();
    let imported = fixture.import_skill("restore-persistent-database-failure");
    let quarantine = fixture.execute_quarantine(&fixture.quarantine_plan(&imported.skill_id));
    fixture.service.force_status_update_failures(2);
    fixture.service.force_move_failure_after(1);
    let restore = fixture
        .service
        .plan_restore(&quarantine.operation_id)
        .unwrap();

    let error = fixture
        .service
        .execute_managed(&restore.confirmation_token.unwrap().token, None)
        .unwrap_err();

    assert_eq!(error.code, "quarantine_partial");
    assert!(fixture
        .target("restore-persistent-database-failure")
        .is_dir());
    assert!(!fixture.quarantine_path(&quarantine.operation_id).exists());
    let stale_status: String = fixture
        .open_database()
        .query_row(
            "SELECT status FROM quarantine_entries WHERE id = ?1",
            [&quarantine.operation_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(stale_status, "quarantined");
    let retry_error = fixture
        .service
        .plan_restore(&quarantine.operation_id)
        .unwrap_err();
    assert_eq!(retry_error.code, "quarantine_not_allowed");
    let converged_status: String = fixture
        .open_database()
        .query_row(
            "SELECT status FROM quarantine_entries WHERE id = ?1",
            [&quarantine.operation_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(converged_status, "restored");
    assert_eq!(
        fixture.service.list_quarantine_entries().unwrap()[0].status,
        "restored"
    );
}

#[test]
fn partial_entry_can_keep_the_active_copy() {
    let fixture = Fixture::new();
    let imported = fixture.import_skill("partial-keep-active");
    fixture.service.force_copy_fallback();
    fixture.service.force_remove_failure();
    let quarantine = fixture.execute_quarantine(&fixture.quarantine_plan(&imported.skill_id));
    let plan = fixture
        .service
        .plan_keep_active(&quarantine.operation_id)
        .unwrap();

    let result = fixture
        .service
        .execute_managed(&plan.confirmation_token.unwrap().token, None)
        .unwrap();

    assert_eq!(result.status, OperationResultStatus::Succeeded);
    assert!(fixture.target("partial-keep-active").is_dir());
    assert!(!fixture.quarantine_path(&quarantine.operation_id).exists());
    assert_eq!(
        fixture.service.list_quarantine_entries().unwrap()[0].status,
        "restored"
    );
}

#[test]
fn partial_entry_can_complete_quarantine() {
    let fixture = Fixture::new();
    let imported = fixture.import_skill("partial-complete-quarantine");
    fixture.service.force_copy_fallback();
    fixture.service.force_remove_failure();
    let quarantine = fixture.execute_quarantine(&fixture.quarantine_plan(&imported.skill_id));
    let plan = fixture
        .service
        .plan_complete_quarantine(&quarantine.operation_id)
        .unwrap();

    let result = fixture
        .service
        .execute_managed(&plan.confirmation_token.unwrap().token, None)
        .unwrap();

    assert_eq!(result.status, OperationResultStatus::Succeeded);
    assert!(!fixture.target("partial-complete-quarantine").exists());
    assert!(fixture.quarantine_path(&quarantine.operation_id).is_dir());
    assert_eq!(
        fixture.service.list_quarantine_entries().unwrap()[0].status,
        "quarantined"
    );
}

#[test]
fn partial_entry_rejects_content_that_no_longer_matches() {
    let fixture = Fixture::new();
    let imported = fixture.import_skill("partial-changed");
    fixture.service.force_copy_fallback();
    fixture.service.force_remove_failure();
    let quarantine = fixture.execute_quarantine(&fixture.quarantine_plan(&imported.skill_id));
    fs::write(
        fixture.target("partial-changed").join("SKILL.md"),
        valid_markdown("partial-changed") + "changed",
    )
    .unwrap();

    let error = fixture
        .service
        .plan_keep_active(&quarantine.operation_id)
        .unwrap_err();

    assert_eq!(error.code, "quarantine_content_changed");
}

#[test]
fn purging_row_converges_after_database_finalization_failure_and_restart() {
    let fixture = Fixture::new();
    let imported = fixture.import_skill("purging-restart");
    let quarantine = fixture.execute_quarantine(&fixture.quarantine_plan(&imported.skill_id));
    let plan = fixture
        .service
        .plan_purge(&quarantine.operation_id)
        .unwrap();
    fixture.service.force_delete_entry_failures(1);

    let error = fixture
        .service
        .execute_managed(
            &plan.confirmation_token.unwrap().token,
            Some("purging-restart"),
        )
        .unwrap_err();

    assert_eq!(error.code, "database_unavailable");
    assert!(!fixture.quarantine_path(&quarantine.operation_id).exists());
    let roots = ProviderRoots::new(
        fixture.home.clone(),
        fixture.temporary.path().join("repository"),
        fixture.temporary.path().join("plugin-cache"),
    );
    let restarted = OperationsService::new(
        Some(fixture.database_path.clone()),
        Some(fixture.temporary.path().join("app-local")),
        SkillCatalog::with_index_path(roots, fixture.database_path.clone()),
        DiagnosticService::new(None, None),
    );

    assert!(restarted.list_quarantine_entries().unwrap().is_empty());
}

#[test]
fn purge_revalidates_quarantine_content_before_deleting() {
    let fixture = Fixture::new();
    let imported = fixture.import_skill("purge-revalidate");
    let quarantine = fixture.execute_quarantine(&fixture.quarantine_plan(&imported.skill_id));
    let plan = fixture
        .service
        .plan_purge(&quarantine.operation_id)
        .unwrap();
    let planned_content = valid_markdown("purge-revalidate");
    // Preserve file count and byte size so only the planned content hash can reject deletion.
    let changed_content = planned_content.replacen("Fixture body.", "Fixture b0dy.", 1);
    assert_eq!(changed_content.len(), planned_content.len());
    fs::write(
        fixture
            .quarantine_path(&quarantine.operation_id)
            .join("SKILL.md"),
        changed_content,
    )
    .unwrap();

    let error = fixture
        .service
        .execute_managed(
            &plan.confirmation_token.unwrap().token,
            Some("purge-revalidate"),
        )
        .unwrap_err();

    assert_eq!(error.code, "quarantine_content_changed");
    assert!(fixture.quarantine_path(&quarantine.operation_id).is_dir());
    assert_eq!(
        fixture.service.list_quarantine_entries().unwrap()[0].status,
        "quarantined"
    );
}

#[test]
fn status_transition_is_atomic_when_management_operation_is_missing() {
    let fixture = Fixture::new();
    let imported = fixture.import_skill("atomic-status");
    let quarantine = fixture.execute_quarantine(&fixture.quarantine_plan(&imported.skill_id));
    fixture
        .open_database()
        .execute(
            "DELETE FROM management_operations WHERE id = ?1",
            [&quarantine.operation_id],
        )
        .unwrap();

    let error = fixture
        .service
        .update_quarantine_status(&quarantine.operation_id, "restored", Some(2))
        .unwrap_err();

    assert_eq!(error.code, "quarantine_entry_not_found");
    let status: String = fixture
        .open_database()
        .query_row(
            "SELECT status FROM quarantine_entries WHERE id = ?1",
            [&quarantine.operation_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(status, "quarantined");
}

#[test]
fn operation_result_dto_hides_hash_and_confirmation_token() {
    let fixture = Fixture::new();
    let result = fixture.import_skill("result-private");
    let plan = fixture.quarantine_plan(&result.skill_id);
    let serialized_result = serde_json::to_string(&result).unwrap();
    let serialized_plan = serde_json::to_string(&plan.plan).unwrap();
    let database = fs::read(&fixture.database_path).unwrap();

    assert!(!serialized_result.contains("installed_hash"));
    assert!(!serialized_plan.contains(&plan.confirmation_token.unwrap().token));
    assert!(!database
        .windows(fixture.temporary.path().as_os_str().len())
        .any(|window| { window == fixture.temporary.path().as_os_str().as_encoded_bytes() }));
}

#[test]
fn invalid_quarantine_entry_id_cannot_select_a_directory() {
    let fixture = Fixture::new();
    fixture.open_database().execute(
        "INSERT INTO quarantine_entries(id, operation_id, skill_id, provider_id, original_relative_path, content_hash, display_name, file_count, total_size_bytes, status, quarantined_at) VALUES('invalid', 'invalid', 'skill', 'user_global', 'escape', 'hash', 'escape', 1, 1, 'quarantined', 1)",
        [],
    ).unwrap();

    let error = fixture.service.plan_purge("invalid").unwrap_err();

    assert_eq!(error.code, "quarantine_not_allowed");
}

#[test]
fn read_only_provider_candidate_is_rejected_before_a_plan_is_issued() {
    let fixture = Fixture::new();
    let repo_skill = fixture
        .temporary
        .path()
        .join("repository/.agents/skills/repo-skill");
    fs::create_dir_all(&repo_skill).unwrap();
    fs::write(repo_skill.join("SKILL.md"), valid_markdown("repo-skill")).unwrap();
    fixture.service.catalog.scan_skills();
    let skill = fixture
        .service
        .catalog
        .list_skills(Default::default())
        .skills
        .into_iter()
        .find(|skill| skill.provider.id == "repo")
        .unwrap();

    let error = fixture.service.plan_quarantine(&skill.id).unwrap_err();

    assert_eq!(error.code, "quarantine_not_allowed");
}
