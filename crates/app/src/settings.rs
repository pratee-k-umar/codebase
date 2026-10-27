//! User settings, persisted between runs.

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use codebase_intel_core::{Config, Language};

/// Everything the app remembers between launches.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Repository that was analyzed last, offered as a shortcut on the start
    /// screen.
    pub last_repo: Option<PathBuf>,
    /// Directory that clones are created in by default.
    pub clone_parent: Option<PathBuf>,
    /// Languages to analyze. Empty means "all supported".
    pub languages: Vec<String>,
    /// Extra exclude globs, on top of the analyzer's defaults.
    pub extra_excludes: Vec<String>,
    /// Whether the incremental cache may be used.
    pub cache_enabled: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            last_repo: None,
            clone_parent: None,
            languages: Vec::new(),
            extra_excludes: Vec::new(),
            cache_enabled: true,
        }
    }
}

impl Settings {
    /// `%APPDATA%\codebase-intel\settings.json` on Windows,
    /// `~/.config/codebase-intel/settings.json` elsewhere.
    pub fn path() -> PathBuf {
        let base = if cfg!(windows) {
            std::env::var_os("APPDATA")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("."))
        } else {
            std::env::var_os("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
                .unwrap_or_else(|| PathBuf::from("."))
        };

        base.join("codebase-intel").join("settings.json")
    }

    /// Load settings, falling back to defaults if the file is missing or
    /// unreadable. A corrupt file must never prevent the app from starting.
    pub fn load() -> Self {
        let path = Self::path();
        let Ok(contents) = fs::read_to_string(&path) else {
            return Self::default();
        };
        serde_json::from_str(&contents).unwrap_or_default()
    }

    pub fn save(&self) -> Result<(), String> {
        let path = Self::path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|err| format!("could not create {}: {err}", parent.display()))?;
        }
        let json = serde_json::to_string_pretty(self).map_err(|err| err.to_string())?;
        fs::write(&path, json).map_err(|err| format!("could not write {}: {err}", path.display()))
    }

    /// Translate settings into the analyzer's configuration.
    pub fn to_config(&self) -> Config {
        let mut config = Config::default();
        config.cache.enabled = self.cache_enabled;

        if !self.languages.is_empty() {
            config.languages = self
                .languages
                .iter()
                .filter_map(|name| parse_language(name))
                .collect();
            // An unparseable list would otherwise disable every language.
            if config.languages.is_empty() {
                config.languages = Config::default().languages;
            }
        }

        for pattern in &self.extra_excludes {
            let pattern = pattern.trim();
            if !pattern.is_empty() {
                config.exclude.push(pattern.to_string());
            }
        }

        config
    }

    pub fn is_language_enabled(&self, language: Language) -> bool {
        self.languages.is_empty()
            || self
                .languages
                .iter()
                .filter_map(|name| parse_language(name))
                .any(|enabled| enabled == language)
    }

    pub fn toggle_language(&mut self, language: Language) {
        // `languages` empty means "all"; the first toggle pins an explicit list.
        if self.languages.is_empty() {
            self.languages = all_language_names();
        }

        let name = language_name(language);
        if let Some(position) = self.languages.iter().position(|entry| entry == name) {
            self.languages.remove(position);
        } else {
            self.languages.push(name.to_string());
        }
    }
}

pub fn all_language_names() -> Vec<String> {
    [
        Language::TypeScript,
        Language::JavaScript,
        Language::Python,
        Language::Go,
        Language::Rust,
    ]
    .into_iter()
    .map(|language| language_name(language).to_string())
    .collect()
}

pub fn language_name(language: Language) -> &'static str {
    match language {
        Language::TypeScript => "typescript",
        Language::JavaScript => "javascript",
        Language::Python => "python",
        Language::Go => "go",
        Language::Rust => "rust",
    }
}

pub fn parse_language(name: &str) -> Option<Language> {
    match name.trim().to_ascii_lowercase().as_str() {
        "ts" | "typescript" => Some(Language::TypeScript),
        "js" | "javascript" => Some(Language::JavaScript),
        "py" | "python" => Some(Language::Python),
        "go" => Some(Language::Go),
        "rs" | "rust" => Some(Language::Rust),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_language_list_means_everything() {
        let settings = Settings::default();
        assert!(settings.is_language_enabled(Language::Rust));
        assert!(settings.is_language_enabled(Language::Python));
        assert_eq!(settings.to_config().languages.len(), 5);
    }

    #[test]
    fn toggling_pins_an_explicit_list() {
        let mut settings = Settings::default();
        settings.toggle_language(Language::Go);

        // Started from "all", so the first toggle materialises the full list and
        // then removes Go.
        assert_eq!(settings.languages.len(), 4);
        assert!(!settings.is_language_enabled(Language::Go));
        assert!(settings.is_language_enabled(Language::Rust));

        settings.toggle_language(Language::Go);
        assert!(settings.is_language_enabled(Language::Go));
    }

    #[test]
    fn extra_excludes_are_trimmed_and_appended() {
        let settings = Settings {
            extra_excludes: vec!["  **/vendor/**  ".into(), "".into()],
            ..Default::default()
        };
        let config = settings.to_config();
        assert!(config.exclude.contains(&"**/vendor/**".to_string()));
        assert!(!config.exclude.iter().any(|pattern| pattern.is_empty()));
    }

    #[test]
    fn unknown_language_names_do_not_disable_analysis() {
        let settings = Settings {
            languages: vec!["cobol".into()],
            ..Default::default()
        };
        // Falls back to the full default set instead of analyzing nothing.
        assert_eq!(settings.to_config().languages.len(), 5);
    }

    #[test]
    fn settings_round_trip_through_json() {
        let settings = Settings {
            last_repo: Some(PathBuf::from("C:/repo")),
            clone_parent: Some(PathBuf::from("C:/projects")),
            languages: vec!["rust".into(), "go".into()],
            extra_excludes: vec!["**/fixtures/**".into()],
            cache_enabled: false,
        };

        let json = serde_json::to_string(&settings).unwrap();
        let restored: Settings = serde_json::from_str(&json).unwrap();

        assert_eq!(restored.languages, settings.languages);
        assert_eq!(restored.extra_excludes, settings.extra_excludes);
        assert_eq!(restored.last_repo, settings.last_repo);
        assert!(!restored.cache_enabled);
    }

    #[test]
    fn partial_settings_file_is_accepted() {
        let partial = r#"{ "cache_enabled": false }"#;
        let settings: Settings = serde_json::from_str(partial).unwrap();
        assert!(!settings.cache_enabled);
        assert!(settings.languages.is_empty());
    }
}
