use std::{fs, path::Path};

use rusqlite::{params, Connection, ErrorCode};
use tempfile::TempDir;

use super::{
    database_path, initialize, initialize_with_migrations,
    migrations::{Migration, CURRENT_SCHEMA_VERSION},
    open_configured_connection, DatabaseDiagnosticCode, DatabaseStatus,
};

const REQUIRED_TABLES: &[&str] = &[
    "artifact_snapshots",
    "providers",
    "skill_analyses",
    "skills",
];

#[test]
fn database_path_is_scoped_to_app_local_data() {
    let app_local_data = Path::new("/isolated-app-data");
    assert_eq!(
        database_path(app_local_data),
        app_local_data.join("data.db")
    );
}

#[test]
fn new_database_creates_v1_schema() {
    let fixture = DatabaseFixture::new();
    let database = initialize(fixture.database_path.clone());

    assert_eq!(
        database.status(),
        DatabaseStatus::Ready {
            schema_version: CURRENT_SCHEMA_VERSION
        }
    );
    assert_eq!(fixture.table_names(), REQUIRED_TABLES);
    assert_eq!(fixture.user_version(), CURRENT_SCHEMA_VERSION);
    assert!(fixture.backup_paths().is_empty());
}

#[test]
fn v1_schema_matches_design_columns() {
    let fixture = DatabaseFixture::new();
    let _database = initialize(fixture.database_path.clone());

    assert_eq!(
        fixture.table_columns("providers"),
        vec![
            "id",
            "kind",
            "root_path",
            "display_name",
            "read_only",
            "capabilities_json",
            "last_scan_at",
        ]
    );
    assert_eq!(
        fixture.table_columns("skills"),
        vec![
            "id",
            "provider_id",
            "relative_path",
            "display_name",
            "scope",
            "lifecycle_state",
            "latest_snapshot_id",
            "first_seen_at",
            "last_seen_at",
        ]
    );
    assert_eq!(
        fixture.table_columns("artifact_snapshots"),
        vec![
            "id",
            "skill_id",
            "content_hash",
            "parser_version",
            "manifest_json",
            "resources_json",
            "diagnostics_json",
            "created_at",
        ]
    );
    assert_eq!(
        fixture.table_columns("skill_analyses"),
        vec![
            "id",
            "snapshot_id",
            "analysis_key",
            "status",
            "passport_json",
            "evidence_json",
            "provider",
            "model",
            "prompt_version",
            "schema_version",
            "language",
            "created_at",
        ]
    );
}

#[test]
fn repeated_initialization_is_idempotent() {
    let fixture = DatabaseFixture::new();
    let _database = initialize(fixture.database_path.clone());
    let first_bytes = fs::read(&fixture.database_path).unwrap();

    let database = initialize(fixture.database_path.clone());

    assert_eq!(
        database.status(),
        DatabaseStatus::Ready {
            schema_version: CURRENT_SCHEMA_VERSION
        }
    );
    assert_eq!(fs::read(&fixture.database_path).unwrap(), first_bytes);
    assert!(fixture.backup_paths().is_empty());
}

#[test]
fn unique_provider_relative_path_is_enforced() {
    let fixture = DatabaseFixture::new();
    let _database = initialize(fixture.database_path.clone());
    let connection = fixture.open_configured();
    insert_provider(&connection);
    insert_skill(&connection, "skill-a", "shared/path").unwrap();
    let error = insert_skill(&connection, "skill-b", "shared/path").unwrap_err();
    assert_eq!(
        error.sqlite_error_code(),
        Some(ErrorCode::ConstraintViolation)
    );
}

#[test]
fn foreign_keys_are_enabled_and_enforced() {
    let fixture = DatabaseFixture::new();
    let _database = initialize(fixture.database_path.clone());
    let connection = fixture.open_configured();
    let enabled: u32 = connection
        .pragma_query_value(None, "foreign_keys", |row| row.get(0))
        .unwrap();
    assert_eq!(enabled, 1);

    let error = insert_skill(&connection, "orphan", "missing/provider").unwrap_err();
    assert_eq!(
        error.sqlite_error_code(),
        Some(ErrorCode::ConstraintViolation)
    );
}

#[test]
fn existing_v0_database_is_backed_up_before_migration() {
    let fixture = DatabaseFixture::new();
    fixture.create_legacy_database();
    let original_bytes = fs::read(&fixture.database_path).unwrap();

    let database = initialize(fixture.database_path.clone());

    assert_eq!(
        database.status(),
        DatabaseStatus::Ready {
            schema_version: CURRENT_SCHEMA_VERSION
        }
    );
    let backups = fixture.backup_paths();
    assert_eq!(backups.len(), 1);
    assert_eq!(fs::read(&backups[0]).unwrap(), original_bytes);
    assert_eq!(fixture.legacy_value(), "preserved");
}

#[test]
fn v2_database_is_backed_up_and_preserves_legacy_tables_during_v3_upgrade() {
    let fixture = DatabaseFixture::new();
    fixture.create_v2_database();
    let original_bytes = fs::read(&fixture.database_path).unwrap();

    let database = initialize(fixture.database_path.clone());

    assert_eq!(
        database.status(),
        DatabaseStatus::Ready {
            schema_version: CURRENT_SCHEMA_VERSION
        }
    );
    assert_eq!(fixture.user_version(), CURRENT_SCHEMA_VERSION);
    assert_eq!(fixture.v2_config_value(), "preserved-config");
    assert_eq!(fixture.v2_summary_value(), "preserved-summary");
    assert_eq!(fixture.backup_paths().len(), 1);
    assert_eq!(
        fs::read(&fixture.backup_paths()[0]).unwrap(),
        original_bytes
    );
    assert!(fixture
        .table_names()
        .iter()
        .all(|table| REQUIRED_TABLES.contains(&table.as_str())
            || table == "ai_config"
            || table == "skill_ai_summaries"));
    assert!(fixture.table_names().iter().any(|table| table == "skills"));
}

#[test]
fn backup_name_collision_does_not_overwrite_existing_backup() {
    let fixture = DatabaseFixture::new();
    fixture.create_legacy_database();
    let existing_backup = fixture
        .database_path
        .parent()
        .unwrap()
        .join("data.db.pre-v0.bak");
    fs::write(&existing_backup, b"keep this backup").unwrap();

    let _database = initialize(fixture.database_path.clone());

    assert_eq!(fs::read(&existing_backup).unwrap(), b"keep this backup");
    assert_eq!(fixture.backup_paths().len(), 2);
}

#[test]
fn failed_migration_restores_original_database() {
    let fixture = DatabaseFixture::new();
    fixture.create_legacy_database();
    let original_bytes = fs::read(&fixture.database_path).unwrap();
    let failing_migrations = [Migration {
        version: 1,
        sql: "CREATE TABLE partial(id INTEGER); INVALID SQL;",
    }];

    let database =
        initialize_with_migrations(fixture.database_path.clone(), &failing_migrations, 1);

    assert_eq!(
        database.status(),
        DatabaseStatus::Diagnostic(super::DatabaseDiagnostic {
            code: DatabaseDiagnosticCode::MigrationFailed,
            read_only: true,
            recovery: "Keep the backup and retry after installing a compatible Codex-O version.",
        })
    );
    assert_eq!(fs::read(&fixture.database_path).unwrap(), original_bytes);
    assert_eq!(fixture.legacy_value(), "preserved");
    assert_eq!(fixture.backup_paths().len(), 1);
}

#[test]
fn corrupt_database_is_preserved_in_diagnostic_mode() {
    let fixture = DatabaseFixture::new();
    let corrupt_bytes = b"not a sqlite database".to_vec();
    fs::write(&fixture.database_path, &corrupt_bytes).unwrap();

    let database = initialize(fixture.database_path.clone());

    assert_eq!(
        database.status(),
        DatabaseStatus::Diagnostic(super::DatabaseDiagnostic {
            code: DatabaseDiagnosticCode::CorruptDatabase,
            read_only: true,
            recovery:
                "Keep the original database unchanged and restore it from a known-good backup.",
        })
    );
    assert_eq!(fs::read(&fixture.database_path).unwrap(), corrupt_bytes);
    assert!(fixture.backup_paths().is_empty());
}

#[test]
fn future_schema_version_is_preserved_in_diagnostic_mode() {
    let fixture = DatabaseFixture::new();
    let connection = Connection::open(&fixture.database_path).unwrap();
    connection
        .pragma_update(None, "user_version", CURRENT_SCHEMA_VERSION + 1)
        .unwrap();
    connection
        .execute("CREATE TABLE future(id INTEGER)", [])
        .unwrap();
    drop(connection);
    let original_bytes = fs::read(&fixture.database_path).unwrap();

    let database = initialize(fixture.database_path.clone());

    assert_eq!(
        database.status(),
        DatabaseStatus::Diagnostic(super::DatabaseDiagnostic {
            code: DatabaseDiagnosticCode::UnsupportedSchemaVersion,
            read_only: true,
            recovery: "Upgrade Codex-O before opening this database again.",
        })
    );
    assert_eq!(fs::read(&fixture.database_path).unwrap(), original_bytes);
    assert!(fixture.backup_paths().is_empty());
}

struct DatabaseFixture {
    _directory: TempDir,
    database_path: std::path::PathBuf,
}

impl DatabaseFixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let database_path = database_path(directory.path());
        fs::create_dir_all(database_path.parent().unwrap()).unwrap();
        Self {
            _directory: directory,
            database_path,
        }
    }

    fn open(&self) -> Connection {
        Connection::open(&self.database_path).unwrap()
    }

    fn open_configured(&self) -> Connection {
        open_configured_connection(&self.database_path).unwrap()
    }

    fn create_legacy_database(&self) {
        let connection = self.open();
        connection
            .execute("CREATE TABLE legacy(value TEXT NOT NULL)", [])
            .unwrap();
        connection
            .execute("INSERT INTO legacy(value) VALUES ('preserved')", [])
            .unwrap();
    }

    fn create_v2_database(&self) {
        let connection = self.open();
        connection
            .execute(
                "CREATE TABLE ai_config(id INTEGER PRIMARY KEY, value TEXT NOT NULL)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "CREATE TABLE skill_ai_summaries(id INTEGER PRIMARY KEY, value TEXT NOT NULL)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO ai_config(value) VALUES ('preserved-config')",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO skill_ai_summaries(value) VALUES ('preserved-summary')",
                [],
            )
            .unwrap();
        connection.pragma_update(None, "user_version", 2).unwrap();
    }

    fn legacy_value(&self) -> String {
        self.open()
            .query_row("SELECT value FROM legacy", [], |row| row.get(0))
            .unwrap()
    }

    fn v2_config_value(&self) -> String {
        self.open()
            .query_row("SELECT value FROM ai_config", [], |row| row.get(0))
            .unwrap()
    }

    fn v2_summary_value(&self) -> String {
        self.open()
            .query_row("SELECT value FROM skill_ai_summaries", [], |row| row.get(0))
            .unwrap()
    }

    fn table_names(&self) -> Vec<String> {
        let connection = self.open();
        let mut statement = connection
            .prepare(
                "
                SELECT name
                FROM sqlite_schema
                WHERE type = 'table' AND name NOT LIKE 'sqlite_%'
                ORDER BY name
                ",
            )
            .unwrap();
        statement
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
    }

    fn table_columns(&self, table: &str) -> Vec<String> {
        let connection = self.open();
        let mut statement = connection
            .prepare(&format!("PRAGMA table_info({table})"))
            .unwrap();
        statement
            .query_map([], |row| row.get(1))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
    }

    fn user_version(&self) -> u32 {
        self.open()
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap()
    }

    fn backup_paths(&self) -> Vec<std::path::PathBuf> {
        let directory = self.database_path.parent().unwrap();
        let mut paths = fs::read_dir(directory)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| {
                path.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("data.db.pre-v")
            })
            .collect::<Vec<_>>();
        paths.sort();
        paths
    }
}

fn insert_provider(connection: &Connection) {
    connection
        .execute(
            "
            INSERT INTO providers(
                id, kind, root_path, display_name, read_only, capabilities_json
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            ",
            params!["provider", "user", "managed-root", "User", 0, "{}"],
        )
        .unwrap();
}

fn insert_skill(connection: &Connection, id: &str, relative_path: &str) -> rusqlite::Result<usize> {
    connection.execute(
        "
        INSERT INTO skills(
            id, provider_id, relative_path, display_name, scope,
            lifecycle_state, first_seen_at, last_seen_at
        ) VALUES (?1, 'provider', ?2, ?1, 'user', 'active', 1, 1)
        ",
        params![id, relative_path],
    )
}
