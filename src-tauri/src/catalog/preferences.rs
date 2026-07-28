use std::{fs, io::Write, path::PathBuf, sync::RwLock};

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct ScanPreferences {
    pub include_plugin_cache: bool,
    #[serde(default)]
    pub initial_scan_notice_seen: bool,
}

impl ScanPreferences {
    pub(super) const fn default_for_desktop() -> Self {
        Self {
            include_plugin_cache: false,
            initial_scan_notice_seen: false,
        }
    }
}

#[derive(Debug)]
pub(super) struct ScanPreferencesStore {
    path: Option<PathBuf>,
    value: RwLock<ScanPreferences>,
}

impl ScanPreferencesStore {
    pub(super) fn in_memory(value: ScanPreferences) -> Self {
        Self {
            path: None,
            value: RwLock::new(value),
        }
    }

    pub(super) fn at_path(path: PathBuf) -> Self {
        let value = fs::read_to_string(&path)
            .ok()
            .and_then(|contents| serde_json::from_str(&contents).ok())
            .unwrap_or_else(ScanPreferences::default_for_desktop);

        Self {
            path: Some(path),
            value: RwLock::new(value),
        }
    }

    pub(super) fn get(&self) -> ScanPreferences {
        *self
            .value
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub(super) fn set(&self, next: ScanPreferences) -> Result<(), ()> {
        if let Some(path) = &self.path {
            save_preferences(path, next)?;
        }
        *self
            .value
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = next;
        Ok(())
    }
}

fn save_preferences(path: &PathBuf, preferences: ScanPreferences) -> Result<(), ()> {
    let parent = path.parent().ok_or(())?;
    fs::create_dir_all(parent).map_err(|_| ())?;
    let temporary_path = parent.join(".scan-preferences.json.tmp");
    let contents = serde_json::to_vec(&preferences).map_err(|_| ())?;
    let mut temporary = fs::File::create(&temporary_path).map_err(|_| ())?;

    temporary.write_all(&contents).map_err(|_| ())?;
    temporary.sync_all().map_err(|_| ())?;
    fs::rename(temporary_path, path).map_err(|_| ())
}
