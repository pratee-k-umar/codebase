//! Core analysis engine for codebase intelligence.
//! 
//! This crate provides the main analysis pipeline: parsing, symbol resolution,
//! dependency graph construction, and export formats.

pub mod config;
pub mod error;
pub mod model;
pub mod discovery;
pub mod parser;
pub mod parsers;
pub mod resolve;
pub mod graph;
pub mod analyzer;
pub mod export;
pub mod incremental;

pub use config::Config;
pub use error::{Error, Result};
pub use model::*;
pub use analyzer::{analyze, analyze_with};
pub use discovery::{discover, DiscoveryReport, DiscoverySummary, IgnoreReason, IgnoredEntry};
pub use incremental::IncrementalAnalyzer;
pub use config::OutputFormat;
pub use parser::{node_to_span, node_text, LanguageParser, Scope};

#[derive(Debug, Clone)]
pub struct DiscoveredFile {
    pub path: std::path::PathBuf,
    pub language: model::Language,
    pub size: u64,
}

#[derive(Debug, Default)]
pub struct ParseResults {
    pub files: Vec<ParsedFile>,
    pub errors: Vec<AnalysisError>,
}

#[derive(Debug)]
pub struct ParsedFile {
    pub file: DiscoveredFile,
    pub ast: tree_sitter::Tree,
    pub symbols: Vec<Symbol>,
    pub imports: Vec<Import>,
    pub exports: Vec<Export>,
}
