pub mod app_error;
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
            let database = app
                .path()
                .home_dir()
                .map(|home_directory| db::initialize(db::database_path(&home_directory)))
                .unwrap_or_else(|_| db::storage_unavailable());
            app.manage(database);
            Ok(())
        })
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
