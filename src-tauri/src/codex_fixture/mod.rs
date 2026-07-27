use std::{path::Path, time::Duration};

use rusqlite::{Connection, OpenFlags};

use crate::app_error::AppError;

const REQUIRED_THREAD_COLUMNS: [&str; 3] = ["id", "created_at", "tokens_used"];
const OPTIONAL_THREAD_COLUMNS: [&str; 9] = [
    "title",
    "cwd",
    "source",
    "model",
    "model_provider",
    "preview",
    "first_user_message",
    "rollout_path",
    "archived",
];
const KNOWN_RELATION_TABLES: [&str; 2] = ["thread_dynamic_tools", "thread_spawn_edges"];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OptionalThreadCapabilities {
    pub title: bool,
    pub cwd: bool,
    pub source: bool,
    pub model: bool,
    pub model_provider: bool,
    pub preview: bool,
    pub first_user_message: bool,
    pub rollout_path: bool,
    pub archived: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct KnownRelationCounts {
    pub dynamic_tools: u64,
    pub spawn_edges: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompatibilityReport {
    pub missing_required_columns: u8,
    pub optional_capabilities: OptionalThreadCapabilities,
    pub foreign_keys_enabled: bool,
    pub known_relation_counts: KnownRelationCounts,
    pub unknown_relation_tables: u8,
    pub json_source_count: u64,
}

impl CompatibilityReport {
    pub const fn is_compatible_for_listing(&self) -> bool {
        self.missing_required_columns == 0
    }
}

pub fn inspect(database_path: &Path) -> Result<CompatibilityReport, AppError> {
    let connection = open_read_only(database_path)?;
    inspect_connection(&connection)
}

fn open_read_only(database_path: &Path) -> Result<Connection, AppError> {
    let connection = Connection::open_with_flags(database_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|_| AppError::database_not_found())?;
    connection
        .busy_timeout(Duration::from_millis(250))
        .map_err(|_| AppError::database_busy())?;

    Ok(connection)
}

fn inspect_connection(connection: &Connection) -> Result<CompatibilityReport, AppError> {
    let table_names = table_names(connection)?;
    let thread_columns = table_columns(connection, "threads")?;
    let missing_required_columns = REQUIRED_THREAD_COLUMNS
        .iter()
        .filter(|column| !thread_columns.iter().any(|available| available == **column))
        .count() as u8;
    let optional_capabilities = OptionalThreadCapabilities {
        title: thread_columns.contains(&OPTIONAL_THREAD_COLUMNS[0].to_owned()),
        cwd: thread_columns.contains(&OPTIONAL_THREAD_COLUMNS[1].to_owned()),
        source: thread_columns.contains(&OPTIONAL_THREAD_COLUMNS[2].to_owned()),
        model: thread_columns.contains(&OPTIONAL_THREAD_COLUMNS[3].to_owned()),
        model_provider: thread_columns.contains(&OPTIONAL_THREAD_COLUMNS[4].to_owned()),
        preview: thread_columns.contains(&OPTIONAL_THREAD_COLUMNS[5].to_owned()),
        first_user_message: thread_columns.contains(&OPTIONAL_THREAD_COLUMNS[6].to_owned()),
        rollout_path: thread_columns.contains(&OPTIONAL_THREAD_COLUMNS[7].to_owned()),
        archived: thread_columns.contains(&OPTIONAL_THREAD_COLUMNS[8].to_owned()),
    };
    let foreign_keys_enabled = connection
        .pragma_query_value(None, "foreign_keys", |row| row.get::<_, i64>(0))
        .map_err(|_| AppError::database_busy())?
        != 0;
    let known_relation_counts = KnownRelationCounts {
        dynamic_tools: count_rows(connection, "thread_dynamic_tools", &table_names)?,
        spawn_edges: count_rows(connection, "thread_spawn_edges", &table_names)?,
    };
    let unknown_relation_tables = table_names
        .iter()
        .filter(|name| {
            name.starts_with("thread_") && !KNOWN_RELATION_TABLES.contains(&name.as_str())
        })
        .count() as u8;
    let json_source_count = if optional_capabilities.source {
        count_json_sources(connection)?
    } else {
        0
    };

    Ok(CompatibilityReport {
        missing_required_columns,
        optional_capabilities,
        foreign_keys_enabled,
        known_relation_counts,
        unknown_relation_tables,
        json_source_count,
    })
}

fn table_names(connection: &Connection) -> Result<Vec<String>, AppError> {
    let mut statement = connection
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table'")
        .map_err(|_| AppError::database_schema_incompatible(0, 0))?;
    let rows = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|_| AppError::database_schema_incompatible(0, 0))?;

    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|_| AppError::database_schema_incompatible(0, 0))
}

fn table_columns(connection: &Connection, table_name: &str) -> Result<Vec<String>, AppError> {
    let query = format!("PRAGMA table_info({table_name})");
    let mut statement = connection
        .prepare(&query)
        .map_err(|_| AppError::database_schema_incompatible(0, 0))?;
    let rows = statement
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|_| AppError::database_schema_incompatible(0, 0))?;

    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|_| AppError::database_schema_incompatible(0, 0))
}

fn count_rows(
    connection: &Connection,
    table_name: &str,
    table_names: &[String],
) -> Result<u64, AppError> {
    if !table_names.iter().any(|name| name == table_name) {
        return Ok(0);
    }

    connection
        .query_row(&format!("SELECT COUNT(*) FROM {table_name}"), [], |row| {
            row.get::<_, i64>(0).map(|count| count as u64)
        })
        .map_err(|_| AppError::database_schema_incompatible(0, 0))
}

fn count_json_sources(connection: &Connection) -> Result<u64, AppError> {
    let mut statement = connection
        .prepare("SELECT source FROM threads WHERE source IS NOT NULL")
        .map_err(|_| AppError::database_schema_incompatible(0, 0))?;
    let mut rows = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|_| AppError::database_schema_incompatible(0, 0))?;

    rows.try_fold(0_u64, |count, source| {
        source
            .map(|source| count + u64::from(source.trim_start().starts_with('{')))
            .map_err(|_| AppError::database_schema_incompatible(0, 0))
    })
}

#[cfg(test)]
mod tests;
