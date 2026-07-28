pub mod app_error;
pub mod catalog;
pub mod codex_fixture;
pub mod db;
pub mod observability;
pub mod parsing;
pub mod providers;
pub mod secrets;

use tauri::Manager;

pub const APP_NAME: &str = "Codex-O";
pub const BUNDLE_IDENTIFIER: &str = "com.zreo.codexo";

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let home_directory = app.path().home_dir().unwrap_or_default();
            let repository_directory =
                std::env::current_dir().unwrap_or_else(|_| home_directory.clone());
            let app_local_data_directory = app.path().app_local_data_dir().ok();
            let database = app_local_data_directory
                .as_ref()
                .map(|directory| db::initialize(db::database_path(directory)))
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
            let catalog = match app_local_data_directory {
                Some(directory) => {
                    catalog.with_preferences_path(directory.join("scan-preferences.json"))
                }
                None => catalog,
            };
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
            catalog::get_skill_detail
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
