//! Persisted user settings. Secrets never live here — OAuth tokens go to the OS keyring; this file
//! only holds the Spotify client ID (public by design) and the scanned music folders.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AppConfig {
    pub spotify_client_id: Option<String>,
    pub music_folders: Vec<String>,
}

impl AppConfig {
    pub fn path(data_dir: &Path) -> PathBuf {
        data_dir.join("settings.json")
    }

    pub fn load(data_dir: &Path) -> Self {
        std::fs::read_to_string(Self::path(data_dir))
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, data_dir: &Path) -> Result<(), String> {
        let path = Self::path(data_dir);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let json = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(&path, json).map_err(|e| format!("could not write settings: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_round_trips_through_disk() {
        let dir = tempfile::tempdir().unwrap();
        let config = AppConfig {
            spotify_client_id: Some("abc123".into()),
            music_folders: vec!["/Users/me/Music".into()],
        };
        config.save(dir.path()).unwrap();

        let loaded = AppConfig::load(dir.path());
        assert_eq!(loaded.spotify_client_id.as_deref(), Some("abc123"));
        assert_eq!(loaded.music_folders, vec!["/Users/me/Music".to_string()]);
    }

    #[test]
    fn a_missing_or_corrupt_file_falls_back_to_defaults() {
        let dir = tempfile::tempdir().unwrap();
        assert!(AppConfig::load(dir.path()).music_folders.is_empty());

        std::fs::write(AppConfig::path(dir.path()), "{ not json").unwrap();
        assert!(AppConfig::load(dir.path()).spotify_client_id.is_none());
    }
}
