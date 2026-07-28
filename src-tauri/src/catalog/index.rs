use std::{
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use rusqlite::{params, Connection, TransactionBehavior};
use serde::{Deserialize, Serialize};

use super::{
    CatalogEntry, CatalogSnapshot, ProviderAvailability, ProviderCapabilities, ProviderKind,
    ProviderView, SkillScope,
};

const ACTIVE_LIFECYCLE_STATE: &str = "active";
const STALE_LIFECYCLE_STATE: &str = "stale";
const MANAGED_ROOT_MARKER: &str = "managed-by-provider-registry";

#[derive(Clone, Debug)]
pub(super) struct CatalogIndex {
    database_path: PathBuf,
}

impl CatalogIndex {
    pub(super) fn new(database_path: PathBuf) -> Self {
        Self { database_path }
    }

    pub(super) fn load(&self) -> Result<Option<CatalogSnapshot>, ()> {
        let connection = self.open_connection()?;
        let providers = load_providers(&connection)?;

        if providers.is_empty() {
            return Ok(None);
        }

        let mut statement = connection
            .prepare(
                "
                SELECT artifact_snapshots.content_hash,
                       artifact_snapshots.parser_version,
                       artifact_snapshots.manifest_json,
                       artifact_snapshots.resources_json,
                       artifact_snapshots.diagnostics_json
                FROM skills
                JOIN artifact_snapshots
                  ON artifact_snapshots.id = skills.latest_snapshot_id
                WHERE skills.lifecycle_state = ?1
                ORDER BY skills.id
                ",
            )
            .map_err(|_| ())?;
        let rows = statement
            .query_map(params![ACTIVE_LIFECYCLE_STATE], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                ))
            })
            .map_err(|_| ())?;

        let mut entries = Vec::new();
        for row in rows {
            let (content_hash, parser_version, manifest_json, resources_json, diagnostics_json) =
                row.map_err(|_| ())?;
            let mut manifest =
                serde_json::from_str::<StoredManifest>(&manifest_json).map_err(|_| ())?;
            let resources = serde_json::from_str(&resources_json).map_err(|_| ())?;
            let diagnostics =
                serde_json::from_str::<Vec<super::CatalogDiagnostic>>(&diagnostics_json)
                    .map_err(|_| ())?;
            manifest.summary.search_headings = manifest
                .headings
                .iter()
                .map(|heading| heading.text.clone())
                .collect();
            manifest.summary.diagnostics = diagnostics.clone();

            entries.push(CatalogEntry {
                skill: None,
                content_hash,
                parser_version,
                headings: manifest.headings,
                resources,
                snapshot: None,
                summary: manifest.summary,
            });
        }

        Ok(Some(CatalogSnapshot {
            providers,
            entries,
            diagnostics: Vec::new(),
        }))
    }

    pub(super) fn save(&self, snapshot: &CatalogSnapshot) -> Result<(), ()> {
        let mut connection = self.open_connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| ())?;
        let timestamp = now_ms()?;

        // Missing entries remain available in historical snapshots but disappear from the live list.
        transaction
            .execute(
                "UPDATE skills SET lifecycle_state = ?1",
                params![STALE_LIFECYCLE_STATE],
            )
            .map_err(|_| ())?;

        for provider in &snapshot.providers {
            let stored_provider = StoredProvider {
                kind: provider.kind,
                availability: provider.availability,
                capabilities: provider.capabilities,
            };
            let capabilities_json = serde_json::to_string(&stored_provider).map_err(|_| ())?;
            let read_only = i64::from(!provider.capabilities.can_import);

            transaction
                .execute(
                    "
                    INSERT INTO providers (
                        id, kind, root_path, display_name, read_only, capabilities_json, last_scan_at
                    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                    ON CONFLICT(id) DO UPDATE SET
                        kind = excluded.kind,
                        root_path = excluded.root_path,
                        display_name = excluded.display_name,
                        read_only = excluded.read_only,
                        capabilities_json = excluded.capabilities_json,
                        last_scan_at = excluded.last_scan_at
                    ",
                    params![
                        provider.id,
                        serde_json::to_string(&provider.kind).map_err(|_| ())?,
                        MANAGED_ROOT_MARKER,
                        provider.display_name,
                        read_only,
                        capabilities_json,
                        timestamp,
                    ],
                )
                .map_err(|_| ())?;
        }

        for entry in &snapshot.entries {
            let snapshot_id = format!("snapshot:{}:{}", entry.summary.id, entry.content_hash);
            let manifest_json = serde_json::to_string(&StoredManifest {
                summary: entry.summary.clone(),
                headings: entry.headings.clone(),
            })
            .map_err(|_| ())?;
            let resources_json = serde_json::to_string(&entry.resources).map_err(|_| ())?;
            let diagnostics_json =
                serde_json::to_string(&entry.summary.diagnostics).map_err(|_| ())?;

            transaction
                .execute(
                    "
                    INSERT INTO skills (
                        id, provider_id, relative_path, display_name, scope, lifecycle_state,
                        latest_snapshot_id, first_seen_at, last_seen_at
                    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, ?7, ?8)
                    ON CONFLICT(id) DO UPDATE SET
                        provider_id = excluded.provider_id,
                        relative_path = excluded.relative_path,
                        display_name = excluded.display_name,
                        scope = excluded.scope,
                        lifecycle_state = excluded.lifecycle_state,
                        last_seen_at = excluded.last_seen_at
                    ",
                    params![
                        entry.summary.id,
                        entry.summary.provider.id,
                        entry
                            .skill
                            .as_ref()
                            .map(|skill| skill.relative_path.as_str())
                            .unwrap_or("indexed"),
                        entry.summary.display_name,
                        scope_name(entry.summary.scope),
                        ACTIVE_LIFECYCLE_STATE,
                        timestamp,
                        timestamp,
                    ],
                )
                .map_err(|_| ())?;
            transaction
                .execute(
                    "
                    INSERT INTO artifact_snapshots (
                        id, skill_id, content_hash, parser_version, manifest_json, resources_json,
                        diagnostics_json, created_at
                    ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                    ON CONFLICT(id) DO NOTHING
                    ",
                    params![
                        snapshot_id,
                        entry.summary.id,
                        entry.content_hash,
                        entry.parser_version,
                        manifest_json,
                        resources_json,
                        diagnostics_json,
                        timestamp,
                    ],
                )
                .map_err(|_| ())?;
            transaction
                .execute(
                    "UPDATE skills SET latest_snapshot_id = ?1 WHERE id = ?2",
                    params![snapshot_id, entry.summary.id],
                )
                .map_err(|_| ())?;
        }

        transaction.commit().map_err(|_| ())
    }

    fn open_connection(&self) -> Result<Connection, ()> {
        let connection = Connection::open(&self.database_path).map_err(|_| ())?;
        connection
            .execute_batch(
                "
                PRAGMA foreign_keys = ON;
                PRAGMA busy_timeout = 5000;
                ",
            )
            .map_err(|_| ())?;
        Ok(connection)
    }
}

fn load_providers(connection: &Connection) -> Result<Vec<ProviderView>, ()> {
    let mut statement = connection
        .prepare(
            "
            SELECT id, display_name, capabilities_json
            FROM providers
            ORDER BY id
            ",
        )
        .map_err(|_| ())?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(|_| ())?;
    let mut providers = Vec::new();

    for row in rows {
        let (id, display_name, capabilities_json) = row.map_err(|_| ())?;
        let stored = serde_json::from_str::<StoredProvider>(&capabilities_json).map_err(|_| ())?;
        providers.push(ProviderView {
            id,
            kind: stored.kind,
            display_name,
            capabilities: stored.capabilities,
            availability: stored.availability,
        });
    }

    Ok(providers)
}

fn now_ms() -> Result<i64, ()> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| ())
        .and_then(|duration| i64::try_from(duration.as_millis()).map_err(|_| ()))
}

fn scope_name(scope: SkillScope) -> &'static str {
    match scope {
        SkillScope::User => "user",
        SkillScope::Repository => "repository",
        SkillScope::LegacyUser => "legacy_user",
        SkillScope::System => "system",
        SkillScope::Plugin => "plugin",
        SkillScope::Bundled => "bundled",
        SkillScope::Additional => "additional",
    }
}

#[derive(Deserialize, Serialize)]
struct StoredManifest {
    summary: super::SkillSummary,
    headings: Vec<super::MarkdownHeading>,
}

#[derive(Deserialize, Serialize)]
struct StoredProvider {
    kind: ProviderKind,
    availability: ProviderAvailability,
    capabilities: ProviderCapabilities,
}
