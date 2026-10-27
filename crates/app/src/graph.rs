//! The repository graph: model, layout, and canvas rendering.
//!
//! Layout is deterministic rather than force-directed: files are grouped by
//! directory, each directory becomes a cluster, and clusters are packed into a
//! grid. That makes the map stable between scans, which matters when you are
//! using it to navigate rather than to admire it.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use codebase_intel_core::resolve::normalize;
use codebase_intel_core::{AnalysisResult, FileDepKind, Language, SymbolKind};
use egui::{
    Align2, Color32, CornerRadius, FontId, Pos2, Rect, Response, Sense, Shape, Stroke, Ui, Vec2,
};

pub const NODE_WIDTH: f32 = 264.0;
pub const HEADER_HEIGHT: f32 = 24.0;
pub const ROW_HEIGHT: f32 = 17.0;
pub const NODE_PADDING: f32 = 6.0;
pub const MAX_VISIBLE_ROWS: usize = 12;
const NODE_GAP: f32 = 26.0;
const CLUSTER_GAP: f32 = 70.0;
const CLUSTER_TITLE_HEIGHT: f32 = 26.0;

const BACKGROUND: Color32 = Color32::from_rgb(18, 20, 24);
const CLUSTER_FILL: Color32 = Color32::from_rgb(24, 27, 33);
const CLUSTER_STROKE: Color32 = Color32::from_rgb(38, 43, 52);
const NODE_BODY: Color32 = Color32::from_rgb(32, 36, 44);
const NODE_BORDER: Color32 = Color32::from_rgb(52, 58, 70);
const TEXT: Color32 = Color32::from_rgb(214, 219, 228);
const TEXT_MUTED: Color32 = Color32::from_rgb(132, 141, 156);
const EDGE: Color32 = Color32::from_rgb(70, 78, 92);
const EDGE_HIGHLIGHT: Color32 = Color32::from_rgb(94, 168, 255);
const ACCENT: Color32 = Color32::from_rgb(94, 168, 255);
const ENTRY: Color32 = Color32::from_rgb(110, 200, 140);

#[derive(Debug, Clone)]
pub struct SymbolRow {
    pub name: String,
    pub kind: SymbolKind,
    pub line: usize,
}

#[derive(Debug, Clone)]
pub struct Node {
    pub id: usize,
    pub path: PathBuf,
    pub label: String,
    pub directory: String,
    pub language: Language,
    pub rows: Vec<SymbolRow>,
    pub total_symbols: usize,
    pub pos: Pos2,
    pub size: Vec2,
    pub inbound: usize,
    pub outbound: usize,
    pub cluster: usize,
}

impl Node {
    pub fn rect(&self) -> Rect {
        Rect::from_min_size(self.pos, self.size)
    }
}

#[derive(Debug, Clone)]
pub struct Edge {
    pub from: usize,
    pub to: usize,
    pub kind: FileDepKind,
    pub label: String,
}

impl Edge {
    pub fn kind_label(&self) -> &'static str {
        match self.kind {
            FileDepKind::Import => "import",
            FileDepKind::ReExport => "re-export",
            FileDepKind::DynamicImport => "dynamic import",
            FileDepKind::TypeOnly => "type-only",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Cluster {
    pub directory: String,
    pub rect: Rect,
}

#[derive(Debug, Clone)]
pub struct Graph {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub clusters: Vec<Cluster>,
    pub bounds: Rect,
    /// Files that look like entry points: nothing imports them, but they import
    /// something. Conventional names (`main`, `index`, `app`, …) are preferred
    /// when there are too many candidates to be useful.
    pub entry_points: Vec<usize>,
}

impl Graph {
    /// Build the graph model from an analysis result.
    pub fn build(result: &AnalysisResult) -> Self {
        let mut nodes: Vec<Node> = result
            .file_dependencies
            .nodes
            .iter()
            .map(|node| Node {
                id: node.id.0,
                path: node.path.clone(),
                label: node
                    .path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| node.path.to_string_lossy().into_owned()),
                directory: describe_directory(&node.path),
                language: node.language,
                rows: Vec::new(),
                total_symbols: 0,
                pos: Pos2::ZERO,
                size: Vec2::new(NODE_WIDTH, HEADER_HEIGHT + NODE_PADDING),
                inbound: 0,
                outbound: 0,
                cluster: 0,
            })
            .collect();

        // Symbols arrive as a flat list; bucket them by file.
        let index_by_path: BTreeMap<PathBuf, usize> = nodes
            .iter()
            .enumerate()
            .map(|(position, node)| (normalize(&node.path), position))
            .collect();

        let mut all_rows: Vec<Vec<SymbolRow>> = vec![Vec::new(); nodes.len()];
        for symbol in &result.symbols {
            let Some(position) = index_by_path.get(&normalize(&symbol.file)) else {
                continue;
            };
            all_rows[*position].push(SymbolRow {
                name: symbol.name.clone(),
                kind: symbol.kind,
                line: symbol.span.start.line,
            });
        }

        for (position, mut rows) in all_rows.into_iter().enumerate() {
            rows.sort_by(|a, b| a.line.cmp(&b.line).then_with(|| a.name.cmp(&b.name)));
            nodes[position].total_symbols = rows.len();
            rows.truncate(MAX_VISIBLE_ROWS);
            nodes[position].rows = rows;
            nodes[position].size = Vec2::new(
                NODE_WIDTH,
                HEADER_HEIGHT + NODE_PADDING + node_body_rows(&nodes[position]) as f32 * ROW_HEIGHT,
            );
        }

        let mut edges = Vec::with_capacity(result.file_dependencies.edges.len());
        for edge in &result.file_dependencies.edges {
            let (from, to) = (edge.from.0, edge.to.0);
            if from >= nodes.len() || to >= nodes.len() {
                continue;
            }
            nodes[from].outbound += 1;
            nodes[to].inbound += 1;

            let label = if edge.imports.is_empty() {
                edge.kind_label().to_string()
            } else {
                let first = edge.imports.first().cloned().unwrap_or_default();
                if edge.imports.len() > 1 {
                    format!("{first} +{}", edge.imports.len() - 1)
                } else {
                    first
                }
            };

            edges.push(Edge {
                from,
                to,
                kind: edge.kind,
                label,
            });
        }

        let mut entry_points: Vec<usize> = nodes
            .iter()
            .filter(|node| node.inbound == 0 && node.outbound > 0)
            .map(|node| node.id)
            .collect();

        // A large repository can have hundreds of roots; fall back to
        // conventional entry-point names so the list stays useful.
        if entry_points.len() > 30 {
            entry_points.retain(|id| {
                nodes
                    .get(*id)
                    .map(|node| is_conventional_entry(&node.label))
                    .unwrap_or(false)
            });
        }
        // Most-connected roots first: they are the best "start here" guesses.
        entry_points.sort_by(|a, b| {
            nodes[*b]
                .outbound
                .cmp(&nodes[*a].outbound)
                .then_with(|| a.cmp(b))
        });
        entry_points.truncate(40);

        let mut graph = Self {
            nodes,
            edges,
            clusters: Vec::new(),
            bounds: Rect::from_min_size(Pos2::ZERO, Vec2::splat(1.0)),
            entry_points,
        };
        graph.layout();
        graph
    }

    /// Arrange nodes into directory clusters and pack the clusters.
    pub fn layout(&mut self) {
        let mut by_directory: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        for (position, node) in self.nodes.iter().enumerate() {
            by_directory
                .entry(node.directory.clone())
                .or_default()
                .push(position);
        }

        let mut cluster_sizes: Vec<(String, Vec<usize>, Vec2)> = Vec::new();

        for (directory, members) in by_directory {
            let columns = (members.len() as f32).sqrt().ceil() as usize;
            let columns = columns.clamp(1, 5);

            // Row heights follow the tallest node in each row.
            let mut heights: Vec<f32> = Vec::new();
            for (offset, position) in members.iter().enumerate() {
                let row = offset / columns;
                let height = self.nodes[*position].size.y;
                if heights.len() <= row {
                    heights.push(height);
                } else {
                    heights[row] = heights[row].max(height);
                }
            }

            let mut cursor = Pos2::new(CLUSTER_GAP / 2.0, CLUSTER_TITLE_HEIGHT);
            for (offset, position) in members.iter().enumerate() {
                let column = offset % columns;
                let row = offset / columns;
                let x = CLUSTER_GAP / 2.0 + column as f32 * (NODE_WIDTH + NODE_GAP);
                let y = CLUSTER_TITLE_HEIGHT
                    + heights.iter().take(row).sum::<f32>()
                    + row as f32 * NODE_GAP;
                self.nodes[*position].pos = Pos2::new(x, y);
                self.nodes[*position].cluster = cluster_sizes.len();
                cursor.x = cursor.x.max(x + NODE_WIDTH);
                cursor.y = cursor.y.max(y + self.nodes[*position].size.y);
            }

            let size = Vec2::new(
                cursor.x + CLUSTER_GAP / 2.0,
                cursor.y + CLUSTER_GAP / 2.0,
            );
            cluster_sizes.push((directory, members, size));
        }

        // Pack clusters into a grid.
        let cluster_count = cluster_sizes.len().max(1);
        let cluster_columns = (cluster_count as f32).sqrt().ceil() as usize;
        let cell = Vec2::new(
            cluster_sizes.iter().map(|(_, _, size)| size.x).fold(0.0, f32::max),
            cluster_sizes.iter().map(|(_, _, size)| size.y).fold(0.0, f32::max),
        );
        let cell = Vec2::new(cell.x + CLUSTER_GAP, cell.y + CLUSTER_GAP);

        let mut clusters = Vec::with_capacity(cluster_sizes.len());
        for (index, (directory, _members, size)) in cluster_sizes.into_iter().enumerate() {
            let column = index % cluster_columns;
            let row = index / cluster_columns;
            let origin = Pos2::new(column as f32 * cell.x, row as f32 * cell.y);

            for position in 0..self.nodes.len() {
                if self.nodes[position].cluster == index {
                    self.nodes[position].pos += origin.to_vec2();
                }
            }

            clusters.push(Cluster {
                directory,
                rect: Rect::from_min_size(origin, size),
            });
        }

        self.clusters = clusters;

        let mut bounds = Rect::NOTHING;
        for node in &self.nodes {
            bounds = bounds.union(node.rect());
        }
        for cluster in &self.clusters {
            bounds = bounds.union(cluster.rect);
        }
        if !bounds.is_finite() || bounds.width() <= 1.0 {
            bounds = Rect::from_min_size(Pos2::ZERO, Vec2::splat(600.0));
        }
        self.bounds = bounds;
    }

    pub fn node(&self, id: usize) -> Option<&Node> {
        self.nodes.iter().find(|node| node.id == id)
    }

    /// Edges touching a node.
    pub fn relations(&self, id: usize) -> (Vec<&Edge>, Vec<&Edge>) {
        let outgoing = self.edges.iter().filter(|edge| edge.from == id).collect();
        let incoming = self.edges.iter().filter(|edge| edge.to == id).collect();
        (incoming, outgoing)
    }
}

/// Number of body rows including the "+N more" indicator.
fn node_body_rows(node: &Node) -> usize {
    let overflow = usize::from(node.total_symbols > node.rows.len());
    (node.rows.len() + overflow).max(1)
}

/// Short directory label: the last two path components, which is enough to
/// disambiguate while staying readable on a canvas.
fn describe_directory(path: &Path) -> String {
    let parent = path.parent().unwrap_or_else(|| Path::new(""));
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

fn is_conventional_entry(file_name: &str) -> bool {
    const STEMS: &[&str] = &[
        "main", "index", "app", "server", "cli", "run", "entry", "bootstrap",
    ];
    let stem = file_name.split('.').next().unwrap_or(file_name).to_ascii_lowercase();
    STEMS.contains(&stem.as_str())
}

pub fn language_color(language: Language) -> Color32 {
    match language {
        Language::TypeScript => Color32::from_rgb(49, 120, 198),
        Language::JavaScript => Color32::from_rgb(203, 168, 48),
        Language::Python => Color32::from_rgb(55, 118, 171),
        Language::Go => Color32::from_rgb(0, 173, 216),
        Language::Rust => Color32::from_rgb(198, 120, 72),
    }
}

pub fn kind_tag(kind: SymbolKind) -> &'static str {
    match kind {
        SymbolKind::Function => "fn",
        SymbolKind::Method => "me",
        SymbolKind::Class => "cl",
        SymbolKind::Interface => "if",
        SymbolKind::TypeAlias => "ty",
        SymbolKind::Enum => "en",
        SymbolKind::Struct => "st",
        SymbolKind::Variable => "va",
        SymbolKind::Constant => "co",
        SymbolKind::Parameter => "pa",
        SymbolKind::Module => "mo",
        SymbolKind::Namespace => "ns",
        SymbolKind::Trait => "tr",
        SymbolKind::Impl => "im",
        SymbolKind::Macro => "ma",
    }
}

trait DepKindLabel {
    fn kind_label(&self) -> &'static str;
}

impl DepKindLabel for codebase_intel_core::FileEdge {
    fn kind_label(&self) -> &'static str {
        match self.kind {
            FileDepKind::Import => "import",
            FileDepKind::ReExport => "re-export",
            FileDepKind::DynamicImport => "dynamic",
            FileDepKind::TypeOnly => "type",
        }
    }
}

/// Which files the canvas draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScopeMode {
    /// Every analyzed file.
    #[default]
    All,
    /// Only the files the user ticked.
    SelectionOnly,
    /// Ticked files plus everything within `hops` relations of them.
    Neighborhood,
}

/// The working set of files.
#[derive(Debug, Clone, Default)]
pub struct Scope {
    pub mode: ScopeMode,
    pub selected: std::collections::BTreeSet<usize>,
    pub hops: usize,
}

/// Interactive canvas state.
pub struct GraphView {
    pub graph: Graph,
    pub zoom: f32,
    pub pan: Vec2,
    pub selected: Option<usize>,
    pub filter: String,
    pub scope: Scope,
    pub fit_requested: bool,
    /// Canvas rect from the last frame, used to centre on a node.
    pub last_canvas: Rect,
    /// Node to open on for large repositories, consumed on the first frame.
    initial_focus: Option<usize>,
    dragging: Option<usize>,
}

impl GraphView {
    pub fn new(graph: Graph) -> Self {
        // Fitting a few thousand files means a 0.04 zoom where nothing is
        // readable. Open on the best entry point instead and let the user zoom
        // out deliberately.
        let initial_focus = if graph.nodes.len() > 300 {
            graph.entry_points.first().copied()
        } else {
            None
        };

        Self {
            graph,
            zoom: 1.0,
            pan: Vec2::ZERO,
            selected: None,
            filter: String::new(),
            scope: Scope::default(),
            fit_requested: true,
            last_canvas: Rect::ZERO,
            initial_focus,
            dragging: None,
        }
    }

    /// Files to draw, or `None` for "everything".
    ///
    /// Scope is resolved every frame. With a few thousand files and a handful
    /// of edges this is cheaper than keeping a cache valid through drags.
    pub fn visible_ids(&self) -> Option<HashSet<usize>> {
        match self.scope.mode {
            ScopeMode::All => None,
            ScopeMode::SelectionOnly => Some(self.scope.selected.iter().copied().collect()),
            ScopeMode::Neighborhood => {
                let mut adjacency: HashMap<usize, Vec<usize>> = HashMap::new();
                for edge in &self.graph.edges {
                    adjacency.entry(edge.from).or_default().push(edge.to);
                    adjacency.entry(edge.to).or_default().push(edge.from);
                }

                let mut visible: HashSet<usize> = self.scope.selected.iter().copied().collect();
                let mut frontier: Vec<usize> = visible.iter().copied().collect();

                for _ in 0..self.scope.hops.max(1) {
                    let mut next = Vec::new();
                    for id in frontier.drain(..) {
                        for neighbour in adjacency.get(&id).into_iter().flatten() {
                            if visible.insert(*neighbour) {
                                next.push(*neighbour);
                            }
                        }
                    }
                    if next.is_empty() {
                        break;
                    }
                    frontier = next;
                }

                Some(visible)
            }
        }
    }

    /// Fit the canvas to just the files in scope.
    pub fn fit_to_scope(&mut self, canvas: Rect) {
        let Some(visible) = self.visible_ids() else {
            self.fit(canvas);
            return;
        };

        let mut bounds = Rect::NOTHING;
        for node in &self.graph.nodes {
            if visible.contains(&node.id) {
                bounds = bounds.union(node.rect());
            }
        }
        if !bounds.is_finite() {
            return;
        }

        let margin = 50.0;
        let scale_x = (canvas.width() - margin * 2.0) / bounds.width().max(1.0);
        let scale_y = (canvas.height() - margin * 2.0) / bounds.height().max(1.0);
        self.zoom = scale_x.min(scale_y).clamp(0.12, 1.6);
        self.pan = canvas.size() / 2.0 - (bounds.center().to_vec2() * self.zoom);
    }

    fn screen_to_world(&self, canvas: Rect, screen: Pos2) -> Pos2 {
        ((screen - canvas.min - self.pan) / self.zoom).to_pos2()
    }

    fn world_to_screen(&self, canvas: Rect, world: Pos2) -> Pos2 {
        canvas.min + self.pan + world.to_vec2() * self.zoom
    }

    /// Zoom and pan so the whole graph fits the canvas.
    pub fn fit(&mut self, canvas: Rect) {
        let bounds = self.graph.bounds;
        if bounds.width() <= 0.0 || bounds.height() <= 0.0 {
            return;
        }

        let margin = 40.0;
        let scale_x = (canvas.width() - margin * 2.0) / bounds.width();
        let scale_y = (canvas.height() - margin * 2.0) / bounds.height();
        self.zoom = scale_x.min(scale_y).clamp(0.04, 1.6);

        self.pan = canvas.size() / 2.0 - (bounds.center().to_vec2() * self.zoom);
    }

    pub fn focus(&mut self, canvas: Rect, id: usize) {
        self.focus_at(canvas, id, self.zoom.max(0.8));
    }

    fn focus_at(&mut self, canvas: Rect, id: usize, zoom: f32) {
        if let Some(node) = self.graph.node(id) {
            self.zoom = zoom.clamp(0.04, 3.0);
            let center = node.rect().center().to_vec2() * self.zoom;
            self.pan = canvas.size() / 2.0 - center;
            self.selected = Some(id);
        }
    }

    fn hit_test(&self, world: Pos2) -> Option<usize> {
        // Later nodes are drawn on top, so search from the end.
        self.graph
            .nodes
            .iter()
            .rev()
            .find(|node| node.rect().contains(world))
            .map(|node| node.id)
    }

    fn matches_filter(&self, node: &Node) -> bool {
        if self.filter.trim().is_empty() {
            return true;
        }
        let needle = self.filter.trim().to_ascii_lowercase();
        node.label.to_ascii_lowercase().contains(&needle)
            || node.directory.to_ascii_lowercase().contains(&needle)
    }

    /// Draw the canvas and handle interaction. Returns the response for the
    /// caller to inspect (e.g. for keyboard focus).
    pub fn ui(&mut self, ui: &mut Ui) -> Response {
        let (response, painter) = ui.allocate_painter(ui.available_size(), Sense::click_and_drag());
        let canvas = response.rect;
        self.last_canvas = canvas;
        painter.rect_filled(canvas, 0.0, BACKGROUND);

        if self.fit_requested {
            if let Some(id) = self.initial_focus.take() {
                self.focus_at(canvas, id, 0.55);
            } else if self.scope.mode == ScopeMode::All {
                self.fit(canvas);
            } else {
                self.fit_to_scope(canvas);
            }
            self.fit_requested = false;
        }

        // Zoom around the pointer so the point under the cursor stays put.
        let scroll = ui.input(|input| input.raw_scroll_delta.y);
        if response.hovered() && scroll.abs() > 0.0 {
            if let Some(pointer) = response.hover_pos() {
                let before = self.screen_to_world(canvas, pointer);
                self.zoom = (self.zoom * (1.0 + scroll * 0.0016)).clamp(0.04, 3.0);
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
                    if let Some(node) = self.graph.nodes.iter_mut().find(|node| node.id == id) {
                        node.pos += delta / self.zoom;
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

        let visible = self.visible_ids();
        let visible = visible.as_ref();

        self.paint_clusters(&painter, canvas, viewport, visible);
        self.paint_edges(&painter, canvas, viewport, visible);
        self.paint_nodes(&painter, canvas, viewport, visible);

        // Legend, bottom-left.
        let legend = [
            "scroll: zoom",
            "drag: pan",
            "drag a box: move it",
            "click: inspect",
        ];
        let mut y = canvas.bottom() - 8.0;
        for line in legend.iter().rev() {
            painter.text(
                Pos2::new(canvas.left() + 12.0, y),
                Align2::LEFT_BOTTOM,
                *line,
                FontId::proportional(11.0),
                TEXT_MUTED,
            );
            y -= 14.0;
        }

        response
    }

    fn paint_clusters(
        &self,
        painter: &egui::Painter,
        canvas: Rect,
        viewport: Rect,
        visible: Option<&HashSet<usize>>,
    ) {
        // When a scope is active the clusters hide the boxes rather than help,
        // because only a handful of files are on screen.
        if visible.is_some() || self.zoom < 0.06 {
            return;
        }

        for cluster in &self.graph.clusters {
            if !cluster.rect.intersects(viewport) {
                continue;
            }

            let screen = Rect::from_min_max(
                self.world_to_screen(canvas, cluster.rect.min),
                self.world_to_screen(canvas, cluster.rect.max),
            );
            painter.rect_filled(screen, CornerRadius::same(10), CLUSTER_FILL);
            painter.rect_filled(
                Rect::from_min_size(screen.min, Vec2::new(screen.width(), 1.0)),
                CornerRadius::ZERO,
                CLUSTER_STROKE,
            );

            if self.zoom > 0.25 {
                painter.text(
                    screen.min + Vec2::new(12.0, 6.0),
                    Align2::LEFT_TOP,
                    &cluster.directory,
                    FontId::proportional(11.5),
                    TEXT_MUTED,
                );
            }
        }
    }

    fn paint_edges(
        &self,
        painter: &egui::Painter,
        canvas: Rect,
        viewport: Rect,
        visible: Option<&HashSet<usize>>,
    ) {
        // Relations are the whole point of this view, so they are drawn at
        // every zoom level: simplified to straight lines when zoomed out.
        let detailed = self.zoom >= 0.32;

        for edge in &self.graph.edges {
            let Some(source) = self.graph.nodes.get(edge.from) else {
                continue;
            };
            let Some(target) = self.graph.nodes.get(edge.to) else {
                continue;
            };

            if let Some(visible) = visible {
                if !visible.contains(&edge.from) && !visible.contains(&edge.to) {
                    continue;
                }
            }

            let highlighted = self.selected == Some(edge.from) || self.selected == Some(edge.to);
            let filtered = self.selected.is_none()
                && !self.filter.trim().is_empty()
                && !(self.matches_filter(source) || self.matches_filter(target));

            if filtered {
                continue;
            }

            let color = if highlighted {
                EDGE_HIGHLIGHT
            } else if self.selected.is_some() {
                EDGE.gamma_multiply(0.35)
            } else {
                EDGE.gamma_multiply(0.75)
            };

            let width = if highlighted { 2.0 } else { 1.0 };

            let source_rect = source.rect();
            let target_rect = target.rect();
            if !source_rect.intersects(viewport) && !target_rect.intersects(viewport) {
                // Still draw when the segment crosses the viewport; cheap check
                // by bounding box of the two endpoints.
                let span = Rect::from_two_pos(source_rect.center(), target_rect.center());
                if !span.intersects(viewport) {
                    continue;
                }
            }

            let (start, end) = if target_rect.center().x >= source_rect.center().x {
                (
                    source_rect.right_center(),
                    target_rect.left_center(),
                )
            } else {
                (
                    source_rect.left_center(),
                    target_rect.right_center(),
                )
            };

            let start = self.world_to_screen(canvas, start);
            let end = self.world_to_screen(canvas, end);

            let points = if detailed {
                let mid_x = (start.x + end.x) / 2.0;
                vec![
                    start,
                    Pos2::new(mid_x, start.y),
                    Pos2::new(mid_x, end.y),
                    end,
                ]
            } else {
                vec![start, end]
            };

            painter.add(Shape::line(points, Stroke::new(width, color)));
            if detailed || highlighted {
                paint_arrowhead(painter, end, end.x >= start.x, color, width);
            }

            if highlighted && self.zoom > 0.55 && !edge.label.is_empty() {
                let mid_x = (start.x + end.x) / 2.0;
                let label_pos = Pos2::new(mid_x + 4.0, (start.y + end.y) / 2.0);
                painter.text(
                    label_pos,
                    Align2::LEFT_CENTER,
                    &edge.label,
                    FontId::proportional(10.0),
                    TEXT_MUTED,
                );
            }
        }
    }

    fn paint_nodes(
        &self,
        painter: &egui::Painter,
        canvas: Rect,
        viewport: Rect,
        visible: Option<&HashSet<usize>>,
    ) {
        for node in &self.graph.nodes {
            if let Some(visible) = visible {
                if !visible.contains(&node.id) {
                    continue;
                }
            }
            if !node.rect().intersects(viewport) {
                continue;
            }

            let rect = Rect::from_min_max(
                self.world_to_screen(canvas, node.pos),
                self.world_to_screen(canvas, node.pos + node.size),
            );
            if !rect.intersects(canvas) {
                continue;
            }

            let dimmed = !self.filter.trim().is_empty() && !self.matches_filter(node);
            let alpha = if dimmed { 0.28 } else { 1.0 };
            let selected = self.selected == Some(node.id);

            let accent = language_color(node.language).gamma_multiply(alpha);
            let body = NODE_BODY.gamma_multiply(alpha);
            let border = if selected { ACCENT } else { NODE_BORDER.gamma_multiply(alpha) };

            // Border by drawing slightly larger rects underneath.
            let border_width = if selected { 2.0 } else { 1.0 };
            painter.rect_filled(
                rect.expand(border_width),
                CornerRadius::same(6),
                border,
            );
            painter.rect_filled(rect, CornerRadius::same(5), body);

            let header_height = HEADER_HEIGHT * self.zoom;
            let header = Rect::from_min_size(rect.min, Vec2::new(rect.width(), header_height));
            painter.rect_filled(header, CornerRadius { nw: 5, ne: 5, sw: 0, se: 0 }, accent.gamma_multiply(0.85));

            // Entry points get a marker so "where does this start?" is visible
            // even when zoomed out.
            if self.graph.entry_points.contains(&node.id) {
                let marker = Pos2::new(header.right() - 9.0, header.center().y);
                painter.circle_filled(marker, (4.0 * self.zoom).clamp(2.0, 4.0), ENTRY);
            }

            if self.zoom < 0.14 {
                continue;
            }

            let title = truncate(&node.label, title_capacity(rect.width()));
            painter.text(
                header.min + Vec2::new(8.0, header_height / 2.0),
                Align2::LEFT_CENTER,
                title,
                FontId::proportional((12.0 * self.zoom).clamp(7.0, 15.0)),
                Color32::from_rgb(245, 247, 250).gamma_multiply(alpha),
            );

            if self.zoom < 0.55 {
                continue;
            }

            let mut y = rect.min.y + header_height + NODE_PADDING * self.zoom;
            let font = FontId::proportional((11.5 * self.zoom).clamp(7.0, 13.0));

            for row in &node.rows {
                let tag = kind_tag(row.kind);
                painter.text(
                    Pos2::new(rect.left() + 8.0, y + ROW_HEIGHT * self.zoom / 2.0),
                    Align2::LEFT_CENTER,
                    tag,
                    FontId::monospace((9.5 * self.zoom).clamp(6.0, 11.0)),
                    TEXT_MUTED.gamma_multiply(alpha),
                );
                painter.text(
                    Pos2::new(rect.left() + 30.0, y + ROW_HEIGHT * self.zoom / 2.0),
                    Align2::LEFT_CENTER,
                    truncate(&row.name, row_capacity(rect.width())),
                    font.clone(),
                    TEXT.gamma_multiply(alpha),
                );
                y += ROW_HEIGHT * self.zoom;
            }

            if node.total_symbols > node.rows.len() {
                painter.text(
                    Pos2::new(rect.left() + 30.0, y + ROW_HEIGHT * self.zoom / 2.0),
                    Align2::LEFT_CENTER,
                    format!("+{} more", node.total_symbols - node.rows.len()),
                    FontId::proportional((10.0 * self.zoom).clamp(6.5, 12.0)),
                    TEXT_MUTED.gamma_multiply(alpha),
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

fn title_capacity(pixel_width: f32) -> usize {
    ((pixel_width - 16.0) / 6.6).max(4.0) as usize
}

fn row_capacity(pixel_width: f32) -> usize {
    ((pixel_width - 38.0) / 5.8).max(4.0) as usize
}

fn truncate(text: &str, capacity: usize) -> String {
    if text.chars().count() <= capacity {
        return text.to_string();
    }
    let keep = capacity.saturating_sub(1);
    let mut out: String = text.chars().take(keep).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use codebase_intel_core::Position;
    use codebase_intel_core::{FileDependencies, FileEdge, FileId, FileNode, Span, Symbol, SymbolId};
    use codebase_intel_core::{
        AnalysisResult, CallGraph, ProjectMetrics, Reference, Visibility,
    };

    fn symbol(path: &str, name: &str, line: usize, kind: SymbolKind) -> Symbol {
        Symbol {
            id: SymbolId(format!("{path}:{name}:{line}")),
            name: name.to_string(),
            kind,
            language: Language::TypeScript,
            file: PathBuf::from(path),
            span: Span {
                start: Position {
                    line,
                    column: 1,
                    offset: 0,
                },
                end: Position {
                    line,
                    column: 1,
                    offset: 0,
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

    fn result_with(files: &[&str], symbols: Vec<Symbol>, edges: Vec<(usize, usize)>) -> AnalysisResult {
        let nodes = files
            .iter()
            .enumerate()
            .map(|(position, path)| FileNode {
                id: FileId(position),
                path: PathBuf::from(path),
                language: Language::TypeScript,
                symbol_count: 0,
            })
            .collect();

        let edges = edges
            .into_iter()
            .map(|(from, to)| FileEdge {
                from: FileId(from),
                to: FileId(to),
                kind: FileDepKind::Import,
                imports: vec!["thing".into()],
            })
            .collect();

        AnalysisResult {
            symbols,
            references: Vec::<Reference>::new(),
            call_graph: CallGraph::new(),
            file_dependencies: FileDependencies { nodes, edges },
            api_endpoints: Vec::new(),
            db_schema: None,
            metrics: ProjectMetrics::default(),
            discovery: Default::default(),
            errors: Vec::<codebase_intel_core::AnalysisError>::new(),
        }
    }

    #[test]
    fn groups_files_into_directory_clusters() {
        let result = result_with(
            &["C:/repo/src/a.ts", "C:/repo/src/b.ts", "C:/repo/lib/c.ts"],
            vec![],
            vec![],
        );
        let graph = Graph::build(&result);

        assert_eq!(graph.nodes.len(), 3);
        assert_eq!(graph.clusters.len(), 2, "src and lib are separate clusters");
        assert!(graph.bounds.width() > 0.0 && graph.bounds.height() > 0.0);
    }

    #[test]
    fn nodes_do_not_overlap() {
        let result = result_with(
            &["C:/repo/src/a.ts", "C:/repo/src/b.ts", "C:/repo/src/c.ts"],
            vec![],
            vec![],
        );
        let graph = Graph::build(&result);

        for (i, first) in graph.nodes.iter().enumerate() {
            for second in graph.nodes.iter().skip(i + 1) {
                assert!(
                    !first.rect().intersects(second.rect()),
                    "{} overlaps {}",
                    first.label,
                    second.label
                );
            }
        }
    }

    #[test]
    fn counts_relations() {
        let result = result_with(
            &["C:/repo/a.ts", "C:/repo/b.ts"],
            vec![],
            vec![(0, 1), (0, 1)],
        );
        let graph = Graph::build(&result);

        assert_eq!(graph.nodes[0].outbound, 2);
        assert_eq!(graph.nodes[1].inbound, 2);
        assert_eq!(graph.edges.len(), 2);
    }

    #[test]
    fn assigns_symbols_to_their_file() {
        let result = result_with(
            &["C:/repo/a.ts", "C:/repo/b.ts"],
            vec![
                symbol("C:/repo/a.ts", "alpha", 3, SymbolKind::Function),
                symbol("C:/repo/a.ts", "beta", 1, SymbolKind::Class),
                symbol("C:/repo/b.ts", "gamma", 2, SymbolKind::Interface),
            ],
            vec![],
        );
        let graph = Graph::build(&result);

        let a = &graph.nodes[0];
        assert_eq!(a.total_symbols, 2);
        assert_eq!(a.rows[0].name, "beta", "rows are sorted by line");
        assert_eq!(graph.nodes[1].rows[0].name, "gamma");
    }

    #[test]
    fn caps_visible_rows_and_reports_the_rest() {
        let symbols = (0..25)
            .map(|index| symbol("C:/repo/a.ts", &format!("fn{index}"), index, SymbolKind::Function))
            .collect();
        let result = result_with(&["C:/repo/a.ts"], symbols, vec![]);
        let graph = Graph::build(&result);

        assert_eq!(graph.nodes[0].total_symbols, 25);
        assert_eq!(graph.nodes[0].rows.len(), MAX_VISIBLE_ROWS);
        assert!(graph.nodes[0].size.y > HEADER_HEIGHT);
    }

    #[test]
    fn truncate_adds_an_ellipsis() {
        assert_eq!(truncate("short", 10), "short");
        assert_eq!(truncate("abcdefghij", 5), "abcd…");
    }

    #[test]
    fn describes_directories_with_two_components() {
        assert_eq!(
            describe_directory(Path::new("C:/repo/packages/core/src/a.ts")),
            "core/src"
        );
        assert_eq!(describe_directory(Path::new("C:/a.ts")), ".");
    }

    #[test]
    fn fit_scales_the_graph_into_the_canvas() {
        let result = result_with(&["C:/repo/a.ts", "C:/repo/b.ts"], vec![], vec![]);
        let mut view = GraphView::new(Graph::build(&result));
        let canvas = Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0));
        view.fit(canvas);

        assert!(view.zoom > 0.0);
        let screen = view.world_to_screen(canvas, view.graph.bounds.min);
        assert!(screen.x.is_finite() && screen.y.is_finite());
    }
}












