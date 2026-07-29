use rusqlite::{Connection, TransactionBehavior};

pub const CURRENT_SCHEMA_VERSION: u32 = 5;

#[derive(Clone, Copy)]
pub(crate) struct Migration {
    pub version: u32,
    pub sql: &'static str,
}

pub(crate) const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        sql: include_str!("v1.sql"),
    },
    Migration {
        version: 2,
        sql: include_str!("v2.sql"),
    },
    Migration {
        version: 3,
        sql: include_str!("v3.sql"),
    },
    Migration {
        version: 4,
        sql: include_str!("v4.sql"),
    },
    Migration {
        version: 5,
        sql: include_str!("v5.sql"),
    },
];

pub(crate) fn apply_migrations(
    connection: &mut Connection,
    current_version: u32,
    migrations: &[Migration],
) -> rusqlite::Result<u32> {
    let pending = migrations
        .iter()
        .filter(|migration| migration.version > current_version);
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let mut applied_version = current_version;

    for migration in pending {
        if migration.version <= applied_version {
            return Err(rusqlite::Error::InvalidQuery);
        }

        transaction.execute_batch(migration.sql)?;
        transaction.pragma_update(None, "user_version", migration.version)?;
        applied_version = migration.version;
    }

    transaction.commit()?;
    Ok(applied_version)
}
