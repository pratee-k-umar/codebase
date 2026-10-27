//! Flowchart view: draws a function's control-flow graph.
//!
//! Laid out top-to-bottom by longest-path rank, with decisions as diamonds,
//! loops as banded boxes, and labelled branches. The whole diagram sits inside
//! a titled container, the way a hand-drawn process diagram groups its stages.

use egui::{
    Align2, Color32, CornerRadius, FontId, Pos2, Rect, Sense, Shape, Stroke, Ui, Vec2,
};

use codebase_intel_core::cfg::{FlowGraph, FlowKind};

const BACKGROUND: Color32 = Color32::from_rgb(18, 20, 24);
const CONTAINER: Color32 = Color32::from_rgb(24, 27, 33);
const CONTAINER_STROKE: Color32 = Color32::from_rgb(46, 52, 62);
const TEXT: Color32 = Color32::from_rgb(226, 231, 239);
const TEXT_MUTED: Color32 = Color32::from_rgb(140, 148, 162);
const EDGE: Color32 = Color32::from_rgb(120, 130, 146);
const EDGE_BACK: Color32 = Color32::from_rgb(180, 140, 90);
const ACCENT: Color32 = Color32::from_rgb(94, 168, 255);
const START: Color32 = Color32::from_rgb(78, 158, 110);
const END: Color32 = Color32::from_rgb(168, 92, 92);
const DECISION: Color32 = Color32::from_rgb(176, 138, 62);
const LOOP: Color32 = Color32::from_rgb(112, 108, 176);
const RETURN: Color32 = Color32::from_rgb(150, 96, 120);
const THROW: Color32 = Color32::from_rgb(178, 78, 78);
const TRY: Color32 = Color32::from_rgb(70, 128, 140);

const RANK_GAP: f32 = 74.0;
const SIBLING_GAP: f32 = 44.0;
const MARGIN: f32 = 48.0;

#[derive(Debug, Clone, Copy)]
struct Layout {
    pos: Pos2,
    size: Vec2,
}

pub struct FlowView {
    pub graph: FlowGraph,
    pub zoom: f32,
    pub pan: Vec2,
    pub selected: Option<usize>,
    pub fit_requested: bool,
    pub last_canvas: Rect,
    layout: Vec<Layout>,
    bounds: Rect,
}

impl FlowView {
    pub fn new(graph: FlowGraph) -> Self {
        let mut view = Self {
            graph,
            zoom: 1.0,
            pan: Vec2::ZERO,
            selected: None,
            fit_requested: true,
            last_canvas: Rect::ZERO,
            layout: Vec::new(),
            bounds: Rect::from_min_size(Pos2::ZERO, Vec2::splat(200.0)),
        };
        view.relayout();
        view
    }

    /// Assign ranks (top to bottom) and positions.
    pub fn relayout(&mut self) {
        let count = self.graph.nodes.len();
        if count == 0 {
            return;
        }

        // Longest path from start, ignoring back edges that close loops.
        let mut rank = vec![0usize; count];
        for _ in 0..count {
            let mut changed = false;
            for edge in &self.graph.edges {
                if edge.back {
                    continue;
                }
                let candidate = rank[edge.from] + 1;
                if candidate < count && rank[edge.to] < candidate {
                    rank[edge.to] = candidate;
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }

        let rows = rank.iter().copied().max().unwrap_or(0) + 1;
        let mut per_row: Vec<Vec<usize>> = vec![Vec::new(); rows];
        for (id, row) in rank.iter().enumerate() {
            per_row[*row].push(id);
        }

        let mut layout = vec![Layout {
            pos: Pos2::ZERO,
            size: Vec2::new(200.0, 56.0),
        }; count];

        let mut y = MARGIN;
        for row in &per_row {
            let heights: Vec<f32> = row
                .iter()
                .map(|id| node_size(self.graph.nodes[*id].kind).y)
                .collect();
            let widths: Vec<f32> = row
                .iter()
                .map(|id| node_size(self.graph.nodes[*id].kind).x)
                .collect();

            let total_width: f32 =
                widths.iter().sum::<f32>() + SIBLING_GAP * (row.len().saturating_sub(1) as f32);
            let mut x = -total_width / 2.0;
            let row_height = heights.iter().copied().fold(0.0, f32::max);

            for (position, id) in row.iter().enumerate() {
                let size = Vec2::new(widths[position], heights[position]);
                // Centre each node vertically inside its row.
                let offset = (row_height - size.y) / 2.0;
                layout[*id] = Layout {
                    pos: Pos2::new(x, y + offset),
                    size,
                };
                x += size.x + SIBLING_GAP;
            }

            y += row_height + RANK_GAP;
        }

        let mut bounds = Rect::NOTHING;
        for entry in &layout {
            bounds = bounds.union(Rect::from_min_size(entry.pos, entry.size));
        }
        self.bounds = bounds.expand(MARGIN);
        self.layout = layout;
    }

    pub fn node_rect(&self, id: usize) -> Option<Rect> {
        self.layout
            .get(id)
            .map(|layout| Rect::from_min_size(layout.pos, layout.size))
    }

    fn screen_to_world(&self, canvas: Rect, screen: Pos2) -> Pos2 {
        ((screen - canvas.min - self.pan) / self.zoom).to_pos2()
    }

    fn world_to_screen(&self, canvas: Rect, world: Pos2) -> Pos2 {
        canvas.min + self.pan + world.to_vec2() * self.zoom
    }

    pub fn fit(&mut self, canvas: Rect) {
        let bounds = self.bounds;
        if bounds.width() <= 1.0 || bounds.height() <= 1.0 {
            return;
        }
        let scale_x = (canvas.width() - 40.0) / bounds.width();
        let scale_y = (canvas.height() - 40.0) / bounds.height();
        self.zoom = scale_x.min(scale_y).clamp(0.12, 1.4);
        self.pan = canvas.size() / 2.0 - (bounds.center().to_vec2() * self.zoom);
    }

    fn hit_test(&self, world: Pos2) -> Option<usize> {
        self.layout
            .iter()
            .enumerate()
            .rev()
            .find(|(_, layout)| Rect::from_min_size(layout.pos, layout.size).contains(world))
            .map(|(id, _)| id)
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
                self.zoom = (self.zoom * (1.0 + scroll * 0.0016)).clamp(0.12, 2.4);
                let after = self.world_to_screen(canvas, before);
                self.pan += pointer - after;
            }
        }

        if response.dragged() {
            self.pan += response.drag_delta();
        }

        if response.clicked() {
            if let Some(pointer) = response.interact_pointer_pos() {
                self.selected = self.hit_test(self.screen_to_world(canvas, pointer));
            }
        }

        let painter = painter.with_clip_rect(canvas);

        // Container behind everything.
        let container = Rect::from_min_max(
            self.world_to_screen(canvas, self.bounds.min),
            self.world_to_screen(canvas, self.bounds.max),
        );
        painter.rect_filled(container, CornerRadius::same(12), CONTAINER);
        painter.rect_filled(
            Rect::from_min_size(container.min, Vec2::new(container.width(), 1.0)),
            CornerRadius::ZERO,
            CONTAINER_STROKE,
        );
        painter.text(
            container.min + Vec2::new(16.0, 10.0),
            Align2::LEFT_TOP,
            format!("{}()", self.graph.function),
            FontId::proportional(12.0),
            TEXT_MUTED,
        );

        self.paint_edges(&painter, canvas);
        self.paint_nodes(&painter, canvas);

        painter.text(
            Pos2::new(canvas.left() + 12.0, canvas.bottom() - 10.0),
            Align2::LEFT_BOTTOM,
            "scroll: zoom   ·   drag: pan   ·   click a block to see its code",
            FontId::proportional(11.0),
            TEXT_MUTED,
        );
    }

    fn paint_edges(&self, painter: &egui::Painter, canvas: Rect) {
        for edge in &self.graph.edges {
            let (Some(source), Some(target)) = (self.node_rect(edge.from), self.node_rect(edge.to))
            else {
                continue;
            };

            let highlighted = self.selected == Some(edge.from) || self.selected == Some(edge.to);
            let color = if highlighted {
                ACCENT
            } else if edge.back {
                EDGE_BACK
            } else {
                EDGE
            };
            let stroke = Stroke::new(if highlighted { 2.0f32 } else { 1.4f32 }, color);

            if edge.back {
                // Loop back: out to the right, up, then into the target's right.
                let start = self.world_to_screen(canvas, source.right_center());
                let end = self.world_to_screen(canvas, target.right_center());
                let out = (40.0 + 12.0 * (self.zoom - 0.4).max(0.0)).max(24.0);
                let points = vec![
                    start,
                    Pos2::new(start.x + out, start.y),
                    Pos2::new(start.x + out, end.y),
                    end,
                ];
                painter.add(Shape::line(points, stroke));
                arrow(painter, end, false, color, stroke.width);
            } else {
                let start = self.world_to_screen(canvas, source.center_bottom());
                let end = self.world_to_screen(canvas, target.center_top());
                let mid_y = (start.y + end.y) / 2.0;
                let points = vec![
                    start,
                    Pos2::new(start.x, mid_y),
                    Pos2::new(end.x, mid_y),
                    end,
                ];
                painter.add(Shape::line(points, stroke));
                arrow(painter, end, true, color, stroke.width);
            }

            if let Some(label) = &edge.label {
                let position = if edge.back {
                    let start = self.world_to_screen(canvas, source.right_center());
                    let end = self.world_to_screen(canvas, target.right_center());
                    Pos2::new(start.x + 30.0, (start.y + end.y) / 2.0)
                } else {
                    let start = self.world_to_screen(canvas, source.center_bottom());
                    let end = self.world_to_screen(canvas, target.center_top());
                    Pos2::new((start.x + end.x) / 2.0 + 4.0, (start.y + end.y) / 2.0)
                };
                painter.text(
                    position,
                    Align2::LEFT_CENTER,
                    label,
                    FontId::proportional(10.5),
                    TEXT_MUTED,
                );
            }
        }
    }

    fn paint_nodes(&self, painter: &egui::Painter, canvas: Rect) {
        for (id, layout) in self.layout.iter().enumerate() {
            let rect = Rect::from_min_max(
                self.world_to_screen(canvas, layout.pos),
                self.world_to_screen(canvas, layout.pos + layout.size),
            );
            if !rect.intersects(canvas) {
                continue;
            }

            let node = &self.graph.nodes[id];
            let selected = self.selected == Some(id);
            let fill = fill_for(node.kind);
            let border = if selected {
                ACCENT
            } else {
                fill.gamma_multiply(1.5)
            };

            match node.kind {
                FlowKind::Decision => {
                    let points = vec![
                        Pos2::new(rect.center().x, rect.top()),
                        Pos2::new(rect.right(), rect.center().y),
                        Pos2::new(rect.center().x, rect.bottom()),
                        Pos2::new(rect.left(), rect.center().y),
                    ];
                    painter.add(Shape::convex_polygon(
                        points,
                        fill,
                        Stroke::new(if selected { 2.0f32 } else { 1.0f32 }, border),
                    ));
                }
                FlowKind::Start | FlowKind::End => {
                    painter.rect_filled(
                        rect,
                        CornerRadius::same((rect.height() / 2.0) as u8),
                        fill,
                    );
                }
                _ => {
                    painter.rect_filled(rect, CornerRadius::same(6), fill);
                    if node.kind == FlowKind::Loop {
                        let band = Rect::from_min_size(rect.min, Vec2::new(rect.width(), 4.0));
                        painter.rect_filled(band, CornerRadius::same(2), border);
                    }
                }
            }

            if self.zoom < 0.34 {
                continue;
            }

            let text = truncate(&node.label, text_capacity(rect.width()));
            painter.text(
                rect.center(),
                Align2::CENTER_CENTER,
                text,
                FontId::proportional((11.5 * self.zoom).clamp(7.0, 13.0)),
                TEXT,
            );

            if self.zoom > 0.75 && matches!(node.kind, FlowKind::Loop | FlowKind::Try | FlowKind::Catch)
            {
                painter.text(
                    rect.left_top() + Vec2::new(8.0, 6.0),
                    Align2::LEFT_TOP,
                    kind_label(node.kind),
                    FontId::proportional(9.0),
                    TEXT_MUTED,
                );
            }
        }
    }
}

fn node_size(kind: FlowKind) -> Vec2 {
    match kind {
        FlowKind::Start | FlowKind::End => Vec2::new(150.0, 46.0),
        FlowKind::Decision => Vec2::new(210.0, 92.0),
        FlowKind::Loop => Vec2::new(210.0, 64.0),
        FlowKind::Return | FlowKind::Throw => Vec2::new(200.0, 52.0),
        FlowKind::Try | FlowKind::Catch => Vec2::new(200.0, 54.0),
        FlowKind::Action => Vec2::new(220.0, 56.0),
    }
}

fn fill_for(kind: FlowKind) -> Color32 {
    match kind {
        FlowKind::Start => START,
        FlowKind::End => END,
        FlowKind::Decision => DECISION,
        FlowKind::Loop => LOOP,
        FlowKind::Return => RETURN,
        FlowKind::Throw => THROW,
        FlowKind::Try | FlowKind::Catch => TRY,
        FlowKind::Action => Color32::from_rgb(48, 54, 66),
    }
}

fn kind_label(kind: FlowKind) -> &'static str {
    match kind {
        FlowKind::Loop => "loop",
        FlowKind::Try => "try",
        FlowKind::Catch => "catch",
        _ => "",
    }
}

fn arrow(painter: &egui::Painter, tip: Pos2, downwards: bool, color: Color32, width: f32) {
    let size = 5.0 + width;
    let base = if downwards {
        Pos2::new(tip.x, tip.y - size)
    } else {
        Pos2::new(tip.x + size, tip.y)
    };

    let points = if downwards {
        vec![
            tip,
            Pos2::new(base.x - size * 0.55, base.y),
            Pos2::new(base.x + size * 0.55, base.y),
        ]
    } else {
        vec![
            tip,
            Pos2::new(base.x, base.y - size * 0.55),
            Pos2::new(base.x, base.y + size * 0.55),
        ]
    };

    painter.add(Shape::convex_polygon(points, color, Stroke::NONE));
}

fn text_capacity(pixel_width: f32) -> usize {
    ((pixel_width - 16.0) / 6.2).max(6.0) as usize
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
    use codebase_intel_core::cfg::{FlowEdge, FlowNode};
    use codebase_intel_core::{Position, Span};
    use std::path::PathBuf;

    fn span() -> Span {
        Span {
            start: Position { line: 1, column: 1, offset: 0 },
            end: Position { line: 1, column: 1, offset: 0 },
        }
    }

    fn node(id: usize, kind: FlowKind) -> FlowNode {
        FlowNode {
            id,
            kind,
            label: format!("n{id}"),
            detail: String::new(),
            span: span(),
        }
    }

    fn view(nodes: Vec<FlowNode>, edges: Vec<(usize, usize)>) -> FlowView {
        FlowView::new(FlowGraph {
            function: "subject".into(),
            file: PathBuf::from("C:/r/a.ts"),
            nodes,
            edges: edges
                .into_iter()
                .map(|(from, to)| FlowEdge { from, to, label: None, back: false })
                .collect(),
        })
    }

    #[test]
    fn ranks_flow_downwards() {
        let view = view(
            vec![
                node(0, FlowKind::Start),
                node(1, FlowKind::Action),
                node(2, FlowKind::Decision),
                node(3, FlowKind::End),
            ],
            vec![(0, 1), (1, 2), (2, 3)],
        );

        let y: Vec<f32> = (0..4).map(|id| view.node_rect(id).unwrap().min.y).collect();
        assert!(y[0] < y[1] && y[1] < y[2] && y[2] < y[3], "y = {y:?}");
    }

    #[test]
    fn siblings_do_not_overlap() {
        let view = view(
            vec![
                node(0, FlowKind::Start),
                node(1, FlowKind::Action),
                node(2, FlowKind::Action),
                node(3, FlowKind::End),
            ],
            vec![(0, 1), (0, 2), (1, 3), (2, 3)],
        );

        assert!(!view.node_rect(1).unwrap().intersects(view.node_rect(2).unwrap()));
    }

    #[test]
    fn back_edges_do_not_break_ranking() {
        let mut view = view(
            vec![
                node(0, FlowKind::Start),
                node(1, FlowKind::Loop),
                node(2, FlowKind::Action),
                node(3, FlowKind::End),
            ],
            vec![(0, 1), (1, 2), (1, 3)],
        );
        view.graph.edges.push(FlowEdge {
            from: 2,
            to: 1,
            label: Some("loop".into()),
            back: true,
        });
        view.relayout();

        assert!(view.node_rect(1).unwrap().min.y < view.node_rect(2).unwrap().min.y);
    }

    #[test]
    fn fit_produces_usable_zoom() {
        let mut view = view(
            vec![node(0, FlowKind::Start), node(1, FlowKind::End)],
            vec![(0, 1)],
        );
        let canvas = Rect::from_min_size(Pos2::ZERO, Vec2::new(900.0, 700.0));
        view.fit(canvas);
        assert!(view.zoom >= 0.12 && view.zoom <= 1.4);
    }
}

