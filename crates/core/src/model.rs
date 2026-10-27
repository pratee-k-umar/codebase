//! Core data models for the analysis engine.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use serde::{Deserialize, Serialize};
use indexmap::IndexMap;
use petgraph::graph::Graph;
use petgraph::Directed;
use petgraph::visit::EdgeRef;

/// Unique identifier for a symbol across the entire codebase.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub struct SymbolId(pub String);

impl SymbolId {
    pub fn new(language: Language, file: &PathBuf, name: &str, kind: SymbolKind, span: Span) -> Self {
        let file_str = file.to_string_lossy().replace(['/', '\\', ':'], "_");
        Self(format!("{}:{}:{}:{}:{}-{}", language.as_str(), file_str, name, kind.as_str(), span.start.line, span.start.column))
    }
}

/// Programming language.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Language {
    TypeScript,
    JavaScript,
    Python,
    Go,
    Rust,
}

impl Language {
    pub fn as_str(&self) -> &'static str {
        match self {
            Language::TypeScript => "ts",
            Language::JavaScript => "js",
            Language::Python => "py",
            Language::Go => "go",
            Language::Rust => "rs",
        }
    }

    pub fn from_path(path: &Path) -> Option<Self> {
        let ext = path.extension()?.to_str()?.to_lowercase();
        match ext.as_str() {
            "ts" | "tsx" => Some(Language::TypeScript),
            "js" | "jsx" | "mjs" | "cjs" => Some(Language::JavaScript),
            "py" => Some(Language::Python),
            "go" => Some(Language::Go),
            "rs" => Some(Language::Rust),
            _ => None,
        }
    }

    pub fn tree_sitter_language(&self) -> tree_sitter::Language {
        match self {
            Language::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            Language::JavaScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            Language::Python => tree_sitter_python::LANGUAGE.into(),
            Language::Go => tree_sitter_go::LANGUAGE.into(),
            Language::Rust => tree_sitter_rust::LANGUAGE.into(),
        }
    }
}

/// Symbol kind classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum SymbolKind {
    #[default]
    Function,
    Method,
    Class,
    Interface,
    TypeAlias,
    Enum,
    Struct,
    Variable,
    Constant,
    Parameter,
    Module,
    Namespace,
    Trait,
    Impl,
    Macro,
}

impl SymbolKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            SymbolKind::Function => "fn",
            SymbolKind::Method => "method",
            SymbolKind::Class => "class",
            SymbolKind::Interface => "interface",
            SymbolKind::TypeAlias => "type",
            SymbolKind::Enum => "enum",
            SymbolKind::Struct => "struct",
            SymbolKind::Variable => "var",
            SymbolKind::Constant => "const",
            SymbolKind::Parameter => "param",
            SymbolKind::Module => "module",
            SymbolKind::Namespace => "namespace",
            SymbolKind::Trait => "trait",
            SymbolKind::Impl => "impl",
            SymbolKind::Macro => "macro",
        }
    }
}

/// Source code span (range).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Span {
    pub start: Position,
    pub end: Position,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Position {
    pub line: usize,      // 1-indexed
    pub column: usize,    // 1-indexed
    pub offset: usize,    // 0-indexed byte offset
}

/// A symbol definition in the codebase.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Symbol {
    pub id: SymbolId,
    pub name: String,
    pub kind: SymbolKind,
    pub language: Language,
    pub file: PathBuf,
    pub span: Span,
    pub signature: Option<Signature>,
    pub visibility: Visibility,
    pub documentation: Option<String>,
    pub attributes: Vec<Attribute>,
    /// Parent symbol (for methods, nested classes, etc.)
    pub parent: Option<SymbolId>,
    /// Child symbols (for classes, modules, etc.)
    pub children: Vec<SymbolId>,
}

/// Function/method signature.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Signature {
    pub parameters: Vec<Parameter>,
    pub return_type: Option<TypeRef>,
    pub type_parameters: Vec<TypeParam>,
    pub is_async: bool,
    pub is_generator: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Parameter {
    pub name: String,
    pub type_ref: Option<TypeRef>,
    pub default_value: Option<String>,
    pub is_optional: bool,
    pub is_rest: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TypeParam {
    pub name: String,
    pub constraint: Option<TypeRef>,
    pub default: Option<TypeRef>,
}

/// Type reference.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TypeRef {
    pub name: String,
    pub generic_args: Vec<TypeRef>,
    pub is_nullable: bool,
    pub is_array: bool,
    pub symbol_id: Option<SymbolId>, // Resolved symbol if available
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Visibility {
    Public,
    Private,
    Protected,
    Internal,
    Package,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Attribute {
    pub name: String,
    pub arguments: Vec<String>,
}

/// A reference (usage) of a symbol.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Reference {
    pub symbol_id: SymbolId,      // The symbol being referenced
    pub file: PathBuf,            // File containing the reference
    pub span: Span,               // Location of the reference
    pub kind: ReferenceKind,      // How it's used
    pub context: Option<String>,  // Surrounding code snippet
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReferenceKind {
    Call,           // Function/method call
    Read,           // Variable/field read
    Write,          // Variable/field write
    Type,           // Type annotation
    Import,         // Import statement
    Export,         // Export statement
    Extends,        // Class extends
    Implements,     // Class implements interface
    Instantiation,  // `new Class()`
    Decorator,      // @Decorator
    Attribute,      // #[attribute] / @attribute
}

/// Import statement.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Import {
    pub file: PathBuf,
    pub span: Span,
    pub module: String,           // Module specifier (e.g., "react", "./utils")
    pub kind: ImportKind,
    pub items: Vec<ImportItem>,
    pub is_type_only: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ImportKind {
    Static,
    Dynamic,      // import()
    Require,      // require()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImportItem {
    pub name: String,
    pub alias: Option<String>,
    pub symbol_id: Option<SymbolId>, // Resolved if available
}

/// Export statement.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Export {
    pub file: PathBuf,
    pub span: Span,
    pub kind: ExportKind,
    pub items: Vec<ExportItem>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExportKind {
    Named,
    Default,
    Namespace,    // export * from
    ReExport,     // export { x } from
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExportItem {
    pub name: String,
    pub alias: Option<String>,
    pub symbol_id: Option<SymbolId>,
}

/// Symbol table mapping SymbolId -> Symbol.
pub type SymbolTable = IndexMap<SymbolId, Symbol>;

/// File-level dependency graph.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileDependencies {
    pub nodes: Vec<FileNode>,
    pub edges: Vec<FileEdge>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileNode {
    pub id: FileId,
    pub path: PathBuf,
    pub language: Language,
    pub symbol_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
pub struct FileId(pub usize);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEdge {
    pub from: FileId,
    pub to: FileId,
    pub kind: FileDepKind,
    pub imports: Vec<String>, // Imported symbols
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Ord, PartialOrd, Serialize, Deserialize)]
pub enum FileDepKind {
    Import,
    ReExport,
    DynamicImport,
    TypeOnly,
}

/// Call graph (function-level) - serializable version.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SerializableCallGraph {
    pub nodes: Vec<CallNode>,
    pub edges: Vec<SerializableCallEdge>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SerializableCallEdge {
    pub from: usize,
    pub to: usize,
    pub kind: CallEdgeKind,
    pub call_site: Span,
}

/// Call graph (function-level) - runtime version using petgraph.
pub type CallGraph = Graph<CallNode, CallEdge, Directed>;

impl From<&CallGraph> for SerializableCallGraph {
    fn from(graph: &CallGraph) -> Self {
        let nodes: Vec<CallNode> = graph.node_weights().cloned().collect();
        let edges: Vec<SerializableCallEdge> = graph
            .edge_references()
            .map(|e| SerializableCallEdge {
                from: e.source().index(),
                to: e.target().index(),
                kind: e.weight().kind,
                call_site: e.weight().call_site,
            })
            .collect();
        Self { nodes, edges }
    }
}

impl From<SerializableCallGraph> for CallGraph {
    fn from(serializable: SerializableCallGraph) -> Self {
        let mut graph = Graph::new();
        let node_indices: Vec<_> = serializable.nodes.iter().map(|_| graph.add_node(CallNode::default())).collect();
        
        // Replace default nodes with actual data
        for (i, node) in serializable.nodes.into_iter().enumerate() {
            graph[node_indices[i]] = node;
        }
        
        for edge in serializable.edges {
            graph.add_edge(node_indices[edge.from], node_indices[edge.to], CallEdge {
                kind: edge.kind,
                call_site: edge.call_site,
            });
        }
        graph
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CallNode {
    pub symbol_id: SymbolId,
    pub name: String,
    pub file: PathBuf,
    pub kind: SymbolKind,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CallEdge {
    pub kind: CallEdgeKind,
    pub call_site: Span,  // Where the call happens
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CallEdgeKind {
    Direct,       // Direct function call
    Method,       // Method call (obj.method())
    Dynamic,      // Indirect/dynamic call
    Callback,     // Passed as callback
    Async,        // Awaited call
}

/// API endpoint detection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiEndpoint {
    pub id: String,
    pub framework: ApiFramework,
    pub method: HttpMethod,
    pub path: String,
    pub handler_symbol: SymbolId,
    pub parameters: Vec<ApiParameter>,
    pub request_schema: Option<TypeRef>,
    pub response_schema: Option<TypeRef>,
    pub middleware: Vec<String>,
    pub file: PathBuf,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ApiFramework {
    Express,
    Fastify,
    Koa,
    Axum,
    Actix,
    Warp,
    FastAPI,
    Flask,
    Django,
    Spring,
    Gin,
    Echo,
    Fiber,
    Hono,
    Other(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Patch,
    Delete,
    Head,
    Options,
    Trace,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiParameter {
    pub name: String,
    pub location: ParamLocation,
    pub type_ref: Option<TypeRef>,
    pub required: bool,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ParamLocation {
    Path,
    Query,
    Header,
    Body,
    Cookie,
}

/// Database schema detection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DbSchema {
    pub tables: Vec<DbTable>,
    pub relationships: Vec<DbRelationship>,
    pub orm: OrmFramework,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DbTable {
    pub name: String,
    pub columns: Vec<DbColumn>,
    pub primary_key: Vec<String>,
    pub indexes: Vec<DbIndex>,
    pub symbol_id: Option<SymbolId>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DbColumn {
    pub name: String,
    pub type_name: String,
    pub nullable: bool,
    pub default: Option<String>,
    pub is_primary: bool,
    pub is_unique: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DbIndex {
    pub name: String,
    pub columns: Vec<String>,
    pub unique: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DbRelationship {
    pub from_table: String,
    pub from_column: String,
    pub to_table: String,
    pub to_column: String,
    pub kind: RelationshipKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RelationshipKind {
    OneToOne,
    OneToMany,
    ManyToOne,
    ManyToMany,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrmFramework {
    Prisma,
    SqlAlchemy,
    TypeOrm,
    Diesel,
    SeaOrm,
    Gorm,
    Hibernate,
    EntityFramework,
    Drizzle,
    Other(String),
}

/// Project-level metrics.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProjectMetrics {
    pub total_files: usize,
    pub total_lines: usize,
    pub total_symbols: usize,
    pub languages: HashMap<Language, usize>,
    pub coupling: CouplingMetrics,
    pub complexity: ComplexityMetrics,
    pub dead_code: DeadCodeMetrics,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CouplingMetrics {
    pub afferent: HashMap<FileId, usize>,   // Incoming deps
    pub efferent: HashMap<FileId, usize>,   // Outgoing deps
    pub instability: HashMap<FileId, f64>,  // Efferent / (Afferent + Efferent)
    pub circular_dependencies: Vec<Vec<FileId>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ComplexityMetrics {
    pub cyclomatic: HashMap<SymbolId, usize>,
    pub cognitive: HashMap<SymbolId, usize>,
    pub nesting_depth: HashMap<SymbolId, usize>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DeadCodeMetrics {
    pub unreachable_symbols: Vec<SymbolId>,
    pub unused_imports: Vec<Import>,
    pub unused_exports: Vec<Export>,
}

/// Analysis errors (non-fatal).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisError {
    pub file: PathBuf,
    pub span: Option<Span>,
    pub kind: ErrorKind,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ErrorKind {
    ParseError,
    UnresolvedReference,
    TypeResolutionFailed,
    CyclicDependency,
    UnsupportedFeature,
    IoError,
}

impl From<&crate::Error> for AnalysisError {
    fn from(err: &crate::Error) -> Self {
        let (file, kind) = match err {
            crate::Error::Parse { file, .. } => (file.clone(), ErrorKind::ParseError),
            crate::Error::Io(_) => (PathBuf::new(), ErrorKind::IoError),
            crate::Error::UnsupportedLanguage(_) | crate::Error::TreeSitter(_) => {
                (PathBuf::new(), ErrorKind::UnsupportedFeature)
            }
            crate::Error::TypeResolution(_) => (PathBuf::new(), ErrorKind::TypeResolutionFailed),
            _ => (PathBuf::new(), ErrorKind::ParseError),
        };
        AnalysisError {
            file,
            span: None,
            kind,
            message: err.to_string(),
        }
    }
}

/// Full analysis result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisResult {
    pub symbols: Vec<Symbol>,
    pub references: Vec<Reference>,
    #[serde(with = "call_graph_serde")]
    pub call_graph: CallGraph,
    pub file_dependencies: FileDependencies,
    pub api_endpoints: Vec<ApiEndpoint>,
    pub db_schema: Option<DbSchema>,
    pub metrics: ProjectMetrics,
    /// Which files were analyzed and which were skipped, with reasons.
    #[serde(default)]
    pub discovery: crate::discovery::DiscoverySummary,
    pub errors: Vec<AnalysisError>,
}

mod call_graph_serde {
    use super::{CallGraph, SerializableCallGraph};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S>(graph: &CallGraph, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let serializable: SerializableCallGraph = graph.into();
        SerializableCallGraph::serialize(&serializable, serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<CallGraph, D::Error>
    where
        D: Deserializer<'de>,
    {
        let serializable = SerializableCallGraph::deserialize(deserializer)?;
        Ok(serializable.into())
    }
}

impl AnalysisResult {
    pub fn symbol_count(&self) -> usize { self.symbols.len() }
    pub fn reference_count(&self) -> usize { self.references.len() }
    pub fn file_count(&self) -> usize { self.file_dependencies.nodes.len() }
}