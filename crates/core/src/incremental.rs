//! Incremental analysis support.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use serde::{Deserialize, Serialize};
use crate::{AnalysisResult, Symbol, SymbolId, Reference, FileDependencies, CallGraph, ApiEndpoint, DbSchema, ProjectMetrics, AnalysisError, Config, Result, DiscoveredFile, ParsedFile, Language, SerializableCallGraph};
use crate::discovery::{self, DiscoverySummary};
use xxhash_rust::xxh3::xxh3_64;
use std::fs;

/// Incremental analyzer that caches previous results.
pub struct IncrementalAnalyzer {
    config: Config,
    cache: AnalysisCache,
    parser_registry: crate::parser::ParserRegistry,
    root: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisCache {
    pub file_hashes: HashMap<PathBuf, u64>,
    pub symbols: HashMap<SymbolId, Symbol>,
    pub references: Vec<Reference>,
    pub file_dependencies: FileDependencies,
    pub call_graph: SerializableCallGraph,
    pub api_endpoints: Vec<ApiEndpoint>,
    pub db_schema: Option<DbSchema>,
    pub metrics: ProjectMetrics,
    #[serde(default)]
    pub discovery: DiscoverySummary,
    pub errors: Vec<AnalysisError>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisDelta {
    pub added_symbols: Vec<Symbol>,
    pub removed_symbols: Vec<SymbolId>,
    pub changed_symbols: Vec<Symbol>,
    pub added_references: Vec<Reference>,
    pub removed_references: Vec<Reference>,
    pub changed_files: Vec<PathBuf>,
}

impl IncrementalAnalyzer {
    pub fn new(config: Config) -> Result<Self> {
        let cache = Self::load_cache(&config)?;
        let parser_registry = crate::parser::ParserRegistry::new(&config)?;
        let root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        Ok(Self { config, cache, parser_registry, root })
    }

    pub fn analyze(&mut self, root: &Path) -> Result<AnalysisResult> {
        self.root = root.to_path_buf();
        let report = discovery::discover(root, &self.config)?;
        self.cache.discovery = report.summary;
        let files = report.files;
        
        // Determine which files changed
        let (changed_files, unchanged_files) = self.partition_files(&files);
        
        if changed_files.is_empty() && unchanged_files.len() == files.len() {
            // No changes - return cached result
            return Ok(self.cache.to_result());
        }

        // Re-parse changed files
        let mut parse_results = self.parser_registry.parse_all(&changed_files)?;
        
        // Keep unchanged parsed files from cache
        for file in unchanged_files {
            // In a real implementation, we'd cache ParsedFile too
            // For now, re-parse (but could be optimized)
            if let Ok(parsed) = self.parser_registry.parse_all(&[file]) {
                parse_results.files.extend(parsed.files);
            }
        }

        // Rebuild symbol table incrementally
        self.rebuild_incremental(&parse_results)?;
        
        // Save cache
        self.save_cache()?;
        
        Ok(self.cache.to_result())
    }

    pub fn update(&mut self, changed_files: &[PathBuf]) -> Result<AnalysisDelta> {
        let discovered: Vec<DiscoveredFile> = changed_files.iter()
            .filter_map(|p| {
                let lang = Language::from_path(p)?;
                let size = fs::metadata(p).ok()?.len();
                Some(DiscoveredFile {
                    path: p.clone(),
                    language: lang,
                    size,
                })
            })
            .collect();

        let old_symbols = self.cache.symbols.clone();
        let old_refs = self.cache.references.clone();

        let parse_results = self.parser_registry.parse_all(&discovered)?;
        self.rebuild_incremental(&parse_results)?;
        self.save_cache()?;

        // Compute delta
        let mut added_symbols = Vec::new();
        let mut changed_symbols = Vec::new();
        
        for (id, symbol) in &self.cache.symbols {
            if !old_symbols.contains_key(id) {
                added_symbols.push(symbol.clone());
            } else if old_symbols[id] != *symbol {
                changed_symbols.push(symbol.clone());
            }
        }
        
        let removed_symbols: Vec<SymbolId> = old_symbols.keys()
            .filter(|id| !self.cache.symbols.contains_key(*id))
            .cloned()
            .collect();

        let added_references = self.cache.references.iter()
            .filter(|r| !old_refs.contains(r))
            .cloned()
            .collect();
        
        let removed_references = old_refs.iter()
            .filter(|r| !self.cache.references.contains(r))
            .cloned()
            .collect();

        Ok(AnalysisDelta {
            added_symbols,
            removed_symbols,
            changed_symbols,
            added_references,
            removed_references,
            changed_files: changed_files.to_vec(),
        })
    }

    fn partition_files(&self, files: &[DiscoveredFile]) -> (Vec<DiscoveredFile>, Vec<DiscoveredFile>) {
        let mut changed = Vec::new();
        let mut unchanged = Vec::new();
        
        for file in files {
            let hash = self.compute_file_hash(&file.path).unwrap_or(0);
            if self.cache.file_hashes.get(&file.path) != Some(&hash) {
                changed.push(file.clone());
            } else {
                unchanged.push(file.clone());
            }
        }
        (changed, unchanged)
    }

    fn compute_file_hash(&self, path: &Path) -> Result<u64> {
        let content = fs::read(path)?;
        Ok(xxh3_64(&content))
    }

    fn rebuild_incremental(&mut self, parse_results: &crate::ParseResults) -> Result<()> {
        // Update file hashes
        for parsed in &parse_results.files {
            let hash = self.compute_file_hash(&parsed.file.path)?;
            self.cache.file_hashes.insert(parsed.file.path.clone(), hash);
        }
        
        // Remove symbols from deleted/changed files
        let changed_files: HashSet<_> = parse_results.files.iter().map(|f| &f.file.path).collect();
        self.cache.symbols.retain(|_, s| !changed_files.contains(&s.file));
        self.cache.references.retain(|r| !changed_files.contains(&r.file));
        
        // Add new symbols
        for parsed in &parse_results.files {
            for symbol in &parsed.symbols {
                self.cache.symbols.insert(symbol.id.clone(), symbol.clone());
            }
        }
        
        // Rebuild references (simplified - full rebuild)
        self.cache.references = self.build_references(&parse_results.files)?;
        
        // Rebuild file dependencies
        // Rebuild file dependencies
        let discovered: Vec<DiscoveredFile> = parse_results
            .files
            .iter()
            .map(|parsed| parsed.file.clone())
            .collect();
        let imports = self.extract_imports(&parse_results.files)?;
        self.cache.file_dependencies = crate::graph::build_file_dependencies(
            &discovered,
            &self.cache.symbols,
            &imports,
            &self.root,
        )?;
        
        // Rebuild call graph
        let call_graph = crate::graph::build_call_graph(&self.cache.symbols, &self.cache.references)?;
        self.cache.call_graph = (&call_graph).into();
        
        // Re-detect APIs and DB
        self.cache.api_endpoints = self.detect_apis(&self.cache.symbols, parse_results)?;
        self.cache.db_schema = self.detect_database(&self.cache.symbols, parse_results)?;
        
        // Recompute metrics - need to convert back for compute_metrics
        let call_graph_for_metrics: CallGraph = self.cache.call_graph.clone().into();
        let call_graph_ref = &call_graph_for_metrics;
        self.cache.metrics = self.compute_metrics(&self.cache.symbols, &self.cache.file_dependencies, call_graph_ref)?;
        
        self.cache.errors = parse_results.errors.clone();
        
        Ok(())
    }

    fn build_references(&self, _files: &[ParsedFile]) -> Result<Vec<Reference>> {
        // Simplified - full implementation would do proper reference resolution
        Ok(Vec::new())
    }

    fn extract_imports(&self, files: &[ParsedFile]) -> Result<Vec<crate::Import>> {
        let mut imports = Vec::new();
        for file in files {
            imports.extend(file.imports.clone());
        }
        Ok(imports)
    }

    fn detect_apis(&self, _symbols: &HashMap<SymbolId, Symbol>, _parse_results: &crate::ParseResults) -> Result<Vec<ApiEndpoint>> {
        // TODO: Implement
        Ok(Vec::new())
    }

    fn detect_database(&self, _symbols: &HashMap<SymbolId, Symbol>, _parse_results: &crate::ParseResults) -> Result<Option<DbSchema>> {
        // TODO: Implement
        Ok(None)
    }

    fn compute_metrics(&self, _symbols: &HashMap<SymbolId, Symbol>, _file_deps: &FileDependencies, _call_graph: &CallGraph) -> Result<ProjectMetrics> {
        // TODO: Implement
        Ok(ProjectMetrics::default())
    }

    fn load_cache(config: &Config) -> Result<AnalysisCache> {
        if !config.cache.enabled {
            return Ok(AnalysisCache::default());
        }
        
        let cache_file = config.cache_file_path();
        if cache_file.exists() {
            let content = fs::read_to_string(&cache_file)?;
            let cache: AnalysisCache = serde_json::from_str(&content)?;
            Ok(cache)
        } else {
            Ok(AnalysisCache::default())
        }
    }

    fn save_cache(&self) -> Result<()> {
        if !self.config.cache.enabled {
            return Ok(());
        }
        
        let cache_file = self.config.cache_file_path();
        if let Some(parent) = cache_file.parent() {
            fs::create_dir_all(parent)?;
        }
        let content = serde_json::to_string_pretty(&self.cache)?;
        fs::write(&cache_file, content)?;
        
        // TODO: Implement cache size limiting and TTL
        
        Ok(())
    }
}

impl Default for AnalysisCache {
    fn default() -> Self {
        Self {
            file_hashes: HashMap::new(),
            symbols: HashMap::new(),
            references: Vec::new(),
            file_dependencies: FileDependencies::default(),
            call_graph: SerializableCallGraph::default(),
            api_endpoints: Vec::new(),
            db_schema: None,
            metrics: ProjectMetrics::default(),
            discovery: DiscoverySummary::default(),
            errors: Vec::new(),
        }
    }
}

impl AnalysisCache {
    fn to_result(&self) -> AnalysisResult {
        let call_graph: CallGraph = self.call_graph.clone().into();
        AnalysisResult {
            symbols: self.symbols.values().cloned().collect(),
            references: self.references.clone(),
            call_graph,
            file_dependencies: self.file_dependencies.clone(),
            api_endpoints: self.api_endpoints.clone(),
            db_schema: self.db_schema.clone(),
            metrics: self.metrics.clone(),
            discovery: self.discovery.clone(),
            errors: self.errors.clone(),
        }
    }
}

impl Default for FileDependencies {
    fn default() -> Self {
        Self { nodes: Vec::new(), edges: Vec::new() }
    }
}

impl Default for SerializableCallGraph {
    fn default() -> Self {
        Self { nodes: Vec::new(), edges: Vec::new() }
    }
}
