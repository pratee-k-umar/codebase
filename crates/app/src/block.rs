//! Block diagram: a document-style view of the selected files.
//!
//! Where the graph is a node-link map, this is a structural view: one card per
//! file, classes and their methods nested inside it, and import arrows between
//! cards. It answers "what is inside these files and what do they depend on"
//! without requiring you to zoom into a canvas.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use codebase_intel_core::resolve::normalize;
use codebase_intel_core::{AnalysisResult, Language, Symbol, SymbolId, SymbolKind};
use egui::{
    Align2, Color32, CornerRadius, FontId, Pos2, Rect, Sense, Shape, Stroke, Ui, Vec2,
};

use crate::graph::{kind_tag, language_color};

const BACKGROUND: Color32 = Color32::from_rgb(18, 20, 24);
const CARD_BODY: Color32 = Color32::from_rgb(32, 36, 44);
const CARD_BORDER: Color32 = Color32::from_rgb(52, 58, 70);
const TEXT: Color32 = Color32::from_rgb(214, 219, 228);
const TEXT_MUTED: Color32 = Color32::from_rgb(132, 141, 156);
const EDGE: Color32 = Color32::from_rgb(70, 78, 92);
const EDGE_HIGHLIGHT: Color32 = Color32::from_rgb(94, 168, 255);
const ENTRY: Color32 = Color32::from_rgb(110, 200, 140);

const CARD_WIDTH: f32 = 330.0;
const HEADER_HEIGHT: f32 = 30.0;
const ROW_HEIGHT: f32 = 18.0;
const PADDING: f32 = 8.0;
const GAP: f32 = 34.0;
const MAX_ROWS: usize = 60;

#[derive(Debug, Clone)]
pub struct BlockRow {
    pub label: String,
    pub kind: SymbolKind,
    pub depth: usize,
    pub line: usize,
}

#[derive(Debug, Clone)]
pub struct BlockCard {
    pub id: usize,
    pub label: String,
    pub directory: String,
    pub language: Language,
    pub rows: Vec<BlockRow>,
    pub total_symbols: usize,
    pub entry: bool,
    pub pos: Pos2,
    pub size: Vec2,
}

impl BlockCard {
    pub fn rect(&self) -> Rect {
        Rect::from_min_size(self.pos, self.size)
    }
}

#[derive(Debug, Clone)]
pub struct BlockEdge {
    pub from: usize,
    pub to: usize,
    pub label: String,
}

#[derive(Debug, Clone)]
pub struct BlockDiagram {
    pub cards: Vec<BlockCard>,
    pub edges: Vec<BlockEdge>,
    pub bounds: Rect,
}

impl BlockDiagram {
    pub fn build(
        result: &AnalysisResult,
        visible: Option<&HashSet<usize>>,
        entry_points: &[usize],
    ) -> Self {
        let entry: HashSet<usize> = entry_points.iter().copied().collect();

        // Symbols by file, so nesting can be resolved per card.
        let mut by_file: HashMap<PathBuf, Vec<&Symbol>> = HashMap::new();
        for symbol in &result.symbols {
            by_file
                .entry(normalize(&symbol.file))
                .or_default()
                .push(symbol);
        }

        let mut cards = Vec::new();
        for node in &result.file_dependencies.nodes {
            if let Some(visible) = visible {
                if !visible.contains(&node.id.0) {
                    continue;
                }
            }

            let key = normalize(&node.path);
            let rows = by_file
                .get(&key)
                .map(|symbols| build_rows(symbols))
                .unwrap_or_default();

            let total_symbols = rows.len();
            let shown = rows.len().min(MAX_ROWS);
            let mut rows = rows;
            rows.truncate(shown);

            let extra = usize::from(total_symbols > shown);
            let height = HEADER_HEIGHT
                + PADDING
                + (rows.len() + extra) as f32 * ROW_HEIGHT
                + PADDING;

            cards.push(BlockCard {
                id: node.id.0,
                label: node
                    .path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| node.path.to_string_lossy().into_owned()),
                directory: describe_directory(&node.path),
                language: node.language,
                rows,
                total_symbols,
                entry: entry.contains(&node.id.0),
                pos: Pos2::ZERO,
                size: Vec2::new(CARD_WIDTH, height),
            });
        }

        cards.sort_by(|a, b| a.directory.cmp(&b.directory).then_with(|| a.label.cmp(&b.label)));

        let mut edges = Vec::new();
        for edge in &result.file_dependencies.edges {
            let (from, to) = (edge.from.0, edge.to.0);
            let from_visible = cards.iter().any(|card| card.id == from);
            let to_visible = cards.iter().any(|card| card.id == to);
            if !from_visible || !to_visible || from == to {
                continue;
            }

            edges.push(BlockEdge {
                from,
                to,
                label: edge.imports.first().cloned().unwrap_or_default(),
            });
        }

        let mut diagram = Self {
            cards,
            edges,
            bounds: Rect::from_min_size(Pos2::ZERO, Vec2::splat(1.0)),
        };
        diagram.layout();
        diagram
    }

    /// Masonry layout: cards fill the shortest column.
    pub fn layout(&mut self) {
        if self.cards.is_empty() {
            self.bounds = Rect::from_min_size(Pos2::ZERO, Vec2::splat(400.0));
            return;
        }

        let columns = ((self.cards.len() as f32).sqrt().ceil() as usize).clamp(1, 5);
        let mut heights = vec![0.0f32; columns];

        for card in &mut self.cards {
            let column = heights
                .iter()
                .enumerate()
                .min_by(|a, b| a.1.total_cmp(b.1))
                .map(|(index, _)| index)
                .unwrap_or(0);

            let x = column as f32 * (CARD_WIDTH + GAP);
            let y = heights[column];
            card.pos = Pos2::new(x, y);
            heights[column] = y + card.size.y + GAP;
        }

        let mut bounds = Rect::NOTHING;
        for card in &self.cards {
            bounds = bounds.union(card.rect());
        }
        if !bounds.is_finite() {
            bounds = Rect::from_min_size(Pos2::ZERO, Vec2::splat(400.0));
        }
        self.bounds = bounds;
    }

    pub fn card(&self, id: usize) -> Option<&BlockCard> {
        self.cards.iter().find(|card| card.id == id)
    }
}

/// Flatten a file's symbols into display rows, nesting children one level in.
fn build_rows(symbols: &[&Symbol]) -> Vec<BlockRow> {
    let by_id: HashMap<&SymbolId, &Symbol> = symbols
        .iter()
        .map(|symbol| (&symbol.id, *symbol))
        .collect();

    let mut roots: Vec<&Symbol> = symbols
        .iter()
        .filter(|symbol| symbol.parent.is_none())
        .copied()
        .collect();
    roots.sort_by(|a, b| a.span.start.line.cmp(&b.span.start.line));

    let mut rows = Vec::new();
    for root in roots {
        push_with_children(&mut rows, root, 0, &by_id);
        if rows.len() >= MAX_ROWS {
            break;
        }
    }

    // Files whose symbols all have parents still deserve a list.
    if rows.is_empty() {
        let mut all: Vec<&Symbol> = symbols.to_vec();
        all.sort_by(|a, b| a.span.start.line.cmp(&b.span.start.line));
        for symbol in all.into_iter().take(MAX_ROWS) {
            rows.push(BlockRow {
                label: symbol.name.clone(),
                kind: symbol.kind,
                depth: 0,
                line: symbol.span.start.line,
            });
        }
    }

    rows
}

fn push_with_children(
    rows: &mut Vec<BlockRow>,
    symbol: &Symbol,
    depth: usize,
    by_id: &HashMap<&SymbolId, &Symbol>,
) {
    if rows.len() >= MAX_ROWS {
        return;
    }

    rows.push(BlockRow {
        label: symbol.name.clone(),
        kind: symbol.kind,
        depth,
        line: symbol.span.start.line,
    });

    let mut children: Vec<&Symbol> = symbol
        .children
        .iter()
        .filter_map(|id| by_id.get(id).copied())
        .collect();
    children.sort_by(|a, b| a.span.start.line.cmp(&b.span.start.line));

    for child in children {
        if depth >= 1 {
            break;
        }
        push_with_children(rows, child, depth + 1, by_id);
    }
}

fn describe_directory(path: &std::path::Path) -> String {
    let parent = path.parent().unwrap_or_else(|| std::path::Path::new(""));
    let components: Vec<String> = parent
        .components()
        .filter_map(|component| match component {
            std::path::Component::Normal(part) => Some(part.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect();
    let start = components.len().saturating_sub(2);
    let label = components[start..].join("/");
    if label.is_empty() {
        ".".to_string()
    } else {
        label
    }
}

/// Interactive state for the block diagram canvas.
pub struct BlockView {
    pub diagram: BlockDiagram,
    pub zoom: f32,
    pub pan: Vec2,
    pub selected: Option<usize>,
    pub fit_requested: bool,
    pub last_canvas: Rect,
    dragging: Option<usize>,
}

impl BlockView {
    pub fn new(diagram: BlockDiagram) -> Self {
        Self {
            diagram,
            zoom: 1.0,
            pan: Vec2::ZERO,
            selected: None,
            fit_requested: true,
            last_canvas: Rect::ZERO,
            dragging: None,
        }
    }

    fn screen_to_world(&self, canvas: Rect, screen: Pos2) -> Pos2 {
        ((screen - canvas.min - self.pan) / self.zoom).to_pos2()
    }

    fn world_to_screen(&self, canvas: Rect, world: Pos2) -> Pos2 {
        canvas.min + self.pan + world.to_vec2() * self.zoom
    }

    pub fn fit(&mut self, canvas: Rect) {
        let bounds = self.diagram.bounds;
        if bounds.width() <= 0.0 || bounds.height() <= 0.0 {
            return;
        }
        let margin = 40.0;
        let scale_x = (canvas.width() - margin * 2.0) / bounds.width();
        let scale_y = (canvas.height() - margin * 2.0) / bounds.height();
        self.zoom = scale_x.min(scale_y).clamp(0.1, 1.4);
        self.pan = canvas.size() / 2.0 - (bounds.center().to_vec2() * self.zoom);
    }

    fn hit_test(&self, world: Pos2) -> Option<usize> {
        self.diagram
            .cards
            .iter()
            .rev()
            .find(|card| card.rect().contains(world))
            .map(|card| card.id)
    }

    pub fn ui(&mut self, ui: &mut Ui) {
        let (response, painter) = ui.allocate_painter(ui.available_size(), Sense::click_and_drag());
        let canvas = response.rect;
        self.last_canvas = canvas;
        painter.rect_filled(canvas, 0.0, BACKGROUND);

        if self.fit_requested {
            self.fit(canvas);
            self.fit_requested = false;
        }

        let scroll = ui.input(|input| input.raw_scroll_delta.y);
        if response.hovered() && scroll.abs() > 0.0 {
            if let Some(pointer) = response.hover_pos() {
                let before = self.screen_to_world(canvas, pointer);
                self.zoom = (self.zoom * (1.0 + scroll * 0.0016)).clamp(0.1, 3.0);
                let after = self.world_to_screen(canvas, before);
                self.pan += pointer - after;
            }
        }

        if response.drag_started() {
            if let Some(pointer) = response.interact_pointer_pos() {
                self.dragging = self.hit_test(self.screen_to_world(canvas, pointer));
            }
        }

        if response.dragged() {
            let delta = response.drag_delta();
            match self.dragging {
                Some(id) => {
                    if let Some(card) = self.diagram.cards.iter_mut().find(|card| card.id == id) {
                        card.pos += delta / self.zoom;
                    }
                }
                None => self.pan += delta,
            }
        }

        if response.drag_stopped() {
            self.dragging = None;
        }

        if response.clicked() {
            if let Some(pointer) = response.interact_pointer_pos() {
                self.selected = self.hit_test(self.screen_to_world(canvas, pointer));
            }
        }

        let painter = painter.with_clip_rect(canvas);
        let viewport = Rect::from_min_max(
            self.screen_to_world(canvas, canvas.min),
            self.screen_to_world(canvas, canvas.max),
        );

        self.paint_edges(&painter, canvas, viewport);
        self.paint_cards(&painter, canvas, viewport);

        painter.text(
            Pos2::new(canvas.left() + 12.0, canvas.bottom() - 10.0),
            Align2::LEFT_BOTTOM,
            "scroll: zoom   ·   drag: pan   ·   click: inspect",
            FontId::proportional(11.0),
            TEXT_MUTED,
        );

        if self.diagram.cards.is_empty() {
            painter.text(
                canvas.center(),
                Align2::CENTER_CENTER,
                "No files in scope.",
                FontId::proportional(14.0),
                TEXT_MUTED,
            );
        }
    }

    fn paint_edges(&self, painter: &egui::Painter, canvas: Rect, viewport: Rect) {
        for edge in &self.diagram.edges {
            let Some(source) = self.diagram.card(edge.from) else {
                continue;
            };
            let Some(target) = self.diagram.card(edge.to) else {
                continue;
            };

            let highlighted = self.selected == Some(edge.from) || self.selected == Some(edge.to);
            let color = if highlighted {
                EDGE_HIGHLIGHT
            } else {
                EDGE.gamma_multiply(0.8)
            };
            let width = if highlighted { 2.0 } else { 1.0 };

            let source_rect = source.rect();
            let target_rect = target.rect();
            let span = Rect::from_two_pos(source_rect.center(), target_rect.center());
            if !source_rect.intersects(viewport) && !target_rect.intersects(viewport) && !span.intersects(viewport) {
                continue;
            }

            let (start, end) = if target_rect.center().x >= source_rect.center().x {
                (source_rect.right_center(), target_rect.left_center())
            } else {
                (source_rect.left_center(), target_rect.right_center())
            };

            let start = self.world_to_screen(canvas, start);
            let end = self.world_to_screen(canvas, end);
            let mid_x = (start.x + end.x) / 2.0;

            painter.add(Shape::line(
                vec![start, Pos2::new(mid_x, start.y), Pos2::new(mid_x, end.y), end],
                Stroke::new(width, color),
            ));
            paint_arrowhead(painter, end, end.x >= start.x, color, width);

            if highlighted && !edge.label.is_empty() {
                painter.text(
                    Pos2::new(mid_x + 5.0, (start.y + end.y) / 2.0),
                    Align2::LEFT_CENTER,
                    &edge.label,
                    FontId::proportional(10.0),
                    TEXT_MUTED,
                );
            }
        }
    }

    fn paint_cards(&self, painter: &egui::Painter, canvas: Rect, viewport: Rect) {
        for card in &self.diagram.cards {
            if !card.rect().intersects(viewport) {
                continue;
            }

            let rect = Rect::from_min_max(
                self.world_to_screen(canvas, card.pos),
                self.world_to_screen(canvas, card.pos + card.size),
            );
            if !rect.intersects(canvas) {
                continue;
            }

            let selected = self.selected == Some(card.id);
            let border = if selected {
                EDGE_HIGHLIGHT
            } else {
                CARD_BORDER
            };

            painter.rect_filled(rect.expand(if selected { 2.0 } else { 1.0 }), CornerRadius::same(7), border);
            painter.rect_filled(rect, CornerRadius::same(6), CARD_BODY);

            let header = Rect::from_min_size(
                rect.min,
                Vec2::new(rect.width(), HEADER_HEIGHT * self.zoom),
            );
            painter.rect_filled(
                header,
                CornerRadius { nw: 6, ne: 6, sw: 0, se: 0 },
                language_color(card.language).gamma_multiply(0.85),
            );

            if self.zoom < 0.25 {
                continue;
            }

            painter.text(
                header.min + Vec2::new(10.0, header.height() / 2.0),
                Align2::LEFT_CENTER,
                &card.label,
                FontId::proportional((13.0 * self.zoom).clamp(7.0, 15.0)),
                Color32::from_rgb(245, 247, 250),
            );

            if card.entry {
                painter.circle_filled(
                    Pos2::new(header.right() - 12.0, header.center().y),
                    (4.0 * self.zoom).clamp(2.5, 4.5),
                    ENTRY,
                );
            }

            painter.text(
                header.min + Vec2::new(10.0, header.height() - 6.0),
                Align2::LEFT_BOTTOM,
                &card.directory,
                FontId::proportional((9.5 * self.zoom).clamp(6.0, 10.5)),
                Color32::from_rgb(230, 236, 245).gamma_multiply(0.75),
            );

            if self.zoom < 0.4 {
                continue;
            }

            let mut y = rect.min.y + header.height() + PADDING * self.zoom;
            for row in &card.rows {
                let indent = 10.0 + row.depth as f32 * 14.0;
                let is_type = matches!(
                    row.kind,
                    SymbolKind::Class
                        | SymbolKind::Struct
                        | SymbolKind::Interface
                        | SymbolKind::Trait
                        | SymbolKind::Enum
                );

                painter.text(
                    Pos2::new(rect.left() + indent * self.zoom, y + ROW_HEIGHT * self.zoom / 2.0),
                    Align2::LEFT_CENTER,
                    kind_tag(row.kind),
                    FontId::monospace((9.5 * self.zoom).clamp(6.0, 11.0)),
                    TEXT_MUTED,
                );
                painter.text(
                    Pos2::new(rect.left() + (indent + 22.0) * self.zoom, y + ROW_HEIGHT * self.zoom / 2.0),
                    Align2::LEFT_CENTER,
                    &row.label,
                    FontId::proportional((12.0 * self.zoom).clamp(6.5, 13.5)),
                    if is_type { TEXT } else { TEXT.gamma_multiply(0.92) },
                );
                painter.text(
                    Pos2::new(rect.right() - 10.0, y + ROW_HEIGHT * self.zoom / 2.0),
                    Align2::RIGHT_CENTER,
                    format!("{}", row.line),
                    FontId::proportional((9.5 * self.zoom).clamp(6.0, 10.5)),
                    TEXT_MUTED,
                );
                y += ROW_HEIGHT * self.zoom;
            }

            if card.total_symbols > card.rows.len() {
                painter.text(
                    Pos2::new(rect.left() + 32.0, y + ROW_HEIGHT * self.zoom / 2.0),
                    Align2::LEFT_CENTER,
                    format!("+{} more", card.total_symbols - card.rows.len()),
                    FontId::proportional((10.0 * self.zoom).clamp(6.5, 12.0)),
                    TEXT_MUTED,
                );
            }
        }
    }
}

fn paint_arrowhead(
    painter: &egui::Painter,
    tip: Pos2,
    pointing_right: bool,
    color: Color32,
    width: f32,
) {
    let size = 5.0 + width;
    let direction = if pointing_right { 1.0 } else { -1.0 };
    let base = Pos2::new(tip.x - direction * size, tip.y);
    painter.add(Shape::convex_polygon(
        vec![
            tip,
            Pos2::new(base.x, base.y - size * 0.55),
            Pos2::new(base.x, base.y + size * 0.55),
        ],
        color,
        Stroke::NONE,
    ));
}

#[cfg(test)]
mod tests {
    use super::*;
    use codebase_intel_core::{
        CallGraph, FileDependencies, FileEdge, FileId, FileNode, FileDepKind, Position, Span,
        SymbolId, Visibility,
    };

    fn symbol(file: &str, name: &str, line: usize, kind: SymbolKind, parent: Option<&str>, children: Vec<&str>) -> Symbol {
        Symbol {
            id: SymbolId(format!("{file}:{name}")),
            name: name.to_string(),
            kind,
            language: Language::TypeScript,
            file: PathBuf::from(file),
            span: Span {
                start: Position { line, column: 1, offset: 0 },
                end: Position { line, column: 1, offset: 0 },
            },
            signature: None,
            visibility: Visibility::Public,
            documentation: None,
            attributes: Vec::new(),
            parent: parent.map(|id| SymbolId(format!("{file}:{id}"))),
            children: children.into_iter().map(|id| SymbolId(format!("{file}:{id}"))).collect(),
        }
    }

    fn result(files: &[&str], symbols: Vec<Symbol>, edges: Vec<(usize, usize)>) -> AnalysisResult {
        AnalysisResult {
            symbols,
            references: Vec::new(),
            call_graph: CallGraph::new(),
            file_dependencies: FileDependencies {
                nodes: files
                    .iter()
                    .enumerate()
                    .map(|(position, path)| FileNode {
                        id: FileId(position),
                        path: PathBuf::from(path),
                        language: Language::TypeScript,
                        symbol_count: 0,
                    })
                    .collect(),
                edges: edges
                    .into_iter()
                    .map(|(from, to)| FileEdge {
                        from: FileId(from),
                        to: FileId(to),
                        kind: FileDepKind::Import,
                        imports: vec!["mod".into()],
                    })
                    .collect(),
            },
            api_endpoints: Vec::new(),
            db_schema: None,
            metrics: Default::default(),
            discovery: Default::default(),
            errors: Vec::new(),
        }
    }

    #[test]
    fn nests_methods_under_their_class() {
        let symbols = vec![
            symbol("C:/r/a.ts", "Service", 1, SymbolKind::Class, None, vec!["run"]),
            symbol("C:/r/a.ts", "run", 5, SymbolKind::Method, Some("Service"), vec![]),
        ];
        let diagram = BlockDiagram::build(&result(&["C:/r/a.ts"], symbols, vec![]), None, &[]);

        let card = &diagram.cards[0];
        assert_eq!(card.rows[0].label, "Service");
        assert_eq!(card.rows[0].depth, 0);
        assert_eq!(card.rows[1].label, "run");
        assert_eq!(card.rows[1].depth, 1);
    }

    #[test]
    fn respects_scope() {
        let result = result(&["C:/r/a.ts", "C:/r/b.ts"], vec![], vec![]);
        let mut visible = HashSet::new();
        visible.insert(1);

        let diagram = BlockDiagram::build(&result, Some(&visible), &[]);
        assert_eq!(diagram.cards.len(), 1);
        assert_eq!(diagram.cards[0].id, 1);
    }

    #[test]
    fn keeps_only_edges_between_visible_cards() {
        let result = result(&["C:/r/a.ts", "C:/r/b.ts", "C:/r/c.ts"], vec![], vec![(0, 1), (1, 2)]);
        let mut visible = HashSet::new();
        visible.insert(0);
        visible.insert(1);

        let diagram = BlockDiagram::build(&result, Some(&visible), &[]);
        assert_eq!(diagram.edges.len(), 1, "the edge to the hidden file is dropped");
    }

    #[test]
    fn marks_entry_points() {
        let result = result(&["C:/r/main.ts", "C:/r/util.ts"], vec![], vec![(0, 1)]);
        let diagram = BlockDiagram::build(&result, None, &[0]);
        assert!(diagram.card(0).unwrap().entry);
        assert!(!diagram.card(1).unwrap().entry);
    }

    #[test]
    fn cards_do_not_overlap() {
        let files: Vec<String> = (0..9).map(|index| format!("C:/r/f{index}.ts")).collect();
        let refs: Vec<&str> = files.iter().map(String::as_str).collect();
        let diagram = BlockDiagram::build(&result(&refs, vec![], vec![]), None, &[]);

        for (index, first) in diagram.cards.iter().enumerate() {
            for second in diagram.cards.iter().skip(index + 1) {
                assert!(!first.rect().intersects(second.rect()));
            }
        }
    }
}


