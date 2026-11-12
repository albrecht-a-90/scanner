// The sunburst: the current folder sits in the centre, every ring is the next level. The
// colour comes from the first-level branch, deeper rings are lighter. The centre describes
// the current folder, or the item under the mouse or picked in the folder panel.
// Summary works like the original Scanner: the drives share the circle by capacity, the
// centre is a pie of their used and free space, and the rings start with their folders.
use std::collections::{HashMap, HashSet};
use windows_sys::Win32::{Foundation::RECT, Graphics::Gdi::*};

use crate::{
    gfx::{self, Brush, G, Path},
    lang::{t, tf},
    scan::{Arena, NONE, ordered},
    theme::{Theme, blend},
    util,
};

pub const MIN_RINGS: u32 = 4; // the minimal depth; the setting is this or 0 for all levels
const MAX_RINGS: u32 = 12;
const MIN_SWEEP: f64 = 0.2; // degrees; narrower segments are not drawn

pub struct Seg {
    pub node: u32,     // NONE for an extra slice (free / not scanned)
    pub label: String, // extra slices: what the slice is
    pub size: u64,     // extra slices: its size
    free: bool,
    slot: i32,  // palette slot of the first-level branch; -1 is grey "other"
    depth: i32, // 0: an extra slice across all rings; -1: a wedge of the Summary centre
    a0: f64,    // degrees, clockwise from 12 o'clock
    a1: f64,
    path: Option<Path>, // built for the current size
}

pub struct Chart {
    segs: Vec<Seg>,
    top_slots: HashMap<u32, i32>,
    next_slot: i32,
    pub focus: u32,
    pub highlight: u32, // picked in the folder panel: it, its parents and its contents stay bright
    hover: Option<usize>,
    hover_center: bool,
    pub show_free: bool,
    pub rings: u32, // the setting; 0 shows every level there is
    shown: u32,     // rings actually drawn
    max_depth: u32,
    bounds: Vec<f64>,
    total: u64, // used + not scanned + free: what the angles add up to
    pub rect: RECT,
    built_for: RECT,
}

// Cumulative ring boundaries as a fraction of the ring area, 0 .. 1. Four rings are equally
// thick; with more (all levels) each gets thinner than the one inside it, up to 18% at twelve
// as the old chart had; past twelve the outermost ring keeps the same share of the innermost.
fn make_bounds(n: u32) -> Vec<f64> {
    let shrink = if n <= MAX_RINGS { 1.0 - 0.18 * (n - MIN_RINGS) as f64 / (MAX_RINGS - MIN_RINGS) as f64 } else { 0.82f64.powf(11.0 / (n - 1) as f64) };
    let (mut b, mut w, mut sum) = (vec![0.0; n as usize + 1], 1.0, 0.0);
    for i in 1..=n as usize {
        sum += w;
        b[i] = sum;
        w *= shrink;
    }
    b.iter().map(|v| v / sum).collect()
}

impl Chart {
    pub fn new() -> Chart {
        let r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        Chart {
            segs: Vec::new(),
            top_slots: HashMap::new(),
            next_slot: 0,
            focus: NONE,
            highlight: NONE,
            hover: None,
            hover_center: false,
            show_free: true,
            rings: MIN_RINGS,
            shown: MIN_RINGS,
            max_depth: 0,
            bounds: make_bounds(MIN_RINGS),
            total: 0,
            rect: r,
            built_for: r,
        }
    }

    pub fn show(&mut self, a: &Arena, focus: u32) {
        (self.focus, self.hover, self.hover_center, self.highlight) = (focus, None, false, NONE);
        self.rebuild(a);
    }

    pub fn hovered(&self) -> Option<&Seg> {
        self.hover.map(|i| &self.segs[i])
    }

    fn summary(&self, a: &Arena) -> bool {
        self.focus != NONE && a.node(self.focus).summary
    }

    // geometry follows the size, so the chart grows with the window
    fn geo(&self) -> (f32, f32, f32, f32) {
        let r = &self.rect;
        let (w, h) = ((r.right - r.left) as f32, (r.bottom - r.top) as f32);
        let d = w.min(h);
        let r0 = d * 0.165;
        (r.left as f32 + w / 2.0, r.top as f32 + h / 2.0, r0, (d / 2.0 - 2.0).max(r0 + 1.0))
    }

    fn ring_r(&self, i: i32) -> f32 {
        let (_, _, r0, outer) = self.geo();
        r0 + (outer - r0) * self.bounds[i as usize] as f32
    }

    fn rebuild(&mut self, a: &Arena) {
        self.segs.clear();
        self.top_slots.clear();
        (self.next_slot, self.total, self.max_depth) = (0, 0, 0);
        if self.focus != NONE && self.summary(a) {
            self.build_summary(a);
        } else if self.focus != NONE {
            let f = a.node(self.focus);
            // free space and what is used but was not scanned, only at the top of a drive
            let mut extras = Vec::new();
            if let (true, Some(&(free, total))) = (self.show_free, a.space.get(&self.focus)) {
                let hidden = total.saturating_sub(free + f.size);
                if hidden > 0 {
                    extras.push(extra(t("chart.hidden"), hidden, false));
                }
                extras.push(extra(t("chart.free"), free, true));
            }
            self.total = f.size + extras.iter().map(|x| x.size).sum::<u64>();
            if self.total > 0 {
                let span = 360.0 * f.size as f64 / self.total as f64;
                self.walk(a, self.focus, 0.0, span, 1, -1);
                let mut at = span;
                for mut x in extras {
                    x.a0 = at;
                    at += 360.0 * x.size as f64 / self.total as f64;
                    x.a1 = at;
                    self.segs.push(x);
                }
            }
        }
        self.shown = if self.rings > 0 { self.rings } else { self.max_depth.max(MIN_RINGS) };
        self.bounds = make_bounds(self.shown);
        self.built_for = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    }

    fn build_summary(&mut self, a: &Arena) {
        // with free space every drive takes its whole capacity, without it only what is used
        let show_free = self.show_free;
        let room = |d: u32| match (show_free, a.space.get(&d)) {
            (true, Some(&(free, total))) => total.max(a.node(d).size + free),
            _ => a.node(d).size,
        };
        self.total = a.node(self.focus).kids.iter().map(|&d| room(d)).sum();
        if self.total == 0 {
            return;
        }
        let whole_total = self.total;
        let deg = |v: u64| 360.0 * v as f64 / whole_total as f64;
        let mut at = 0.0;
        for (i, d) in ordered(a, self.focus).into_iter().enumerate() {
            let (used, whole) = (deg(a.node(d).size), deg(room(d)));
            self.segs.push(Seg { node: d, slot: i as i32, depth: -1, a0: at, a1: at + used, ..extra(String::new(), 0, false) });
            self.walk(a, d, at, at + used, 1, -1);
            if whole > used {
                // what is neither scanned nor free is shown apart from the free space
                let (free, total) = a.space.get(&d).copied().unwrap_or((0, 0));
                let hidden = total.saturating_sub(free + a.node(d).size);
                let name = a.path(d).trim_end_matches('\\').to_string();
                let h = deg(hidden);
                if h > 0.0 {
                    self.segs.push(Seg { depth: -1, a0: at + used, a1: at + used + h, ..extra(tf("chart.hidden.on", &[&name]), hidden, false) });
                }
                self.segs.push(Seg { depth: -1, a0: at + used + h, a1: at + whole, ..extra(tf("chart.free.on", &[&name]), free, true) });
            }
            at += whole;
        }
    }

    // Depth first like a recursion would go, but with a stack of its own: with every level
    // shown the tree can be thousands of folders deep.
    fn walk(&mut self, a: &Arena, n: u32, a0: f64, a1: f64, depth: i32, slot: i32) {
        struct Frame {
            kids: std::vec::IntoIter<u32>,
            k: f64, // degrees per byte
            at: f64,
            depth: i32,
            slot: i32,
        }
        let frame = |n: u32, a0: f64, a1: f64, depth: i32, slot: i32| {
            let size = a.node(n).size;
            (size > 0).then(|| Frame { kids: ordered(a, n).into_iter(), k: (a1 - a0) / size as f64, at: a0, depth, slot })
        };
        let mut stack: Vec<Frame> = frame(n, a0, a1, depth, slot).into_iter().collect();
        while let Some(f) = stack.last_mut() {
            let Some(c) = f.kids.next() else {
                stack.pop();
                continue;
            };
            let (at, sw, depth, slot) = (f.at, a.node(c).size as f64 * f.k, f.depth, f.slot);
            f.at += sw;
            if sw < MIN_SWEEP {
                continue;
            }
            // every visible first-level branch takes the next palette colour, so
            // neighbours differ; the branch keeps it all the way out
            let mut s = slot;
            if depth == 1 {
                s = self.next_slot % 12;
                self.next_slot += 1;
                self.top_slots.insert(c, s);
            }
            self.segs.push(Seg { node: c, slot: s, depth, a0: at, a1: at + sw, ..extra(String::new(), 0, false) });
            self.max_depth = self.max_depth.max(depth as u32);
            let cn = a.node(c);
            if cn.dir && !cn.kids.is_empty() && (self.rings == 0 || (depth as u32) < self.rings) {
                stack.extend(frame(c, at, at + sw, depth + 1, s));
            }
        }
    }

    // the outermost ring is 45% of the branch colour, the rings between are spread evenly
    fn shade(&self, t: &Theme, slot: i32, depth: i32) -> u32 {
        let c = if slot >= 0 { t.slots[slot as usize] } else { t.other };
        if depth > 1 { blend(c, t.back, 1.0 - 0.55 * (depth - 1) as f64 / (self.shown - 1) as f64) } else { c }
    }

    // the used part of a drive in the Summary centre: neutral, every other drive lighter
    fn pie_color(t: &Theme, slot: i32) -> u32 {
        blend(t.sys, t.center, if slot % 2 == 0 { 0.85 } else { 0.6 })
    }

    // the colour the chart gives an item, for the swatches of the folder panel
    pub fn color_of(&self, a: &Arena, n: u32, t: &Theme) -> u32 {
        if self.focus == NONE {
            return t.other;
        }
        // the path from the item up to its first-level ancestor; in Summary the drives
        // keep no parent, so they are found in the list
        let mut path = Vec::new();
        let mut p = n;
        while p != NONE && p != self.focus {
            path.push(p);
            p = a.node(p).parent;
        }
        let Some(&top) = path.last() else { return t.other };
        if a.node(top).parent != self.focus && !a.node(self.focus).kids.contains(&top) {
            return t.other;
        }
        if self.summary(a) {
            if path.len() == 1 {
                return Self::pie_color(t, ordered(a, self.focus).iter().position(|&d| d == top).unwrap_or(0) as i32);
            }
            path.pop();
        }
        let top = *path.last().unwrap();
        self.shade(t, *self.top_slots.get(&top).unwrap_or(&-1), path.len().min(self.shown as usize) as i32)
    }

    fn brush(&self, s: &Seg, t: &Theme, on: bool) -> Brush {
        let dim = if on { 1.0 } else { 0.3 };
        if s.free {
            return Brush::hatch(blend(t.hatch_line, t.back, dim), blend(t.hatch_back, t.back, dim));
        }
        let c = if s.node == NONE { t.sys } else if s.depth < 0 { Self::pie_color(t, s.slot) } else { self.shade(t, s.slot, s.depth) };
        Brush::solid(blend(c, t.back, dim))
    }

    fn shape(&self, s: &Seg) -> Path {
        let (cx, cy, r0, _) = self.geo();
        let (start, sweep) = (s.a0 as f32 - 90.0, (s.a1 - s.a0) as f32);
        match s.depth {
            -1 => Path::pie(cx, cy, r0 - 4.0, start, sweep),
            0 => Path::ring(cx, cy, r0 + 1.0, self.ring_r(self.shown as i32), start, sweep),
            d => Path::ring(cx, cy, self.ring_r(d - 1) + 1.0, self.ring_r(d), start, sweep),
        }
    }

    pub fn paint_shapes(&mut self, g: &G, a: &Arena, t: &Theme) {
        if self.focus == NONE {
            return;
        }
        let (cx, cy, r0, _) = self.geo();
        if r0 < 10.0 {
            return;
        }
        // the shapes depend on the size only, so they are kept between repaints
        if !gfx::same(&self.built_for, &self.rect) {
            for i in 0..self.segs.len() {
                self.segs[i].path = Some(self.shape(&self.segs[i]));
            }
            self.built_for = self.rect;
        }
        // highlighted: the chain from the hovered segment to the centre, or the item picked
        // in the folder panel (or a drive in the Summary centre) with everything inside it
        let hover = self.hovered();
        let picked = hover.map_or(self.highlight, |s| s.node);
        let inside = hover.is_none_or(|s| s.depth < 0);
        let mut chain = HashSet::new();
        let mut p = picked;
        while p != NONE && p != self.focus {
            chain.insert(p);
            p = a.node(p).parent;
        }
        // the centre goes first, so the Summary wedges sit under the text
        g.disc(cx, cy, r0 - 4.0, if self.hover_center && a.node(self.focus).parent != NONE { t.center_hot } else { t.center });
        for s in &self.segs {
            let on = match hover {
                Some(h) if h.node == NONE => std::ptr::eq(s, h),
                _ => picked == NONE || s.node != NONE && (chain.contains(&s.node) || inside && a.under(s.node, picked)),
            };
            let path = s.path.as_ref().unwrap();
            g.fill(path, &self.brush(s, t, on));
            g.outline(path, t.back);
        }
    }

    // what the centre says: a name over the size, up to two lines under it
    fn center_text(&self, a: &Arena) -> [String; 4] {
        if let Some(h) = self.hovered().filter(|h| h.node == NONE) {
            let of = if self.summary(a) { "chart.of.all" } else { "chart.of.drive" };
            return [h.label.clone(), util::size(h.size), tf(of, &[&util::pct(h.size, self.total)]), String::new()];
        }
        let n = self.hovered().map_or(if self.highlight != NONE { self.highlight } else { self.focus }, |h| h.node);
        let node = a.node(n);
        let files = if node.dir { tf("chart.files", &[&util::num(node.files)]) } else { t("chart.file") };
        let focus = a.node(self.focus);
        if n == self.focus && focus.summary {
            let used = if self.show_free { tf("chart.used", &[&util::size(focus.size)]) } else { t("chart.used.only") };
            return [t("chart.total"), util::size(if self.show_free { self.total } else { focus.size }), used, files];
        }
        if n == self.focus {
            return [a.label(n), util::size(node.size), files, String::new()];
        }
        [a.label(n), util::size(node.size), format!("{}%", util::pct(node.size, focus.size)), files]
    }

    pub fn paint_text(&self, hdc: HDC, a: &Arena, t: &Theme, icon_face: &str) {
        if self.focus == NONE {
            return;
        }
        let (cx, cy, r0, _) = self.geo();
        if r0 < 10.0 {
            return;
        }
        let [name, size, sub, sub2] = self.center_text(a);
        let (big, small, arrow) = (
            gfx::font("Segoe UI", (r0 * 0.28).max(8.0) as i32, true),
            gfx::font("Segoe UI", (r0 * 0.15).max(11.0) as i32, false),
            gfx::font(icon_face, (r0 * 0.14).max(10.0) as i32, false),
        );
        let (bh, sh) = (gfx::line_height(hdc, big), gfx::line_height(hdc, small));
        let (cx, cy, rc) = (cx as i32, cy as i32, r0 as i32 - 4);
        let w = (rc as f32 * 1.8) as i32;
        // over the Summary pie the small lines need full contrast too
        let muted = if a.node(self.focus).summary { t.text } else { t.muted };
        let center = gfx::LINE | DT_CENTER | DT_END_ELLIPSIS;
        gfx::text(hdc, &size, &gfx::rect(cx - rc, cy - bh / 2, 2 * rc, bh), t.text, big, center);
        gfx::text(hdc, &name, &gfx::rect(cx - w / 2, cy - bh / 2 - sh, w, sh), muted, small, center);
        gfx::text(hdc, &sub, &gfx::rect(cx - w / 2, cy + bh / 2, w, sh), muted, small, center);
        gfx::text(hdc, &sub2, &gfx::rect(cx - w / 2, cy + bh / 2 + sh, w, sh), muted, small, center);
        if a.node(self.focus).parent != NONE && (self.hover.is_none() || self.hover_center) {
            gfx::text(hdc, "\u{E74A}", &gfx::rect(cx - rc, cy - bh / 2 - 2 * sh - 2, 2 * rc, sh), muted, arrow, center);
        }
        unsafe {
            DeleteObject(big);
            DeleteObject(small);
            DeleteObject(arrow);
        }
    }

    // the segment at a point; `center` is the disc in the middle (outside Summary)
    fn hit(&self, a: &Arena, x: i32, y: i32) -> (Option<usize>, bool) {
        let (cx, cy, r0, _) = self.geo();
        let (dx, dy) = (x as f32 - cx, y as f32 - cy);
        let r = (dx * dx + dy * dy).sqrt();
        let mut deg = dx.atan2(-dy).to_degrees() as f64;
        if deg < 0.0 {
            deg += 360.0;
        }
        let at = |s: &Seg| deg >= s.a0 && deg < s.a1;
        if r < r0 - 4.0 {
            // in Summary the centre is the pie of the drives
            return if self.summary(a) { (self.segs.iter().position(|s| s.depth < 0 && at(s)), false) } else { (None, true) };
        }
        if r < r0 || r > self.ring_r(self.shown as i32) {
            return (None, false);
        }
        let mut depth = 1;
        while depth < self.shown as i32 && r >= self.ring_r(depth) {
            depth += 1;
        }
        (self.segs.iter().position(|s| at(s) && (s.depth == 0 || s.depth == depth)), false)
    }

    // the point under the mouse; true when what is under it changed
    pub fn mouse_move(&mut self, a: &Arena, x: i32, y: i32) -> bool {
        let (hover, center) = if self.focus == NONE { (None, false) } else { self.hit(a, x, y) };
        let changed = hover != self.hover || center != self.hover_center;
        (self.hover, self.hover_center) = (hover, center);
        changed
    }

    pub fn leave(&mut self) -> bool {
        let changed = self.hover.is_some() || self.hover_center;
        (self.hover, self.hover_center) = (None, false);
        changed
    }

    pub fn center_hot(&self) -> bool {
        self.hover_center
    }

    pub fn total(&self) -> u64 {
        self.total
    }
}

fn extra(label: String, size: u64, free: bool) -> Seg {
    Seg { node: NONE, label, size, free, slot: -1, depth: 0, a0: 0.0, a1: 0.0, path: None }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_level_of_a_deep_tree() {
        std::thread::Builder::new()
            .stack_size(1 << 20) // the window thread's
            .spawn(|| {
                let mut a = Arena::default();
                let mut n = a.add(&[b'C' as u16, b':' as u16, 0x5C], NONE, true);
                let root = n;
                for _ in 0..16_000 {
                    n = a.add(&[b'a' as u16], n, true);
                }
                a.propagate(n, 1, 1);
                let mut c = Chart::new();
                c.rings = 0;
                c.show_free = false;
                c.show(&a, root);
                assert_eq!(c.max_depth, 16_000);
                assert_eq!(c.segs.len(), 16_000);
            })
            .unwrap()
            .join()
            .unwrap();
    }

    #[test]
    fn first_level_colours_and_order() {
        let mut a = Arena::default();
        let root = a.add(&[b'C' as u16], NONE, true);
        for (i, size) in [30u64, 20, 10].into_iter().enumerate() {
            let d = a.add(&[b'a' as u16 + i as u16], root, true);
            let f = a.add(&[b'f' as u16], d, false);
            a.node_mut(f).size = size;
            a.propagate(f, size as i64, 1);
        }
        let mut c = Chart::new();
        c.show_free = false;
        c.show(&a, root);
        let order: Vec<(u32, i32, i32)> = c.segs.iter().map(|s| (s.node, s.depth, s.slot)).collect();
        // each folder (depth 1, own colour) is followed by its file (depth 2, same colour)
        assert_eq!(order, vec![(1, 1, 0), (2, 2, 0), (3, 1, 1), (4, 2, 1), (5, 1, 2), (6, 2, 2)]);
    }
}
