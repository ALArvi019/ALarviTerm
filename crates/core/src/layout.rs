//! Disposición de paneles: un árbol binario de divisiones cuyas hojas son
//! paneles con pestañas. Es un modelo puro, sin widgets, para poder probarlo y
//! guardarlo. La interfaz se limita a dibujar lo que diga.
//!
//! Invariantes: los identificadores son únicos, siempre hay al menos un panel,
//! el panel con foco existe y cada sesión aparece como mucho en un panel.

use serde_json::{json, Value};

use crate::{Error, Result};

pub type NodeId = u32;

const MIN_RATIO: f64 = 0.08;
const EPSILON: f64 = 1e-6;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Orientation {
    /// Paneles uno al lado del otro.
    Horizontal,
    /// Paneles uno encima del otro.
    Vertical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Before,
    After,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preset {
    Single,
    Columns2,
    Rows2,
    Columns3,
    Grid4,
}

impl Preset {
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "single" => Preset::Single,
            "columns2" => Preset::Columns2,
            "rows2" => Preset::Rows2,
            "columns3" => Preset::Columns3,
            "grid4" => Preset::Grid4,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pane {
    pub id: NodeId,
    /// Identificadores de sesión, en el orden de las pestañas.
    pub tabs: Vec<String>,
    pub active: usize,
}

impl Pane {
    fn empty(id: NodeId) -> Self {
        Self {
            id,
            tabs: Vec::new(),
            active: 0,
        }
    }

    pub fn active_session(&self) -> Option<&str> {
        self.tabs.get(self.active).map(String::as_str)
    }

    fn remove(&mut self, session: &str) -> bool {
        let Some(index) = self.tabs.iter().position(|t| t == session) else {
            return false;
        };
        self.tabs.remove(index);
        if index < self.active || self.active >= self.tabs.len() {
            self.active = self.active.saturating_sub(1);
        }
        true
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Split {
    pub id: NodeId,
    pub orientation: Orientation,
    /// Fracción del espacio que ocupa el primer hijo.
    pub ratio: f64,
    pub first: Box<Node>,
    pub second: Box<Node>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    Pane(Pane),
    Split(Split),
}

/// Rectángulo en el cuadrado unidad que ocupa un panel.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    root: Node,
    focused: NodeId,
    next_id: NodeId,
}

impl Default for Layout {
    fn default() -> Self {
        Self::new()
    }
}

impl Layout {
    /// Un único panel vacío.
    pub fn new() -> Self {
        Self {
            root: Node::Pane(Pane::empty(1)),
            focused: 1,
            next_id: 2,
        }
    }

    /// Disposición predefinida, repartiendo las sesiones dadas por turnos
    /// entre sus paneles.
    pub fn preset(preset: Preset, sessions: &[String]) -> Self {
        let mut layout = Self::new();
        let h = Orientation::Horizontal;
        let v = Orientation::Vertical;
        match preset {
            Preset::Single => {}
            Preset::Columns2 => {
                layout.split(1, h, Side::After);
            }
            Preset::Rows2 => {
                layout.split(1, v, Side::After);
            }
            Preset::Columns3 => {
                let second = layout.split(1, h, Side::After);
                if let Some(second) = second {
                    layout.split(second, h, Side::After);
                }
                // Tres columnas iguales: un tercio y el resto a medias.
                if let Node::Split(split) = &mut layout.root {
                    split.ratio = 1.0 / 3.0;
                }
            }
            Preset::Grid4 => {
                if let Some(bottom) = layout.split(1, v, Side::After) {
                    layout.split(1, h, Side::After);
                    layout.split(bottom, h, Side::After);
                }
            }
        }
        let ids: Vec<NodeId> = layout.panes().iter().map(|p| p.id).collect();
        for (index, session) in sessions.iter().enumerate() {
            if let Some(pane) = layout.pane_mut(ids[index % ids.len()]) {
                pane.tabs.push(session.clone());
            }
        }
        layout.focused = ids[0];
        layout
    }

    pub fn root(&self) -> &Node {
        &self.root
    }

    pub fn focused(&self) -> NodeId {
        self.focused
    }

    pub fn set_focused(&mut self, pane: NodeId) -> bool {
        if self.pane(pane).is_some() {
            self.focused = pane;
            true
        } else {
            false
        }
    }

    /// Paneles en orden de lectura del árbol.
    pub fn panes(&self) -> Vec<&Pane> {
        let mut panes = Vec::new();
        collect_panes(&self.root, &mut panes);
        panes
    }

    pub fn pane(&self, id: NodeId) -> Option<&Pane> {
        self.panes().into_iter().find(|p| p.id == id)
    }

    fn pane_mut(&mut self, id: NodeId) -> Option<&mut Pane> {
        find_pane_mut(&mut self.root, id)
    }

    /// Sesión visible en el panel con foco.
    pub fn active_session(&self) -> Option<&str> {
        self.pane(self.focused).and_then(Pane::active_session)
    }

    /// Todas las sesiones abiertas en algún panel.
    pub fn sessions(&self) -> Vec<&str> {
        self.panes()
            .into_iter()
            .flat_map(|p| p.tabs.iter().map(String::as_str))
            .collect()
    }

    pub fn locate(&self, session: &str) -> Option<NodeId> {
        self.panes()
            .into_iter()
            .find(|p| p.tabs.iter().any(|t| t == session))
            .map(|p| p.id)
    }

    /// Abre la sesión como pestaña activa del panel indicado y le da el foco.
    /// Si estaba en otro panel, se mueve.
    pub fn open(&mut self, pane: NodeId, session: &str) -> bool {
        if self.pane(pane).is_none() {
            return false;
        }
        if self.locate(session) != Some(pane) {
            self.close_tab(session);
            if let Some(target) = self.pane_mut(pane) {
                let at = (target.active + 1).min(target.tabs.len());
                target.tabs.insert(at, session.to_string());
            }
        }
        self.activate(pane, session);
        self.focused = pane;
        true
    }

    /// Lleva el foco a la sesión allí donde esté abierta. Si no lo está, la
    /// abre en el panel con foco. Devuelve el panel resultante.
    pub fn show(&mut self, session: &str) -> NodeId {
        let pane = self.locate(session).unwrap_or(self.focused);
        self.open(pane, session);
        pane
    }

    pub fn activate(&mut self, pane: NodeId, session: &str) -> bool {
        let Some(pane) = self.pane_mut(pane) else {
            return false;
        };
        match pane.tabs.iter().position(|t| t == session) {
            Some(index) => {
                pane.active = index;
                true
            }
            None => false,
        }
    }

    /// Pasa a la pestaña siguiente (o anterior) del panel con foco.
    pub fn cycle_tab(&mut self, step: i64) -> bool {
        let focused = self.focused;
        let Some(pane) = self.pane_mut(focused) else {
            return false;
        };
        if pane.tabs.len() < 2 {
            return false;
        }
        pane.active = (pane.active as i64 + step).rem_euclid(pane.tabs.len() as i64) as usize;
        true
    }

    /// Quita la sesión del panel donde esté. El panel se conserva.
    pub fn close_tab(&mut self, session: &str) -> bool {
        match self.locate(session) {
            Some(pane) => self.pane_mut(pane).is_some_and(|p| p.remove(session)),
            None => false,
        }
    }

    /// Descarta las sesiones que ya no existen.
    pub fn retain_sessions(&mut self, keep: impl Fn(&str) -> bool) {
        let gone: Vec<String> = self
            .sessions()
            .into_iter()
            .filter(|s| !keep(s))
            .map(str::to_string)
            .collect();
        for session in gone {
            self.close_tab(&session);
        }
    }

    /// Divide un panel y devuelve el identificador del panel nuevo, vacío y
    /// con el foco.
    pub fn split(&mut self, pane: NodeId, orientation: Orientation, side: Side) -> Option<NodeId> {
        self.pane(pane)?;
        let split_id = self.next_id;
        let new_id = self.next_id + 1;
        let done = split_node(&mut self.root, pane, orientation, side, split_id, new_id);
        if !done {
            return None;
        }
        self.next_id += 2;
        self.focused = new_id;
        Some(new_id)
    }

    /// Cierra un panel y sus pestañas. El hermano ocupa su sitio. El último
    /// panel no se cierra, se vacía.
    pub fn close_pane(&mut self, pane: NodeId) -> bool {
        if self.pane(pane).is_none() {
            return false;
        }
        if matches!(self.root, Node::Pane(_)) {
            if let Some(only) = self.pane_mut(pane) {
                only.tabs.clear();
                only.active = 0;
            }
            return true;
        }
        let root = std::mem::replace(&mut self.root, Node::Pane(Pane::empty(0)));
        // Hay más de un panel, así que siempre queda algo tras quitar uno.
        self.root = remove_pane(root, pane).unwrap_or_else(|| Node::Pane(Pane::empty(pane)));
        if self.pane(self.focused).is_none() {
            self.focused = self.panes()[0].id;
        }
        true
    }

    pub fn set_ratio(&mut self, split: NodeId, ratio: f64) -> bool {
        match find_split_mut(&mut self.root, split) {
            Some(node) if ratio.is_finite() => {
                node.ratio = ratio.clamp(MIN_RATIO, 1.0 - MIN_RATIO);
                true
            }
            _ => false,
        }
    }

    pub fn rects(&self) -> Vec<(NodeId, Rect)> {
        let mut rects = Vec::new();
        let unit = Rect {
            x: 0.0,
            y: 0.0,
            w: 1.0,
            h: 1.0,
        };
        collect_rects(&self.root, unit, &mut rects);
        rects
    }

    /// Panel contiguo en la dirección dada, si lo hay.
    pub fn neighbor(&self, pane: NodeId, direction: Direction) -> Option<NodeId> {
        let rects = self.rects();
        let current = rects.iter().find(|(id, _)| *id == pane)?.1;
        let center = (current.x + current.w / 2.0, current.y + current.h / 2.0);
        rects
            .iter()
            .filter(|(id, _)| *id != pane)
            .filter_map(|(id, r)| {
                let (distance, overlap, offset) = match direction {
                    Direction::Right => (
                        r.x - (current.x + current.w),
                        overlap(current.y, current.h, r.y, r.h),
                        (r.y + r.h / 2.0 - center.1).abs(),
                    ),
                    Direction::Left => (
                        current.x - (r.x + r.w),
                        overlap(current.y, current.h, r.y, r.h),
                        (r.y + r.h / 2.0 - center.1).abs(),
                    ),
                    Direction::Down => (
                        r.y - (current.y + current.h),
                        overlap(current.x, current.w, r.x, r.w),
                        (r.x + r.w / 2.0 - center.0).abs(),
                    ),
                    Direction::Up => (
                        current.y - (r.y + r.h),
                        overlap(current.x, current.w, r.x, r.w),
                        (r.x + r.w / 2.0 - center.0).abs(),
                    ),
                };
                (distance > -EPSILON && overlap > EPSILON).then_some((*id, distance, offset))
            })
            .min_by(|a, b| {
                (a.1, a.2)
                    .partial_cmp(&(b.1, b.2))
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|(id, _, _)| id)
    }

    pub fn focus_direction(&mut self, direction: Direction) -> bool {
        match self.neighbor(self.focused, direction) {
            Some(pane) => {
                self.focused = pane;
                true
            }
            None => false,
        }
    }

    /// Texto que identifica la forma del árbol, sin proporciones ni pestañas.
    pub fn structure_signature(&self) -> String {
        let mut out = String::new();
        write_signature(&self.root, &mut out);
        out
    }

    pub fn to_json(&self) -> String {
        json!({ "focused": self.focused, "root": node_to_json(&self.root) }).to_string()
    }

    pub fn from_json(text: &str) -> Result<Self> {
        let invalid = |what: &str| Error::Invalid(format!("disposición no válida: {what}"));
        let value: Value = serde_json::from_str(text).map_err(|e| invalid(&e.to_string()))?;
        let mut root = node_from_json(value.get("root").ok_or_else(|| invalid("falta la raíz"))?)
            .ok_or_else(|| invalid("nodo mal formado"))?;

        let mut ids = Vec::new();
        collect_ids(&root, &mut ids);
        let mut unique = ids.clone();
        unique.sort_unstable();
        unique.dedup();
        if unique.len() != ids.len() {
            return Err(invalid("identificadores repetidos"));
        }
        let mut seen = Vec::new();
        dedupe_sessions(&mut root, &mut seen);

        let mut layout = Self {
            root,
            focused: 0,
            next_id: unique.last().copied().unwrap_or(0) + 1,
        };
        let wanted = value.get("focused").and_then(Value::as_u64).unwrap_or(0) as NodeId;
        layout.focused = if layout.pane(wanted).is_some() {
            wanted
        } else {
            layout.panes()[0].id
        };
        Ok(layout)
    }
}

fn overlap(a: f64, a_len: f64, b: f64, b_len: f64) -> f64 {
    (a + a_len).min(b + b_len) - a.max(b)
}

fn collect_panes<'a>(node: &'a Node, out: &mut Vec<&'a Pane>) {
    match node {
        Node::Pane(pane) => out.push(pane),
        Node::Split(split) => {
            collect_panes(&split.first, out);
            collect_panes(&split.second, out);
        }
    }
}

fn collect_ids(node: &Node, out: &mut Vec<NodeId>) {
    match node {
        Node::Pane(pane) => out.push(pane.id),
        Node::Split(split) => {
            out.push(split.id);
            collect_ids(&split.first, out);
            collect_ids(&split.second, out);
        }
    }
}

fn find_pane_mut(node: &mut Node, id: NodeId) -> Option<&mut Pane> {
    match node {
        Node::Pane(pane) => (pane.id == id).then_some(pane),
        Node::Split(split) => {
            find_pane_mut(&mut split.first, id).or_else(|| find_pane_mut(&mut split.second, id))
        }
    }
}

fn find_split_mut(node: &mut Node, id: NodeId) -> Option<&mut Split> {
    match node {
        Node::Pane(_) => None,
        Node::Split(split) => {
            if split.id == id {
                return Some(split);
            }
            match find_split_mut(&mut split.first, id) {
                Some(found) => Some(found),
                None => find_split_mut(&mut split.second, id),
            }
        }
    }
}

fn split_node(
    node: &mut Node,
    pane: NodeId,
    orientation: Orientation,
    side: Side,
    split_id: NodeId,
    new_id: NodeId,
) -> bool {
    match node {
        Node::Pane(existing) if existing.id == pane => {
            let old = std::mem::replace(node, Node::Pane(Pane::empty(0)));
            let new = Node::Pane(Pane::empty(new_id));
            let (first, second) = match side {
                Side::Before => (new, old),
                Side::After => (old, new),
            };
            *node = Node::Split(Split {
                id: split_id,
                orientation,
                ratio: 0.5,
                first: Box::new(first),
                second: Box::new(second),
            });
            true
        }
        Node::Pane(_) => false,
        Node::Split(split) => {
            split_node(&mut split.first, pane, orientation, side, split_id, new_id)
                || split_node(&mut split.second, pane, orientation, side, split_id, new_id)
        }
    }
}

/// Devuelve el árbol sin el panel indicado, o `None` si el nodo era ese panel.
fn remove_pane(node: Node, id: NodeId) -> Option<Node> {
    match node {
        Node::Pane(pane) if pane.id == id => None,
        Node::Pane(pane) => Some(Node::Pane(pane)),
        Node::Split(split) => {
            let Split {
                id: split_id,
                orientation,
                ratio,
                first,
                second,
            } = split;
            match (remove_pane(*first, id), remove_pane(*second, id)) {
                (Some(first), Some(second)) => Some(Node::Split(Split {
                    id: split_id,
                    orientation,
                    ratio,
                    first: Box::new(first),
                    second: Box::new(second),
                })),
                (Some(remaining), None) | (None, Some(remaining)) => Some(remaining),
                (None, None) => None,
            }
        }
    }
}

fn collect_rects(node: &Node, area: Rect, out: &mut Vec<(NodeId, Rect)>) {
    match node {
        Node::Pane(pane) => out.push((pane.id, area)),
        Node::Split(split) => {
            let (first, second) = match split.orientation {
                Orientation::Horizontal => {
                    let w = area.w * split.ratio;
                    (
                        Rect { w, ..area },
                        Rect {
                            x: area.x + w,
                            w: area.w - w,
                            ..area
                        },
                    )
                }
                Orientation::Vertical => {
                    let h = area.h * split.ratio;
                    (
                        Rect { h, ..area },
                        Rect {
                            y: area.y + h,
                            h: area.h - h,
                            ..area
                        },
                    )
                }
            };
            collect_rects(&split.first, first, out);
            collect_rects(&split.second, second, out);
        }
    }
}

fn write_signature(node: &Node, out: &mut String) {
    match node {
        Node::Pane(pane) => out.push_str(&format!("p{}", pane.id)),
        Node::Split(split) => {
            out.push_str(&format!(
                "{}{}(",
                match split.orientation {
                    Orientation::Horizontal => 'h',
                    Orientation::Vertical => 'v',
                },
                split.id
            ));
            write_signature(&split.first, out);
            out.push(',');
            write_signature(&split.second, out);
            out.push(')');
        }
    }
}

fn node_to_json(node: &Node) -> Value {
    match node {
        Node::Pane(pane) => json!({ "pane": pane.id, "tabs": pane.tabs, "active": pane.active }),
        Node::Split(split) => json!({
            "split": split.id,
            "orientation": match split.orientation {
                Orientation::Horizontal => "h",
                Orientation::Vertical => "v",
            },
            "ratio": split.ratio,
            "first": node_to_json(&split.first),
            "second": node_to_json(&split.second),
        }),
    }
}

fn node_from_json(value: &Value) -> Option<Node> {
    if let Some(id) = value.get("pane").and_then(Value::as_u64) {
        let tabs: Vec<String> = value
            .get("tabs")?
            .as_array()?
            .iter()
            .map(|t| t.as_str().map(str::to_string))
            .collect::<Option<_>>()?;
        let active = value.get("active").and_then(Value::as_u64).unwrap_or(0) as usize;
        return Some(Node::Pane(Pane {
            id: NodeId::try_from(id).ok()?,
            active: active.min(tabs.len().saturating_sub(1)),
            tabs,
        }));
    }
    let id = value.get("split").and_then(Value::as_u64)?;
    let orientation = match value.get("orientation")?.as_str()? {
        "h" => Orientation::Horizontal,
        "v" => Orientation::Vertical,
        _ => return None,
    };
    let ratio = value.get("ratio").and_then(Value::as_f64).unwrap_or(0.5);
    Some(Node::Split(Split {
        id: NodeId::try_from(id).ok()?,
        orientation,
        ratio: if ratio.is_finite() {
            ratio.clamp(MIN_RATIO, 1.0 - MIN_RATIO)
        } else {
            0.5
        },
        first: Box::new(node_from_json(value.get("first")?)?),
        second: Box::new(node_from_json(value.get("second")?)?),
    }))
}

/// Si un fichero guardado repite una sesión, se queda con la primera.
fn dedupe_sessions(node: &mut Node, seen: &mut Vec<String>) {
    match node {
        Node::Pane(pane) => {
            pane.tabs.retain(|tab| {
                let fresh = !seen.contains(tab);
                if fresh {
                    seen.push(tab.clone());
                }
                fresh
            });
            pane.active = pane.active.min(pane.tabs.len().saturating_sub(1));
        }
        Node::Split(split) => {
            dedupe_sessions(&mut split.first, seen);
            dedupe_sessions(&mut split.second, seen);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const H: Orientation = Orientation::Horizontal;
    const V: Orientation = Orientation::Vertical;

    fn ids(layout: &Layout) -> Vec<NodeId> {
        layout.panes().iter().map(|p| p.id).collect()
    }

    fn tabs(layout: &Layout, pane: NodeId) -> Vec<&str> {
        layout
            .pane(pane)
            .expect("panel")
            .tabs
            .iter()
            .map(String::as_str)
            .collect()
    }

    #[test]
    fn starts_with_one_empty_focused_pane() {
        let layout = Layout::new();
        assert_eq!(ids(&layout), vec![1]);
        assert_eq!(layout.focused(), 1);
        assert_eq!(layout.active_session(), None);
    }

    #[test]
    fn show_opens_in_focused_pane_and_then_only_refocuses() {
        let mut layout = Layout::new();
        layout.show("a");
        layout.show("b");
        assert_eq!(tabs(&layout, 1), vec!["a", "b"]);
        assert_eq!(layout.active_session(), Some("b"));

        let right = layout.split(1, H, Side::After).expect("divide");
        assert_eq!(layout.focused(), right);
        layout.show("c");
        assert_eq!(tabs(&layout, right), vec!["c"]);

        // «a» ya está abierta a la izquierda: no se duplica, se enfoca.
        assert_eq!(layout.show("a"), 1);
        assert_eq!(layout.focused(), 1);
        assert_eq!(layout.active_session(), Some("a"));
        assert_eq!(layout.sessions().len(), 3);
    }

    #[test]
    fn open_moves_a_session_between_panes() {
        let mut layout = Layout::new();
        layout.show("a");
        layout.show("b");
        let right = layout.split(1, H, Side::After).expect("divide");
        assert!(layout.open(right, "a"));
        assert_eq!(tabs(&layout, 1), vec!["b"]);
        assert_eq!(tabs(&layout, right), vec!["a"]);
        assert_eq!(layout.pane(1).expect("panel").active_session(), Some("b"));
        assert!(!layout.open(999, "a"));
    }

    #[test]
    fn closing_tabs_keeps_a_valid_active_tab() {
        let mut layout = Layout::new();
        for s in ["a", "b", "c"] {
            layout.show(s);
        }
        layout.activate(1, "b");
        assert!(layout.close_tab("a"));
        assert_eq!(layout.active_session(), Some("b"));
        assert!(layout.close_tab("c"));
        assert_eq!(layout.active_session(), Some("b"));
        assert!(layout.close_tab("b"));
        assert_eq!(layout.active_session(), None);
        assert!(!layout.close_tab("b"));
    }

    #[test]
    fn cycle_tab_wraps() {
        let mut layout = Layout::new();
        for s in ["a", "b", "c"] {
            layout.show(s);
        }
        assert!(layout.cycle_tab(1));
        assert_eq!(layout.active_session(), Some("a"));
        assert!(layout.cycle_tab(-1));
        assert_eq!(layout.active_session(), Some("c"));
    }

    #[test]
    fn split_side_controls_order() {
        let mut layout = Layout::new();
        let left = layout.split(1, H, Side::Before).expect("divide");
        assert_eq!(ids(&layout), vec![left, 1]);
        let below = layout.split(1, V, Side::After).expect("divide");
        assert_eq!(ids(&layout), vec![left, 1, below]);
        assert!(layout.split(999, H, Side::After).is_none());
    }

    #[test]
    fn closing_a_pane_promotes_its_sibling_and_moves_focus() {
        let mut layout = Layout::new();
        layout.show("a");
        let right = layout.split(1, H, Side::After).expect("divide");
        layout.show("b");
        let below = layout.split(right, V, Side::After).expect("divide");
        layout.show("c");

        assert!(layout.close_pane(below));
        assert_eq!(ids(&layout), vec![1, right]);
        assert!(layout.pane(layout.focused()).is_some());
        assert_eq!(layout.locate("c"), None);

        assert!(layout.close_pane(1));
        assert_eq!(ids(&layout), vec![right]);
        assert_eq!(layout.focused(), right);
        assert!(matches!(layout.root(), Node::Pane(_)));
    }

    #[test]
    fn last_pane_is_emptied_not_removed() {
        let mut layout = Layout::new();
        layout.show("a");
        assert!(layout.close_pane(1));
        assert_eq!(ids(&layout), vec![1]);
        assert!(layout.sessions().is_empty());
    }

    #[test]
    fn neighbors_in_a_grid() {
        let layout = Layout::preset(Preset::Grid4, &[]);
        let p = ids(&layout);
        let (top_left, top_right, bottom_left, bottom_right) = (p[0], p[1], p[2], p[3]);
        assert_eq!(layout.neighbor(top_left, Direction::Right), Some(top_right));
        assert_eq!(layout.neighbor(top_left, Direction::Down), Some(bottom_left));
        assert_eq!(layout.neighbor(top_left, Direction::Left), None);
        assert_eq!(layout.neighbor(top_left, Direction::Up), None);
        assert_eq!(layout.neighbor(bottom_right, Direction::Up), Some(top_right));
        assert_eq!(layout.neighbor(bottom_right, Direction::Left), Some(bottom_left));
    }

    #[test]
    fn neighbor_prefers_the_closest_pane_across_an_uneven_split() {
        // Izquierda a toda altura, derecha partida en dos.
        let mut layout = Layout::new();
        let right_top = layout.split(1, H, Side::After).expect("divide");
        let right_bottom = layout.split(right_top, V, Side::After).expect("divide");
        assert_eq!(layout.neighbor(right_bottom, Direction::Left), Some(1));
        assert_eq!(layout.neighbor(right_top, Direction::Down), Some(right_bottom));
        // Desde la izquierda gana el panel alineado con su centro: empate
        // resuelto por el orden, pero siempre uno de los dos de la derecha.
        let to_right = layout.neighbor(1, Direction::Right).expect("vecino");
        assert!([right_top, right_bottom].contains(&to_right));
        layout.set_focused(right_bottom);
        assert!(layout.focus_direction(Direction::Left));
        assert_eq!(layout.focused(), 1);
        assert!(!layout.focus_direction(Direction::Left));
    }

    #[test]
    fn ratios_are_clamped_and_shape_rects() {
        let mut layout = Layout::preset(Preset::Columns2, &[]);
        let Node::Split(split) = layout.root() else {
            panic!("se esperaba una división");
        };
        let split_id = split.id;
        assert!(layout.set_ratio(split_id, 0.25));
        let rects = layout.rects();
        assert!((rects[0].1.w - 0.25).abs() < 1e-9);
        assert!((rects[1].1.x - 0.25).abs() < 1e-9);
        assert!(layout.set_ratio(split_id, 5.0));
        assert!(layout.rects()[0].1.w < 1.0);
        assert!(!layout.set_ratio(split_id, f64::NAN));
        assert!(!layout.set_ratio(999, 0.5));
    }

    #[test]
    fn presets_distribute_sessions_in_turns() {
        let sessions: Vec<String> = ["a", "b", "c", "d", "e"].iter().map(|s| s.to_string()).collect();
        let layout = Layout::preset(Preset::Columns3, &sessions);
        let p = ids(&layout);
        assert_eq!(p.len(), 3);
        assert_eq!(tabs(&layout, p[0]), vec!["a", "d"]);
        assert_eq!(tabs(&layout, p[1]), vec!["b", "e"]);
        assert_eq!(tabs(&layout, p[2]), vec!["c"]);
        let widths: Vec<f64> = layout.rects().iter().map(|(_, r)| r.w).collect();
        assert!(widths.iter().all(|w| (w - 1.0 / 3.0).abs() < 1e-9));
        assert_eq!(Layout::preset(Preset::Grid4, &[]).panes().len(), 4);
        assert_eq!(Preset::parse("rows2"), Some(Preset::Rows2));
        assert_eq!(Preset::parse("otro"), None);
    }

    #[test]
    fn json_roundtrip_preserves_everything() {
        let mut layout = Layout::new();
        layout.show("a");
        let right = layout.split(1, H, Side::After).expect("divide");
        layout.show("b");
        layout.show("c");
        layout.activate(right, "b");
        let below = layout.split(1, V, Side::After).expect("divide");
        layout.set_focused(right);
        let restored = Layout::from_json(&layout.to_json()).expect("válido");
        assert_eq!(restored, layout);
        // Los identificadores nuevos no chocan con los restaurados.
        let mut restored = restored;
        let fresh = restored.split(below, H, Side::After).expect("divide");
        assert!(!ids(&layout).contains(&fresh));
    }

    #[test]
    fn from_json_rejects_garbage_and_repairs_what_it_can() {
        assert!(Layout::from_json("no es json").is_err());
        assert!(Layout::from_json("{}").is_err());
        let duplicated_ids = r#"{"focused":1,"root":{"split":1,"orientation":"h","ratio":0.5,
            "first":{"pane":1,"tabs":[],"active":0},"second":{"pane":2,"tabs":[],"active":0}}}"#;
        assert!(Layout::from_json(duplicated_ids).is_err());

        let repairable = r#"{"focused":77,"root":{"split":3,"orientation":"v","ratio":9,
            "first":{"pane":1,"tabs":["a","b"],"active":8},"second":{"pane":2,"tabs":["a"],"active":0}}}"#;
        let layout = Layout::from_json(repairable).expect("reparable");
        assert_eq!(layout.focused(), 1);
        assert_eq!(layout.pane(1).expect("panel").active, 1);
        assert!(tabs(&layout, 2).is_empty(), "la sesión repetida se queda en el primero");
        assert!(layout.rects()[0].1.h < 1.0);
    }

    #[test]
    fn retain_drops_unknown_sessions() {
        let mut layout = Layout::preset(
            Preset::Columns2,
            &["a".to_string(), "b".to_string(), "c".to_string()],
        );
        layout.retain_sessions(|s| s != "a");
        assert_eq!(layout.sessions(), vec!["c", "b"]);
    }

    #[test]
    fn signature_tracks_shape_only() {
        let mut layout = Layout::new();
        let before = layout.structure_signature();
        layout.show("a");
        assert_eq!(layout.structure_signature(), before);
        layout.split(1, H, Side::After);
        assert_ne!(layout.structure_signature(), before);
    }
}
