//! A readable EXPLAIN: `explain (format json …)` output parsed into a tree of
//! plan nodes, drawn one row per node with folding, each node's share of the
//! time (or cost) as a bar, and the conditions of the selected node under it.

use super::grid::Action;
use crate::render::Metrics;
use crate::ui::{icon, with_alpha, Icon, Theme};
use gpui::{div, prelude::*, px, AnyElement, Hsla, Keystroke, Pixels, Point, Size};
use serde_json::Value;
use std::collections::HashSet;

const PAD: f32 = 16.;
/// indent per level
const STEP: f32 = 12.;
const CHEVRON: f32 = 16.;
const ROW_H: f32 = 28.;
const HEADER_H: f32 = 26.;
const DETAIL_H: f32 = 18.;
const DETAIL_PAD: f32 = 6.;
const BAR_W: f32 = 40.;

pub struct Plan {
    /// the JSON as Postgres returned it
    pub raw: String,
    /// depth-first, parents before children
    pub nodes: Vec<Node>,
    pub planning_ms: Option<f64>,
    pub execution_ms: Option<f64>,
    /// run with ANALYZE: nodes have actual numbers
    pub analyzed: bool,
}

pub struct Node {
    pub depth: usize,
    pub parent: Option<usize>,
    /// index past this node's subtree
    pub end: usize,
    /// "Index Scan on users using users_email_idx"
    pub label: String,
    /// "InitPlan 1 (returns $0)", "CTE recent"
    pub tag: Option<String>,
    /// conditions, keys, sort and buffer details
    pub details: Vec<String>,
    /// what looks wrong, for `warning`
    pub warnings: Vec<String>,
    /// rows per loop, as planned
    pub est_rows: f64,
    pub cost: f64,
    pub actual: Option<Actual>,
    /// this node's own share (without its children) of the whole plan's time,
    /// or of its cost without ANALYZE: 0..=1
    pub share: f32,
    /// the actual rows are far from the estimate
    pub misestimate: bool,
}

pub struct Actual {
    /// over all loops
    pub ms: f64,
    /// per loop, like the estimate
    pub rows: f64,
    pub loops: f64,
}

impl Node {
    fn has_children(&self, i: usize) -> bool {
        self.end > i + 1
    }
}

fn num(v: &Value, k: &str) -> Option<f64> {
    v.get(k).and_then(Value::as_f64)
}

fn text<'a>(v: &'a Value, k: &str) -> Option<&'a str> {
    v.get(k).and_then(Value::as_str)
}

/// Parse `explain (format json)` output, `None` if it isn't a plan.
pub fn parse(raw: &str) -> Option<Plan> {
    let v: Value = serde_json::from_str(raw.trim()).ok()?;
    let top = match &v {
        Value::Array(a) => a.first()?,
        o @ Value::Object(_) => o,
        _ => return None,
    };
    let root = top.get("Plan").filter(|p| p.is_object())?;
    let mut nodes = vec![];
    walk(root, 0, None, &mut nodes);
    let analyzed = nodes.iter().any(|n| n.actual.is_some());

    // own share: inclusive minus the direct children's inclusive
    let inclusive = |n: &Node| match (&n.actual, analyzed) {
        (Some(a), true) => a.ms,
        (None, true) => 0.,
        (_, false) => n.cost,
    };
    let own: Vec<f64> = (0..nodes.len())
        .map(|i| {
            let kids: f64 = (i + 1..nodes[i].end)
                .filter(|j| nodes[*j].parent == Some(i))
                .map(|j| inclusive(&nodes[j]))
                .sum();
            (inclusive(&nodes[i]) - kids).max(0.)
        })
        .collect();
    let total: f64 = own.iter().sum();
    for (n, o) in nodes.iter_mut().zip(&own) {
        n.share = if total > 0. { (o / total) as f32 } else { 0. };
        if !analyzed && n.label.starts_with("Seq Scan") && n.share >= 0.5 && n.cost >= 10_000. {
            n.warnings
                .push(format!("Whole-table scan, {:.0}% of the cost", n.share * 100.));
        }
    }
    Some(Plan {
        raw: raw.trim().to_string(),
        nodes,
        planning_ms: num(top, "Planning Time"),
        execution_ms: num(top, "Execution Time"),
        analyzed,
    })
}

fn walk(v: &Value, depth: usize, parent: Option<usize>, out: &mut Vec<Node>) {
    let i = out.len();
    out.push(node(v, depth, parent));
    if let Some(kids) = v.get("Plans").and_then(Value::as_array) {
        for k in kids {
            walk(k, depth + 1, Some(i), out);
        }
    }
    out[i].end = out.len();
}

fn node(v: &Value, depth: usize, parent: Option<usize>) -> Node {
    let est_rows = num(v, "Plan Rows").unwrap_or(0.);
    let loops = num(v, "Actual Loops");
    let actual = loops.map(|loops| Actual {
        ms: num(v, "Actual Total Time").unwrap_or(0.) * loops,
        rows: num(v, "Actual Rows").unwrap_or(0.),
        loops,
    });
    let mut warnings = vec![];
    let mut misestimate = false;
    if let Some(a) = actual.as_ref().filter(|a| a.loops > 0.) {
        let (e, r) = (est_rows.max(1.), a.rows.max(1.));
        if e / r > 10. || r / e > 10. {
            misestimate = true;
            warnings.push(format!(
                "Estimated {} row{}, got {}",
                count(est_rows),
                if est_rows == 1. { "" } else { "s" },
                count(a.rows)
            ));
        }
        let removed = num(v, "Rows Removed by Filter").unwrap_or(0.);
        if text(v, "Node Type") == Some("Seq Scan") && removed >= 1000. && removed > 10. * a.rows.max(1.) {
            warnings.push(format!("Filter removed {} rows", count(removed * a.loops)));
        }
    }
    Node {
        depth,
        parent,
        end: 0,
        label: label(v),
        tag: text(v, "Subplan Name").map(str::to_string),
        details: details(v, actual.as_ref().map_or(1., |a| a.loops)),
        warnings,
        est_rows,
        cost: num(v, "Total Cost").unwrap_or(0.),
        actual,
        share: 0.,
        misestimate,
    }
}

/// The node as psql would name it, with what it reads.
fn label(v: &Value) -> String {
    let mut ty = text(v, "Node Type").unwrap_or("Node").to_string();
    match ty.as_str() {
        "Aggregate" => {
            ty = match text(v, "Strategy") {
                Some("Sorted") => "GroupAggregate",
                Some("Hashed") => "HashAggregate",
                Some("Mixed") => "MixedAggregate",
                _ => "Aggregate",
            }
            .into()
        }
        "SetOp" => {
            let op = if text(v, "Strategy") == Some("Hashed") { "HashSetOp" } else { "SetOp" };
            ty = match text(v, "Command") {
                Some(c) => format!("{op} {c}"),
                None => op.into(),
            };
        }
        "ModifyTable" => {
            if let Some(op) = text(v, "Operation") {
                ty = op.into();
            }
        }
        _ => {}
    }
    if let Some(pm) = text(v, "Partial Mode").filter(|m| *m != "Simple") {
        ty = format!("{pm} {ty}");
    }
    if text(v, "Scan Direction") == Some("Backward") {
        ty.push_str(" Backward");
    }
    if let Some(jt) = text(v, "Join Type").filter(|j| *j != "Inner") {
        ty = match ty.strip_suffix(" Join") {
            Some(base) => format!("{base} {jt} Join"),
            None => format!("{ty} {jt} Join"),
        };
    }
    let index = text(v, "Index Name");
    if let Some(rel) = text(v, "Relation Name") {
        ty.push_str(" on ");
        ty.push_str(rel);
        if let Some(alias) = text(v, "Alias").filter(|a| *a != rel) {
            ty.push(' ');
            ty.push_str(alias);
        }
        if let Some(idx) = index {
            ty.push_str(" using ");
            ty.push_str(idx);
        }
    } else if let Some(on) = index.or(text(v, "CTE Name")).or(text(v, "Function Name")) {
        ty.push_str(" on ");
        ty.push_str(on);
    }
    ty
}

/// Secondary lines: conditions, keys, removed rows, sorts, hashes, buffers.
fn details(v: &Value, loops: f64) -> Vec<String> {
    let mut out = vec![];
    for k in [
        "Hash Cond",
        "Merge Cond",
        "Index Cond",
        "Recheck Cond",
        "TID Cond",
        "Join Filter",
        "Filter",
        "One-Time Filter",
    ] {
        if let Some(s) = text(v, k) {
            out.push(format!("{k}: {s}"));
        }
    }
    for (k, what) in [
        ("Rows Removed by Index Recheck", "index recheck"),
        ("Rows Removed by Join Filter", "join filter"),
        ("Rows Removed by Filter", "filter"),
    ] {
        if let Some(n) = num(v, k).filter(|n| *n > 0.) {
            out.push(format!("Rows removed by {what}: {}", count(n * loops)));
        }
    }
    for k in ["Sort Key", "Presorted Key", "Group Key"] {
        if let Some(keys) = v.get(k).and_then(Value::as_array) {
            let keys: Vec<&str> = keys.iter().filter_map(Value::as_str).collect();
            out.push(format!("{k}: {}", keys.join(", ")));
        }
    }
    if let Some(m) = text(v, "Sort Method") {
        let space = match (num(v, "Sort Space Used"), text(v, "Sort Space Type")) {
            (Some(n), Some(t)) => format!(", {} kB {}", count(n), t.to_lowercase()),
            _ => String::new(),
        };
        out.push(format!("Sort: {m}{space}"));
    }
    if let Some(b) = num(v, "Hash Buckets") {
        let batches = num(v, "Hash Batches").unwrap_or(1.);
        let mem = num(v, "Peak Memory Usage").map_or(String::new(), |m| format!(", {} kB", count(m)));
        out.push(format!(
            "Hash: {} buckets, {} batch{}{mem}",
            count(b),
            count(batches),
            if batches == 1. { "" } else { "es" }
        ));
    }
    if let Some(n) = num(v, "Heap Fetches") {
        out.push(format!("Heap fetches: {}", count(n)));
    }
    if let Some(p) = num(v, "Workers Planned") {
        out.push(match num(v, "Workers Launched") {
            Some(l) => format!("Workers: {} planned, {} launched", count(p), count(l)),
            None => format!("Workers: {} planned", count(p)),
        });
    }
    let blocks = |kind: &str, keys: &[(&str, &str)]| {
        let parts: Vec<String> = keys
            .iter()
            .filter_map(|(k, name)| {
                let n = num(v, &format!("{kind} {k} Blocks"))?;
                (n > 0.).then(|| format!("{name}={}", n as u64))
            })
            .collect();
        (!parts.is_empty()).then(|| format!("{} {}", kind.to_lowercase(), parts.join(" ")))
    };
    let buffers: Vec<String> = [
        blocks("Shared", &[("Hit", "hit"), ("Read", "read"), ("Dirtied", "dirtied"), ("Written", "written")]),
        blocks("Local", &[("Hit", "hit"), ("Read", "read"), ("Dirtied", "dirtied"), ("Written", "written")]),
        blocks("Temp", &[("Read", "read"), ("Written", "written")]),
    ]
    .into_iter()
    .flatten()
    .collect();
    if !buffers.is_empty() {
        out.push(format!("Buffers: {}", buffers.join(", ")));
    }
    out
}

fn thousands(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Rows and counts: `1,234`, then `123k`, `1.2M`.
pub fn count(n: f64) -> String {
    let n = n.max(0.).round();
    if n < 100_000. {
        thousands(n as u64)
    } else if n < 1_000_000. {
        format!("{:.0}k", n / 1e3)
    } else if n < 1e9 {
        format!("{:.1}M", n / 1e6)
    } else {
        format!("{:.1}B", n / 1e9)
    }
}

fn cost(c: f64) -> String {
    if c < 10. {
        format!("{c:.2}")
    } else if c < 100. {
        format!("{c:.1}")
    } else {
        count(c)
    }
}

pub fn ms(t: f64) -> String {
    if t < 1. {
        format!("{t:.3}")
    } else if t < 10. {
        format!("{t:.2}")
    } else if t < 1000. {
        format!("{t:.1}")
    } else {
        thousands(t.round() as u64)
    }
}

/// The tree's cursor, folds and scroll.
#[derive(Default)]
pub struct PlanView {
    /// the result these belong to
    seq: u64,
    pub selected: usize,
    folded: HashSet<usize>,
    /// leaves whose details stay open (↵)
    pinned: HashSet<usize>,
    /// first visible row, as an index into `visible()`
    top: usize,
    /// rows that fit, from the last draw
    fit: usize,
    /// from the last draw: (node, top, bottom, chevron's left edge)
    hits: Vec<(usize, f32, f32, f32)>,
}

impl PlanView {
    pub fn sync(&mut self, seq: u64) {
        if self.seq != seq {
            *self = PlanView {
                seq,
                ..Default::default()
            };
        }
    }

    /// Nodes not hidden inside a folded one, in order.
    fn visible(&self, plan: &Plan) -> Vec<usize> {
        let mut out = vec![];
        let mut i = 0;
        while i < plan.nodes.len() {
            out.push(i);
            i = if self.folded.contains(&i) { plan.nodes[i].end } else { i + 1 };
        }
        out
    }

    fn shows_details(&self, i: usize, plan: &Plan) -> bool {
        let n = &plan.nodes[i];
        (i == self.selected || self.pinned.contains(&i)) && !(n.details.is_empty() && n.warnings.is_empty())
    }

    fn height(&self, i: usize, plan: &Plan) -> f32 {
        let n = &plan.nodes[i];
        if self.shows_details(i, plan) {
            ROW_H + (n.details.len() + n.warnings.len()) as f32 * DETAIL_H + DETAIL_PAD
        } else {
            ROW_H
        }
    }

    fn toggle(&mut self, i: usize, plan: &Plan) {
        let n = &plan.nodes[i];
        if n.has_children(i) {
            if !self.folded.remove(&i) {
                self.folded.insert(i);
                if (i + 1..n.end).contains(&self.selected) {
                    self.selected = i;
                }
            }
        } else if !self.pinned.remove(&i) {
            self.pinned.insert(i);
        }
    }

    /// `None` if the key isn't a plan key.
    pub fn key(&mut self, ks: &Keystroke, plan: &Plan) -> Option<Option<Action>> {
        if plan.nodes.is_empty() {
            return None;
        }
        let vis = self.visible(plan);
        let at = vis.iter().position(|i| *i == self.selected).unwrap_or(0);
        let sel = self.selected;
        let node = &plan.nodes[sel];
        let m = &ks.modifiers;
        if m.control || m.platform {
            return None;
        }
        match ks.key.as_str() {
            "j" | "down" => self.selected = vis[(at + 1).min(vis.len() - 1)],
            "k" | "up" => self.selected = vis[at.saturating_sub(1)],
            "g" if m.shift => self.selected = *vis.last().unwrap(),
            "g" => self.selected = 0,
            "h" | "left" => {
                if node.has_children(sel) && !self.folded.contains(&sel) {
                    self.folded.insert(sel);
                } else if let Some(p) = node.parent {
                    self.selected = p;
                }
            }
            "l" | "right" => {
                if self.folded.contains(&sel) {
                    self.folded.remove(&sel);
                } else if node.has_children(sel) {
                    self.selected = sel + 1;
                }
            }
            "enter" => self.toggle(sel, plan),
            "y" => return Some(Some(Action::Copy(plan.raw.clone(), "Copied the plan".into()))),
            "escape" | "i" => return Some(Some(Action::Editor)),
            _ => return None,
        }
        Some(None)
    }

    /// A click at `pos`, relative to the plan's top-left: selects the row;
    /// on its chevron (or a double-click) also folds or unfolds it.
    pub fn click(&mut self, pos: Point<Pixels>, clicks: usize, plan: &Plan) {
        let (x, y) = (f32::from(pos.x), f32::from(pos.y));
        let Some(&(i, _, _, cx)) = self.hits.iter().find(|(_, a, b, _)| y >= *a && y < *b) else {
            return;
        };
        self.selected = i;
        let on_chevron = (cx..cx + CHEVRON).contains(&x) && plan.nodes[i].has_children(i);
        if on_chevron || clicks == 2 {
            self.toggle(i, plan);
        }
    }

    /// The wheel: scroll by `lines` (up is positive), keeping the cursor in view.
    pub fn scroll(&mut self, lines: isize, plan: &Plan) {
        let vis = self.visible(plan);
        if vis.is_empty() {
            return;
        }
        self.top = (self.top as isize - lines).clamp(0, vis.len() as isize - 1) as usize;
        let at = vis.iter().position(|i| *i == self.selected).unwrap_or(0);
        let last = (self.top + self.fit.max(1) - 1).min(vis.len() - 1);
        self.selected = vis[at.clamp(self.top, last)];
    }

    pub fn render(&mut self, plan: &Plan, m: &Metrics, t: &Theme, focused: bool, size: Size<Pixels>) -> AnyElement {
        let vis = self.visible(plan);
        let avail = f32::from(size.height) - HEADER_H;
        let at = vis.iter().position(|i| *i == self.selected).unwrap_or(0);
        // keep the selected row (and its details) on screen
        self.top = self.top.min(at);
        while self.top < at && vis[self.top..=at].iter().map(|i| self.height(*i, plan)).sum::<f32>() > avail {
            self.top += 1;
        }

        // metadata columns: (header, values), right-aligned in mono
        let mut columns: Vec<(&str, Vec<(String, Hsla)>)> = vec![
            (
                if plan.analyzed { "est" } else { "rows" },
                plan.nodes.iter().map(|n| (count(n.est_rows), t.fg_subtle)).collect(),
            ),
            ("cost", plan.nodes.iter().map(|n| (cost(n.cost), t.fg_subtle)).collect()),
        ];
        if plan.analyzed {
            let actual = |f: &dyn Fn(&Node, &Actual) -> (String, Hsla)| {
                plan.nodes
                    .iter()
                    .map(|n| match &n.actual {
                        Some(a) if a.loops > 0. => f(n, a),
                        _ => ("–".to_string(), t.fg_subtle),
                    })
                    .collect::<Vec<_>>()
            };
            columns.push(("ms", actual(&|_, a| (ms(a.ms), t.fg_subtle))));
            columns.push((
                "rows",
                actual(&|n, a| (count(a.rows), if n.misestimate { t.warning } else { t.fg_subtle })),
            ));
            columns.push(("loops", actual(&|_, a| (count(a.loops), t.fg_subtle))));
        }
        let char_w = f32::from(m.cell_w) * 11. / f32::from(m.font_size).max(1.);
        let widths: Vec<f32> = columns
            .iter()
            .map(|(h, vals)| {
                let chars = vals.iter().map(|(s, _)| s.chars().count()).max().unwrap_or(0).max(h.len());
                (chars + 2) as f32 * char_w
            })
            .collect();
        let mono = m.font.family.clone();

        let header = div()
            .h(px(HEADER_H))
            .flex_none()
            .flex()
            .items_center()
            .pr(px(PAD))
            .border_b_1()
            .border_color(t.border)
            .text_size(px(11.))
            .text_color(t.fg_subtle)
            .child(div().flex_1())
            .child(div().w(px(BAR_W + 12.)).flex_none())
            .children(
                columns
                    .iter()
                    .zip(&widths)
                    .map(|((h, _), w)| div().w(px(*w)).flex_none().flex().justify_end().child(h.to_string())),
            );

        let bar = if focused { t.accent } else { with_alpha(t.fg_subtle, 0.5) };
        let mut hits = vec![];
        let mut y = HEADER_H;
        let mut rows = vec![];
        let mut fit = 0;
        for &i in &vis[self.top..] {
            if y >= HEADER_H + avail {
                break;
            }
            let n = &plan.nodes[i];
            let h = self.height(i, plan);
            let indent = PAD + n.depth as f32 * STEP;
            hits.push((i, y, y + h, indent));
            y += h;
            if y <= HEADER_H + avail {
                fit += 1;
            }
            let selected = i == self.selected;
            let chevron = n.has_children(i).then(|| {
                let which = if self.folded.contains(&i) { Icon::ChevronRight } else { Icon::ChevronDown };
                icon(which, 12., if selected { t.fg } else { t.fg_subtle })
            });
            let share = n.share.clamp(0., 1.);
            let meter = div()
                .ml(px(12.))
                .w(px(BAR_W))
                .h(px(4.))
                .flex_none()
                .rounded_sm()
                .bg(with_alpha(t.fg, 0.06))
                .child(
                    div()
                        .h_full()
                        .w(px(BAR_W * share))
                        .rounded_sm()
                        .bg(with_alpha(t.fg, 0.25 + 0.55 * share)),
                );
            let line = div()
                .h(px(ROW_H))
                .flex_none()
                .flex()
                .items_center()
                .pl(px(indent))
                .pr(px(PAD))
                .child(
                    div()
                        .w(px(CHEVRON))
                        .flex_none()
                        .flex()
                        .items_center()
                        .children(chevron),
                )
                .children((!n.warnings.is_empty()).then(|| {
                    div().mr(px(6.)).flex_none().child(icon(Icon::Alert, 12., t.warning))
                }))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .child(
                            div()
                                .min_w_0()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .text_color(t.fg)
                                .child(n.label.clone()),
                        )
                        .children(n.tag.as_ref().map(|tag| {
                            div()
                                .flex_none()
                                .text_size(px(12.))
                                .text_color(t.fg_subtle)
                                .whitespace_nowrap()
                                .child(tag.clone())
                        })),
                )
                .child(meter)
                .child(
                    div()
                        .flex_none()
                        .flex()
                        .font_family(mono.clone())
                        .text_size(px(11.))
                        .children(columns.iter().zip(&widths).map(|((_, vals), w)| {
                            let (s, c) = &vals[i];
                            div().w(px(*w)).flex_none().flex().justify_end().text_color(*c).child(s.clone())
                        })),
                );
            let details = self.shows_details(i, plan).then(|| {
                let lines = n
                    .warnings
                    .iter()
                    .map(|w| (w.clone(), t.warning))
                    .chain(n.details.iter().map(|d| (d.clone(), t.fg_muted)));
                div()
                    .pl(px(indent + CHEVRON))
                    .pr(px(PAD))
                    .pb(px(DETAIL_PAD))
                    .flex()
                    .flex_col()
                    .font_family(mono.clone())
                    .text_size(px(11.))
                    .children(lines.map(|(s, c)| {
                        div()
                            .h(px(DETAIL_H))
                            .flex()
                            .items_center()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_color(c)
                            .child(s)
                    }))
            });
            rows.push(
                div()
                    .relative()
                    .h(px(h))
                    .flex_none()
                    .flex()
                    .flex_col()
                    .when(selected, |d| {
                        d.bg(with_alpha(t.fg, if focused { 0.07 } else { 0.04 }))
                            .child(div().absolute().left_0().top_0().bottom_0().w(px(2.)).bg(bar))
                    })
                    .child(line)
                    .children(details),
            );
        }
        self.hits = hits;
        self.fit = fit.max(1);
        div()
            .size_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .text_size(px(13.))
            .child(header)
            .children(rows)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `explain (format json) select u.email, count(*) from users u join
    /// orders o on o.user_id = u.id where u.email like '%@example.com' group by 1`
    const PLAIN: &str = r#"[
  {
    "Plan": {
      "Node Type": "Aggregate",
      "Strategy": "Hashed",
      "Partial Mode": "Simple",
      "Parallel Aware": false,
      "Async Capable": false,
      "Startup Cost": 48210.55,
      "Total Cost": 48260.55,
      "Plan Rows": 5000,
      "Plan Width": 40,
      "Group Key": ["u.email"],
      "Plans": [
        {
          "Node Type": "Hash Join",
          "Parent Relationship": "Outer",
          "Parallel Aware": false,
          "Async Capable": false,
          "Join Type": "Inner",
          "Startup Cost": 21510.00,
          "Total Cost": 46985.55,
          "Plan Rows": 245000,
          "Plan Width": 32,
          "Inner Unique": true,
          "Hash Cond": "(o.user_id = u.id)",
          "Plans": [
            {
              "Node Type": "Seq Scan",
              "Parent Relationship": "Outer",
              "Parallel Aware": false,
              "Async Capable": false,
              "Relation Name": "orders",
              "Alias": "o",
              "Startup Cost": 0.00,
              "Total Cost": 18334.00,
              "Plan Rows": 1000000,
              "Plan Width": 8
            },
            {
              "Node Type": "Hash",
              "Parent Relationship": "Inner",
              "Parallel Aware": false,
              "Async Capable": false,
              "Startup Cost": 20834.00,
              "Total Cost": 20834.00,
              "Plan Rows": 54080,
              "Plan Width": 32,
              "Plans": [
                {
                  "Node Type": "Seq Scan",
                  "Parent Relationship": "Outer",
                  "Parallel Aware": false,
                  "Async Capable": false,
                  "Relation Name": "users",
                  "Alias": "u",
                  "Startup Cost": 0.00,
                  "Total Cost": 20834.00,
                  "Plan Rows": 54080,
                  "Plan Width": 32,
                  "Filter": "((email)::text ~~ '%@example.com'::text)"
                }
              ]
            }
          ]
        }
      ]
    }
  }
]"#;

    /// `explain (analyze, buffers, format json) select * from users where
    /// email = 'a@b.c' and created_at > now() - interval '1 day' order by id
    /// limit 10`, with a subquery in the select list
    const ANALYZED: &str = r#"[
  {
    "Plan": {
      "Node Type": "Limit",
      "Parallel Aware": false,
      "Async Capable": false,
      "Startup Cost": 8.45,
      "Total Cost": 8.46,
      "Plan Rows": 1,
      "Plan Width": 72,
      "Actual Startup Time": 152.310,
      "Actual Total Time": 152.315,
      "Actual Rows": 10,
      "Actual Loops": 1,
      "Shared Hit Blocks": 4,
      "Shared Read Blocks": 2210,
      "Plans": [
        {
          "Node Type": "Seq Scan",
          "Parent Relationship": "InitPlan",
          "Subplan Name": "InitPlan 1 (returns $0)",
          "Parallel Aware": false,
          "Async Capable": false,
          "Relation Name": "settings",
          "Alias": "settings",
          "Startup Cost": 0.00,
          "Total Cost": 1.01,
          "Plan Rows": 1,
          "Plan Width": 8,
          "Actual Startup Time": 0.010,
          "Actual Total Time": 0.011,
          "Actual Rows": 1,
          "Actual Loops": 1
        },
        {
          "Node Type": "Sort",
          "Parent Relationship": "Outer",
          "Parallel Aware": false,
          "Async Capable": false,
          "Startup Cost": 8.45,
          "Total Cost": 8.46,
          "Plan Rows": 1,
          "Plan Width": 72,
          "Actual Startup Time": 152.300,
          "Actual Total Time": 152.302,
          "Actual Rows": 10,
          "Actual Loops": 1,
          "Sort Key": ["users.id"],
          "Sort Method": "top-N heapsort",
          "Sort Space Used": 26,
          "Sort Space Type": "Memory",
          "Plans": [
            {
              "Node Type": "Seq Scan",
              "Parent Relationship": "Outer",
              "Parallel Aware": false,
              "Async Capable": false,
              "Relation Name": "users",
              "Alias": "users",
              "Startup Cost": 0.00,
              "Total Cost": 8.44,
              "Plan Rows": 1,
              "Plan Width": 72,
              "Actual Startup Time": 0.020,
              "Actual Total Time": 150.100,
              "Actual Rows": 1450,
              "Actual Loops": 1,
              "Filter": "((created_at > (now() - '1 day'::interval)) AND (email = 'a@b.c'::text))",
              "Rows Removed by Filter": 98550,
              "Shared Hit Blocks": 4,
              "Shared Read Blocks": 2210
            }
          ]
        },
        {
          "Node Type": "Index Scan",
          "Parent Relationship": "SubPlan",
          "Subplan Name": "SubPlan 2",
          "Parallel Aware": false,
          "Async Capable": false,
          "Scan Direction": "Forward",
          "Index Name": "orders_user_id_idx",
          "Relation Name": "orders",
          "Alias": "o",
          "Startup Cost": 0.42,
          "Total Cost": 8.44,
          "Plan Rows": 1,
          "Plan Width": 8,
          "Actual Startup Time": 0.003,
          "Actual Total Time": 0.004,
          "Actual Rows": 0,
          "Actual Loops": 0,
          "Index Cond": "(user_id = users.id)"
        }
      ]
    },
    "Planning": {
      "Shared Hit Blocks": 12,
      "Shared Read Blocks": 0
    },
    "Planning Time": 0.215,
    "Triggers": [],
    "Execution Time": 152.402
  }
]"#;

    #[test]
    fn plain_plan_tree_and_labels() {
        let p = parse(PLAIN).unwrap();
        assert!(!p.analyzed);
        assert_eq!(p.planning_ms, None);
        let labels: Vec<_> = p.nodes.iter().map(|n| (n.depth, n.label.as_str())).collect();
        assert_eq!(
            labels,
            vec![
                (0, "HashAggregate"),
                (1, "Hash Join"),
                (2, "Seq Scan on orders o"),
                (2, "Hash"),
                (3, "Seq Scan on users u"),
            ]
        );
        assert_eq!(p.nodes[0].end, 5);
        assert_eq!(p.nodes[3].end, 5);
        assert_eq!(p.nodes[4].parent, Some(3));
        assert_eq!(p.nodes[1].details, vec!["Hash Cond: (o.user_id = u.id)"]);
        assert_eq!(p.nodes[0].details, vec!["Group Key: u.email"]);
        // own cost: the users scan is the biggest single piece
        let biggest = (0..5).max_by(|a, b| p.nodes[*a].share.total_cmp(&p.nodes[*b].share)).unwrap();
        assert_eq!(biggest, 4);
        let sum: f32 = p.nodes.iter().map(|n| n.share).sum();
        assert!((sum - 1.).abs() < 1e-4);
        // the hash node costs nothing of its own
        assert_eq!(p.nodes[3].share, 0.);
    }

    #[test]
    fn analyzed_plan_times_and_warnings() {
        let p = parse(ANALYZED).unwrap();
        assert!(p.analyzed);
        assert_eq!(p.planning_ms, Some(0.215));
        assert_eq!(p.execution_ms, Some(152.402));
        let scan = &p.nodes[3];
        assert_eq!(scan.label, "Seq Scan on users");
        assert_eq!(scan.depth, 2);
        assert!(scan.misestimate);
        assert_eq!(
            scan.warnings,
            vec!["Estimated 1 row, got 1,450", "Filter removed 98,550 rows"]
        );
        assert!(scan.details.iter().any(|d| d == "Rows removed by filter: 98,550"));
        assert!(scan.details.iter().any(|d| d == "Buffers: shared hit=4 read=2210"));
        // the scan is where the time goes
        assert!(scan.share > 0.9, "{}", scan.share);
        let sort = &p.nodes[2];
        assert_eq!(sort.details, vec!["Sort Key: users.id", "Sort: top-N heapsort, 26 kB memory"]);
        assert!(sort.warnings.is_empty());
        assert_eq!(p.nodes[1].tag.as_deref(), Some("InitPlan 1 (returns $0)"));
        // never executed: no warning, no time
        let sub = &p.nodes[4];
        assert_eq!(sub.label, "Index Scan on orders o using orders_user_id_idx");
        assert!(sub.warnings.is_empty());
        assert_eq!(sub.actual.as_ref().unwrap().ms, 0.);
    }

    #[test]
    fn join_and_index_labels() {
        let v: Value = serde_json::from_str(
            r#"{"Node Type": "Nested Loop", "Join Type": "Left"}"#,
        )
        .unwrap();
        assert_eq!(label(&v), "Nested Loop Left Join");
        let v: Value = serde_json::from_str(r#"{"Node Type": "Hash Join", "Join Type": "Anti"}"#).unwrap();
        assert_eq!(label(&v), "Hash Anti Join");
        let v: Value = serde_json::from_str(
            r#"{"Node Type": "Bitmap Index Scan", "Index Name": "users_email_idx"}"#,
        )
        .unwrap();
        assert_eq!(label(&v), "Bitmap Index Scan on users_email_idx");
        let v: Value = serde_json::from_str(
            r#"{"Node Type": "Index Only Scan", "Scan Direction": "Backward", "Index Name": "users_pkey", "Relation Name": "users", "Alias": "users"}"#,
        )
        .unwrap();
        assert_eq!(label(&v), "Index Only Scan Backward on users using users_pkey");
        let v: Value = serde_json::from_str(
            r#"{"Node Type": "Aggregate", "Strategy": "Plain", "Partial Mode": "Finalize"}"#,
        )
        .unwrap();
        assert_eq!(label(&v), "Finalize Aggregate");
        let v: Value =
            serde_json::from_str(r#"{"Node Type": "ModifyTable", "Operation": "Update", "Relation Name": "users"}"#)
                .unwrap();
        assert_eq!(label(&v), "Update on users");
    }

    #[test]
    fn not_a_plan() {
        assert!(parse("Seq Scan on users  (cost=0.00..1.01 rows=1 width=8)").is_none());
        assert!(parse(r#"[{"a": 1}]"#).is_none());
        assert!(parse("[]").is_none());
    }

    #[test]
    fn folding_moves_and_hides() {
        let p = parse(PLAIN).unwrap();
        let mut v = PlanView::default();
        let key = |k: &str| Keystroke::parse(k).unwrap();
        assert_eq!(v.visible(&p), vec![0, 1, 2, 3, 4]);
        v.key(&key("shift-g"), &p);
        assert_eq!(v.selected, 4);
        v.key(&key("h"), &p); // a leaf: up to its parent
        assert_eq!(v.selected, 3);
        v.key(&key("h"), &p); // fold
        assert_eq!(v.visible(&p), vec![0, 1, 2, 3]);
        v.key(&key("l"), &p); // unfold
        v.key(&key("l"), &p); // into the first child
        assert_eq!(v.selected, 4);
        v.selected = 1;
        v.key(&key("enter"), &p);
        assert_eq!(v.visible(&p), vec![0, 1]);
        v.key(&key("j"), &p);
        assert_eq!(v.selected, 1);
        assert!(matches!(v.key(&key("y"), &p), Some(Some(Action::Copy(..)))));
        assert!(matches!(v.key(&key("escape"), &p), Some(Some(Action::Editor))));
    }

    #[test]
    fn numbers() {
        assert_eq!(count(1450.), "1,450");
        assert_eq!(count(98550.), "98,550");
        assert_eq!(count(245000.), "245k");
        assert_eq!(count(1_200_000.), "1.2M");
        assert_eq!(cost(8.46), "8.46");
        assert_eq!(cost(48260.55), "48,261");
        assert_eq!(ms(0.0114), "0.011");
        assert_eq!(ms(152.402), "152.4");
        assert_eq!(ms(12345.6), "12,346");
    }
}
