//! Pairing details and the Pause switch, saved in the app's config folder.
//! The device token is not here; it is in the keychain. Check output is never saved.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::flavor::FLAVOR;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Pairing {
    pub host: String,
    pub device_id: String,
    pub user_id: String,
    pub device_name: String,
    pub paired_at_ms: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub pairing: Option<Pairing>,
    #[serde(default)]
    pub paused: bool,
}

pub struct SettingsFile {
    path: PathBuf,
}

impl SettingsFile {
    pub fn new(config_dir: &Path) -> Self {
        SettingsFile {
            path: config_dir.join("settings.json"),
        }
    }

    pub fn load(&self) -> Settings {
        let mut settings: Settings = std::fs::read(&self.path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        // A pairing made against another host is never reused.
        if settings
            .pairing
            .as_ref()
            .is_some_and(|p| p.host != FLAVOR.host)
        {
            settings.pairing = None;
        }
        settings
    }

    pub fn save(&self, settings: &Settings) -> Result<(), String> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let bytes = serde_json::to_vec_pretty(settings).map_err(|e| e.to_string())?;
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, bytes).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &self.path).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("wtf-helper-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn round_trips_and_ignores_foreign_hosts() {
        let dir = temp_dir("settings");
        let file = SettingsFile::new(&dir);
        assert_eq!(file.load(), Settings::default());
        let mut settings = Settings {
            pairing: Some(Pairing {
                host: FLAVOR.host.into(),
                device_id: "d1".into(),
                user_id: "u1".into(),
                device_name: "Mac".into(),
                paired_at_ms: 1,
            }),
            paused: true,
        };
        file.save(&settings).unwrap();
        assert_eq!(file.load(), settings);
        let saved = std::fs::read_to_string(dir.join("settings.json")).unwrap();
        assert!(!saved.to_lowercase().contains("token"));

        settings.pairing.as_mut().unwrap().host = "evil.example".into();
        file.save(&settings).unwrap();
        assert_eq!(file.load().pairing, None);
        assert!(file.load().paused);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
