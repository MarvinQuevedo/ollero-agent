use std::path::PathBuf;

use anyhow::Result;
use serde::{Deserialize, Serialize};

pub const CONFIG_VERSION: &str = "1.0.1";

fn default_config_version() -> String {
    CONFIG_VERSION.to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Written to `config.toml` for forward-compatible migrations.
    #[serde(default = "default_config_version")]
    pub config_version: String,
    pub ollama_url: String,
    pub model: String,
    pub context_size: u32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            config_version: default_config_version(),
            ollama_url: "http://localhost:11434".into(),
            // Must support tool calling in Ollama.
            model: "llama3.2".into(),
            context_size: 8192,
        }
    }
}

impl Config {
    pub fn config_path() -> PathBuf {
        Self::config_dir().join("config.toml")
    }

    pub fn config_dir() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("ollero")
    }

    /// Returns None when config does not exist yet.
    pub fn load() -> Result<Option<Self>> {
        let path = Self::config_path();
        if !path.exists() {
            return Ok(None);
        }
        let content = std::fs::read_to_string(&path)?;
        let config: Self = toml::from_str(&content)?;
        Ok(Some(config))
    }

    pub fn save(&self) -> Result<()> {
        let dir = Self::config_dir();
        std::fs::create_dir_all(&dir)?;
        let content = toml::to_string_pretty(self)?;
        std::fs::write(Self::config_path(), content)?;
        Ok(())
    }

    #[cfg(test)]
    pub fn save_to(&self, path: &PathBuf) -> Result<()> {
        let content = toml::to_string_pretty(self)?;
        std::fs::write(path, content)?;
        Ok(())
    }

    #[cfg(test)]
    pub fn load_from(path: &PathBuf) -> Result<Option<Self>> {
        if !path.exists() {
            return Ok(None);
        }
        let content = std::fs::read_to_string(path)?;
        let config: Self = toml::from_str(&content)?;
        Ok(Some(config))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config_values() {
        let cfg = Config::default();
        assert_eq!(cfg.config_version, CONFIG_VERSION);
        assert_eq!(cfg.ollama_url, "http://localhost:11434");
        assert!(!cfg.model.is_empty());
        assert!(cfg.context_size > 0);
    }

    #[test]
    fn test_save_and_load_roundtrip() {
        let dir = std::env::temp_dir().join("ollero_test_config");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");

        let original = Config {
            config_version: CONFIG_VERSION.to_string(),
            ollama_url: "http://localhost:11434".into(),
            model: "test-model:7b".into(),
            context_size: 4096,
        };
        original.save_to(&path).unwrap();

        let loaded = Config::load_from(&path).unwrap().expect("Config should exist");
        assert_eq!(loaded.config_version, original.config_version);
        assert_eq!(loaded.ollama_url, original.ollama_url);
        assert_eq!(loaded.model, original.model);
        assert_eq!(loaded.context_size, original.context_size);

        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn test_load_from_nonexistent_returns_none() {
        let path = std::env::temp_dir().join("ollero_nonexistent_config.toml");
        let _ = std::fs::remove_file(&path);
        let result = Config::load_from(&path).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_toml_serialization_is_valid() {
        let cfg = Config::default();
        let toml_str = toml::to_string_pretty(&cfg).unwrap();
        assert!(toml_str.contains("config_version"));
        assert!(toml_str.contains("ollama_url"));
        assert!(toml_str.contains("model"));
        assert!(toml_str.contains("context_size"));
    }

    #[test]
    fn test_legacy_toml_without_config_version_gets_default() {
        let dir = std::env::temp_dir().join("ollero_test_legacy_config");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(
            &path,
            r#"ollama_url = "http://localhost:11434"
model = "legacy-model"
context_size = 2048
"#,
        )
        .unwrap();

        let loaded = Config::load_from(&path).unwrap().expect("Config should exist");
        assert_eq!(loaded.config_version, CONFIG_VERSION);
        assert_eq!(loaded.model, "legacy-model");
        std::fs::remove_file(&path).unwrap();
    }
}
