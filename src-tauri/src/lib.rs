pub mod analysis;
pub mod app_error;
pub mod catalog;
pub mod codex_fixture;
pub mod db;
pub mod observability;
pub mod parsing;
pub mod providers;
pub mod secrets;
pub mod settings;

use std::sync::Arc;

use tauri::Manager;

pub const APP_NAME: &str = "Codex-O";
pub const BUNDLE_IDENTIFIER: &str = "com.zreo.codexo";

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let home_directory = app.path().home_dir().unwrap_or_default();
            let repository_directory =
                std::env::current_dir().unwrap_or_else(|_| home_directory.clone());
            let app_local_data_directory = app.path().app_local_data_dir().ok();
            let database_path = app_local_data_directory
                .as_ref()
                .map(|directory| db::database_path(directory));
            let database = database_path
                .as_ref()
                .map(|path| db::initialize(path.clone()))
                .unwrap_or_else(db::storage_unavailable);
            let roots = providers::ProviderRoots::new(
                home_directory.clone(),
                repository_directory,
                home_directory.join(".codex/plugins/cache"),
            );
            let catalog = match (&database, app_local_data_directory.as_ref()) {
                (db::AppDatabase::Ready(_), Some(directory)) => {
                    catalog::SkillCatalog::with_index_path(roots, db::database_path(directory))
                }
                _ => catalog::SkillCatalog::new(roots),
            };
            let catalog = match app_local_data_directory.as_ref() {
                Some(directory) => {
                    catalog.with_preferences_path(directory.join("scan-preferences.json"))
                }
                None => catalog,
            };
            let analysis_cache: Arc<dyn analysis::AnalysisCache> = match (&database, database_path)
            {
                (db::AppDatabase::Ready(_), Some(path)) => {
                    Arc::new(analysis::SqliteAnalysisCache::new(path))
                }
                _ => Arc::new(analysis::UnavailableAnalysisCache),
            };
            let analysis_service = Arc::new(analysis::AnalysisService::new(
                catalog.clone(),
                analysis_cache,
                Some(home_directory.clone()),
            ));
            let analysis_queue = analysis::AnalysisQueue::new(
                Arc::clone(&analysis_service),
                Arc::new(analysis::TauriAnalysisProgressSink::new(
                    app.handle().clone(),
                )),
            );
            let settings_service = app_local_data_directory
                .as_ref()
                .map(|directory| {
                    Arc::new(settings::SettingsService::new(
                        settings::config_path(directory),
                        settings::system_secret_store(),
                        Arc::clone(&analysis_service),
                        catalog.clone(),
                        database.status(),
                        Some(home_directory.join(".codex/state_5.sqlite")),
                    ))
                })
                .unwrap_or_else(|| {
                    // App-local path resolution failure keeps settings read-only instead of escaping to cwd.
                    Arc::new(settings::SettingsService::without_storage(
                        settings::system_secret_store(),
                        Arc::clone(&analysis_service),
                        catalog.clone(),
                        database.status(),
                        None,
                    ))
                });
            app.manage(analysis_queue);
            app.manage(analysis_service);
            app.manage(settings_service);
            app.manage(catalog);
            app.manage(database);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            catalog::list_providers,
            catalog::scan_skills,
            catalog::load_catalog,
            catalog::get_scan_preferences,
            catalog::update_scan_preferences,
            catalog::acknowledge_initial_scan_notice,
            catalog::list_skills,
            catalog::get_skill_detail,
            analysis::queue::analyze_skill,
            analysis::get_skill_analysis,
            analysis::read_evidence_excerpt,
            analysis::compare_skills,
            settings::get_ai_config,
            settings::save_ai_config,
            settings::test_ai_connection,
            settings::get_environment_health,
            settings::list_additional_roots,
            settings::select_additional_root,
            settings::remove_additional_root
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::{APP_NAME, BUNDLE_IDENTIFIER};

    #[test]
    fn application_metadata_is_stable() {
        assert_eq!(APP_NAME, "Codex-O");
        assert_eq!(BUNDLE_IDENTIFIER, "com.zreo.codexo");
    }
}
