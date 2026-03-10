use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

/// Application configuration
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Config {
    #[serde(default)]
    pub settings: Settings,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Settings {
    /// Start FocusPlay when Windows starts
    #[serde(default)]
    pub autostart: bool,

    /// Show Windows notifications for events
    #[serde(default = "default_true")]
    pub show_notifications: bool,
}

fn default_true() -> bool {
    true
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            autostart: false,
            show_notifications: true,
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            settings: Settings::default(),
        }
    }
}

impl Config {
    /// Get the config file path: %APPDATA%\focusplay\config.toml
    pub fn path() -> Result<PathBuf> {
        let app_data = dirs::config_dir().context("Failed to get config directory")?;
        Ok(app_data.join("focusplay").join("config.toml"))
    }

    /// Load config from file, or create default if not exists
    pub fn load() -> Result<Self> {
        let path = Self::path()?;

        if !path.exists() {
            let config = Config::default();
            config.save()?;
            return Ok(config);
        }

        let contents = fs::read_to_string(&path)
            .with_context(|| format!("Failed to read config file: {}", path.display()))?;

        let config: Config = toml::from_str(&contents)
            .with_context(|| format!("Failed to parse config file: {}", path.display()))?;

        Ok(config)
    }

    /// Save config to file
    pub fn save(&self) -> Result<()> {
        let path = Self::path()?;

        // Create parent directory if needed
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!("Failed to create config directory: {}", parent.display())
            })?;
        }

        let contents = toml::to_string_pretty(self).context("Failed to serialize config")?;

        fs::write(&path, contents)
            .with_context(|| format!("Failed to write config file: {}", path.display()))?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert!(!config.settings.autostart);
        assert!(config.settings.show_notifications);
    }

    #[test]
    fn test_serialize_deserialize() {
        let config = Config::default();
        let serialized = toml::to_string_pretty(&config).unwrap();
        let deserialized: Config = toml::from_str(&serialized).unwrap();
        assert_eq!(config, deserialized);
    }

    #[test]
    fn test_parse_config() {
        let toml_str = r#"
[settings]
autostart = true
show_notifications = false
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert!(config.settings.autostart);
        assert!(!config.settings.show_notifications);
    }

    #[test]
    fn test_parse_partial_config() {
        // Missing fields should use defaults
        let toml_str = r#"
[settings]
autostart = true
"#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert!(config.settings.autostart);
        assert!(config.settings.show_notifications); // default
    }

    #[test]
    fn test_parse_empty_config() {
        let toml_str = "";
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config, Config::default());
    }
}
