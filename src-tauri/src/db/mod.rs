mod migrations;

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use migrations::{apply_migrations, Migration, CURRENT_SCHEMA_VERSION, MIGRATIONS};
use rusqlite::{Connection, OpenFlags};

const DATABASE_FILE_NAME: &str = "data.db";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DatabaseDiagnosticCode {
    CorruptDatabase,
    UnsupportedSchemaVersion,
    MigrationFailed,
    StorageUnavailable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DatabaseDiagnostic {
    pub code: DatabaseDiagnosticCode,
    pub read_only: bool,
    pub recovery: &'static str,
}

pub struct ReadyDatabase {
    pub schema_version: u32,
}

pub enum AppDatabase {
    Ready(ReadyDatabase),
    Diagnostic(DatabaseDiagnostic),
}

impl AppDatabase {
    pub fn status(&self) -> DatabaseStatus {
        match self {
            Self::Ready(database) => DatabaseStatus::Ready {
                schema_version: database.schema_version,
            },
            Self::Diagnostic(diagnostic) => DatabaseStatus::Diagnostic(*diagnostic),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DatabaseStatus {
    Ready { schema_version: u32 },
    Diagnostic(DatabaseDiagnostic),
}

pub fn database_path(app_local_data_directory: &Path) -> PathBuf {
    app_local_data_directory.join(DATABASE_FILE_NAME)
}

pub fn initialize(database_path: PathBuf) -> AppDatabase {
    initialize_with_migrations(database_path, MIGRATIONS, CURRENT_SCHEMA_VERSION)
}

pub fn storage_unavailable() -> AppDatabase {
    diagnostic(
        DatabaseDiagnosticCode::StorageUnavailable,
        "Check that the Codex-O data directory is writable, then restart the application.",
    )
}

fn initialize_with_migrations(
    database_path: PathBuf,
    migrations: &[Migration],
    target_version: u32,
) -> AppDatabase {
    let Some(parent_directory) = database_path.parent() else {
        return storage_unavailable();
    };

    if fs::create_dir_all(parent_directory).is_err() {
        return storage_unavailable();
    }

    let existing_database = fs::metadata(&database_path)
        .map(|metadata| metadata.len() > 0)
        .unwrap_or(false);
    let current_version = if existing_database {
        match inspect_existing_database(&database_path) {
            Ok(version) => version,
            Err(()) => {
                return diagnostic(
                    DatabaseDiagnosticCode::CorruptDatabase,
                    "Keep the original database unchanged and restore it from a known-good backup.",
                );
            }
        }
    } else {
        0
    };

    if current_version > target_version {
        return diagnostic(
            DatabaseDiagnosticCode::UnsupportedSchemaVersion,
            "Upgrade Codex-O before opening this database again.",
        );
    }

    let backup_path = if existing_database && current_version < target_version {
        match create_backup(&database_path, current_version) {
            Ok(path) => Some(path),
            Err(()) => return storage_unavailable(),
        }
    } else {
        None
    };

    let mut connection = match open_configured_connection(&database_path) {
        Ok(connection) => connection,
        Err(()) => return storage_unavailable(),
    };

    if current_version < target_version {
        let migration_result = apply_migrations(&mut connection, current_version, migrations)
            .and_then(|version| {
                if version == target_version {
                    verify_database(&connection)?;
                    Ok(version)
                } else {
                    Err(rusqlite::Error::InvalidQuery)
                }
            });

        if migration_result.is_err() {
            drop(connection);

            // The backup is restored only after SQLite has released the database file.
            if let Some(backup_path) = backup_path {
                let _ = fs::copy(backup_path, &database_path);
            }

            return diagnostic(
                DatabaseDiagnosticCode::MigrationFailed,
                "Keep the backup and retry after installing a compatible Codex-O version.",
            );
        }
    } else if verify_database(&connection).is_err() {
        return diagnostic(
            DatabaseDiagnosticCode::CorruptDatabase,
            "Keep the original database unchanged and restore it from a known-good backup.",
        );
    }

    AppDatabase::Ready(ReadyDatabase {
        schema_version: target_version,
    })
}

fn inspect_existing_database(database_path: &Path) -> Result<u32, ()> {
    let connection = Connection::open_with_flags(database_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|_| ())?;
    verify_database(&connection).map_err(|_| ())?;
    connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(|_| ())
}

fn configure_connection(connection: &Connection) -> rusqlite::Result<()> {
    connection.execute_batch(
        "
        PRAGMA foreign_keys = ON;
        PRAGMA busy_timeout = 5000;
        ",
    )
}

fn open_configured_connection(database_path: &Path) -> Result<Connection, ()> {
    let connection = Connection::open(database_path).map_err(|_| ())?;
    configure_connection(&connection).map_err(|_| ())?;
    Ok(connection)
}

fn verify_database(connection: &Connection) -> rusqlite::Result<()> {
    let result: String = connection.query_row("PRAGMA quick_check(1)", [], |row| row.get(0))?;
    if result == "ok" {
        Ok(())
    } else {
        Err(rusqlite::Error::InvalidQuery)
    }
}

fn create_backup(database_path: &Path, current_version: u32) -> Result<PathBuf, ()> {
    let parent_directory = database_path.parent().ok_or(())?;
    let base_name = format!("{DATABASE_FILE_NAME}.pre-v{current_version}.bak");

    for suffix in 0..u32::MAX {
        let file_name = if suffix == 0 {
            base_name.clone()
        } else {
            format!("{base_name}.{suffix}")
        };
        let backup_path = parent_directory.join(file_name);

        let mut backup_file = match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&backup_path)
        {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(_) => return Err(()),
        };

        let mut source_file = match fs::File::open(database_path) {
            Ok(file) => file,
            Err(_) => {
                let _ = fs::remove_file(&backup_path);
                return Err(());
            }
        };

        if io::copy(&mut source_file, &mut backup_file).is_err() {
            let _ = fs::remove_file(&backup_path);
            return Err(());
        }

        return Ok(backup_path);
    }

    Err(())
}

fn diagnostic(code: DatabaseDiagnosticCode, recovery: &'static str) -> AppDatabase {
    AppDatabase::Diagnostic(DatabaseDiagnostic {
        code,
        read_only: true,
        recovery,
    })
}

#[cfg(test)]
mod tests;
