//! Error types for the analysis engine.

use std::path::PathBuf;
use thiserror::Error;
use glob;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Error)]
pub enum Error {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Parse error in {file}: {message}")]
    Parse { file: PathBuf, message: String },

    #[error("Unsupported language: {0}")]
    UnsupportedLanguage(String),

    #[error("Configuration error: {0}")]
    Config(#[from] config::ConfigError),

    #[error("TOML parse error: {0}")]
    Toml(#[from] toml::de::Error),

    #[error("JSON serialize error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("UTF-8 conversion error: {0}")]
    Utf8(#[from] std::string::FromUtf8Error),

    #[error("YAML serialize error: {0}")]
    Yaml(#[from] serde_yaml::Error),

    #[error("GraphML error: {0}")]
    GraphMl(String),

    #[error("Tree-sitter error: {0}")]
    TreeSitter(String),

    #[error("Walker error: {0}")]
    Walker(#[from] ignore::Error),

    #[error("Cache error: {0}")]
    Cache(String),

    #[error("Analysis error: {0}")]
    Analysis(String),

    #[error("Symbol not found: {0}")]
    SymbolNotFound(String),

    #[error("Type resolution failed: {0}")]
    TypeResolution(String),

    #[error("Invalid glob pattern: {0}")]
    Glob(#[from] glob::PatternError),
}

impl Error {
    pub fn parse(file: PathBuf, message: impl Into<String>) -> Self {
        Error::Parse { file, message: message.into() }
    }

    pub fn tree_sitter(msg: impl Into<String>) -> Self {
        Error::TreeSitter(msg.into())
    }

    pub fn cache(msg: impl Into<String>) -> Self {
        Error::Cache(msg.into())
    }

    pub fn analysis(msg: impl Into<String>) -> Self {
        Error::Analysis(msg.into())
    }
}