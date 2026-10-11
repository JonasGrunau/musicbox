//! Library locations as configured in rekordboxAgent's `options.json`. The library can be moved
//! by the user ("Move Master Database"), so these paths are never hard-coded.

use crate::{Error, Result};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct LibraryPaths {
    pub db_path: PathBuf,
    pub analysis_data_root: PathBuf,
    pub settings_root: PathBuf,
    /// Encrypted passphrase (`dp`), see [`crate::derive_passphrase`].
    pub dp: String,
}

impl LibraryPaths {
    /// `~/Library/Application Support/Pioneer/rekordboxAgent/storage/options.json` on macOS.
    pub fn default_options_json() -> Option<PathBuf> {
        std::env::var_os("HOME").map(|home| {
            PathBuf::from(home)
                .join("Library/Application Support/Pioneer/rekordboxAgent/storage/options.json")
        })
    }

    pub fn from_options_json(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path).map_err(|source| Error::Io {
            path: path.display().to_string(),
            source,
        })?;
        Self::parse_options_json(&text)
    }

    /// `options.json` holds `{"options": [[key, value], ...], "defaults": {...}}`.
    pub fn parse_options_json(text: &str) -> Result<Self> {
        let root: serde_json::Value =
            serde_json::from_str(text).map_err(|e| Error::Options(e.to_string()))?;
        let options = root
            .get("options")
            .and_then(|v| v.as_array())
            .ok_or_else(|| Error::Options("missing `options` list".into()))?;
        let get = |name: &str| -> Result<String> {
            options
                .iter()
                .filter_map(|pair| pair.as_array())
                .find(|pair| pair.first().and_then(|k| k.as_str()) == Some(name))
                .and_then(|pair| pair.get(1)?.as_str().map(str::to_owned))
                .ok_or_else(|| Error::Options(format!("missing `{name}`")))
        };
        Ok(Self {
            db_path: get("db-path")?.into(),
            analysis_data_root: get("analysis-data-root-path")?.into(),
            settings_root: get("settings-root-path")?.into(),
            dp: get("dp")?,
        })
    }
}
