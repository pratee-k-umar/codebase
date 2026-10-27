//! Parser registry and language-specific parser adapters.

use std::collections::HashMap;
use std::path::Path;
use tree_sitter::{Parser, Tree, Node};
use crate::{DiscoveredFile, ParsedFile, ParseResults, Symbol, Import, Export, Language as OurLanguage, SymbolId, Span, Position, Error, Result, AnalysisError};
use crate::config::Config;

/// Registry of language parsers.
pub struct ParserRegistry {
    parsers: HashMap<OurLanguage, Box<dyn LanguageParser>>,
}

impl ParserRegistry {
    pub fn new(config: &Config) -> Result<Self> {
        let mut parsers: HashMap<OurLanguage, Box<dyn LanguageParser>> = HashMap::new();

        // Validate that tree-sitter can load each configured language up front.
        for lang in &config.languages {
            let mut parser = Parser::new();
            parser.set_language(&lang.tree_sitter_language())
                .map_err(|e| Error::tree_sitter(format!("Failed to set language {:?}: {}", lang, e)))?;
        }

        // Register language-specific adapters
        if config.languages.contains(&OurLanguage::TypeScript) || config.languages.contains(&OurLanguage::JavaScript) {
            parsers.insert(OurLanguage::TypeScript, Box::new(crate::parsers::typescript::TypeScriptParser::new()));
            parsers.insert(OurLanguage::JavaScript, Box::new(crate::parsers::typescript::TypeScriptParser::new()));
        }
        if config.languages.contains(&OurLanguage::Python) {
            parsers.insert(OurLanguage::Python, Box::new(crate::parsers::python::PythonParser::new()));
        }
        if config.languages.contains(&OurLanguage::Go) {
            parsers.insert(OurLanguage::Go, Box::new(crate::parsers::go::GoParser::new()));
        }
        if config.languages.contains(&OurLanguage::Rust) {
            parsers.insert(OurLanguage::Rust, Box::new(crate::parsers::rust::RustParser::new()));
        }

        Ok(Self { parsers })
    }

    /// Parse all files in parallel.
    pub fn parse_all(&self, files: &[DiscoveredFile]) -> Result<ParseResults> {
        use rayon::prelude::*;

        let results: Vec<Result<ParsedFile>> = files.par_iter().map(|file| {
            self.parse_file(file)
        }).collect();

        let mut parsed_files = Vec::new();
        let mut errors = Vec::new();

        for result in results {
            match result {
                Ok(parsed) => parsed_files.push(parsed),
                Err(e) => errors.push(AnalysisError::from(&e)),
            }
        }

        Ok(ParseResults { files: parsed_files, errors })
    }

    /// Parse a single file.
    fn parse_file(&self, file: &DiscoveredFile) -> Result<ParsedFile> {
        let content = std::fs::read_to_string(&file.path)?;
        
        // Check file size limit
        let max_size = self.get_max_file_size(file.language);
        if content.len() > max_size * 1024 * 1024 {
            return Err(Error::analysis(format!("File too large: {} ({} bytes)", file.path.display(), content.len())));
        }

        // A tree-sitter parser is not Sync, so create one per file. Creation is
        // cheap relative to parsing and keeps `parse_all` parallel-safe.
        let mut parser = Parser::new();
        parser.set_language(&file.language.tree_sitter_language())
            .map_err(|e| Error::tree_sitter(format!("Failed to set language {:?}: {}", file.language, e)))?;

        let tree = parser.parse(&content, None)
            .ok_or_else(|| Error::parse(file.path.clone(), "Failed to parse"))?;

        // Get language-specific parser
        let lang_parser = self.parsers.get(&file.language)
            .ok_or_else(|| Error::UnsupportedLanguage(format!("{:?}", file.language)))?;

        // Extract symbols, imports, exports
        let symbols = lang_parser.extract_symbols(&tree, &content, &file.path)?;
        let imports = lang_parser.extract_imports(&tree, &content, &file.path)?;
        let exports = lang_parser.extract_exports(&tree, &content, &file.path)?;

        Ok(ParsedFile {
            file: file.clone(),
            ast: tree,
            symbols,
            imports,
            exports,
        })
    }

    fn get_max_file_size(&self, lang: OurLanguage) -> usize {
        match lang {
            OurLanguage::TypeScript | OurLanguage::JavaScript => 10,
            OurLanguage::Python => 10,
            OurLanguage::Go => 10,
            OurLanguage::Rust => 10,
        }
    }
}

/// Trait for language-specific parsing logic.
pub trait LanguageParser: Send + Sync {
    fn extract_symbols(&self, tree: &Tree, content: &str, file: &Path) -> Result<Vec<Symbol>>;
    fn extract_imports(&self, tree: &Tree, content: &str, file: &Path) -> Result<Vec<Import>>;
    fn extract_exports(&self, tree: &Tree, content: &str, file: &Path) -> Result<Vec<Export>>;
    fn resolve_type(&self, node: Node, content: &str, scope: &Scope) -> Result<Option<crate::TypeRef>>;
}

/// Scope for type resolution.
#[derive(Debug, Clone, Default)]
pub struct Scope {
    pub imports: HashMap<String, SymbolId>,
    pub local_symbols: HashMap<String, SymbolId>,
    pub parent: Option<Box<Scope>>,
}

impl Scope {
    pub fn new() -> Self { Self::default() }
    pub fn with_parent(parent: Scope) -> Self {
        Self { parent: Some(Box::new(parent)), ..Default::default() }
    }
    
    pub fn lookup(&self, name: &str) -> Option<&SymbolId> {
        self.local_symbols.get(name)
            .or_else(|| self.imports.get(name))
            .or_else(|| self.parent.as_ref()?.lookup(name))
    }
}

/// Helper to convert tree-sitter node to our Span.
pub fn node_to_span(node: Node, _content: &str) -> Span {
    let start_pos = node.start_position();
    let end_pos = node.end_position();
    
    let start_offset = node.start_byte();
    let end_offset = node.end_byte();
    
    Span {
        start: Position { line: start_pos.row + 1, column: start_pos.column + 1, offset: start_offset },
        end: Position { line: end_pos.row + 1, column: end_pos.column + 1, offset: end_offset },
    }
}

/// Helper to get node text.
pub fn node_text<'a>(node: Node, content: &'a str) -> &'a str {
    &content[node.start_byte()..node.end_byte()]
}