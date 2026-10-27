//! Main analysis orchestration.

use std::path::Path;
use std::collections::HashMap;
use crate::{AnalysisResult, Symbol, SymbolId, Reference, FileDependencies, CallGraph, ApiEndpoint, DbSchema, ProjectMetrics, Config, Result, ParsedFile, Language, SymbolKind, Import, DeadCodeMetrics};
use crate::discovery::{self, DiscoveryReport};
use crate::parser::ParserRegistry;
use crate::graph::{build_file_dependencies, build_call_graph, compute_coupling, detect_dead_code};

/// Main analysis entry point.
pub fn analyze(root: &Path, config: Config) -> Result<AnalysisResult> {
    analyze_with(root, config, &mut |_| {})
}

/// Analyze and report coarse progress to `progress`.
///
/// The callback receives short human-readable stage descriptions. It is called
/// from the analyzing thread, so callers that move work to a background thread
/// can forward these to the UI.
pub fn analyze_with(
    root: &Path,
    config: Config,
    progress: &mut dyn FnMut(&str),
) -> Result<AnalysisResult> {
    let mut analyzer = Analyzer::new(config)?;
    analyzer.analyze(root, progress)
}

/// Analyzer orchestrates the full analysis pipeline.
pub struct Analyzer {
    config: Config,
    parser_registry: ParserRegistry,
}

impl Analyzer {
    pub fn new(config: Config) -> Result<Self> {
        let parser_registry = ParserRegistry::new(&config)?;
        Ok(Self { config, parser_registry })
    }

    pub fn analyze(
        &mut self,
        root: &Path,
        progress: &mut dyn FnMut(&str),
    ) -> Result<AnalysisResult> {
        progress("Discovering files");
        // 1. Discover files, recording why anything was skipped
        let DiscoveryReport { files, summary: discovery } = discovery::discover(root, &self.config)?;
        eprintln!(
            "Discovered {} files ({} skipped, {} unsupported)",
            files.len(),
            discovery.total_skipped(),
            discovery.files_unsupported
        );

        // 2. Parse all files in parallel
        progress(&format!("Parsing {} files", files.len()));
        let parse_results = self.parser_registry.parse_all(&files)?;
        eprintln!("Parsed {} files, {} errors", parse_results.files.len(), parse_results.errors.len());

        // 3. Build symbol table
        let (symbol_table, references) = self.build_symbol_table(&parse_results)?;
        eprintln!("Found {} symbols, {} references", symbol_table.len(), references.len());

        // 4. Construct dependency graphs
        progress("Resolving imports");
        let imports = self.extract_imports(&parse_results);
        let file_deps = build_file_dependencies(&files, &symbol_table, &imports, root)?;
        eprintln!(
            "File dependencies: {} nodes, {} edges",
            file_deps.nodes.len(),
            file_deps.edges.len()
        );

        progress("Building call graph");
        let call_graph = build_call_graph(&symbol_table, &references)?;
        eprintln!("Call graph: {} nodes, {} edges", call_graph.node_count(), call_graph.edge_count());

        // 5. Detect APIs and database schemas
        progress("Detecting APIs and schemas");
        let api_endpoints = self.detect_apis(&symbol_table, &parse_results)?;
        eprintln!("API endpoints: {}", api_endpoints.len());

        let db_schema = self.detect_database(&symbol_table, &parse_results)?;
        if let Some(ref schema) = db_schema {
            eprintln!("Database tables: {}", schema.tables.len());
        }

        // 6. Compute metrics
        progress("Computing metrics");
        let metrics = self.compute_metrics(&symbol_table, &file_deps, &call_graph)?;
        
        Ok(AnalysisResult {
            symbols: symbol_table.into_values().collect(),
            references,
            call_graph,
            file_dependencies: file_deps,
            api_endpoints,
            db_schema,
            metrics,
            discovery,
            errors: parse_results.errors,
        })
    }

    fn build_symbol_table(&self, parse_results: &crate::ParseResults) -> Result<(HashMap<SymbolId, Symbol>, Vec<Reference>)> {
        let mut symbols = HashMap::new();
        let mut all_references = Vec::new();

        // Collect all symbols from parsed files
        for parsed in &parse_results.files {
            for symbol in &parsed.symbols {
                symbols.insert(symbol.id.clone(), symbol.clone());
            }
        }

        // Resolve references (simplified - real implementation would be more complex)
        for parsed in &parse_results.files {
            let refs = self.resolve_references(&parsed, &symbols)?;
            all_references.extend(refs);
        }

        Ok((symbols, all_references))
    }

    fn resolve_references(&self, parsed: &ParsedFile, symbols: &HashMap<SymbolId, Symbol>) -> Result<Vec<Reference>> {
        let mut references = Vec::new();
        
        // For each symbol in this file, find references to other symbols
        // This is a simplified version - real implementation would walk the AST
        for _symbol in &parsed.symbols {
            // Find calls within this symbol's span
            // For now, we'll create placeholder references from imports
            for import in &parsed.imports {
                for item in &import.items {
                    if let Some(symbol_id) = &item.symbol_id {
                        if symbols.contains_key(symbol_id) {
                            references.push(Reference {
                                symbol_id: symbol_id.clone(),
                                file: parsed.file.path.clone(),
                                span: import.span,
                                kind: crate::ReferenceKind::Import,
                                context: Some(import.module.clone()),
                            });
                        }
                    }
                }
            }
        }
        
        Ok(references)
    }

    fn extract_imports(&self, parse_results: &crate::ParseResults) -> Vec<Import> {
        let mut imports = Vec::new();
        for parsed in &parse_results.files {
            imports.extend(parsed.imports.clone());
        }
        imports
    }

    fn detect_apis(&self, symbols: &HashMap<SymbolId, Symbol>, parse_results: &crate::ParseResults) -> Result<Vec<ApiEndpoint>> {
        let mut endpoints = Vec::new();
        
        for symbol in symbols.values() {
            // Check for framework-specific patterns
            if let Some(endpoint) = self.detect_api_endpoint(symbol, parse_results)? {
                endpoints.push(endpoint);
            }
        }
        
        Ok(endpoints)
    }

    fn detect_api_endpoint(&self, symbol: &Symbol, _parse_results: &crate::ParseResults) -> Result<Option<ApiEndpoint>> {
        // Look for framework decorators/attributes
        for attr in &symbol.attributes {
            let framework = match (symbol.language, attr.name.as_str()) {
                (Language::TypeScript | Language::JavaScript, "Get") | 
                (Language::TypeScript | Language::JavaScript, "Post") |
                (Language::TypeScript | Language::JavaScript, "Put") |
                (Language::TypeScript | Language::JavaScript, "Delete") |
                (Language::TypeScript | Language::JavaScript, "Patch") => Some(crate::ApiFramework::Express),
                
                (Language::Python, "app.get") | (Language::Python, "app.post") |
                (Language::Python, "app.put") | (Language::Python, "app.delete") => Some(crate::ApiFramework::FastAPI),
                
                (Language::Rust, "get") | (Language::Rust, "post") | (Language::Rust, "put") | (Language::Rust, "delete") => Some(crate::ApiFramework::Axum),
                
                (Language::Go, "GET") | (Language::Go, "POST") | (Language::Go, "PUT") | (Language::Go, "DELETE") => Some(crate::ApiFramework::Gin),
                
                _ => None,
            };

            if let Some(framework) = framework {
                let method = self.extract_http_method(&attr.name);
                let path = self.extract_route_path(&attr.arguments).unwrap_or_else(|| "/".to_string());
                
                return Ok(Some(ApiEndpoint {
                    id: format!("api_{}", symbol.id.0),
                    framework,
                    method,
                    path,
                    handler_symbol: symbol.id.clone(),
                    parameters: Vec::new(),
                    request_schema: None,
                    response_schema: None,
                    middleware: Vec::new(),
                    file: symbol.file.clone(),
                    span: symbol.span,
                }));
            }
        }
        Ok(None)
    }

    fn extract_http_method(&self, attr_name: &str) -> crate::HttpMethod {
        let upper = attr_name.to_uppercase();
        match upper.as_str() {
            "GET" => crate::HttpMethod::Get,
            "POST" => crate::HttpMethod::Post,
            "PUT" => crate::HttpMethod::Put,
            "DELETE" => crate::HttpMethod::Delete,
            "PATCH" => crate::HttpMethod::Patch,
            "HEAD" => crate::HttpMethod::Head,
            "OPTIONS" => crate::HttpMethod::Options,
            "TRACE" => crate::HttpMethod::Trace,
            _ => crate::HttpMethod::Get,
        }
    }

    fn extract_route_path(&self, args: &[String]) -> Option<String> {
        args.first().cloned()
    }

    fn detect_database(&self, symbols: &HashMap<SymbolId, Symbol>, _parse_results: &crate::ParseResults) -> Result<Option<DbSchema>> {
        // Look for ORM models
        let tables = Vec::new();
        let mut orm = None;

        for symbol in symbols.values() {
            if matches!(symbol.kind, SymbolKind::Class | SymbolKind::Struct) {
                for attr in &symbol.attributes {
                    let detected_orm = match (symbol.language, attr.name.as_str()) {
                        (Language::TypeScript | Language::JavaScript, "Entity") | 
                        (Language::TypeScript | Language::JavaScript, "Table") => Some(crate::OrmFramework::TypeOrm),
                        
                        (Language::Python, "Table") | (Language::Python, "Model") => Some(crate::OrmFramework::SqlAlchemy),
                        
                        (Language::Rust, "Table") | (Language::Rust, "Model") => Some(crate::OrmFramework::SeaOrm),
                        (Language::Rust, "Entity") => Some(crate::OrmFramework::Diesel),
                        
                        (Language::Go, "Table") => Some(crate::OrmFramework::Gorm),
                        
                        _ => None,
                    };
                    
                    if let Some(detected) = detected_orm {
                        orm = Some(detected);
                        
                        // Extract table schema from struct fields
                        // This is simplified - real implementation would parse field definitions
                        if let Some(_signature) = &symbol.signature {
                            // For structs, we'd need to parse fields
                        }
                    }
                }
            }
        }

        if tables.is_empty() {
            Ok(None)
        } else {
            Ok(Some(DbSchema {
                tables,
                relationships: Vec::new(),
                orm: orm.unwrap_or(crate::OrmFramework::Other("unknown".into())),
            }))
        }
    }

    fn compute_metrics(&self, symbols: &HashMap<SymbolId, Symbol>, file_deps: &FileDependencies, _call_graph: &CallGraph) -> Result<ProjectMetrics> {
        let mut languages = HashMap::new();
        for symbol in symbols.values() {
            *languages.entry(symbol.language).or_default() += 1;
        }

        let coupling = compute_coupling(file_deps);
        let dead_code_report = detect_dead_code(symbols, &self.extract_references(symbols)?, &self.find_entry_points(symbols)?);

        Ok(ProjectMetrics {
            total_files: file_deps.nodes.len(),
            total_lines: symbols.values().map(|s| s.span.end.line - s.span.start.line).sum(),
            total_symbols: symbols.len(),
            languages,
            coupling,
            complexity: Default::default(), // TODO: Compute cyclomatic complexity
            dead_code: DeadCodeMetrics {
                unreachable_symbols: dead_code_report.unreachable_symbols,
                unused_imports: Vec::new(),
                unused_exports: Vec::new(),
            },
        })
    }

    fn extract_references(&self, _symbols: &HashMap<SymbolId, Symbol>) -> Result<Vec<Reference>> {
        // Placeholder - would be populated during symbol resolution
        Ok(Vec::new())
    }

    fn find_entry_points(&self, symbols: &HashMap<SymbolId, Symbol>) -> Result<Vec<SymbolId>> {
        // Find main functions, HTTP handlers, test functions, etc.
        let mut entry_points = Vec::new();
        for symbol in symbols.values() {
            if symbol.name == "main" || symbol.name.contains("test") || symbol.attributes.iter().any(|a| a.name == "test") {
                entry_points.push(symbol.id.clone());
            }
        }
        Ok(entry_points)
    }
}
