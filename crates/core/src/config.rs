//! Configuration for the analysis engine.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use serde::{Deserialize, Serialize};
use crate::{Language, Result};
use glob;

/// Root configuration.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct Config {
    pub include: Vec<String>,
    pub exclude: Vec<String>,
    pub languages: HashSet<Language>,
    pub parsers: ParserConfig,
    pub analysis: AnalysisConfig,
    pub output: OutputConfig,
    pub cache: CacheConfig,
}

impl Default for Config {
    fn default() -> Self {
        let mut languages = HashSet::new();
        languages.extend([
            Language::TypeScript,
            Language::JavaScript,
            Language::Python,
            Language::Go,
            Language::Rust,
        ]);

        Self {
            include: vec!["**/*".into()],
            exclude: vec![
                "**/node_modules/**".into(),
                "**/target/**".into(),
                "**/.git/**".into(),
                "**/dist/**".into(),
                "**/build/**".into(),
                "**/*.min.js".into(),
                "**/*.d.ts".into(),
                "**/__pycache__/**".into(),
                "**/.venv/**".into(),
                "**/venv/**".into(),
            ],
            languages,
            parsers: ParserConfig::default(),
            analysis: AnalysisConfig::default(),
            output: OutputConfig::default(),
            cache: CacheConfig::default(),
        }
    }
}

impl Config {
    pub fn load(path: Option<&Path>) -> Result<Self> {
        let mut config = Config::default();
        
        if let Some(path) = path {
            if path.exists() {
                let content = std::fs::read_to_string(path)?;
                let file_config: Config = toml::from_str(&content)?;
                config.merge(file_config);
            }
        }
        
        // Also check for .codebase-intel.toml in current dir
        let default_config = PathBuf::from(".codebase-intel.toml");
        if default_config.exists() && path != Some(default_config.as_path()) {
            let content = std::fs::read_to_string(&default_config)?;
            let file_config: Config = toml::from_str(&content)?;
            config.merge(file_config);
        }
        
        Ok(config)
    }

    fn merge(&mut self, other: Config) {
        if !other.include.is_empty() { self.include = other.include; }
        if !other.exclude.is_empty() { self.exclude = other.exclude; }
        if !other.languages.is_empty() { self.languages = other.languages; }
        self.parsers.merge(other.parsers);
        self.analysis.merge(other.analysis);
        self.output.merge(other.output);
        self.cache.merge(other.cache);
    }

    pub fn should_parse(&self, lang: Language) -> bool {
        self.languages.contains(&lang)
    }

    pub fn is_excluded(&self, path: &Path) -> bool {
        let path_str = path.to_string_lossy();
        for pattern in &self.exclude {
            if Self::match_glob(pattern, &path_str) {
                return true;
            }
        }
        false
    }

    /// Maximum size, in bytes, for a file of the given language.
    ///
    /// A configured value of `0` is treated as "no limit" so a malformed config
    /// cannot silently skip every file.
    pub fn max_file_size_bytes(&self, lang: Language) -> usize {
        let megabytes = match lang {
            Language::TypeScript | Language::JavaScript => self.parsers.typescript.max_file_size_mb,
            Language::Python => self.parsers.python.max_file_size_mb,
            Language::Go => self.parsers.go.max_file_size_mb,
            Language::Rust => self.parsers.rust.max_file_size_mb,
        };

        if megabytes == 0 {
            usize::MAX
        } else {
            megabytes.saturating_mul(1024 * 1024)
        }
    }

    /// Path of the incremental analysis cache file.
    ///
    /// Defaults to a `codebase-intel` directory in the system temp directory so
    /// a scan never writes into the analyzed repository unless configured to.
    pub fn cache_file_path(&self) -> PathBuf {
        let directory = self
            .cache
            .directory
            .clone()
            .unwrap_or_else(|| std::env::temp_dir().join("codebase-intel"));
        directory.join("cache.json")
    }

    fn match_glob(pattern: &str, path: &str) -> bool {
        glob::Pattern::new(pattern)
            .map(|p| p.matches(path))
            .unwrap_or(false)
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct ParserConfig {
    pub typescript: TypeScriptParserConfig,
    pub python: PythonParserConfig,
    pub go: GoParserConfig,
    pub rust: RustParserConfig,
}

impl Default for ParserConfig {
    fn default() -> Self {
        Self {
            typescript: TypeScriptParserConfig::default(),
            python: PythonParserConfig::default(),
            go: GoParserConfig::default(),
            rust: RustParserConfig::default(),
        }
    }
}

impl ParserConfig {
    fn merge(&mut self, other: ParserConfig) {
        self.typescript = other.typescript;
        self.python = other.python;
        self.go = other.go;
        self.rust = other.rust;
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct TypeScriptParserConfig {
    pub use_project_references: bool,
    pub skip_lib_check: bool,
    pub max_file_size_mb: usize,
}

impl Default for TypeScriptParserConfig {
    fn default() -> Self {
        Self {
            use_project_references: true,
            skip_lib_check: true,
            max_file_size_mb: 10,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct PythonParserConfig {
    pub follow_imports: bool,
    pub infer_types: bool,
    pub max_file_size_mb: usize,
}

impl Default for PythonParserConfig {
    fn default() -> Self {
        Self {
            follow_imports: true,
            infer_types: false,
            max_file_size_mb: 10,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct GoParserConfig {
    pub parse_tests: bool,
    pub cgo_enabled: bool,
    pub max_file_size_mb: usize,
}

impl Default for GoParserConfig {
    fn default() -> Self {
        Self {
            parse_tests: false,
            cgo_enabled: false,
            max_file_size_mb: 10,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct RustParserConfig {
    pub edition: String,
    pub features: Vec<String>,
    pub max_file_size_mb: usize,
}

impl Default for RustParserConfig {
    fn default() -> Self {
        Self {
            edition: "2021".into(),
            features: vec![],
            max_file_size_mb: 10,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct AnalysisConfig {
    pub resolve_types: bool,
    pub build_call_graph: bool,
    pub detect_apis: bool,
    pub detect_database: bool,
    pub compute_metrics: bool,
    pub max_call_graph_depth: usize,
    pub follow_dynamic_imports: bool,
}

impl Default for AnalysisConfig {
    fn default() -> Self {
        Self {
            resolve_types: true,
            build_call_graph: true,
            detect_apis: true,
            detect_database: true,
            compute_metrics: true,
            max_call_graph_depth: 10,
            follow_dynamic_imports: false,
        }
    }
}

impl AnalysisConfig {
    fn merge(&mut self, other: AnalysisConfig) {
        *self = other;
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct OutputConfig {
    pub format: OutputFormat,
    pub include_source_snippets: bool,
    pub max_snippet_length: usize,
    pub graph_format: GraphFormat,
}

impl Default for OutputConfig {
    fn default() -> Self {
        Self {
            format: OutputFormat::Json,
            include_source_snippets: true,
            max_snippet_length: 200,
            graph_format: GraphFormat::GraphML,
        }
    }
}

impl OutputConfig {
    fn merge(&mut self, other: OutputConfig) {
        *self = other;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum OutputFormat {
    Json,
    Yaml,
    Summary,
    Mermaid,
    GraphML,
    Dot,
    Csv,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum GraphFormat {
    GraphML,
    Dot,
    Mermaid,
    Gexf,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct CacheConfig {
    pub enabled: bool,
    pub directory: Option<PathBuf>,
    pub max_size_mb: usize,
    pub ttl_days: u32,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            directory: None, // Use system temp dir
            max_size_mb: 500,
            ttl_days: 30,
        }
    }
}

impl CacheConfig {
    fn merge(&mut self, other: CacheConfig) {
        *self = other;
    }
}

