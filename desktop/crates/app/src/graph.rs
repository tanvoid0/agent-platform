//! Graph layout math ported from `web/src/lib/dagGraphLayout.ts` and
//! `web/src/lib/teamRosterLayout.ts`, plus the iced canvas that draws them.
//!
//! Layout is pure and tested; the canvas only renders and hit-tests.

use crate::domain;
use crate::ui::theme;
use agent_platform_client::types::{RosterRole, SubagentNode, TaskNodeRecord};
use iced::widget::canvas::{self, Frame, Geometry, Path, Stroke, Text};
use iced::{mouse, Color, Point, Rectangle, Renderer, Size, Theme, Vector};
use std::collections::HashMap;

/// Node box, and the grid it sits on (`COL_W`/`ROW_H` in the TS original).
const COL_W: f32 = 230.0;
const ROW_H: f32 = 130.0;
pub const NODE_W: f32 = 190.0;
pub const NODE_H: f32 = 74.0;

/// Roster cards are wider and shorter (`NODE_X`/`NODE_Y` in the TS original).
const ROSTER_X: f32 = 300.0;
const ROSTER_Y: f32 = 112.0;

/// Nesting depth from `parent_client_uuid` chains (0 = top-level).
pub fn task_depth_by_uuid(tasks: &[TaskNodeRecord]) -> HashMap<String, usize> {
    let by_uuid: HashMap<&str, &TaskNodeRecord> =
        tasks.iter().map(|t| (t.client_uuid.as_str(), t)).collect();
    let mut memo: HashMap<String, usize> = HashMap::new();

    for task in tasks {
        // Walk to the root, guarding against a parent cycle.
        // `base` is the depth of the chain's topmost entry: 0 when the walk
        // reached a root (the root is in the chain), or parent_depth + 1 when it
        // hit an already-known node (which is not in the chain).
        let mut chain: Vec<&str> = Vec::new();
        let mut cursor = task.client_uuid.as_str();
        let base = loop {
            if let Some(d) = memo.get(cursor) {
                break d + 1;
            }
            if chain.contains(&cursor) {
                break 0; // cycle: treat as root
            }
            chain.push(cursor);
            let parent = by_uuid
                .get(cursor)
                .and_then(|t| t.parent_client_uuid.as_deref())
                .map(str::trim)
                .filter(|p| !p.is_empty() && by_uuid.contains_key(p));
            match parent {
                None => break 0,
                Some(p) => cursor = p,
            }
        };
        for (i, id) in chain.iter().rev().enumerate() {
            memo.insert((*id).to_string(), base + i);
        }
    }
    memo
}

pub fn max_lineage_depth(tasks: &[TaskNodeRecord]) -> usize {
    task_depth_by_uuid(tasks).values().copied().max().unwrap_or(0)
}

/// How much of the sub-DAG lineage the graph shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lineage {
    All,
    DepthLe1,
    Roots,
}

impl Lineage {
    pub const ALL: [Lineage; 3] = [Lineage::All, Lineage::DepthLe1, Lineage::Roots];

    pub fn label(self) -> &'static str {
        match self {
            Lineage::All => "All",
            Lineage::DepthLe1 => "Depth ≤ 1",
            Lineage::Roots => "Roots",
        }
    }

    pub fn max_depth(self) -> Option<usize> {
        match self {
            Lineage::All => None,
            Lineage::DepthLe1 => Some(1),
            Lineage::Roots => Some(0),
        }
    }
}

#[derive(Debug, Clone)]
pub struct GraphNode {
    pub uuid: String,
    pub role: String,
    pub position: Point,
    pub column: domain::BoardColumn,
    pub depth: usize,
    pub parent_hint: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct GraphLayout {
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<(String, String)>,
}

impl GraphLayout {
    pub fn bounds(&self) -> Size {
        let max_x = self.nodes.iter().map(|n| n.position.x).fold(0.0, f32::max) + NODE_W;
        let max_y = self.nodes.iter().map(|n| n.position.y).fold(0.0, f32::max) + NODE_H;
        Size::new(max_x, max_y)
    }

    pub fn node_at(&self, point: Point) -> Option<&GraphNode> {
        self.nodes.iter().find(|n| {
            point.x >= n.position.x
                && point.x <= n.position.x + NODE_W
                && point.y >= n.position.y
                && point.y <= n.position.y + NODE_H
        })
    }
}

/// Layered grid: row = lineage depth, column = planner order within the row.
pub fn dag_layout(
    subagents: &[SubagentNode],
    tasks: &[TaskNodeRecord],
    lineage: Lineage,
) -> GraphLayout {
    let depths = task_depth_by_uuid(tasks);
    let by_task: HashMap<&str, &TaskNodeRecord> =
        tasks.iter().map(|t| (t.client_uuid.as_str(), t)).collect();
    let max_depth = lineage.max_depth();

    // Visible set, then bucket by depth preserving planner order.
    let mut by_depth: Vec<(usize, Vec<&SubagentNode>)> = Vec::new();
    for sub in subagents {
        let depth = depths.get(&sub.client_uuid).copied().unwrap_or(0);
        if max_depth.is_some_and(|cap| depth > cap) {
            continue;
        }
        match by_depth.iter_mut().find(|(d, _)| *d == depth) {
            Some((_, bucket)) => bucket.push(sub),
            None => by_depth.push((depth, vec![sub])),
        }
    }
    by_depth.sort_by_key(|(d, _)| *d);

    let mut nodes = Vec::new();
    for (depth, bucket) in &by_depth {
        for (i, sub) in bucket.iter().enumerate() {
            let column = by_task
                .get(sub.client_uuid.as_str())
                .map(|t| domain::normalize_task_status(&t.status))
                .unwrap_or(domain::BoardColumn::Pending);
            nodes.push(GraphNode {
                uuid: sub.client_uuid.clone(),
                role: sub.role.clone(),
                position: Point::new(i as f32 * COL_W, *depth as f32 * ROW_H),
                column,
                depth: *depth,
                parent_hint: parent_hint(tasks, &sub.client_uuid),
            });
        }
    }

    let visible: Vec<&str> = nodes.iter().map(|n| n.uuid.as_str()).collect();
    let edges = subagents
        .iter()
        .filter(|s| visible.contains(&s.client_uuid.as_str()))
        .flat_map(|s| {
            s.dependencies
                .as_deref()
                .unwrap_or_default()
                .iter()
                .filter(|d| visible.contains(&d.as_str()))
                .map(|d| (d.clone(), s.client_uuid.clone()))
                .collect::<Vec<_>>()
        })
        .collect();

    GraphLayout { nodes, edges }
}

/// "↑ parent role" hint for sub-DAG children.
pub fn parent_hint(tasks: &[TaskNodeRecord], uuid: &str) -> Option<String> {
    let by_uuid: HashMap<&str, &TaskNodeRecord> =
        tasks.iter().map(|t| (t.client_uuid.as_str(), t)).collect();
    let parent = by_uuid.get(uuid)?.parent_client_uuid.as_deref()?.trim();
    if parent.is_empty() {
        return None;
    }
    match by_uuid.get(parent).map(|t| t.role.trim()).filter(|r| !r.is_empty()) {
        Some(role) => Some(format!("↑ {role}")),
        None => Some(format!("↑ {}", domain::short_uuid(parent))),
    }
}

/// Roster tree: roles layered by parent edges, each row centered on x = 0.
/// Unreachable roles (cycles) land in a trailing row.
pub fn roster_layout(roles: &[RosterRole]) -> HashMap<String, Point> {
    let ids: Vec<&str> = roles.iter().map(|r| r.id.as_str()).collect();
    let mut children: HashMap<&str, Vec<&str>> = HashMap::new();
    for r in roles {
        if let Some(p) = r.parent_id.as_deref().filter(|p| ids.contains(p)) {
            children.entry(p).or_default().push(r.id.as_str());
        }
    }

    let mut pos = HashMap::new();
    let mut frontier: Vec<&str> = roles
        .iter()
        .filter(|r| r.parent_id.as_deref().is_none_or(|p| !ids.contains(&p)))
        .map(|r| r.id.as_str())
        .collect();
    let mut level = 0usize;

    while !frontier.is_empty() {
        let width = frontier.len() as f32;
        for (i, id) in frontier.iter().enumerate() {
            let x = (i as f32 - (width - 1.0) / 2.0) * ROSTER_X;
            pos.insert((*id).to_string(), Point::new(x, level as f32 * ROSTER_Y));
        }
        let next: Vec<&str> = frontier
            .iter()
            .flat_map(|id| children.get(id).cloned().unwrap_or_default())
            .filter(|c| !pos.contains_key(*c))
            .collect();
        frontier = next;
        level += 1;
    }

    let mut orphan_col = 0.0;
    for r in roles {
        if !pos.contains_key(&r.id) {
            pos.insert(r.id.clone(), Point::new(orphan_col * ROSTER_X, level as f32 * ROSTER_Y));
            orphan_col += 1.0;
        }
    }
    pos
}

/// Roster tree as a drawable graph, so the roster reuses the DAG canvas.
/// Positions are centered on x = 0, so they are shifted right to keep the
/// leftmost card at the origin like the DAG layout.
pub fn roster_graph(roles: &[RosterRole]) -> GraphLayout {
    let pos = roster_layout(roles);
    let min_x = pos.values().map(|p| p.x).fold(f32::MAX, f32::min);
    let shift = if min_x.is_finite() { -min_x } else { 0.0 };
    let ids: Vec<&str> = roles.iter().map(|r| r.id.as_str()).collect();

    let nodes = roles
        .iter()
        .map(|r| {
            let p = pos.get(&r.id).copied().unwrap_or(Point::ORIGIN);
            GraphNode {
                uuid: r.id.clone(),
                role: if r.name.trim().is_empty() { r.id.clone() } else { r.name.clone() },
                position: Point::new(p.x + shift, p.y),
                column: domain::BoardColumn::Pending,
                depth: 0,
                parent_hint: r
                    .parent_id
                    .as_deref()
                    .filter(|p| ids.contains(p))
                    .map(|p| format!("↑ {p}")),
            }
        })
        .collect();

    let edges = roles
        .iter()
        .filter_map(|r| {
            let parent = r.parent_id.as_deref().filter(|p| ids.contains(p))?;
            Some((parent.to_string(), r.id.clone()))
        })
        .collect();

    GraphLayout { nodes, edges }
}

// ---------------------------------------------------------------------------
// Canvas
// ---------------------------------------------------------------------------

/// Pan/zoom state owned by the screen so it survives re-renders.
#[derive(Debug, Clone, Copy)]
pub struct Viewport {
    pub offset: Vector,
    pub scale: f32,
}

impl Default for Viewport {
    fn default() -> Self {
        Self { offset: Vector::new(24.0, 24.0), scale: 1.0 }
    }
}

impl Viewport {
    pub fn zoom(&mut self, delta: f32) {
        self.scale = (self.scale * (1.0 + delta * 0.1)).clamp(0.35, 2.5);
    }
    pub fn pan(&mut self, delta: Vector) {
        self.offset = self.offset + delta;
    }
    fn to_canvas(&self, point: Point) -> Point {
        Point::new((point.x - self.offset.x) / self.scale, (point.y - self.offset.y) / self.scale)
    }
}

/// Owns its layout: the view builds the layout per render, so there is nothing
/// longer-lived to borrow from.
pub struct DagCanvas {
    pub layout: GraphLayout,
    pub viewport: Viewport,
    pub selected: Option<String>,
}

#[derive(Debug, Clone)]
pub enum CanvasEvent {
    Selected(String),
    Panned(Vector),
    Zoomed(f32),
}

#[derive(Default)]
pub struct CanvasState {
    dragging: Option<Point>,
}

impl<Message> canvas::Program<Message> for DagCanvas
where
    Message: From<CanvasEvent>,
{
    type State = CanvasState;

    fn update(
        &self,
        state: &mut Self::State,
        event: &iced::Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<canvas::Action<Message>> {
        let position = cursor.position_in(bounds)?;

        match event {
            iced::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                let hit = self.layout.node_at(self.viewport.to_canvas(position));
                state.dragging = Some(position);
                hit.map(|node| {
                    canvas::Action::publish(CanvasEvent::Selected(node.uuid.clone()).into())
                        .and_capture()
                })
            }
            iced::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                state.dragging = None;
                None
            }
            iced::Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                let last = state.dragging?;
                let delta = Vector::new(position.x - last.x, position.y - last.y);
                state.dragging = Some(position);
                Some(canvas::Action::publish(CanvasEvent::Panned(delta).into()).and_capture())
            }
            iced::Event::Mouse(mouse::Event::WheelScrolled { delta }) => {
                let y = match delta {
                    mouse::ScrollDelta::Lines { y, .. } => *y,
                    mouse::ScrollDelta::Pixels { y, .. } => y / 40.0,
                };
                Some(canvas::Action::publish(CanvasEvent::Zoomed(y).into()).and_capture())
            }
            _ => None,
        }
    }

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &Renderer,
        theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let t = theme::tokens(theme);
        let mut frame = Frame::new(renderer, bounds.size());
        frame.translate(self.viewport.offset);
        frame.scale(self.viewport.scale);

        let by_uuid: HashMap<&str, &GraphNode> =
            self.layout.nodes.iter().map(|n| (n.uuid.as_str(), n)).collect();

        // Edges first so nodes paint over them.
        for (from, to) in &self.layout.edges {
            let (Some(a), Some(b)) = (by_uuid.get(from.as_str()), by_uuid.get(to.as_str())) else {
                continue;
            };
            let start = Point::new(a.position.x + NODE_W / 2.0, a.position.y + NODE_H);
            let end = Point::new(b.position.x + NODE_W / 2.0, b.position.y);
            let path = Path::new(|p| {
                p.move_to(start);
                // Vertical-tangent bezier reads as a flow between layers.
                let mid = (start.y + end.y) / 2.0;
                p.bezier_curve_to(
                    Point::new(start.x, mid),
                    Point::new(end.x, mid),
                    end,
                );
            });
            frame.stroke(&path, Stroke::default().with_color(t.border).with_width(1.5));
        }

        for node in &self.layout.nodes {
            let accent = theme::tone_color(&t, node.column.tone());
            let selected = self.selected.as_deref() == Some(node.uuid.as_str());
            let rect = Path::rounded_rectangle(
                node.position,
                Size::new(NODE_W, NODE_H),
                theme::radius::LG.into(),
            );
            frame.fill(&rect, if node.depth > 0 { blend(t.card, accent, 0.08) } else { t.card });
            frame.stroke(
                &rect,
                Stroke::default()
                    .with_color(if selected { t.ring } else { accent })
                    .with_width(if selected { 2.5 } else { 1.5 }),
            );

            frame.fill_text(Text {
                content: truncate(&node.role, 22),
                position: Point::new(node.position.x + 12.0, node.position.y + 14.0),
                color: t.foreground,
                size: 14.0.into(),
                ..Text::default()
            });
            frame.fill_text(Text {
                content: node.column.label().to_string(),
                position: Point::new(node.position.x + 12.0, node.position.y + 36.0),
                color: accent,
                size: 12.0.into(),
                ..Text::default()
            });
            if let Some(hint) = &node.parent_hint {
                frame.fill_text(Text {
                    content: truncate(hint, 24),
                    position: Point::new(node.position.x + 12.0, node.position.y + 54.0),
                    color: t.muted_foreground,
                    size: 11.0.into(),
                    ..Text::default()
                });
            }
        }

        vec![frame.into_geometry()]
    }
}

fn blend(base: Color, other: Color, amount: f32) -> Color {
    Color::from_rgb(
        base.r + (other.r - base.r) * amount,
        base.g + (other.g - base.g) * amount,
        base.b + (other.b - base.b) * amount,
    )
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(max).collect::<String>())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sub(id: &str, deps: &[&str]) -> SubagentNode {
        serde_json::from_value(json!({
            "client_uuid": id, "role": format!("role-{id}"),
            "system_prompt": "s", "instructions": "i", "dependencies": deps,
        }))
        .unwrap()
    }

    fn task(uuid: &str, parent: Option<&str>, status: &str) -> TaskNodeRecord {
        serde_json::from_value(json!({
            "id": 1, "process_id": 1, "client_uuid": uuid, "parent_client_uuid": parent,
            "role": format!("role-{uuid}"), "system_prompt": "s", "instructions": "i",
            "llm_model": null, "dependencies_json": "[]", "status": status,
            "output": null, "tokens_used": 0, "started_at": null, "completed_at": null,
        }))
        .unwrap()
    }

    #[test]
    fn depth_follows_parent_chain() {
        let tasks = vec![
            task("a", None, "completed"),
            task("b", Some("a"), "running"),
            task("c", Some("b"), "pending"),
        ];
        let depths = task_depth_by_uuid(&tasks);
        assert_eq!(depths["a"], 0);
        assert_eq!(depths["b"], 1);
        assert_eq!(depths["c"], 2);
        assert_eq!(max_lineage_depth(&tasks), 2);
    }

    #[test]
    fn depth_survives_a_parent_cycle() {
        let tasks = vec![task("a", Some("b"), "pending"), task("b", Some("a"), "pending")];
        let depths = task_depth_by_uuid(&tasks);
        assert!(depths.values().all(|d| *d < 3));
    }

    #[test]
    fn lineage_cap_hides_deep_nodes() {
        let subs = vec![sub("a", &[]), sub("b", &["a"]), sub("c", &["b"])];
        let tasks = vec![
            task("a", None, "completed"),
            task("b", Some("a"), "running"),
            task("c", Some("b"), "pending"),
        ];
        assert_eq!(dag_layout(&subs, &tasks, Lineage::All).nodes.len(), 3);
        assert_eq!(dag_layout(&subs, &tasks, Lineage::DepthLe1).nodes.len(), 2);
        assert_eq!(dag_layout(&subs, &tasks, Lineage::Roots).nodes.len(), 1);
    }

    #[test]
    fn grid_places_rows_by_depth_and_columns_by_order() {
        let subs = vec![sub("a", &[]), sub("b", &[]), sub("c", &["a"])];
        // The parent's own task row must exist for depth to count it, matching
        // `taskDepthByUuid` in the TS original.
        let tasks = vec![task("a", None, "completed"), task("c", Some("a"), "pending")];
        let layout = dag_layout(&subs, &tasks, Lineage::All);
        let by: HashMap<_, _> = layout.nodes.iter().map(|n| (n.uuid.as_str(), n)).collect();
        assert_eq!(by["a"].position, Point::new(0.0, 0.0));
        assert_eq!(by["b"].position, Point::new(COL_W, 0.0));
        assert_eq!(by["c"].position, Point::new(0.0, ROW_H));
    }

    #[test]
    fn edges_are_dropped_when_an_endpoint_is_hidden() {
        let subs = vec![sub("a", &[]), sub("b", &["a"])];
        let tasks = vec![task("a", None, "completed"), task("b", Some("a"), "pending")];
        assert_eq!(dag_layout(&subs, &tasks, Lineage::All).edges.len(), 1);
        assert!(dag_layout(&subs, &tasks, Lineage::Roots).edges.is_empty());
    }

    #[test]
    fn hit_test_matches_node_boxes() {
        let subs = vec![sub("a", &[])];
        let layout = dag_layout(&subs, &[], Lineage::All);
        assert_eq!(layout.node_at(Point::new(5.0, 5.0)).unwrap().uuid, "a");
        assert!(layout.node_at(Point::new(NODE_W + 10.0, 5.0)).is_none());
    }

    #[test]
    fn roster_rows_center_and_orphans_trail() {
        let roles: Vec<RosterRole> = serde_json::from_value(json!([
            {"id": "root", "name": "Root"},
            {"id": "kid1", "name": "K1", "parent_id": "root"},
            {"id": "kid2", "name": "K2", "parent_id": "root"},
            {"id": "loop", "name": "L", "parent_id": "loop"},
        ]))
        .unwrap();
        let pos = roster_layout(&roles);
        assert_eq!(pos["root"], Point::new(0.0, 0.0));
        // Two children straddle x = 0 on the next row.
        assert_eq!(pos["kid1"].x, -ROSTER_X / 2.0);
        assert_eq!(pos["kid2"].x, ROSTER_X / 2.0);
        assert_eq!(pos["kid1"].y, ROSTER_Y);
        // Self-parented role is unreachable and trails below everything.
        assert!(pos["loop"].y > pos["kid1"].y);
    }

    #[test]
    fn viewport_zoom_is_clamped() {
        let mut v = Viewport::default();
        for _ in 0..100 {
            v.zoom(1.0);
        }
        assert!(v.scale <= 2.5);
        for _ in 0..200 {
            v.zoom(-1.0);
        }
        assert!(v.scale >= 0.35);
    }
}
