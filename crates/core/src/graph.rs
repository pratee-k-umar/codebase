//! Dependency graph construction and analysis.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::path::PathBuf;
use serde::{Deserialize, Serialize};
use crate::{Symbol, SymbolId, Reference, FileDependencies, FileId, FileNode, FileEdge, FileDepKind, CallGraph, CallNode, CallEdge, CallEdgeKind, Result, Import, SymbolKind, Span, CouplingMetrics, DiscoveredFile};
use crate::resolve::FileIndex;
use petgraph::graph::{Graph, NodeIndex};
use petgraph::Direction;
use petgraph::algo::tarjan_scc;
use petgraph::visit::{EdgeRef, IntoNodeIdentifiers};

/// Build the file-level dependency graph.
///
/// Every analyzed file becomes a node, even when it defines no symbols, so the
/// graph reflects the real shape of the tree. Edges come from resolving each
/// import to a scanned file; unresolved imports (external packages) are dropped.
pub fn build_file_dependencies(
    files: &[DiscoveredFile],
    symbols: &HashMap<SymbolId, Symbol>,
    imports: &[Import],
    root: &Path,
) -> Result<FileDependencies> {
    let index = FileIndex::new(files, root);

    let mut nodes: Vec<FileNode> = files
        .iter()
        .enumerate()
        .map(|(position, file)| FileNode {
            id: FileId(position),
            path: file.path.clone(),
            language: file.language,
            symbol_count: 0,
        })
        .collect();

    // Attribute symbols to their file. Index map avoids a linear scan per symbol.
    let by_path: HashMap<PathBuf, usize> = nodes
        .iter()
        .enumerate()
        .map(|(position, node)| (crate::resolve::normalize(&node.path), position))
        .collect();

    for symbol in symbols.values() {
        if let Some(position) = by_path.get(&crate::resolve::normalize(&symbol.file)) {
            nodes[*position].symbol_count += 1;
        }
    }

    // Aggregate edges per (from, to, kind) so a file pair appears once with all
    // the names it imports.
    let mut aggregated: BTreeMap<(usize, usize, FileDepKind), Vec<String>> = BTreeMap::new();

    for import in imports {
        let Some(from) = index.id_of(&import.file).map(|id| id.0) else {
            continue;
        };
        let Some(to) = index.resolve(import).map(|id| id.0) else {
            continue;
        };
        if from == to {
            continue;
        }

        let kind = match import.kind {
            crate::ImportKind::Static => FileDepKind::Import,
            crate::ImportKind::Dynamic => FileDepKind::DynamicImport,
            crate::ImportKind::Require => FileDepKind::Import,
        };

        let names = aggregated.entry((from, to, kind)).or_default();
        if !import.module.is_empty() && !names.contains(&import.module) {
            names.push(import.module.clone());
        }
        for item in &import.items {
            if !names.contains(&item.name) {
                names.push(item.name.clone());
            }
        }
    }

    let edges = aggregated
        .into_iter()
        .map(|((from, to, kind), imports)| FileEdge {
            from: FileId(from),
            to: FileId(to),
            kind,
            imports,
        })
        .collect();

    Ok(FileDependencies { nodes, edges })
}

/// Build call graph from references.
pub fn build_call_graph(
    symbols: &HashMap<SymbolId, Symbol>,
    references: &[Reference],
) -> Result<CallGraph> {
    let mut graph = Graph::new();
    let mut symbol_to_node: HashMap<SymbolId, NodeIndex> = HashMap::new();

    // Add nodes for all function/method symbols
    for symbol in symbols.values() {
        if matches!(symbol.kind, SymbolKind::Function | SymbolKind::Method) {
            let node_idx = graph.add_node(CallNode {
                symbol_id: symbol.id.clone(),
                name: symbol.name.clone(),
                file: symbol.file.clone(),
                kind: symbol.kind,
            });
            symbol_to_node.insert(symbol.id.clone(), node_idx);
        }
    }

    // Add edges for calls
    for reference in references {
        if reference.kind == crate::ReferenceKind::Call {
            if let Some(caller_id) = find_containing_function(&reference.symbol_id, symbols, references) {
                if let (Some(&caller_idx), Some(&callee_idx)) = (
                    symbol_to_node.get(&caller_id),
                    symbol_to_node.get(&reference.symbol_id),
                ) {
                    graph.add_edge(caller_idx, callee_idx, CallEdge {
                        kind: CallEdgeKind::Direct,
                        call_site: reference.span,
                    });
                }
            }
        }
    }

    Ok(graph)
}

/// Find the function that contains a given reference.
fn find_containing_function(
    symbol_id: &SymbolId,
    symbols: &HashMap<SymbolId, Symbol>,
    references: &[Reference],
) -> Option<SymbolId> {
    // Find the reference in the references list
    let reference = references.iter().find(|r| r.symbol_id == *symbol_id)?;
    
    // Find which function contains this reference by checking span containment
    for symbol in symbols.values() {
        if matches!(symbol.kind, SymbolKind::Function | SymbolKind::Method) {
            if span_contains(&symbol.span, &reference.span) {
                return Some(symbol.id.clone());
            }
        }
    }
    None
}

fn span_contains(outer: &Span, inner: &Span) -> bool {
    inner.start.offset >= outer.start.offset && inner.end.offset <= outer.end.offset
}

/// Analyze graph for strongly connected components (circular dependencies).
pub fn find_circular_dependencies(deps: &FileDependencies) -> Vec<Vec<FileId>> {
    let mut graph = Graph::new();
    let mut file_id_to_node: HashMap<FileId, NodeIndex> = HashMap::new();

    for node in &deps.nodes {
        let idx = graph.add_node(node.id);
        file_id_to_node.insert(node.id, idx);
    }

    for edge in &deps.edges {
        if let (Some(&from), Some(&to)) = (file_id_to_node.get(&edge.from), file_id_to_node.get(&edge.to)) {
            graph.add_edge(from, to, edge.kind);
        }
    }

    let sccs = tarjan_scc(&graph);
    sccs.into_iter()
        .filter(|scc| scc.len() > 1)
        .map(|scc| scc.into_iter().map(|n| graph[n]).collect())
        .collect()
}

/// Compute centrality metrics for symbols.
pub fn compute_centrality(call_graph: &CallGraph) -> HashMap<SymbolId, CentralityMetrics> {
    let mut metrics = HashMap::new();
    
    // Out-degree centrality (how many functions this calls)
    for node_idx in call_graph.node_identifiers() {
        let symbol_id = call_graph[node_idx].symbol_id.clone();
        let out_degree = call_graph.edges_directed(node_idx, Direction::Outgoing).count();
        let in_degree = call_graph.edges_directed(node_idx, Direction::Incoming).count();
        
        let entry = metrics.entry(symbol_id).or_insert_with(CentralityMetrics::default);
        entry.out_degree = out_degree;
        entry.in_degree = in_degree;
    }

    // Betweenness centrality (simplified - sample-based)
    // Full betweenness is O(VE), too expensive for large graphs
    // Use Brandes algorithm approximation if needed

    metrics
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CentralityMetrics {
    pub out_degree: usize,
    pub in_degree: usize,
    pub betweenness: f64,
    pub pagerank: f64,
}

/// Compute coupling metrics for files.
pub fn compute_coupling(deps: &FileDependencies) -> CouplingMetrics {
    let mut afferent: HashMap<FileId, usize> = HashMap::new();
    let mut efferent: HashMap<FileId, usize> = HashMap::new();

    for edge in &deps.edges {
        *efferent.entry(edge.from).or_default() += 1;
        *afferent.entry(edge.to).or_default() += 1;
    }

    let mut instability = HashMap::new();
    for node in &deps.nodes {
        let a = *afferent.get(&node.id).unwrap_or(&0) as f64;
        let e = *efferent.get(&node.id).unwrap_or(&0) as f64;
        if a + e > 0.0 {
            instability.insert(node.id, e / (a + e));
        } else {
            instability.insert(node.id, 0.0);
        }
    }

    CouplingMetrics {
        afferent,
        efferent,
        instability,
        circular_dependencies: find_circular_dependencies(deps),
    }
}

/// Impact analysis - find all symbols affected by a change.
pub fn impact_analysis(
    call_graph: &CallGraph,
    changed_symbols: &[SymbolId],
) -> ImpactReport {
    let mut affected = HashSet::new();
    let mut visited = HashSet::new();
    let mut queue = Vec::new();

    // Start from changed symbols
    for sym_id in changed_symbols {
        if let Some(node_idx) = find_node_by_symbol(call_graph, sym_id) {
            queue.push(node_idx);
            visited.insert(node_idx);
        }
    }

    // BFS upstream (callers)
    while let Some(current) = queue.pop() {
        affected.insert(call_graph[current].symbol_id.clone());
        
        for edge in call_graph.edges_directed(current, Direction::Incoming) {
            let caller = edge.source();
            if visited.insert(caller) {
                queue.push(caller);
            }
        }
    }

    let total_affected = affected.len();
    ImpactReport {
        changed_symbols: changed_symbols.to_vec(),
        affected_symbols: affected.into_iter().collect(),
        total_affected,
    }
}

fn find_node_by_symbol(graph: &CallGraph, symbol_id: &SymbolId) -> Option<NodeIndex> {
    graph.node_identifiers().find(|&n| graph[n].symbol_id == *symbol_id)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImpactReport {
    pub changed_symbols: Vec<SymbolId>,
    pub affected_symbols: Vec<SymbolId>,
    pub total_affected: usize,
}

/// Dead code detection.
pub fn detect_dead_code(
    symbols: &HashMap<SymbolId, Symbol>,
    references: &[Reference],
    entry_points: &[SymbolId],
) -> DeadCodeReport {
    let mut reachable = HashSet::new();
    let call_graph = build_call_graph(symbols, references).unwrap_or_default();

    // BFS from entry points
    let mut queue = Vec::new();
    for ep in entry_points {
        if let Some(idx) = find_node_by_symbol(&call_graph, ep) {
            queue.push(idx);
            reachable.insert(idx);
        }
    }

    while let Some(current) = queue.pop() {
        for edge in call_graph.edges_directed(current, Direction::Outgoing) {
            let next = edge.target();
            if reachable.insert(next) {
                queue.push(next);
            }
        }
    }

    let unreachable: Vec<SymbolId> = symbols
        .values()
        .filter(|s| matches!(s.kind, SymbolKind::Function | SymbolKind::Method))
        .filter_map(|s| find_node_by_symbol(&call_graph, &s.id))
        .filter(|&n| !reachable.contains(&n))
        .map(|n| call_graph[n].symbol_id.clone())
        .collect();

    DeadCodeReport { unreachable_symbols: unreachable }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeadCodeReport {
    pub unreachable_symbols: Vec<SymbolId>,
}