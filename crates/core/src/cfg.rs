//! Control-flow graphs.
//!
//! Turns a function body into a flowchart: start, actions, decisions with
//! labelled branches, loops, returns and throws, ending at a single end node.
//! This is what the block-diagram view draws, and it is deliberately derived
//! from real syntax rather than guessed from symbol names.
//!
//! Construction uses a classic "fragment" approach: each statement compiles to
//! a fragment with one entry and a set of dangling exits. Statements are then
//! chained by connecting the previous exits to the next entry, which keeps
//! nesting (if inside a loop inside a function) straightforward.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tree_sitter::{Node, Parser};

use crate::parser::{node_to_span, node_text};
use crate::{Error, Language, Result, Span, Symbol};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FlowKind {
    Start,
    End,
    Action,
    Decision,
    Loop,
    Return,
    Throw,
    Try,
    Catch,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlowNode {
    pub id: usize,
    pub kind: FlowKind,
    /// Short text drawn inside the node.
    pub label: String,
    /// Full source of the statement, for the code panel.
    pub detail: String,
    pub span: Span,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlowEdge {
    pub from: usize,
    pub to: usize,
    pub label: Option<String>,
    /// Back edges close a loop and are drawn differently.
    pub back: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlowGraph {
    pub function: String,
    pub file: PathBuf,
    pub nodes: Vec<FlowNode>,
    pub edges: Vec<FlowEdge>,
}

impl FlowGraph {
    pub fn start(&self) -> Option<usize> {
        self.nodes
            .iter()
            .find(|node| node.kind == FlowKind::Start)
            .map(|node| node.id)
    }

    pub fn node(&self, id: usize) -> Option<&FlowNode> {
        self.nodes.iter().find(|node| node.id == id)
    }

    pub fn outgoing(&self, id: usize) -> impl Iterator<Item = &FlowEdge> {
        self.edges.iter().filter(move |edge| edge.from == id)
    }
}

/// Build the control-flow graph for one function.
///
/// Returns `Ok(None)` when the declaration cannot be located, which happens for
/// synthetic symbols such as interface members.
pub fn analyze_function(path: &Path, function: &Symbol) -> Result<Option<FlowGraph>> {
    let source = std::fs::read_to_string(path)?;

    let mut parser = Parser::new();
    parser
        .set_language(&function.language.tree_sitter_language())
        .map_err(|err| Error::tree_sitter(format!("cannot load grammar: {err}")))?;

    let tree = parser
        .parse(&source, None)
        .ok_or_else(|| Error::parse(path.to_path_buf(), "failed to parse file"))?;

    let root = tree.root_node();
    let Some(declaration) = find_declaration(root, function.span.start.offset, function.language)
    else {
        return Ok(None);
    };

    let Some(body) = function_body(declaration, function.language) else {
        return Ok(None);
    };

    let mut builder = Builder::new(&source);
    builder.compile_function(body);

    Ok(Some(builder.finish(function)))
}

fn find_declaration<'a>(node: Node<'a>, offset: usize, language: Language) -> Option<Node<'a>> {
    if node.start_byte() == offset && is_function_like(node.kind(), language) {
        return Some(node);
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.start_byte() > offset {
            break;
        }
        if let Some(found) = find_declaration(child, offset, language) {
            return Some(found);
        }
    }

    None
}

fn is_function_like(kind: &str, language: Language) -> bool {
    match language {
        Language::TypeScript | Language::JavaScript => matches!(
            kind,
            "function_declaration"
                | "function_expression"
                | "arrow_function"
                | "method_definition"
                | "generator_function_declaration"
                | "generator_function"
        ),
        Language::Python => kind == "function_definition",
        Language::Rust => kind == "function_item",
        Language::Go => matches!(kind, "function_declaration" | "method_declaration"),
    }
}

fn function_body<'a>(declaration: Node<'a>, language: Language) -> Option<Node<'a>> {
    if let Some(body) = declaration.child_by_field_name("body") {
        return Some(body);
    }

    // Python's grammar exposes `body`; Rust/Go expose `body` too, so the
    // fallback is only for odd shapes such as expression-bodied arrows.
    let mut cursor = declaration.walk();
    let child = declaration
        .children(&mut cursor)
        .find(|child| matches!(child.kind(), "block" | "statement_block" | "declaration_list"));

    child.or_else(|| match language {
        Language::TypeScript | Language::JavaScript => Some(declaration),
        _ => None,
    })
}

/// A compiled statement: one entry point and edges still looking for a target.
struct Fragment {
    entry: Option<usize>,
    exits: Vec<(usize, Option<String>)>,
}

impl Fragment {
    fn terminal(entry: usize) -> Self {
        Self {
            entry: Some(entry),
            exits: Vec::new(),
        }
    }
}

struct LoopContext {
    head: usize,
    breaks: Vec<usize>,
}

struct Builder<'a> {
    source: &'a str,
    nodes: Vec<FlowNode>,
    edges: Vec<FlowEdge>,
    loops: Vec<LoopContext>,
    returns: Vec<usize>,
    throws: Vec<usize>,
    /// Entry of the function body, and the exits still awaiting a target.
    entry: Option<usize>,
    exits: Vec<(usize, Option<String>)>,
}

impl<'a> Builder<'a> {
    fn new(source: &'a str) -> Self {
        Self {
            source,
            nodes: Vec::new(),
            edges: Vec::new(),
            loops: Vec::new(),
            returns: Vec::new(),
            throws: Vec::new(),
            entry: None,
            exits: Vec::new(),
        }
    }

    fn finish(mut self, function: &Symbol) -> FlowGraph {
        let start = self.push(FlowKind::Start, "Start".to_string(), function.span, String::new());
        let end = self.push(FlowKind::End, "End".to_string(), function.span, String::new());

        if let Some(entry) = self.entry {
            self.edge(start, entry, None, false);
        } else {
            self.edge(start, end, None, false);
        }

        for (exit, label) in std::mem::take(&mut self.exits) {
            self.edge(exit, end, label, false);
        }
        for id in std::mem::take(&mut self.returns) {
            self.edge(id, end, Some("return".to_string()), false);
        }
        for id in std::mem::take(&mut self.throws) {
            self.edge(id, end, Some("throw".to_string()), false);
        }

        FlowGraph {
            function: function.name.clone(),
            file: function.file.clone(),
            nodes: self.nodes,
            edges: self.edges,
        }
    }

    fn compile_function(&mut self, body: Node) {
        // Expression-bodied arrows have no block to iterate.
        let fragment = if matches!(body.kind(), "statement_block" | "block" | "declaration_list") {
            self.compile_block(body)
        } else {
            self.compile_statement(body)
        };
        self.entry = fragment.entry;
        self.exits = fragment.exits;
    }

    fn compile_block(&mut self, block: Node) -> Fragment {
        let mut exits: Vec<(usize, Option<String>)> = Vec::new();
        let mut entry = None;

        let mut cursor = block.walk();
        for child in block.named_children(&mut cursor) {
            if matches!(child.kind(), "comment" | "line_comment" | "block_comment") {
                continue;
            }

            let fragment = self.compile_statement(child);

            if let Some(fragment_entry) = fragment.entry {
                for (from, label) in exits.drain(..) {
                    self.edge(from, fragment_entry, label, false);
                }
                if entry.is_none() {
                    entry = Some(fragment_entry);
                }
            }

            exits.extend(fragment.exits);
        }

        Fragment { entry, exits }
    }

    fn compile_statement(&mut self, node: Node) -> Fragment {
        match node.kind() {
            "if_statement" | "if_expression" => self.compile_if(node),
            "for_statement" | "for_in_statement" | "for_expression" | "while_statement"
            | "while_expression" | "loop_expression" | "do_statement" => self.compile_loop(node),
            "switch_statement" | "match_expression" | "expression_switch_statement"
            | "type_switch_statement" => self.compile_switch(node),
            "try_statement" | "try_expression" => self.compile_try(node),
            "return_statement" | "return_expression" => {
                let id = self.block_node(FlowKind::Return, "return", node);
                self.returns.push(id);
                Fragment::terminal(id)
            }
            "throw_statement" | "raise_statement" => {
                let id = self.block_node(FlowKind::Throw, "raise", node);
                self.throws.push(id);
                Fragment::terminal(id)
            }
            "break_statement" | "break_expression" => {
                let id = self.block_node(FlowKind::Action, "break", node);
                if let Some(context) = self.loops.last_mut() {
                    context.breaks.push(id);
                    Fragment::terminal(id)
                } else {
                    Fragment {
                        entry: Some(id),
                        exits: vec![(id, None)],
                    }
                }
            }
            "continue_statement" | "continue_expression" => {
                let id = self.block_node(FlowKind::Action, "continue", node);
                if let Some(context) = self.loops.last() {
                    let head = context.head;
                    self.edge(id, head, Some("continue".to_string()), true);
                    Fragment::terminal(id)
                } else {
                    Fragment {
                        entry: Some(id),
                        exits: vec![(id, None)],
                    }
                }
            }
            _ => {
                let id = self.action_node(node);
                Fragment {
                    entry: Some(id),
                    exits: vec![(id, None)],
                }
            }
        }
    }

    fn compile_if(&mut self, node: Node) -> Fragment {
        let condition = node
            .child_by_field_name("condition")
            .map(|condition| self.text(condition))
            .unwrap_or_else(|| "if".to_string());

        let decision = self.block_node(FlowKind::Decision, &condition, node);

        let mut exits = Vec::new();

        if let Some(consequence) = node.child_by_field_name("consequence") {
            let branch = self.compile_branch(consequence);
            if let Some(entry) = branch.entry {
                self.edge(decision, entry, Some("yes".to_string()), false);
            }
            exits.extend(branch.exits);
        }

        match node.child_by_field_name("alternative") {
            Some(alternative) => {
                // `else if` arrives as an if-statement inside the else clause.
                let branch = self.compile_branch(alternative);
                if let Some(entry) = branch.entry {
                    self.edge(decision, entry, Some("no".to_string()), false);
                }
                exits.extend(branch.exits);
            }
            None => exits.push((decision, Some("no".to_string()))),
        }

        Fragment {
            entry: Some(decision),
            exits,
        }
    }

    /// Compile the body of a branch, which may be a block or a single statement.
    fn compile_branch(&mut self, node: Node) -> Fragment {
        match node.kind() {
            "statement_block" | "block" | "declaration_list" => self.compile_block(node),
            "else_clause" => {
                // `else_clause` wraps a block; unwrap it so we do not create a
                // pointless action node for the keyword.
                match node.named_child(0) {
                    Some(inner) => self.compile_branch(inner),
                    None => Fragment {
                        entry: None,
                        exits: Vec::new(),
                    },
                }
            }
            "elif_clause" => {
                let condition = node
                    .child_by_field_name("condition")
                    .map(|condition| self.text(condition))
                    .unwrap_or_else(|| "elif".to_string());
                let decision = self.block_node(FlowKind::Decision, &condition, node);
                let mut exits = Vec::new();
                if let Some(consequence) = node.child_by_field_name("consequence") {
                    let branch = self.compile_branch(consequence);
                    if let Some(entry) = branch.entry {
                        self.edge(decision, entry, Some("yes".to_string()), false);
                    }
                    exits.extend(branch.exits);
                }
                match node.child_by_field_name("alternative") {
                    Some(alternative) => {
                        let branch = self.compile_branch(alternative);
                        if let Some(entry) = branch.entry {
                            self.edge(decision, entry, Some("no".to_string()), false);
                        }
                        exits.extend(branch.exits);
                    }
                    None => exits.push((decision, Some("no".to_string()))),
                }
                Fragment {
                    entry: Some(decision),
                    exits,
                }
            }
            _ => self.compile_statement(node),
        }
    }

    fn compile_loop(&mut self, node: Node) -> Fragment {
        let header = node
            .child_by_field_name("condition")
            .or_else(|| node.child_by_field_name("left"))
            .map(|condition| self.text(condition))
            .unwrap_or_else(|| self.text(node));

        let head = self.block_node(FlowKind::Loop, &header, node);
        self.loops.push(LoopContext {
            head,
            breaks: Vec::new(),
        });

        let body = node
            .child_by_field_name("body")
            .map(|body| self.compile_branch(body))
            .unwrap_or(Fragment {
                entry: None,
                exits: Vec::new(),
            });

        let mut exits = Vec::new();
        if let Some(entry) = body.entry {
            self.edge(head, entry, Some("body".to_string()), false);
        }

        for (from, label) in body.exits {
            self.edge(
                from,
                head,
                Some(label.unwrap_or_else(|| "loop".to_string())),
                true,
            );
        }

        if let Some(context) = self.loops.pop() {
            for breaker in context.breaks {
                exits.push((breaker, Some("break".to_string())));
            }
        }

        exits.push((head, Some("exit".to_string())));

        Fragment {
            entry: Some(head),
            exits,
        }
    }

    fn compile_switch(&mut self, node: Node) -> Fragment {
        let subject = node
            .child_by_field_name("value")
            .or_else(|| node.child_by_field_name("subject"))
            .map(|value| self.text(value))
            .unwrap_or_else(|| "match".to_string());

        let decision = self.block_node(FlowKind::Decision, &subject, node);
        let mut exits = Vec::new();
        let mut matched_any = false;

        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            let is_case = child.kind().contains("case") || child.kind().contains("arm");
            if !is_case {
                continue;
            }

            matched_any = true;
            let label = child
                .child_by_field_name("value")
                .or_else(|| child.child_by_field_name("pattern"))
                .map(|value| self.text(value))
                .unwrap_or_else(|| "case".to_string());

            // The last named child of a case/arm is usually its body.
            let body = child.named_child(child.named_child_count().saturating_sub(1));
            let branch = match body {
                Some(body) => self.compile_branch(body),
                None => Fragment {
                    entry: None,
                    exits: Vec::new(),
                },
            };

            if let Some(entry) = branch.entry {
                self.edge(decision, entry, Some(label), false);
            }
            exits.extend(branch.exits);
        }

        if !matched_any {
            exits.push((decision, None));
        }

        Fragment {
            entry: Some(decision),
            exits,
        }
    }

    fn compile_try(&mut self, node: Node) -> Fragment {
        let try_node = self.block_node(FlowKind::Try, "try", node);
        let mut exits = Vec::new();

        if let Some(body) = node.child_by_field_name("body") {
            let body = self.compile_branch(body);
            if let Some(entry) = body.entry {
                self.edge(try_node, entry, None, false);
            }
            exits.extend(body.exits);
        }

        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            if child.kind() != "catch_clause" && child.kind() != "except_clause" {
                continue;
            }

            let catch = self.block_node(FlowKind::Catch, "catch", child);
            self.edge(try_node, catch, Some("error".to_string()), false);

            if let Some(body) = child.child_by_field_name("body") {
                let body = self.compile_branch(body);
                if let Some(entry) = body.entry {
                    self.edge(catch, entry, None, false);
                }
                exits.extend(body.exits);
            } else {
                exits.push((catch, None));
            }
        }

        if exits.is_empty() {
            exits.push((try_node, None));
        }

        Fragment {
            entry: Some(try_node),
            exits,
        }
    }

    fn action_node(&mut self, node: Node) -> usize {
        let text = self.text(node);
        let detail = self.detail(node);
        self.push(FlowKind::Action, text, node_to_span(node, self.source), detail)
    }

    fn block_node(&mut self, kind: FlowKind, label: &str, node: Node) -> usize {
        let label = truncate(label, 46);
        let detail = self.detail(node);
        self.push(kind, label, node_to_span(node, self.source), detail)
    }

    fn push(&mut self, kind: FlowKind, label: String, span: Span, detail: String) -> usize {
        let id = self.nodes.len();
        self.nodes.push(FlowNode {
            id,
            kind,
            label,
            detail,
            span,
        });
        id
    }

    /// Full statement text for the code panel, capped so a huge block does not
    /// bloat the graph.
    fn detail(&self, node: Node) -> String {
        let raw = node_text(node, self.source).trim();
        let capped: String = raw.chars().take(600).collect();
        capped
    }

    fn edge(&mut self, from: usize, to: usize, label: Option<String>, back: bool) {
        if from == to {
            return;
        }
        let already = self
            .edges
            .iter()
            .any(|edge| edge.from == from && edge.to == to && edge.label == label);
        if already {
            return;
        }
        self.edges.push(FlowEdge {
            from,
            to,
            label,
            back,
        });
    }

    /// Short, single-line text for a node label.
    fn text(&self, node: Node) -> String {
        let raw = node_text(node, self.source);
        let first_line = raw.lines().next().unwrap_or("").trim();
        let condensed = first_line.split_whitespace().collect::<Vec<_>>().join(" ");
        truncate(&condensed, 60)
    }
}

fn truncate(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    let mut out: String = text.chars().take(limit.saturating_sub(1)).collect();
    out.push('…');
    out
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Position, SymbolId, SymbolKind, Visibility};

    fn function_symbol(path: &Path, language: Language, offset: usize) -> Symbol {
        Symbol {
            id: SymbolId("test".into()),
            name: "subject".into(),
            kind: SymbolKind::Function,
            language,
            file: path.to_path_buf(),
            span: Span {
                start: Position {
                    line: 1,
                    column: 1,
                    offset,
                },
                end: Position {
                    line: 1,
                    column: 1,
                    offset,
                },
            },
            signature: None,
            visibility: Visibility::Public,
            documentation: None,
            attributes: Vec::new(),
            parent: None,
            children: Vec::new(),
        }
    }

    fn flow_for(source: &str, language: Language, marker: &str) -> FlowGraph {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("subject.ts");
        std::fs::write(&path, source).unwrap();

        let offset = source.find(marker).expect("marker");
        let symbol = function_symbol(&path, language, offset);
        analyze_function(&path, &symbol).unwrap().expect("flow graph")
    }

    #[test]
    fn simple_sequence_has_start_and_end() {
        let flow = flow_for(
            "function subject() {\n  const a = 1;\n  const b = 2;\n}\n",
            Language::TypeScript,
            "function subject",
        );

        assert_eq!(flow.nodes.iter().filter(|n| n.kind == FlowKind::Start).count(), 1);
        assert_eq!(flow.nodes.iter().filter(|n| n.kind == FlowKind::End).count(), 1);
        assert!(flow.nodes.iter().filter(|n| n.kind == FlowKind::Action).count() >= 2);
        assert!(!flow.edges.is_empty());
    }

    #[test]
    fn if_statement_creates_a_labelled_decision() {
        let flow = flow_for(
            "function subject(x) {\n  if (x > 0) {\n    return 1;\n  } else {\n    return 2;\n  }\n}\n",
            Language::TypeScript,
            "function subject",
        );

        let decision = flow
            .nodes
            .iter()
            .find(|node| node.kind == FlowKind::Decision)
            .expect("decision node");

        let labels: Vec<&str> = flow
            .outgoing(decision.id)
            .filter_map(|edge| edge.label.as_deref())
            .collect();

        assert!(labels.contains(&"yes"), "labels: {labels:?}");
        assert!(labels.contains(&"no"), "labels: {labels:?}");
        assert_eq!(
            flow.nodes.iter().filter(|n| n.kind == FlowKind::Return).count(),
            2
        );
    }

    #[test]
    fn loops_produce_back_edges() {
        let flow = flow_for(
            "function subject(items) {\n  for (const item of items) {\n    consume(item);\n  }\n}\n",
            Language::TypeScript,
            "function subject",
        );

        assert!(flow.nodes.iter().any(|node| node.kind == FlowKind::Loop));
        assert!(
            flow.edges.iter().any(|edge| edge.back),
            "a loop must have a back edge"
        );
    }

    #[test]
    fn loops_with_break_and_continue_stay_connected() {
        let flow = flow_for(
            "function subject(items) {\n  for (const item of items) {\n    if (!item) { continue; }\n    if (item.done) { break; }\n    consume(item);\n  }\n}\n",
            Language::TypeScript,
            "function subject",
        );

        assert!(flow.edges.iter().filter(|edge| edge.back).count() >= 1);
        assert!(flow.edges.iter().any(|edge| edge.label.as_deref() == Some("break")));
    }

    #[test]
    fn python_if_else_is_supported() {
        let flow = flow_for(
            "def subject(x):\n    if x > 0:\n        return 1\n    else:\n        return 2\n",
            Language::Python,
            "def subject",
        );

        assert!(flow.nodes.iter().any(|n| n.kind == FlowKind::Decision));
        assert_eq!(
            flow.nodes.iter().filter(|n| n.kind == FlowKind::Return).count(),
            2
        );
    }

    #[test]
    fn try_catch_is_supported() {
        let flow = flow_for(
            "function subject() {\n  try {\n    risky();\n  } catch (error) {\n    handle(error);\n  }\n}\n",
            Language::TypeScript,
            "function subject",
        );

        assert!(flow.nodes.iter().any(|n| n.kind == FlowKind::Try));
        assert!(flow.nodes.iter().any(|n| n.kind == FlowKind::Catch));
    }

    #[test]
    fn every_edge_points_at_a_real_node() {
        let flow = flow_for(
            "function subject(items) {\n  for (const item of items) {\n    if (item.ok) { use(item); } else { skip(item); }\n  }\n}\n",
            Language::TypeScript,
            "function subject",
        );

        for edge in &flow.edges {
            assert!(flow.node(edge.from).is_some(), "edge from {} is dangling", edge.from);
            assert!(flow.node(edge.to).is_some(), "edge to {} is dangling", edge.to);
        }
    }
}


