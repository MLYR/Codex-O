use std::path::PathBuf;

use rusqlite::{Connection, ErrorCode};
use tempfile::TempDir;

use crate::app_error::AppErrorCode;

use super::{inspect, inspect_connection, open_read_only, CompatibilityReport};

const CURRENT_SCHEMA: &str = include_str!("schema.sql");
const MISSING_OPTIONAL_SCHEMA: &str = include_str!("missing_optional.sql");

#[test]
fn current_fixture_opens_read_only_and_rejects_writes() {
    let fixture = Fixture::current();
    let connection = open_read_only(&fixture.path()).unwrap();

    let error = connection
        .execute(
            "INSERT INTO threads (id, created_at, tokens_used) VALUES ('write-test', 3, 1)",
            [],
        )
        .unwrap_err();

    assert_eq!(error.sqlite_error_code(), Some(ErrorCode::ReadOnly));
}

#[test]
fn current_fixture_has_all_required_and_optional_thread_fields() {
    let report = Fixture::current().inspect();

    assert!(report.is_compatible_for_listing());
    assert_eq!(report.missing_required_columns, 0);
    assert!(report.optional_capabilities.title);
    assert!(report.optional_capabilities.cwd);
    assert!(report.optional_capabilities.source);
    assert!(report.optional_capabilities.model);
    assert!(report.optional_capabilities.model_provider);
    assert!(report.optional_capabilities.preview);
    assert!(report.optional_capabilities.first_user_message);
    assert!(report.optional_capabilities.rollout_path);
    assert!(report.optional_capabilities.archived);
}

#[test]
fn foreign_keys_disabled_fixture_still_reports_known_relations() {
    let fixture = Fixture::current();
    let connection = open_read_only(&fixture.path()).unwrap();
    // This is connection-local fixture setup; the adapter only reads the setting.
    connection
        .execute_batch("PRAGMA foreign_keys = OFF")
        .unwrap();
    let report = inspect_connection(&connection).unwrap();

    assert!(!report.foreign_keys_enabled);
    assert_eq!(report.known_relation_counts.dynamic_tools, 1);
    assert_eq!(report.known_relation_counts.spawn_edges, 1);
}

#[test]
fn json_source_is_counted_without_returning_its_content() {
    let report = Fixture::current().inspect();

    assert_eq!(report.json_source_count, 1);
    assert!(!format!("{report:?}").contains("subagent"));
}

#[test]
fn missing_optional_columns_only_disable_optional_capabilities() {
    let report = Fixture::with_schema(MISSING_OPTIONAL_SCHEMA).inspect();

    assert!(report.is_compatible_for_listing());
    assert_eq!(report.missing_required_columns, 0);
    assert_eq!(report.optional_capabilities, Default::default());
    assert_eq!(report.json_source_count, 0);
}

#[test]
fn missing_required_columns_are_reported_without_a_write_attempt() {
    let fixture = Fixture::with_schema(
        "
        CREATE TABLE threads (id TEXT PRIMARY KEY, created_at INTEGER NOT NULL);
        ",
    );

    let report = fixture.inspect();

    assert!(!report.is_compatible_for_listing());
    assert_eq!(report.missing_required_columns, 1);
}

#[test]
fn unknown_thread_relations_are_counted_for_future_delete_gating() {
    let fixture = Fixture::current();
    fixture.execute("CREATE TABLE thread_future_links (thread_id TEXT NOT NULL)");

    let report = fixture.inspect();

    assert_eq!(report.unknown_relation_tables, 1);
}

#[test]
fn missing_fixture_path_is_mapped_to_a_stable_error() {
    let temporary_directory = TempDir::new().unwrap();
    let missing_path = temporary_directory.path().join("missing.db");

    let error = inspect(&missing_path).unwrap_err();

    assert_eq!(error.code, AppErrorCode::DatabaseNotFound);
    assert_eq!(
        error.to_string(),
        "The selected Codex data source is unavailable."
    );
}

#[test]
fn report_never_contains_the_fixture_path() {
    let fixture = Fixture::current();
    let report = fixture.inspect();

    assert!(!format!("{report:?}").contains(fixture.path().to_str().unwrap()));
}

struct Fixture {
    directory: TempDir,
}

impl Fixture {
    fn current() -> Self {
        Self::with_schema(CURRENT_SCHEMA)
    }

    fn with_schema(schema: &str) -> Self {
        let directory = TempDir::new().unwrap();
        let connection = Connection::open(directory.path().join("fixture.db")).unwrap();
        connection.execute_batch(schema).unwrap();
        drop(connection);

        Self { directory }
    }

    fn path(&self) -> PathBuf {
        self.directory.path().join("fixture.db")
    }

    fn inspect(&self) -> CompatibilityReport {
        inspect(&self.path()).unwrap()
    }

    fn execute(&self, statement: &str) {
        Connection::open(self.path())
            .unwrap()
            .execute_batch(statement)
            .unwrap();
    }
}
