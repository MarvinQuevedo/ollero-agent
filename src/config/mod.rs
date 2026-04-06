use std::path::PathBuf;

use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub ollama_url: String,
    pub model: String,
    pub context_size: u32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            ollama_url: "http://localhost:11434".into(),
            // Models must support Ollama tool calling; Gemma 3 currently does not.
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

    /// Returns None if no config file exists (first run).
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

    /// Save to an arbitrary path (used in tests).
    #[cfg(test)]
    pub fn save_to(&self, path: &PathBuf) -> Result<()> {
        let content = toml::to_string_pretty(self)?;
        std::fs::write(path, content)?;
        Ok(())
    }

    /// Load from an arbitrary path (used in tests).
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
            ollama_url: "http://localhost:11434".into(),
            model: "test-model:7b".into(),
            context_size: 4096,
        };
        original.save_to(&path).unwrap();

        let loaded = Config::load_from(&path).unwrap().expect("Config should exist");
        assert_eq!(loaded.ollama_url, original.ollama_url);
        assert_eq!(loaded.model, original.model);
        assert_eq!(loaded.context_size, original.context_size);

        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn test_load_from_nonexistent_returns_none() {
        let path = std::env::temp_dir().join("ollero_nonexistent_config.toml");
        // Make sure it doesn't exist
        let _ = std::fs::remove_file(&path);
        let result = Config::load_from(&path).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_toml_serialization_is_valid() {
        let cfg = Config::default();
        let toml_str = toml::to_string_pretty(&cfg).unwrap();
        assert!(toml_str.contains("ollama_url"));
        assert!(toml_str.contains("model"));
        assert!(toml_str.contains("context_size"));
    }
}
