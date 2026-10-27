//! Export formats for analysis results.

use std::io::Write;
use crate::{AnalysisResult, Result, Language, SymbolKind, FileDepKind};

// Re-export for convenience
pub use crate::OutputFormat;

/// Export analysis result to various formats.
pub fn export(result: &AnalysisResult, format: OutputFormat, writer: &mut dyn Write) -> Result<()> {
    match format {
        OutputFormat::Json => export_json(result, writer),
        OutputFormat::Yaml => export_yaml(result, writer),
        OutputFormat::Summary => export_summary(result, writer),
        OutputFormat::Mermaid => export_mermaid(result, writer),
        OutputFormat::GraphML => export_graphml(result, writer),
        OutputFormat::Dot => export_dot(result, writer),
        OutputFormat::Csv => export_csv(result, writer),
    }
}

fn export_json(result: &AnalysisResult, writer: &mut dyn Write) -> Result<()> {
    serde_json::to_writer_pretty(writer, result)?;
    Ok(())
}

fn export_yaml(result: &AnalysisResult, writer: &mut dyn Write) -> Result<()> {
    serde_yaml::to_writer(writer, result)?;
    Ok(())
}

fn export_summary(result: &AnalysisResult, writer: &mut dyn Write) -> Result<()> {
    // `files_included` is what discovery selected and the parser read. Files
    // with no symbols (re-export barrels, empty modules) never reach the
    // dependency graph, so `file_count()` alone understates the work done.
    let files_parsed = if result.discovery.files_included > 0 {
        result.discovery.files_included
    } else {
        result.file_count()
    };

    writeln!(writer, "=== Codebase Analysis Summary ===")?;
    writeln!(writer)?;
    writeln!(writer, "Files analyzed: {}", files_parsed)?;
    writeln!(writer, "Files with symbols: {}", result.file_count())?;
    writeln!(writer, "Total symbols: {}", result.symbol_count())?;
    writeln!(writer, "Total references: {}", result.reference_count())?;
    writeln!(writer, "API endpoints: {}", result.api_endpoints.len())?;
    writeln!(writer, "Database tables: {}", result.db_schema.as_ref().map(|s| s.tables.len()).unwrap_or(0))?;
    writeln!(writer)?;
    
    writeln!(writer, "=== Discovery ===")?;
    writeln!(writer, "  Files scanned: {}", result.discovery.files_scanned)?;
    writeln!(writer, "  Files included: {}", result.discovery.files_included)?;
    writeln!(writer, "  Files ignored: {}", result.discovery.files_ignored)?;
    writeln!(writer, "  Directories pruned: {}", result.discovery.dirs_pruned)?;
    writeln!(writer, "  Unsupported files: {}", result.discovery.files_unsupported)?;
    if !result.discovery.ignored_by_reason.is_empty() {
        writeln!(writer, "  Skipped by reason:")?;
        for (reason, count) in &result.discovery.ignored_by_reason {
            writeln!(writer, "    {}: {}", reason.as_str(), count)?;
        }
    }
    if result.discovery.ignored_truncated {
        writeln!(writer, "  (skipped-entry sample truncated)")?;
    }
    
    writeln!(writer)?;
    writeln!(writer, "=== By Language ===")?;
    for (lang, count) in &result.metrics.languages {
        writeln!(writer, "  {:?}: {} symbols", lang, count)?;
    }
    
    writeln!(writer)?;
    writeln!(writer, "=== Coupling ===")?;
    writeln!(writer, "  Circular dependencies: {}", result.metrics.coupling.circular_dependencies.len())?;
    
    writeln!(writer)?;
    writeln!(writer, "=== Complexity ===")?;
    let avg_cyclomatic = if result.metrics.complexity.cyclomatic.is_empty() {
        0.0
    } else {
        result.metrics.complexity.cyclomatic.values().sum::<usize>() as f64 / result.metrics.complexity.cyclomatic.len() as f64
    };
    writeln!(writer, "  Avg cyclomatic complexity: {:.1}", avg_cyclomatic)?;
    
    writeln!(writer)?;
    writeln!(writer, "=== Dead Code ===")?;
    writeln!(writer, "  Unreachable symbols: {}", result.metrics.dead_code.unreachable_symbols.len())?;
    writeln!(writer, "  Unused imports: {}", result.metrics.dead_code.unused_imports.len())?;
    writeln!(writer, "  Unused exports: {}", result.metrics.dead_code.unused_exports.len())?;
    
    if !result.errors.is_empty() {
        writeln!(writer)?;
        writeln!(writer, "=== Errors ({}) ===", result.errors.len())?;
        for err in &result.errors {
            writeln!(writer, "  {}: {}", err.file.display(), err.message)?;
        }
    }
    
    Ok(())
}

fn export_mermaid(result: &AnalysisResult, writer: &mut dyn Write) -> Result<()> {
    writeln!(writer, "```mermaid")?;
    writeln!(writer, "graph TD")?;
    
    // File dependency graph
    writeln!(writer, "  subgraph Files")?;
    for node in &result.file_dependencies.nodes {
        let label = node.path.file_name().unwrap_or_default().to_string_lossy();
        writeln!(writer, "    F{:?}[\"{}\"]", node.id.0, label)?;
    }
    writeln!(writer, "  end")?;
    
    for edge in &result.file_dependencies.edges {
        let label = match edge.kind {
            FileDepKind::Import => "imports",
            FileDepKind::DynamicImport => "dynamic imports",
            FileDepKind::ReExport => "re-exports",
            FileDepKind::TypeOnly => "types",
        };
        writeln!(writer, "  F{:?} -->|{}| F{:?}", edge.from.0, label, edge.to.0)?;
    }
    
    writeln!(writer, "```")?;
    Ok(())
}

fn export_graphml(result: &AnalysisResult, writer: &mut dyn Write) -> Result<()> {
    // Write GraphML manually since graphml crate API is unstable
    writeln!(writer, "<?xml version=\"1.0\" encoding=\"UTF-8\"?>")?;
    writeln!(writer, "<graphml xmlns=\"http://graphml.graphdrawing.org/xmlns\"")?;
    writeln!(writer, "       xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\"")?;
    writeln!(writer, "       xsi:schemaLocation=\"http://graphml.graphdrawing.org/xmlns")?;
    writeln!(writer, "       http://graphml.graphdrawing.org/xmlns/1.0/graphml.xsd\">")?;
    writeln!(writer, "  <key id=\"label\" for=\"node\" attr.name=\"label\" attr.type=\"string\"/>")?;
    writeln!(writer, "  <key id=\"kind\" for=\"node\" attr.name=\"kind\" attr.type=\"string\"/>")?;
    writeln!(writer, "  <key id=\"language\" for=\"node\" attr.name=\"language\" attr.type=\"string\"/>")?;
    writeln!(writer, "  <key id=\"edge_kind\" for=\"edge\" attr.name=\"kind\" attr.type=\"string\"/>")?;
    writeln!(writer, "  <key id=\"edge_label\" for=\"edge\" attr.name=\"label\" attr.type=\"string\"/>")?;
    writeln!(writer, "  <graph id=\"codebase\" edgedefault=\"directed\">")?;
    
    // Add file nodes
    for node in &result.file_dependencies.nodes {
        let label = node.path.file_name().unwrap_or_default().to_string_lossy().escape_xml().to_string();
        writeln!(writer, "    <node id=\"file_{}\">", node.id.0)?;
        writeln!(writer, "      <data key=\"label\">{}</data>", label)?;
        writeln!(writer, "      <data key=\"kind\">file</data>")?;
        writeln!(writer, "      <data key=\"language\">{:?}</data>", node.language)?;
        writeln!(writer, "    </node>")?;
    }
    
    // Add symbol nodes (functions only for brevity)
    for symbol in &result.symbols {
        if matches!(symbol.kind, SymbolKind::Function | SymbolKind::Method) {
            let id = format!("sym_{}", symbol.id.0.replace([':', '.', '/', '\\'], "_"));
            let label = symbol.name.escape_xml().to_string();
            writeln!(writer, "    <node id=\"{}\">", id)?;
            writeln!(writer, "      <data key=\"label\">{}</data>", label)?;
            writeln!(writer, "      <data key=\"kind\">{:?}</data>", symbol.kind)?;
            writeln!(writer, "      <data key=\"language\">{:?}</data>", symbol.language)?;
            writeln!(writer, "    </node>")?;
        }
    }
    
    // Add file dependency edges
    for edge in &result.file_dependencies.edges {
        writeln!(writer, "    <edge source=\"file_{}\" target=\"file_{}\">", edge.from.0, edge.to.0)?;
        writeln!(writer, "      <data key=\"edge_kind\">{:?}</data>", edge.kind)?;
        writeln!(writer, "      <data key=\"edge_label\">{}</data>", edge.imports.join(", ").escape_xml().to_string())?;
        writeln!(writer, "    </edge>")?;
    }
    
    // Add call edges using SerializableCallGraph
    // We need to iterate through the call graph nodes by index
    // For now, we'll skip call edges in GraphML export to avoid complexity
    // They can be added by converting to SerializableCallGraph first
    
    writeln!(writer, "  </graph>")?;
    writeln!(writer, "</graphml>")?;
    Ok(())
}

// Helper trait for XML escaping
trait EscapeXml {
    fn escape_xml(&self) -> String;
}

impl EscapeXml for &str {
    fn escape_xml(&self) -> String {
        self.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
            .replace('\'', "&apos;")
    }
}

impl EscapeXml for String {
    fn escape_xml(&self) -> String {
        self.as_str().escape_xml()
    }
}

impl EscapeXml for std::borrow::Cow<'_, str> {
    fn escape_xml(&self) -> String {
        self.as_ref().escape_xml()
    }
}

fn export_dot(result: &AnalysisResult, writer: &mut dyn Write) -> Result<()> {
    writeln!(writer, "digraph codebase {{")?;
    writeln!(writer, "  rankdir=LR;")?;
    writeln!(writer, "  node [shape=box, style=filled];")?;
    
    // File nodes
    for node in &result.file_dependencies.nodes {
        let label = node.path.file_name().unwrap_or_default().to_string_lossy();
        let color = match node.language {
            Language::TypeScript | Language::JavaScript => "lightblue",
            Language::Python => "lightyellow",
            Language::Go => "lightcyan",
            Language::Rust => "lightcoral",
        };
        writeln!(writer, "  F{} [label=\"{}\", fillcolor={}];", node.id.0, label, color)?;
    }
    
    // File edges
    for edge in &result.file_dependencies.edges {
        writeln!(writer, "  F{} -> F{} [label=\"{:?}\"];", edge.from.0, edge.to.0, edge.kind)?;
    }
    
    writeln!(writer, "}}")?;
    Ok(())
}

fn export_csv(result: &AnalysisResult, writer: &mut dyn Write) -> Result<()> {
    // Symbols CSV
    writeln!(writer, "id,name,kind,language,file,line,visibility")?;
    for symbol in &result.symbols {
        writeln!(writer, "{},{},{:?},{:?},{},{},{:?}",
            symbol.id.0,
            symbol.name,
            symbol.kind,
            symbol.language,
            symbol.file.display(),
            symbol.span.start.line,
            symbol.visibility
        )?;
    }
    
    writeln!(writer)?;
    writeln!(writer, "# References")?;
    writeln!(writer, "symbol_id,file,line,column,kind")?;
    for reference in &result.references {
        writeln!(writer, "{},{},{},{},{:?}",
            reference.symbol_id.0,
            reference.file.display(),
            reference.span.start.line,
            reference.span.start.column,
            reference.kind
        )?;
    }
    
    Ok(())
}
