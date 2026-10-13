//! Rust port of the part of Clipper 6.4.2 (Angus Johnson, Boost Software
//! License 1.0) that `clipping` uses: `Clipper::Execute` on closed paths into a
//! `PolyTree`, with ReverseSolution, StrictlySimple and PreserveCollinear off.
//!
//! It exists for targets without a C++ toolchain (WASM); native builds keep
//! using clipper-sys. The port follows the C++ statement by statement,
//! including its integer and floating point expressions, so results match
//! clipper-sys exactly, vertex order included. Open paths, Z fill, the
//! StrictlySimple pass and ClipperOffset are left out. `std::sort` calls are
//! stable sorts here; MSVC and libstdc++ use insertion sort below 16-32
//! elements, which is stable, and worldgen clips two small polygons.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Pt { x: i64, y: i64 }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClipType { Intersection, Union, Difference, Xor }
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PolyType { #[default] Subject, Clip }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FillType { EvenOdd, NonZero, Positive, Negative }

const HORIZONTAL: f64 = -1.0E+40;
const UNASSIGNED: i32 = -1;
const LO_RANGE: i64 = 0x3FFFFFFF;
const HI_RANGE: i64 = 0x3FFFFFFFFFFFFFFF;
const LEFT: u8 = 1;
const RIGHT: u8 = 2;

/// A C++ `clipperException`; `ExecuteInternal` turns it into failure.
struct Failed;
type R<T> = Result<T, Failed>;

#[derive(Clone, Default)]
struct Edge {
    bot: Pt, curr: Pt, top: Pt, dx: f64, poly_typ: PolyType, side: u8,
    wind_delta: i32, wind_cnt: i32, wind_cnt2: i32, out_idx: i32,
    next: usize, prev: usize, next_in_lml: Option<usize>,
    next_in_ael: Option<usize>, prev_in_ael: Option<usize>, next_in_sel: Option<usize>, prev_in_sel: Option<usize>,
}

struct LocalMinimum { y: i64, left: Option<usize>, right: Option<usize> }
struct OutPt { idx: i32, pt: Pt, next: usize, prev: usize }
struct OutRec { idx: i32, is_hole: bool, first_left: Option<usize>, poly_nd: Option<usize>, pts: Option<usize>, bottom_pt: Option<usize> }
struct Join { op1: usize, op2: usize, off_pt: Pt }
struct IntersectNode { e1: usize, e2: usize, pt: Pt }

fn round(v: f64) -> i64 { if v < 0.0 { (v - 0.5) as i64 } else { (v + 0.5) as i64 } }

fn slopes_equal3(p1: Pt, p2: Pt, p3: Pt, full: bool) -> bool {
    if full { (p1.y - p2.y) as i128 * (p2.x - p3.x) as i128 == (p1.x - p2.x) as i128 * (p2.y - p3.y) as i128 }
    else { (p1.y - p2.y) * (p2.x - p3.x) == (p1.x - p2.x) * (p2.y - p3.y) }
}

fn slopes_equal4(p1: Pt, p2: Pt, p3: Pt, p4: Pt, full: bool) -> bool {
    if full { (p1.y - p2.y) as i128 * (p3.x - p4.x) as i128 == (p1.x - p2.x) as i128 * (p3.y - p4.y) as i128 }
    else { (p1.y - p2.y) * (p3.x - p4.x) == (p1.x - p2.x) * (p3.y - p4.y) }
}

fn get_dx(p1: Pt, p2: Pt) -> f64 { if p1.y == p2.y { HORIZONTAL } else { (p2.x - p1.x) as f64 / (p2.y - p1.y) as f64 } }

fn horz_segments_overlap(mut a1: i64, mut a2: i64, mut b1: i64, mut b2: i64) -> bool {
    if a1 > a2 { std::mem::swap(&mut a1, &mut a2); }
    if b1 > b2 { std::mem::swap(&mut b1, &mut b2); }
    a1 < b2 && b1 < a2
}

fn get_overlap(a1: i64, a2: i64, b1: i64, b2: i64) -> Option<(i64, i64)> {
    let (l, r) = if a1 < a2 {
        if b1 < b2 { (a1.max(b1), a2.min(b2)) } else { (a1.max(b2), a2.min(b1)) }
    } else if b1 < b2 { (a2.max(b1), a1.min(b2)) } else { (a2.max(b2), a1.min(b1)) };
    (l < r).then_some((l, r))
}

struct Clipper {
    e: Vec<Edge>,
    minima: Vec<LocalMinimum>,
    current_lm: usize,
    use_full_range: bool,
    scanbeam: std::collections::BinaryHeap<i64>,
    active_edges: Option<usize>,
    sorted_edges: Option<usize>,
    op: Vec<OutPt>,
    outs: Vec<OutRec>,
    joins: Vec<Join>,
    ghost_joins: Vec<Join>,
    intersects: Vec<IntersectNode>,
    clip_type: ClipType,
    subj_fill: FillType,
    clip_fill: FillType,
}

/// `Clipper.Execute(clipType, polyTree, subjFill, clipFill)` for one closed
/// subject and one closed clip path, as clipper-sys `execute` returns it: one
/// entry per outer contour (breadth first through the PolyTree), holding the
/// contour followed by its holes. Empty when Execute fails.
pub fn execute(clip_type: ClipType, subject: &[[i64; 2]], clip: &[[i64; 2]], subj_fill: FillType, clip_fill: FillType)
    -> Vec<Vec<Vec<[i64; 2]>>> {
    // A thread's earlier Clipper, emptied, keeps its buffers' capacity.
    thread_local! { static SPARE: std::cell::RefCell<Option<Clipper>> = const { std::cell::RefCell::new(None) }; }
    let mut c = match SPARE.with(|s| s.borrow_mut().take()) {
        Some(mut c) => {
            c.e.clear(); c.minima.clear(); c.scanbeam.clear(); c.op.clear(); c.outs.clear();
            c.joins.clear(); c.ghost_joins.clear(); c.intersects.clear();
            c.current_lm = 0; c.use_full_range = false; c.active_edges = None; c.sorted_edges = None;
            c.clip_type = clip_type; c.subj_fill = subj_fill; c.clip_fill = clip_fill;
            c
        }
        None => Clipper { e: Vec::new(), minima: Vec::new(), current_lm: 0, use_full_range: false, scanbeam: Default::default(),
            active_edges: None, sorted_edges: None, op: Vec::new(), outs: Vec::new(), joins: Vec::new(), ghost_joins: Vec::new(),
            intersects: Vec::new(), clip_type, subj_fill, clip_fill },
    };
    let pts = |p: &[[i64; 2]]| p.iter().map(|v| Pt { x: v[0], y: v[1] }).collect::<Vec<_>>();
    let mut result = Vec::new();
    if [(pts(subject), PolyType::Subject), (pts(clip), PolyType::Clip)].iter().all(|(path, typ)| c.add_path(path, *typ).is_ok()) && c.execute_internal() {
        result = c.build_result2();
    }
    SPARE.with(|s| *s.borrow_mut() = Some(c));
    result
}

impl Clipper {
    fn is_horizontal(&self, e: usize) -> bool { self.e[e].dx == HORIZONTAL }

    fn top_x(&self, e: usize, y: i64) -> i64 {
        let e = &self.e[e];
        if y == e.top.y { e.top.x } else { e.bot.x + round(e.dx * (y - e.bot.y) as f64) }
    }

    fn range_test(&mut self, p: Pt) -> R<()> {
        if self.use_full_range {
            if p.x > HI_RANGE || p.y > HI_RANGE || -p.x > HI_RANGE || -p.y > HI_RANGE { return Err(Failed); }
        } else if p.x > LO_RANGE || p.y > LO_RANGE || -p.x > LO_RANGE || -p.y > LO_RANGE {
            self.use_full_range = true;
            return self.range_test(p);
        }
        Ok(())
    }

    fn init_edge(&mut self, e: usize, next: usize, prev: usize, pt: Pt) {
        self.e[e] = Edge { next, prev, curr: pt, out_idx: UNASSIGNED, ..Edge::default() };
    }

    fn init_edge2(&mut self, e: usize, typ: PolyType) {
        let next_curr = self.e[self.e[e].next].curr;
        let ed = &mut self.e[e];
        if ed.curr.y >= next_curr.y { ed.bot = ed.curr; ed.top = next_curr; } else { ed.top = ed.curr; ed.bot = next_curr; }
        let dy = ed.top.y - ed.bot.y;
        ed.dx = if dy == 0 { HORIZONTAL } else { (ed.top.x - ed.bot.x) as f64 / dy as f64 };
        ed.poly_typ = typ;
    }

    fn remove_edge(&mut self, e: usize) -> usize {
        let (prev, next) = (self.e[e].prev, self.e[e].next);
        self.e[prev].next = next;
        self.e[next].prev = prev;
        next
    }

    fn reverse_horizontal(&mut self, e: usize) {
        let ed = &mut self.e[e];
        std::mem::swap(&mut ed.top.x, &mut ed.bot.x);
    }

    fn find_next_loc_min(&self, mut e: usize) -> usize {
        loop {
            while self.e[e].bot != self.e[self.e[e].prev].bot || self.e[e].curr == self.e[e].top { e = self.e[e].next; }
            if !self.is_horizontal(e) && !self.is_horizontal(self.e[e].prev) { break; }
            while self.is_horizontal(self.e[e].prev) { e = self.e[e].prev; }
            let e2 = e;
            while self.is_horizontal(e) { e = self.e[e].next; }
            if self.e[e].top.y == self.e[self.e[e].prev].bot.y { continue; }
            if self.e[self.e[e2].prev].bot.x < self.e[e].bot.x { e = e2; }
            break;
        }
        e
    }

    /// `ProcessBound` without the Skip-edge branches, which only open paths reach.
    fn process_bound(&mut self, mut e: usize, forward: bool) -> usize {
        let mut result = e;
        if self.is_horizontal(e) {
            let start = if forward { self.e[e].prev } else { self.e[e].next };
            if self.is_horizontal(start) {
                if self.e[start].bot.x != self.e[e].bot.x && self.e[start].top.x != self.e[e].bot.x { self.reverse_horizontal(e); }
            } else if self.e[start].bot.x != self.e[e].bot.x { self.reverse_horizontal(e); }
        }
        let start = e;
        if forward {
            while self.e[result].top.y == self.e[self.e[result].next].bot.y { result = self.e[result].next; }
            if self.is_horizontal(result) {
                let mut horz = result;
                while self.is_horizontal(self.e[horz].prev) { horz = self.e[horz].prev; }
                if self.e[self.e[horz].prev].top.x > self.e[self.e[result].next].top.x { result = self.e[horz].prev; }
            }
            while e != result {
                self.e[e].next_in_lml = Some(self.e[e].next);
                if self.is_horizontal(e) && e != start && self.e[e].bot.x != self.e[self.e[e].prev].top.x { self.reverse_horizontal(e); }
                e = self.e[e].next;
            }
            if self.is_horizontal(e) && e != start && self.e[e].bot.x != self.e[self.e[e].prev].top.x { self.reverse_horizontal(e); }
            result = self.e[result].next;
        } else {
            while self.e[result].top.y == self.e[self.e[result].prev].bot.y { result = self.e[result].prev; }
            if self.is_horizontal(result) {
                let mut horz = result;
                while self.is_horizontal(self.e[horz].next) { horz = self.e[horz].next; }
                let (hx, rx) = (self.e[self.e[horz].next].top.x, self.e[self.e[result].prev].top.x);
                if hx == rx || hx > rx { result = self.e[horz].next; }
            }
            while e != result {
                self.e[e].next_in_lml = Some(self.e[e].prev);
                if self.is_horizontal(e) && e != start && self.e[e].bot.x != self.e[self.e[e].next].top.x { self.reverse_horizontal(e); }
                e = self.e[e].prev;
            }
            if self.is_horizontal(e) && e != start && self.e[e].bot.x != self.e[self.e[e].next].top.x { self.reverse_horizontal(e); }
            result = self.e[result].prev;
        }
        result
    }

    /// `ClipperBase::AddPath` for a closed path.
    fn add_path(&mut self, pg: &[Pt], typ: PolyType) -> R<bool> {
        let mut high = pg.len() as isize - 1;
        while high > 0 && pg[high as usize] == pg[0] { high -= 1; }
        while high > 0 && pg[high as usize] == pg[high as usize - 1] { high -= 1; }
        if high < 2 { return Ok(false); }
        let high = high as usize;
        let base = self.e.len();
        self.e.resize(base + high + 1, Edge::default());
        self.e[base + 1].curr = pg[1];
        self.range_test(pg[0])?;
        self.range_test(pg[high])?;
        self.init_edge(base, base + 1, base + high, pg[0]);
        self.init_edge(base + high, base, base + high - 1, pg[high]);
        for i in (1..high).rev() {
            self.range_test(pg[i])?;
            self.init_edge(base + i, base + i + 1, base + i - 1, pg[i]);
        }
        // Remove duplicate vertices and collinear edges.
        let mut start = base;
        let mut e = start;
        let mut loop_stop = start;
        loop {
            if self.e[e].curr == self.e[self.e[e].next].curr {
                if e == self.e[e].next { break; }
                if e == start { start = self.e[e].next; }
                e = self.remove_edge(e);
                loop_stop = e;
                continue;
            }
            if self.e[e].prev == self.e[e].next { break; }
            if slopes_equal3(self.e[self.e[e].prev].curr, self.e[e].curr, self.e[self.e[e].next].curr, self.use_full_range) {
                if e == start { start = self.e[e].next; }
                e = self.remove_edge(e);
                e = self.e[e].prev;
                loop_stop = e;
                continue;
            }
            e = self.e[e].next;
            if e == loop_stop { break; }
        }
        if self.e[e].prev == self.e[e].next { return Ok(false); }
        let mut is_flat = true;
        e = start;
        loop {
            self.init_edge2(e, typ);
            e = self.e[e].next;
            if is_flat && self.e[e].curr.y != self.e[start].curr.y { is_flat = false; }
            if e == start { break; }
        }
        if is_flat { return Ok(false); }
        let mut e_min = None;
        if self.e[self.e[e].prev].bot == self.e[self.e[e].prev].top { e = self.e[e].next; }
        loop {
            e = self.find_next_loc_min(e);
            if Some(e) == e_min { break; }
            if e_min.is_none() { e_min = Some(e); }
            let y = self.e[e].bot.y;
            let (left, right, left_forward) =
                if self.e[e].dx < self.e[self.e[e].prev].dx { (self.e[e].prev, e, false) } else { (e, self.e[e].prev, true) };
            self.e[left].wind_delta = if self.e[left].next == right { -1 } else { 1 };
            self.e[right].wind_delta = -self.e[left].wind_delta;
            e = self.process_bound(left, left_forward);
            let e2 = self.process_bound(right, !left_forward);
            self.minima.push(LocalMinimum { y, left: Some(left), right: Some(right) });
            if !left_forward { e = e2; }
        }
        Ok(true)
    }

    fn reset(&mut self) {
        self.current_lm = 0;
        if self.minima.is_empty() { return; }
        self.minima.sort_by(|a, b| b.y.cmp(&a.y));
        self.scanbeam.clear();
        for i in 0..self.minima.len() {
            self.scanbeam.push(self.minima[i].y);
            for (e, side) in [(self.minima[i].left, LEFT), (self.minima[i].right, RIGHT)] {
                if let Some(e) = e {
                    let ed = &mut self.e[e];
                    ed.curr = ed.bot;
                    ed.side = side;
                    ed.out_idx = UNASSIGNED;
                }
            }
        }
        self.active_edges = None;
        self.current_lm = 0;
    }

    fn pop_scanbeam(&mut self) -> Option<i64> {
        let y = self.scanbeam.pop()?;
        while self.scanbeam.peek() == Some(&y) { self.scanbeam.pop(); }
        Some(y)
    }

    fn pop_local_minima(&mut self, y: i64) -> Option<(Option<usize>, Option<usize>)> {
        let lm = self.minima.get(self.current_lm).filter(|lm| lm.y == y)?;
        let bounds = (lm.left, lm.right);
        self.current_lm += 1;
        Some(bounds)
    }

    fn execute_internal(&mut self) -> bool {
        let succeeded = (|| -> R<bool> {
            self.reset();
            self.sorted_edges = None;
            let Some(mut bot_y) = self.pop_scanbeam() else { return Ok(false) };
            self.insert_local_minima_into_ael(bot_y)?;
            let mut top_y = bot_y;
            loop {
                match self.pop_scanbeam() {
                    Some(y) => top_y = y,
                    None if self.current_lm < self.minima.len() => {}
                    None => break,
                }
                self.process_horizontals()?;
                self.ghost_joins.clear();
                if !self.process_intersections(top_y)? { return Ok(false); }
                self.process_edges_at_top_of_scanbeam(top_y)?;
                bot_y = top_y;
                self.insert_local_minima_into_ael(bot_y)?;
            }
            Ok(true)
        })().unwrap_or(false);
        if succeeded {
            for i in 0..self.outs.len() {
                let Some(pts) = self.outs[i].pts else { continue };
                if self.outs[i].is_hole == (self.area(pts) > 0.0) { self.reverse_poly_pt_links(pts); }
            }
            if !self.joins.is_empty() { self.join_common_edges(); }
            for i in 0..self.outs.len() {
                if self.outs[i].pts.is_some() { self.fixup_out_polygon(i); }
            }
        }
        self.joins.clear();
        self.ghost_joins.clear();
        succeeded
    }

    fn set_winding_count(&mut self, edge: usize) {
        let typ = self.e[edge].poly_typ;
        let mut e = self.e[edge].prev_in_ael;
        while let Some(i) = e {
            if self.e[i].poly_typ == typ && self.e[i].wind_delta != 0 { break; }
            e = self.e[i].prev_in_ael;
        }
        let mut cursor;
        match e {
            None => {
                let wd = self.e[edge].wind_delta;
                self.e[edge].wind_cnt = if wd == 0 {
                    let pft = if typ == PolyType::Subject { self.subj_fill } else { self.clip_fill };
                    if pft == FillType::Negative { -1 } else { 1 }
                } else { wd };
                self.e[edge].wind_cnt2 = 0;
                cursor = self.active_edges;
            }
            Some(e) if self.e[edge].wind_delta == 0 && self.clip_type != ClipType::Union => {
                self.e[edge].wind_cnt = 1;
                self.e[edge].wind_cnt2 = self.e[e].wind_cnt2;
                cursor = self.e[e].next_in_ael;
            }
            Some(e) if self.is_even_odd_fill_type(edge) => {
                if self.e[edge].wind_delta == 0 {
                    let mut inside = true;
                    let mut e2 = self.e[e].prev_in_ael;
                    while let Some(i) = e2 {
                        if self.e[i].poly_typ == self.e[e].poly_typ && self.e[i].wind_delta != 0 { inside = !inside; }
                        e2 = self.e[i].prev_in_ael;
                    }
                    self.e[edge].wind_cnt = if inside { 0 } else { 1 };
                } else {
                    self.e[edge].wind_cnt = self.e[edge].wind_delta;
                }
                self.e[edge].wind_cnt2 = self.e[e].wind_cnt2;
                cursor = self.e[e].next_in_ael;
            }
            Some(e) => {
                let (ewc, ewd, wd) = (self.e[e].wind_cnt, self.e[e].wind_delta, self.e[edge].wind_delta);
                self.e[edge].wind_cnt = if ewc * ewd < 0 {
                    if ewc.abs() > 1 {
                        if ewd * wd < 0 { ewc } else { ewc + wd }
                    } else if wd == 0 { 1 } else { wd }
                } else if wd == 0 {
                    if ewc < 0 { ewc - 1 } else { ewc + 1 }
                } else if ewd * wd < 0 { ewc } else { ewc + wd };
                self.e[edge].wind_cnt2 = self.e[e].wind_cnt2;
                cursor = self.e[e].next_in_ael;
            }
        }
        if self.is_even_odd_alt_fill_type(edge) {
            while let Some(i) = cursor.filter(|&i| i != edge) {
                if self.e[i].wind_delta != 0 { self.e[edge].wind_cnt2 = if self.e[edge].wind_cnt2 == 0 { 1 } else { 0 }; }
                cursor = self.e[i].next_in_ael;
            }
        } else {
            while let Some(i) = cursor.filter(|&i| i != edge) {
                self.e[edge].wind_cnt2 += self.e[i].wind_delta;
                cursor = self.e[i].next_in_ael;
            }
        }
    }

    fn fills(&self, e: usize) -> (FillType, FillType) {
        if self.e[e].poly_typ == PolyType::Subject { (self.subj_fill, self.clip_fill) } else { (self.clip_fill, self.subj_fill) }
    }
    fn is_even_odd_fill_type(&self, e: usize) -> bool { self.fills(e).0 == FillType::EvenOdd }
    fn is_even_odd_alt_fill_type(&self, e: usize) -> bool { self.fills(e).1 == FillType::EvenOdd }

    fn is_contributing(&self, edge: usize) -> bool {
        let (pft, pft2) = self.fills(edge);
        let e = &self.e[edge];
        match pft {
            FillType::EvenOdd => if e.wind_delta == 0 && e.wind_cnt != 1 { return false; },
            FillType::NonZero => if e.wind_cnt.abs() != 1 { return false; },
            FillType::Positive => if e.wind_cnt != 1 { return false; },
            FillType::Negative => if e.wind_cnt != -1 { return false; },
        }
        let inside = |c: i32| match pft2 { FillType::EvenOdd | FillType::NonZero => c != 0, FillType::Positive => c > 0, FillType::Negative => c < 0 };
        let outside = |c: i32| match pft2 { FillType::EvenOdd | FillType::NonZero => c == 0, FillType::Positive => c <= 0, FillType::Negative => c >= 0 };
        match self.clip_type {
            ClipType::Intersection => inside(e.wind_cnt2),
            ClipType::Union => outside(e.wind_cnt2),
            ClipType::Difference => if e.poly_typ == PolyType::Subject { outside(e.wind_cnt2) } else { inside(e.wind_cnt2) },
            ClipType::Xor => if e.wind_delta == 0 { outside(e.wind_cnt2) } else { true },
        }
    }

    fn add_local_min_poly(&mut self, e1: usize, e2: usize, pt: Pt) -> usize {
        let (result, e, prev_e);
        if self.is_horizontal(e2) || self.e[e1].dx > self.e[e2].dx {
            result = self.add_out_pt(e1, pt);
            self.e[e2].out_idx = self.e[e1].out_idx;
            self.e[e1].side = LEFT;
            self.e[e2].side = RIGHT;
            e = e1;
            prev_e = if self.e[e].prev_in_ael == Some(e2) { self.e[e2].prev_in_ael } else { self.e[e].prev_in_ael };
        } else {
            result = self.add_out_pt(e2, pt);
            self.e[e1].out_idx = self.e[e2].out_idx;
            self.e[e1].side = RIGHT;
            self.e[e2].side = LEFT;
            e = e2;
            prev_e = if self.e[e].prev_in_ael == Some(e1) { self.e[e1].prev_in_ael } else { self.e[e].prev_in_ael };
        }
        if let Some(prev_e) = prev_e {
            if self.e[prev_e].out_idx >= 0 && self.e[prev_e].top.y < pt.y && self.e[e].top.y < pt.y {
                let x_prev = self.top_x(prev_e, pt.y);
                let x_e = self.top_x(e, pt.y);
                if x_prev == x_e && self.e[e].wind_delta != 0 && self.e[prev_e].wind_delta != 0
                    && slopes_equal4(Pt { x: x_prev, y: pt.y }, self.e[prev_e].top, Pt { x: x_e, y: pt.y }, self.e[e].top, self.use_full_range) {
                    let out_pt = self.add_out_pt(prev_e, pt);
                    let top = self.e[e].top;
                    self.joins.push(Join { op1: result, op2: out_pt, off_pt: top });
                }
            }
        }
        result
    }

    fn add_local_max_poly(&mut self, e1: usize, e2: usize, pt: Pt) {
        self.add_out_pt(e1, pt);
        if self.e[e2].wind_delta == 0 { self.add_out_pt(e2, pt); }
        if self.e[e1].out_idx == self.e[e2].out_idx {
            self.e[e1].out_idx = UNASSIGNED;
            self.e[e2].out_idx = UNASSIGNED;
        } else if self.e[e1].out_idx < self.e[e2].out_idx {
            self.append_polygon(e1, e2);
        } else {
            self.append_polygon(e2, e1);
        }
    }

    fn add_edge_to_sel(&mut self, edge: usize) {
        match self.sorted_edges {
            None => {
                self.sorted_edges = Some(edge);
                self.e[edge].prev_in_sel = None;
                self.e[edge].next_in_sel = None;
            }
            Some(s) => {
                self.e[edge].next_in_sel = Some(s);
                self.e[edge].prev_in_sel = None;
                self.e[s].prev_in_sel = Some(edge);
                self.sorted_edges = Some(edge);
            }
        }
    }

    fn copy_ael_to_sel(&mut self) {
        let mut e = self.active_edges;
        self.sorted_edges = e;
        while let Some(i) = e {
            self.e[i].prev_in_sel = self.e[i].prev_in_ael;
            self.e[i].next_in_sel = self.e[i].next_in_ael;
            e = self.e[i].next_in_ael;
        }
    }

    fn insert_local_minima_into_ael(&mut self, bot_y: i64) -> R<()> {
        while let Some((lb, rb)) = self.pop_local_minima(bot_y) {
            let mut op1 = None;
            match (lb, rb) {
                (None, Some(rb)) => {
                    self.insert_edge_into_ael(rb, None);
                    self.set_winding_count(rb);
                    if self.is_contributing(rb) { op1 = Some(self.add_out_pt(rb, self.e[rb].bot)); }
                }
                (Some(lb), None) => {
                    self.insert_edge_into_ael(lb, None);
                    self.set_winding_count(lb);
                    if self.is_contributing(lb) { op1 = Some(self.add_out_pt(lb, self.e[lb].bot)); }
                    self.scanbeam.push(self.e[lb].top.y);
                }
                (Some(lb), Some(rb)) => {
                    self.insert_edge_into_ael(lb, None);
                    self.insert_edge_into_ael(rb, Some(lb));
                    self.set_winding_count(lb);
                    self.e[rb].wind_cnt = self.e[lb].wind_cnt;
                    self.e[rb].wind_cnt2 = self.e[lb].wind_cnt2;
                    if self.is_contributing(lb) { op1 = Some(self.add_local_min_poly(lb, rb, self.e[lb].bot)); }
                    self.scanbeam.push(self.e[lb].top.y);
                }
                (None, None) => {}
            }
            if let Some(rb) = rb {
                if self.is_horizontal(rb) {
                    self.add_edge_to_sel(rb);
                    if let Some(n) = self.e[rb].next_in_lml { self.scanbeam.push(self.e[n].top.y); }
                } else {
                    self.scanbeam.push(self.e[rb].top.y);
                }
            }
            let (Some(lb), Some(rb)) = (lb, rb) else { continue };
            if let Some(op1) = op1 {
                if self.is_horizontal(rb) && !self.ghost_joins.is_empty() && self.e[rb].wind_delta != 0 {
                    for i in 0..self.ghost_joins.len() {
                        let (jop, joff) = (self.ghost_joins[i].op1, self.ghost_joins[i].off_pt);
                        if horz_segments_overlap(self.op[jop].pt.x, joff.x, self.e[rb].bot.x, self.e[rb].top.x) {
                            self.joins.push(Join { op1: jop, op2: op1, off_pt: joff });
                        }
                    }
                }
            }
            if self.e[lb].out_idx >= 0 {
                if let Some(p) = self.e[lb].prev_in_ael {
                    if self.e[p].curr.x == self.e[lb].bot.x && self.e[p].out_idx >= 0
                        && slopes_equal4(self.e[p].bot, self.e[p].top, self.e[lb].curr, self.e[lb].top, self.use_full_range)
                        && self.e[lb].wind_delta != 0 && self.e[p].wind_delta != 0 {
                        let op2 = self.add_out_pt(p, self.e[lb].bot);
                        let top = self.e[lb].top;
                        self.joins.push(Join { op1: op1.ok_or(Failed)?, op2, off_pt: top });
                    }
                }
            }
            if self.e[lb].next_in_ael != Some(rb) {
                let p = self.e[rb].prev_in_ael.ok_or(Failed)?;
                if self.e[rb].out_idx >= 0 && self.e[p].out_idx >= 0
                    && slopes_equal4(self.e[p].curr, self.e[p].top, self.e[rb].curr, self.e[rb].top, self.use_full_range)
                    && self.e[rb].wind_delta != 0 && self.e[p].wind_delta != 0 {
                    let op2 = self.add_out_pt(p, self.e[rb].bot);
                    let top = self.e[rb].top;
                    self.joins.push(Join { op1: op1.ok_or(Failed)?, op2, off_pt: top });
                }
                let mut e = self.e[lb].next_in_ael;
                while let Some(i) = e.filter(|&i| i != rb) {
                    // IntersectEdges assumes param1 is right of param2 above the intersection.
                    let curr = self.e[lb].curr;
                    self.intersect_edges(rb, i, curr);
                    e = self.e[i].next_in_ael;
                }
            }
        }
        Ok(())
    }

    fn delete_from_sel(&mut self, e: usize) {
        let (prev, next) = (self.e[e].prev_in_sel, self.e[e].next_in_sel);
        if prev.is_none() && next.is_none() && Some(e) != self.sorted_edges { return; }
        match prev { Some(p) => self.e[p].next_in_sel = next, None => self.sorted_edges = next }
        if let Some(n) = next { self.e[n].prev_in_sel = prev; }
        self.e[e].next_in_sel = None;
        self.e[e].prev_in_sel = None;
    }

    fn delete_from_ael(&mut self, e: usize) {
        let (prev, next) = (self.e[e].prev_in_ael, self.e[e].next_in_ael);
        if prev.is_none() && next.is_none() && Some(e) != self.active_edges { return; }
        match prev { Some(p) => self.e[p].next_in_ael = next, None => self.active_edges = next }
        if let Some(n) = next { self.e[n].prev_in_ael = prev; }
        self.e[e].next_in_ael = None;
        self.e[e].prev_in_ael = None;
    }

    fn intersect_edges(&mut self, e1: usize, e2: usize, pt: Pt) {
        let e1_contributing = self.e[e1].out_idx >= 0;
        let e2_contributing = self.e[e2].out_idx >= 0;
        // Closed paths only: the open-path branch never runs.
        if self.e[e1].poly_typ == self.e[e2].poly_typ {
            if self.is_even_odd_fill_type(e1) {
                let old = self.e[e1].wind_cnt;
                self.e[e1].wind_cnt = self.e[e2].wind_cnt;
                self.e[e2].wind_cnt = old;
            } else {
                let (wd1, wd2) = (self.e[e1].wind_delta, self.e[e2].wind_delta);
                if self.e[e1].wind_cnt + wd2 == 0 { self.e[e1].wind_cnt = -self.e[e1].wind_cnt; } else { self.e[e1].wind_cnt += wd2; }
                if self.e[e2].wind_cnt - wd1 == 0 { self.e[e2].wind_cnt = -self.e[e2].wind_cnt; } else { self.e[e2].wind_cnt -= wd1; }
            }
        } else {
            if !self.is_even_odd_fill_type(e2) { self.e[e1].wind_cnt2 += self.e[e2].wind_delta; }
            else { self.e[e1].wind_cnt2 = if self.e[e1].wind_cnt2 == 0 { 1 } else { 0 }; }
            if !self.is_even_odd_fill_type(e1) { self.e[e2].wind_cnt2 -= self.e[e1].wind_delta; }
            else { self.e[e2].wind_cnt2 = if self.e[e2].wind_cnt2 == 0 { 1 } else { 0 }; }
        }
        let ((f1, f1b), (f2, f2b)) = (self.fills(e1), self.fills(e2));
        let wc = |f: FillType, c: i32| -> i64 { match f { FillType::Positive => c as i64, FillType::Negative => -(c as i64), _ => (c as i64).abs() } };
        let e1_wc = wc(f1, self.e[e1].wind_cnt);
        let e2_wc = wc(f2, self.e[e2].wind_cnt);
        if e1_contributing && e2_contributing {
            if (e1_wc != 0 && e1_wc != 1) || (e2_wc != 0 && e2_wc != 1) || (self.e[e1].poly_typ != self.e[e2].poly_typ && self.clip_type != ClipType::Xor) {
                self.add_local_max_poly(e1, e2, pt);
            } else {
                self.add_out_pt(e1, pt);
                self.add_out_pt(e2, pt);
                self.swap_sides_and_indexes(e1, e2);
            }
        } else if e1_contributing {
            if e2_wc == 0 || e2_wc == 1 {
                self.add_out_pt(e1, pt);
                self.swap_sides_and_indexes(e1, e2);
            }
        } else if e2_contributing {
            if e1_wc == 0 || e1_wc == 1 {
                self.add_out_pt(e2, pt);
                self.swap_sides_and_indexes(e1, e2);
            }
        } else if (e1_wc == 0 || e1_wc == 1) && (e2_wc == 0 || e2_wc == 1) {
            let e1_wc2 = wc(f1b, self.e[e1].wind_cnt2);
            let e2_wc2 = wc(f2b, self.e[e2].wind_cnt2);
            if self.e[e1].poly_typ != self.e[e2].poly_typ {
                self.add_local_min_poly(e1, e2, pt);
            } else if e1_wc == 1 && e2_wc == 1 {
                let add = match self.clip_type {
                    ClipType::Intersection => e1_wc2 > 0 && e2_wc2 > 0,
                    ClipType::Union => e1_wc2 <= 0 && e2_wc2 <= 0,
                    ClipType::Difference => (self.e[e1].poly_typ == PolyType::Clip && e1_wc2 > 0 && e2_wc2 > 0)
                        || (self.e[e1].poly_typ == PolyType::Subject && e1_wc2 <= 0 && e2_wc2 <= 0),
                    ClipType::Xor => true,
                };
                if add { self.add_local_min_poly(e1, e2, pt); }
            } else {
                let s = self.e[e1].side;
                self.e[e1].side = self.e[e2].side;
                self.e[e2].side = s;
            }
        }
    }

    fn swap_sides_and_indexes(&mut self, e1: usize, e2: usize) {
        let (s, o) = (self.e[e1].side, self.e[e1].out_idx);
        self.e[e1].side = self.e[e2].side;
        self.e[e2].side = s;
        self.e[e1].out_idx = self.e[e2].out_idx;
        self.e[e2].out_idx = o;
    }

    fn set_hole_state(&mut self, e: usize, outrec: usize) {
        let mut e2 = self.e[e].prev_in_ael;
        let mut tmp: Option<usize> = None;
        while let Some(i) = e2 {
            if self.e[i].out_idx >= 0 && self.e[i].wind_delta != 0 {
                match tmp {
                    None => tmp = Some(i),
                    Some(t) if self.e[t].out_idx == self.e[i].out_idx => tmp = None,
                    _ => {}
                }
            }
            e2 = self.e[i].prev_in_ael;
        }
        match tmp {
            None => { self.outs[outrec].first_left = None; self.outs[outrec].is_hole = false; }
            Some(t) => {
                let fl = self.e[t].out_idx as usize;
                self.outs[outrec].first_left = Some(fl);
                self.outs[outrec].is_hole = !self.outs[fl].is_hole;
            }
        }
    }

    fn first_is_bottom_pt(&self, b1: usize, b2: usize) -> bool {
        let dx = |b: usize, forward: bool| {
            let step = |p: usize| if forward { self.op[p].next } else { self.op[p].prev };
            let mut p = step(b);
            while self.op[p].pt == self.op[b].pt && p != b { p = step(p); }
            get_dx(self.op[b].pt, self.op[p].pt).abs()
        };
        let (dx1p, dx1n, dx2p, dx2n) = (dx(b1, false), dx(b1, true), dx(b2, false), dx(b2, true));
        if dx1p.max(dx1n) == dx2p.max(dx2n) && dx1p.min(dx1n) == dx2p.min(dx2n) { self.area(b1) > 0.0 }
        else { (dx1p >= dx2p && dx1p >= dx2n) || (dx1n >= dx2p && dx1n >= dx2n) }
    }

    fn get_bottom_pt(&self, mut pp: usize) -> usize {
        let mut dups = None;
        let mut p = self.op[pp].next;
        while p != pp {
            if self.op[p].pt.y > self.op[pp].pt.y {
                pp = p;
                dups = None;
            } else if self.op[p].pt.y == self.op[pp].pt.y && self.op[p].pt.x <= self.op[pp].pt.x {
                if self.op[p].pt.x < self.op[pp].pt.x {
                    dups = None;
                    pp = p;
                } else if self.op[p].next != pp && self.op[p].prev != pp {
                    dups = Some(p);
                }
            }
            p = self.op[p].next;
        }
        if let Some(mut d) = dups {
            while d != p {
                if !self.first_is_bottom_pt(p, d) { pp = d; }
                d = self.op[d].next;
                while self.op[d].pt != self.op[pp].pt { d = self.op[d].next; }
            }
        }
        pp
    }

    fn get_lowermost_rec(&mut self, r1: usize, r2: usize) -> usize {
        if self.outs[r1].bottom_pt.is_none() { self.outs[r1].bottom_pt = Some(self.get_bottom_pt(self.outs[r1].pts.unwrap())); }
        if self.outs[r2].bottom_pt.is_none() { self.outs[r2].bottom_pt = Some(self.get_bottom_pt(self.outs[r2].pts.unwrap())); }
        let (o1, o2) = (self.outs[r1].bottom_pt.unwrap(), self.outs[r2].bottom_pt.unwrap());
        let (p1, p2) = (self.op[o1].pt, self.op[o2].pt);
        if p1.y > p2.y { r1 } else if p1.y < p2.y { r2 } else if p1.x < p2.x { r1 } else if p1.x > p2.x { r2 }
        else if self.op[o1].next == o1 { r2 } else if self.op[o2].next == o2 { r1 }
        else if self.first_is_bottom_pt(o1, o2) { r1 } else { r2 }
    }

    fn outrec1_right_of_outrec2(&self, mut r1: usize, r2: usize) -> bool {
        loop {
            match self.outs[r1].first_left {
                Some(fl) if fl == r2 => return true,
                Some(fl) => r1 = fl,
                None => return false,
            }
        }
    }

    fn get_out_rec(&self, idx: i32) -> usize {
        let mut r = idx as usize;
        while r != self.outs[r].idx as usize { r = self.outs[r].idx as usize; }
        r
    }

    fn append_polygon(&mut self, e1: usize, e2: usize) {
        let r1 = self.e[e1].out_idx as usize;
        let r2 = self.e[e2].out_idx as usize;
        let hole_state_rec = if self.outrec1_right_of_outrec2(r1, r2) { r2 }
            else if self.outrec1_right_of_outrec2(r2, r1) { r1 }
            else { self.get_lowermost_rec(r1, r2) };
        let p1_lft = self.outs[r1].pts.unwrap();
        let p1_rt = self.op[p1_lft].prev;
        let p2_lft = self.outs[r2].pts.unwrap();
        let p2_rt = self.op[p2_lft].prev;
        if self.e[e1].side == LEFT {
            if self.e[e2].side == LEFT {
                self.reverse_poly_pt_links(p2_lft);
                self.op[p2_lft].next = p1_lft;
                self.op[p1_lft].prev = p2_lft;
                self.op[p1_rt].next = p2_rt;
                self.op[p2_rt].prev = p1_rt;
                self.outs[r1].pts = Some(p2_rt);
            } else {
                self.op[p2_rt].next = p1_lft;
                self.op[p1_lft].prev = p2_rt;
                self.op[p2_lft].prev = p1_rt;
                self.op[p1_rt].next = p2_lft;
                self.outs[r1].pts = Some(p2_lft);
            }
        } else if self.e[e2].side == RIGHT {
            self.reverse_poly_pt_links(p2_lft);
            self.op[p1_rt].next = p2_rt;
            self.op[p2_rt].prev = p1_rt;
            self.op[p2_lft].next = p1_lft;
            self.op[p1_lft].prev = p2_lft;
        } else {
            self.op[p1_rt].next = p2_lft;
            self.op[p2_lft].prev = p1_rt;
            self.op[p1_lft].prev = p2_rt;
            self.op[p2_rt].next = p1_lft;
        }
        self.outs[r1].bottom_pt = None;
        if hole_state_rec == r2 {
            if self.outs[r2].first_left != Some(r1) { self.outs[r1].first_left = self.outs[r2].first_left; }
            self.outs[r1].is_hole = self.outs[r2].is_hole;
        }
        self.outs[r2].pts = None;
        self.outs[r2].bottom_pt = None;
        self.outs[r2].first_left = Some(r1);
        let ok_idx = self.e[e1].out_idx;
        let obsolete_idx = self.e[e2].out_idx;
        self.e[e1].out_idx = UNASSIGNED;
        self.e[e2].out_idx = UNASSIGNED;
        let mut e = self.active_edges;
        while let Some(i) = e {
            if self.e[i].out_idx == obsolete_idx {
                self.e[i].out_idx = ok_idx;
                self.e[i].side = self.e[e1].side;
                break;
            }
            e = self.e[i].next_in_ael;
        }
        self.outs[r2].idx = self.outs[r1].idx;
    }

    fn new_out_pt(&mut self, idx: i32, pt: Pt, next: usize, prev: usize) -> usize {
        self.op.push(OutPt { idx, pt, next, prev });
        self.op.len() - 1
    }

    fn add_out_pt(&mut self, e: usize, pt: Pt) -> usize {
        if self.e[e].out_idx < 0 {
            let idx = self.outs.len();
            self.outs.push(OutRec { idx: idx as i32, is_hole: false, first_left: None, poly_nd: None, pts: None, bottom_pt: None });
            let n = self.op.len();
            let new_op = self.new_out_pt(idx as i32, pt, n, n);
            self.outs[idx].pts = Some(new_op);
            self.set_hole_state(e, idx);
            self.e[e].out_idx = idx as i32;
            new_op
        } else {
            let r = self.e[e].out_idx as usize;
            let op = self.outs[r].pts.unwrap();
            let to_front = self.e[e].side == LEFT;
            if to_front && pt == self.op[op].pt { return op; }
            if !to_front && pt == self.op[self.op[op].prev].pt { return self.op[op].prev; }
            let prev = self.op[op].prev;
            let new_op = self.new_out_pt(self.outs[r].idx, pt, op, prev);
            self.op[prev].next = new_op;
            self.op[op].prev = new_op;
            if to_front { self.outs[r].pts = Some(new_op); }
            new_op
        }
    }

    fn get_last_out_pt(&self, e: usize) -> usize {
        let pts = self.outs[self.e[e].out_idx as usize].pts.unwrap();
        if self.e[e].side == LEFT { pts } else { self.op[pts].prev }
    }

    fn process_horizontals(&mut self) -> R<()> {
        while let Some(h) = self.sorted_edges {
            self.delete_from_sel(h);
            self.process_horizontal(h)?;
        }
        Ok(())
    }

    fn get_maxima_pair(&self, e: usize) -> Option<usize> {
        let (n, p) = (self.e[e].next, self.e[e].prev);
        if self.e[n].top == self.e[e].top && self.e[n].next_in_lml.is_none() { Some(n) }
        else if self.e[p].top == self.e[e].top && self.e[p].next_in_lml.is_none() { Some(p) }
        else { None }
    }

    fn get_maxima_pair_ex(&self, e: usize) -> Option<usize> {
        let r = self.get_maxima_pair(e)?;
        if self.e[r].next_in_ael == self.e[r].prev_in_ael && !self.is_horizontal(r) { None } else { Some(r) }
    }

    fn swap_positions_in_sel(&mut self, e1: usize, e2: usize) {
        if self.e[e1].next_in_sel.is_none() && self.e[e1].prev_in_sel.is_none() { return; }
        if self.e[e2].next_in_sel.is_none() && self.e[e2].prev_in_sel.is_none() { return; }
        if self.e[e1].next_in_sel == Some(e2) {
            let next = self.e[e2].next_in_sel;
            if let Some(n) = next { self.e[n].prev_in_sel = Some(e1); }
            let prev = self.e[e1].prev_in_sel;
            if let Some(p) = prev { self.e[p].next_in_sel = Some(e2); }
            self.e[e2].prev_in_sel = prev;
            self.e[e2].next_in_sel = Some(e1);
            self.e[e1].prev_in_sel = Some(e2);
            self.e[e1].next_in_sel = next;
        } else if self.e[e2].next_in_sel == Some(e1) {
            let next = self.e[e1].next_in_sel;
            if let Some(n) = next { self.e[n].prev_in_sel = Some(e2); }
            let prev = self.e[e2].prev_in_sel;
            if let Some(p) = prev { self.e[p].next_in_sel = Some(e1); }
            self.e[e1].prev_in_sel = prev;
            self.e[e1].next_in_sel = Some(e2);
            self.e[e2].prev_in_sel = Some(e1);
            self.e[e2].next_in_sel = next;
        } else {
            let next = self.e[e1].next_in_sel;
            let prev = self.e[e1].prev_in_sel;
            self.e[e1].next_in_sel = self.e[e2].next_in_sel;
            if let Some(n) = self.e[e1].next_in_sel { self.e[n].prev_in_sel = Some(e1); }
            self.e[e1].prev_in_sel = self.e[e2].prev_in_sel;
            if let Some(p) = self.e[e1].prev_in_sel { self.e[p].next_in_sel = Some(e1); }
            self.e[e2].next_in_sel = next;
            if let Some(n) = next { self.e[n].prev_in_sel = Some(e2); }
            self.e[e2].prev_in_sel = prev;
            if let Some(p) = prev { self.e[p].next_in_sel = Some(e2); }
        }
        if self.e[e1].prev_in_sel.is_none() { self.sorted_edges = Some(e1); }
        else if self.e[e2].prev_in_sel.is_none() { self.sorted_edges = Some(e2); }
    }

    fn swap_positions_in_ael(&mut self, e1: usize, e2: usize) {
        if self.e[e1].next_in_ael == self.e[e1].prev_in_ael || self.e[e2].next_in_ael == self.e[e2].prev_in_ael { return; }
        if self.e[e1].next_in_ael == Some(e2) {
            let next = self.e[e2].next_in_ael;
            if let Some(n) = next { self.e[n].prev_in_ael = Some(e1); }
            let prev = self.e[e1].prev_in_ael;
            if let Some(p) = prev { self.e[p].next_in_ael = Some(e2); }
            self.e[e2].prev_in_ael = prev;
            self.e[e2].next_in_ael = Some(e1);
            self.e[e1].prev_in_ael = Some(e2);
            self.e[e1].next_in_ael = next;
        } else if self.e[e2].next_in_ael == Some(e1) {
            let next = self.e[e1].next_in_ael;
            if let Some(n) = next { self.e[n].prev_in_ael = Some(e2); }
            let prev = self.e[e2].prev_in_ael;
            if let Some(p) = prev { self.e[p].next_in_ael = Some(e1); }
            self.e[e1].prev_in_ael = prev;
            self.e[e1].next_in_ael = Some(e2);
            self.e[e2].prev_in_ael = Some(e1);
            self.e[e2].next_in_ael = next;
        } else {
            let next = self.e[e1].next_in_ael;
            let prev = self.e[e1].prev_in_ael;
            self.e[e1].next_in_ael = self.e[e2].next_in_ael;
            if let Some(n) = self.e[e1].next_in_ael { self.e[n].prev_in_ael = Some(e1); }
            self.e[e1].prev_in_ael = self.e[e2].prev_in_ael;
            if let Some(p) = self.e[e1].prev_in_ael { self.e[p].next_in_ael = Some(e1); }
            self.e[e2].next_in_ael = next;
            if let Some(n) = next { self.e[n].prev_in_ael = Some(e2); }
            self.e[e2].prev_in_ael = prev;
            if let Some(p) = prev { self.e[p].next_in_ael = Some(e2); }
        }
        if self.e[e1].prev_in_ael.is_none() { self.active_edges = Some(e1); }
        else if self.e[e2].prev_in_ael.is_none() { self.active_edges = Some(e2); }
    }

    fn update_edge_into_ael(&mut self, e: usize) -> R<usize> {
        let n = self.e[e].next_in_lml.ok_or(Failed)?;
        self.e[n].out_idx = self.e[e].out_idx;
        let (prev, next) = (self.e[e].prev_in_ael, self.e[e].next_in_ael);
        match prev { Some(p) => self.e[p].next_in_ael = Some(n), None => self.active_edges = Some(n) }
        if let Some(x) = next { self.e[x].prev_in_ael = Some(n); }
        self.e[n].side = self.e[e].side;
        self.e[n].wind_delta = self.e[e].wind_delta;
        self.e[n].wind_cnt = self.e[e].wind_cnt;
        self.e[n].wind_cnt2 = self.e[e].wind_cnt2;
        self.e[n].curr = self.e[n].bot;
        self.e[n].prev_in_ael = prev;
        self.e[n].next_in_ael = next;
        if !self.is_horizontal(n) { self.scanbeam.push(self.e[n].top.y); }
        Ok(n)
    }

    fn horz_direction(&self, e: usize) -> (bool, i64, i64) {
        let (b, t) = (self.e[e].bot.x, self.e[e].top.x);
        if b < t { (true, b, t) } else { (false, t, b) }
    }

    fn horz_joins(&mut self, horz: usize, op1: usize) {
        let mut next_horz = self.sorted_edges;
        while let Some(n) = next_horz {
            if self.e[n].out_idx >= 0 && horz_segments_overlap(self.e[horz].bot.x, self.e[horz].top.x, self.e[n].bot.x, self.e[n].top.x) {
                let op2 = self.get_last_out_pt(n);
                let top = self.e[n].top;
                self.joins.push(Join { op1: op2, op2: op1, off_pt: top });
            }
            next_horz = self.e[n].next_in_sel;
        }
    }

    /// `ProcessHorizontal` without the StrictlySimple maxima list (always empty here).
    fn process_horizontal(&mut self, mut horz: usize) -> R<()> {
        let (mut left_to_right, mut horz_left, mut horz_right) = self.horz_direction(horz);
        let mut last_horz = horz;
        let mut max_pair = None;
        while let Some(n) = self.e[last_horz].next_in_lml.filter(|&n| self.is_horizontal(n)) { last_horz = n; }
        if self.e[last_horz].next_in_lml.is_none() { max_pair = self.get_maxima_pair(last_horz); }
        let mut op1 = None;
        loop {
            let is_last_horz = horz == last_horz;
            let step = |c: &Self, e: usize| if left_to_right { c.e[e].next_in_ael } else { c.e[e].prev_in_ael };
            let mut e = step(self, horz);
            while let Some(ei) = e {
                if (left_to_right && self.e[ei].curr.x > horz_right) || (!left_to_right && self.e[ei].curr.x < horz_left) { break; }
                // Also stop at the end of an intermediate horizontal edge.
                if self.e[ei].curr.x == self.e[horz].top.x {
                    if let Some(n) = self.e[horz].next_in_lml { if self.e[ei].dx < self.e[n].dx { break; } }
                }
                if self.e[horz].out_idx >= 0 {
                    let o = self.add_out_pt(horz, self.e[ei].curr);
                    op1 = Some(o);
                    self.horz_joins(horz, o);
                    let bot = self.e[horz].bot;
                    self.ghost_joins.push(Join { op1: o, op2: usize::MAX, off_pt: bot });
                }
                if Some(ei) == max_pair && is_last_horz {
                    if self.e[horz].out_idx >= 0 { self.add_local_max_poly(horz, ei, self.e[horz].top); }
                    self.delete_from_ael(horz);
                    self.delete_from_ael(ei);
                    return Ok(());
                }
                let pt = Pt { x: self.e[ei].curr.x, y: self.e[horz].curr.y };
                if left_to_right { self.intersect_edges(horz, ei, pt); } else { self.intersect_edges(ei, horz, pt); }
                let next = step(self, ei);
                self.swap_positions_in_ael(horz, ei);
                e = next;
            }
            match self.e[horz].next_in_lml {
                Some(n) if self.is_horizontal(n) => {}
                _ => break,
            }
            horz = self.update_edge_into_ael(horz)?;
            if self.e[horz].out_idx >= 0 { self.add_out_pt(horz, self.e[horz].bot); }
            (left_to_right, horz_left, horz_right) = self.horz_direction(horz);
        }
        if self.e[horz].out_idx >= 0 && op1.is_none() {
            let o = self.get_last_out_pt(horz);
            self.horz_joins(horz, o);
            let top = self.e[horz].top;
            self.ghost_joins.push(Join { op1: o, op2: usize::MAX, off_pt: top });
        }
        if self.e[horz].next_in_lml.is_some() {
            if self.e[horz].out_idx >= 0 {
                let o = self.add_out_pt(horz, self.e[horz].top);
                horz = self.update_edge_into_ael(horz)?;
                if self.e[horz].wind_delta == 0 { return Ok(()); }
                // horz is no longer horizontal here.
                let (prev, next) = (self.e[horz].prev_in_ael, self.e[horz].next_in_ael);
                let (bot, top) = (self.e[horz].bot, self.e[horz].top);
                let joinable = |c: &Self, x: usize| c.e[x].curr.x == bot.x && c.e[x].curr.y == bot.y && c.e[x].wind_delta != 0
                    && c.e[x].out_idx >= 0 && c.e[x].curr.y > c.e[x].top.y && c.slopes_equal_edges(horz, x);
                if let Some(p) = prev.filter(|&p| joinable(self, p)) {
                    let op2 = self.add_out_pt(p, bot);
                    self.joins.push(Join { op1: o, op2, off_pt: top });
                } else if let Some(n) = next.filter(|&n| joinable(self, n)) {
                    let op2 = self.add_out_pt(n, bot);
                    self.joins.push(Join { op1: o, op2, off_pt: top });
                }
            } else {
                self.update_edge_into_ael(horz)?;
            }
        } else {
            if self.e[horz].out_idx >= 0 { self.add_out_pt(horz, self.e[horz].top); }
            self.delete_from_ael(horz);
        }
        Ok(())
    }

    /// `SlopesEqual(TEdge&, TEdge&)`.
    fn slopes_equal_edges(&self, a: usize, b: usize) -> bool {
        let (e1, e2) = (&self.e[a], &self.e[b]);
        if self.use_full_range {
            (e1.top.y - e1.bot.y) as i128 * (e2.top.x - e2.bot.x) as i128 == (e1.top.x - e1.bot.x) as i128 * (e2.top.y - e2.bot.y) as i128
        } else {
            (e1.top.y - e1.bot.y) * (e2.top.x - e2.bot.x) == (e1.top.x - e1.bot.x) * (e2.top.y - e2.bot.y)
        }
    }

    fn process_intersections(&mut self, top_y: i64) -> R<bool> {
        if self.active_edges.is_none() { return Ok(true); }
        self.build_intersect_list(top_y);
        let n = self.intersects.len();
        if n == 0 { return Ok(true); }
        if n == 1 || self.fixup_intersection_order() {
            self.process_intersect_list();
        } else {
            return Ok(false);
        }
        self.sorted_edges = None;
        Ok(true)
    }

    fn intersect_point(&self, a: usize, b: usize) -> Pt {
        let (e1, e2) = (&self.e[a], &self.e[b]);
        let mut ip = Pt::default();
        if e1.dx == e2.dx {
            ip.y = e1.curr.y;
            ip.x = self.top_x(a, ip.y);
            return ip;
        } else if e1.dx == 0.0 {
            ip.x = e1.bot.x;
            if e2.dx == HORIZONTAL { ip.y = e2.bot.y; } else {
                let b2 = e2.bot.y as f64 - (e2.bot.x as f64 / e2.dx);
                ip.y = round(ip.x as f64 / e2.dx + b2);
            }
        } else if e2.dx == 0.0 {
            ip.x = e2.bot.x;
            if e1.dx == HORIZONTAL { ip.y = e1.bot.y; } else {
                let b1 = e1.bot.y as f64 - (e1.bot.x as f64 / e1.dx);
                ip.y = round(ip.x as f64 / e1.dx + b1);
            }
        } else {
            let b1 = e1.bot.x as f64 - e1.bot.y as f64 * e1.dx;
            let b2 = e2.bot.x as f64 - e2.bot.y as f64 * e2.dx;
            let q = (b2 - b1) / (e1.dx - e2.dx);
            ip.y = round(q);
            ip.x = if e1.dx.abs() < e2.dx.abs() { round(e1.dx * q + b1) } else { round(e2.dx * q + b2) };
        }
        if ip.y < e1.top.y || ip.y < e2.top.y {
            ip.y = if e1.top.y > e2.top.y { e1.top.y } else { e2.top.y };
            ip.x = if e1.dx.abs() < e2.dx.abs() { self.top_x(a, ip.y) } else { self.top_x(b, ip.y) };
        }
        // Don't allow ip below curr.y (the bottom of the scanbeam).
        if ip.y > e1.curr.y {
            ip.y = e1.curr.y;
            ip.x = if e1.dx.abs() > e2.dx.abs() { self.top_x(b, ip.y) } else { self.top_x(a, ip.y) };
        }
        ip
    }

    fn build_intersect_list(&mut self, top_y: i64) {
        let Some(first) = self.active_edges else { return };
        let mut e = Some(first);
        self.sorted_edges = e;
        while let Some(i) = e {
            self.e[i].prev_in_sel = self.e[i].prev_in_ael;
            self.e[i].next_in_sel = self.e[i].next_in_ael;
            self.e[i].curr.x = self.top_x(i, top_y);
            e = self.e[i].next_in_ael;
        }
        loop {
            let mut modified = false;
            let mut e = self.sorted_edges.unwrap();
            while let Some(next) = self.e[e].next_in_sel {
                if self.e[e].curr.x > self.e[next].curr.x {
                    let mut pt = self.intersect_point(e, next);
                    if pt.y < top_y { pt = Pt { x: self.top_x(e, top_y), y: top_y }; }
                    self.intersects.push(IntersectNode { e1: e, e2: next, pt });
                    self.swap_positions_in_sel(e, next);
                    modified = true;
                } else {
                    e = next;
                }
            }
            match self.e[e].prev_in_sel { Some(p) => self.e[p].next_in_sel = None, None => break }
            if !modified { break; }
        }
        self.sorted_edges = None;
    }

    fn process_intersect_list(&mut self) {
        for node in std::mem::take(&mut self.intersects) {
            self.intersect_edges(node.e1, node.e2, node.pt);
            self.swap_positions_in_ael(node.e1, node.e2);
        }
    }

    fn edges_adjacent(&self, n: &IntersectNode) -> bool {
        self.e[n.e1].next_in_sel == Some(n.e2) || self.e[n.e1].prev_in_sel == Some(n.e2)
    }

    fn fixup_intersection_order(&mut self) -> bool {
        self.copy_ael_to_sel();
        self.intersects.sort_by(|a, b| b.pt.y.cmp(&a.pt.y));
        let n = self.intersects.len();
        for i in 0..n {
            if !self.edges_adjacent(&self.intersects[i]) {
                let mut j = i + 1;
                while j < n && !self.edges_adjacent(&self.intersects[j]) { j += 1; }
                if j == n { return false; }
                self.intersects.swap(i, j);
            }
            let (e1, e2) = (self.intersects[i].e1, self.intersects[i].e2);
            self.swap_positions_in_sel(e1, e2);
        }
        true
    }

    fn do_maxima(&mut self, e: usize) -> R<()> {
        let Some(max_pair) = self.get_maxima_pair_ex(e) else {
            if self.e[e].out_idx >= 0 { self.add_out_pt(e, self.e[e].top); }
            self.delete_from_ael(e);
            return Ok(());
        };
        let mut next = self.e[e].next_in_ael;
        while let Some(n) = next.filter(|&n| n != max_pair) {
            let top = self.e[e].top;
            self.intersect_edges(e, n, top);
            self.swap_positions_in_ael(e, n);
            next = self.e[e].next_in_ael;
        }
        if self.e[e].out_idx == UNASSIGNED && self.e[max_pair].out_idx == UNASSIGNED {
            self.delete_from_ael(e);
            self.delete_from_ael(max_pair);
        } else if self.e[e].out_idx >= 0 && self.e[max_pair].out_idx >= 0 {
            self.add_local_max_poly(e, max_pair, self.e[e].top);
            self.delete_from_ael(e);
            self.delete_from_ael(max_pair);
        } else {
            return Err(Failed);
        }
        Ok(())
    }

    fn is_intermediate(&self, e: usize, y: i64) -> bool { self.e[e].top.y == y && self.e[e].next_in_lml.is_some() }

    fn process_edges_at_top_of_scanbeam(&mut self, top_y: i64) -> R<()> {
        let mut e = self.active_edges;
        while let Some(ei) = e {
            // Maxima are treated as bent horizontal edges, excluding maxima with horizontals.
            let mut is_maxima = self.e[ei].top.y == top_y && self.e[ei].next_in_lml.is_none();
            if is_maxima {
                let pair = self.get_maxima_pair_ex(ei);
                is_maxima = pair.is_none_or(|p| !self.is_horizontal(p));
            }
            if is_maxima {
                let prev = self.e[ei].prev_in_ael;
                self.do_maxima(ei)?;
                e = match prev { None => self.active_edges, Some(p) => self.e[p].next_in_ael };
            } else {
                let mut ei = ei;
                if self.is_intermediate(ei, top_y) && self.is_horizontal(self.e[ei].next_in_lml.unwrap()) {
                    ei = self.update_edge_into_ael(ei)?;
                    if self.e[ei].out_idx >= 0 { self.add_out_pt(ei, self.e[ei].bot); }
                    self.add_edge_to_sel(ei);
                } else {
                    self.e[ei].curr.x = self.top_x(ei, top_y);
                    self.e[ei].curr.y = top_y;
                }
                e = self.e[ei].next_in_ael;
            }
        }
        self.process_horizontals()?;
        // Promote intermediate vertices.
        let mut e = self.active_edges;
        while let Some(mut ei) = e {
            if self.is_intermediate(ei, top_y) {
                let op = if self.e[ei].out_idx >= 0 { Some(self.add_out_pt(ei, self.e[ei].top)) } else { None };
                ei = self.update_edge_into_ael(ei)?;
                // Output polygons that share an edge are joined later.
                let (prev, next) = (self.e[ei].prev_in_ael, self.e[ei].next_in_ael);
                let joinable = |c: &Self, x: usize| c.e[x].curr.x == c.e[ei].bot.x && c.e[x].curr.y == c.e[ei].bot.y && op.is_some()
                    && c.e[x].out_idx >= 0 && c.e[x].curr.y > c.e[x].top.y
                    && slopes_equal4(c.e[ei].curr, c.e[ei].top, c.e[x].curr, c.e[x].top, c.use_full_range)
                    && c.e[ei].wind_delta != 0 && c.e[x].wind_delta != 0;
                if let Some(p) = prev.filter(|&p| joinable(self, p)) {
                    let op2 = self.add_out_pt(p, self.e[ei].bot);
                    let top = self.e[ei].top;
                    self.joins.push(Join { op1: op.unwrap(), op2, off_pt: top });
                } else if let Some(n) = next.filter(|&n| joinable(self, n)) {
                    let op2 = self.add_out_pt(n, self.e[ei].bot);
                    let top = self.e[ei].top;
                    self.joins.push(Join { op1: op.unwrap(), op2, off_pt: top });
                }
            }
            e = self.e[ei].next_in_ael;
        }
        Ok(())
    }

    fn fixup_out_polygon(&mut self, r: usize) {
        let mut last_ok = None;
        self.outs[r].bottom_pt = None;
        let mut pp = self.outs[r].pts.unwrap();
        loop {
            let (prev, next) = (self.op[pp].prev, self.op[pp].next);
            if prev == pp || prev == next {
                self.outs[r].pts = None;
                return;
            }
            let (p, a, b) = (self.op[prev].pt, self.op[pp].pt, self.op[next].pt);
            if a == b || a == p || slopes_equal3(p, a, b, self.use_full_range) {
                last_ok = None;
                self.op[prev].next = next;
                self.op[next].prev = prev;
                pp = prev;
            } else if Some(pp) == last_ok {
                break;
            } else {
                if last_ok.is_none() { last_ok = Some(pp); }
                pp = next;
            }
        }
        self.outs[r].pts = Some(pp);
    }

    fn point_count(&self, pts: Option<usize>) -> usize {
        let Some(start) = pts else { return 0 };
        let mut n = 0;
        let mut p = start;
        loop {
            n += 1;
            p = self.op[p].next;
            if p == start { return n; }
        }
    }

    fn fix_hole_linkage(&mut self, r: usize) {
        let Some(fl) = self.outs[r].first_left else { return };
        if self.outs[r].is_hole != self.outs[fl].is_hole && self.outs[fl].pts.is_some() { return; }
        let mut orfl = Some(fl);
        while let Some(o) = orfl.filter(|&o| self.outs[o].is_hole == self.outs[r].is_hole || self.outs[o].pts.is_none()) {
            orfl = self.outs[o].first_left;
        }
        self.outs[r].first_left = orfl;
    }

    /// `BuildResult2`, then flattened the way clipper-sys returns a PolyTree:
    /// outer contours breadth first, each followed by its holes.
    fn build_result2(&mut self) -> Vec<Vec<Vec<[i64; 2]>>> {
        let mut nodes: Vec<Vec<[i64; 2]>> = Vec::new();
        for i in 0..self.outs.len() {
            let cnt = self.point_count(self.outs[i].pts);
            if cnt < 3 { continue; }
            self.fix_hole_linkage(i);
            let mut contour = Vec::with_capacity(cnt);
            let mut op = self.op[self.outs[i].pts.unwrap()].prev;
            for _ in 0..cnt {
                contour.push([self.op[op].pt.x, self.op[op].pt.y]);
                op = self.op[op].prev;
            }
            self.outs[i].poly_nd = Some(nodes.len());
            nodes.push(contour);
        }
        let mut children: Vec<Vec<usize>> = vec![Vec::new(); nodes.len()];
        let mut queue = std::collections::VecDeque::new();
        for i in 0..self.outs.len() {
            let Some(nd) = self.outs[i].poly_nd else { continue };
            match self.outs[i].first_left.and_then(|fl| self.outs[fl].poly_nd) {
                Some(parent) => children[parent].push(nd),
                None => queue.push_back(nd),
            }
        }
        let mut out = Vec::new();
        while let Some(nd) = queue.pop_front() {
            let mut polygon = vec![nodes[nd].clone()];
            for &child in &children[nd] {
                polygon.push(nodes[child].clone());
                queue.extend(children[child].iter().copied());
            }
            out.push(polygon);
        }
        out
    }

    fn area(&self, start: usize) -> f64 {
        let mut a = 0.0;
        let mut op = start;
        loop {
            let prev = self.op[op].prev;
            a += (self.op[prev].pt.x + self.op[op].pt.x) as f64 * (self.op[prev].pt.y - self.op[op].pt.y) as f64;
            op = self.op[op].next;
            if op == start { return a * 0.5; }
        }
    }

    fn reverse_poly_pt_links(&mut self, start: usize) {
        let mut p1 = start;
        loop {
            let p2 = self.op[p1].next;
            self.op[p1].next = self.op[p1].prev;
            self.op[p1].prev = p2;
            p1 = p2;
            if p1 == start { break; }
        }
    }

    fn e2_inserts_before_e1(&self, e1: usize, e2: usize) -> bool {
        let (a, b) = (&self.e[e1], &self.e[e2]);
        if b.curr.x == a.curr.x {
            if b.top.y > a.top.y { b.top.x < self.top_x(e1, b.top.y) } else { a.top.x > self.top_x(e2, a.top.y) }
        } else { b.curr.x < a.curr.x }
    }

    fn insert_edge_into_ael(&mut self, edge: usize, start: Option<usize>) {
        match self.active_edges {
            None => {
                self.e[edge].prev_in_ael = None;
                self.e[edge].next_in_ael = None;
                self.active_edges = Some(edge);
            }
            Some(ae) if start.is_none() && self.e2_inserts_before_e1(ae, edge) => {
                self.e[edge].prev_in_ael = None;
                self.e[edge].next_in_ael = Some(ae);
                self.e[ae].prev_in_ael = Some(edge);
                self.active_edges = Some(edge);
            }
            Some(ae) => {
                let mut s = start.unwrap_or(ae);
                while let Some(n) = self.e[s].next_in_ael.filter(|&n| !self.e2_inserts_before_e1(n, edge)) { s = n; }
                self.e[edge].next_in_ael = self.e[s].next_in_ael;
                if let Some(n) = self.e[s].next_in_ael { self.e[n].prev_in_ael = Some(edge); }
                self.e[edge].prev_in_ael = Some(s);
                self.e[s].next_in_ael = Some(edge);
            }
        }
    }

    fn dup_out_pt(&mut self, op: usize, insert_after: bool) -> usize {
        let (pt, idx) = (self.op[op].pt, self.op[op].idx);
        if insert_after {
            let next = self.op[op].next;
            let r = self.new_out_pt(idx, pt, next, op);
            self.op[next].prev = r;
            self.op[op].next = r;
            r
        } else {
            let prev = self.op[op].prev;
            let r = self.new_out_pt(idx, pt, op, prev);
            self.op[prev].next = r;
            self.op[op].prev = r;
            r
        }
    }

    /// One side of `JoinHorz`: walk `op` along the horizontal to `pt` and
    /// duplicate it there. Returns (op, opb).
    fn join_horz_side(&mut self, mut op: usize, left_to_right: bool, pt: Pt, discard_left: bool) -> (usize, usize) {
        let mut opb;
        if left_to_right {
            loop {
                let n = self.op[op].next;
                if self.op[n].pt.x <= pt.x && self.op[n].pt.x >= self.op[op].pt.x && self.op[n].pt.y == pt.y { op = n; } else { break; }
            }
            if discard_left && self.op[op].pt.x != pt.x { op = self.op[op].next; }
            opb = self.dup_out_pt(op, !discard_left);
            if self.op[opb].pt != pt {
                op = opb;
                self.op[op].pt = pt;
                opb = self.dup_out_pt(op, !discard_left);
            }
        } else {
            loop {
                let n = self.op[op].next;
                if self.op[n].pt.x >= pt.x && self.op[n].pt.x <= self.op[op].pt.x && self.op[n].pt.y == pt.y { op = n; } else { break; }
            }
            if !discard_left && self.op[op].pt.x != pt.x { op = self.op[op].next; }
            opb = self.dup_out_pt(op, discard_left);
            if self.op[opb].pt != pt {
                op = opb;
                self.op[op].pt = pt;
                opb = self.dup_out_pt(op, discard_left);
            }
        }
        (op, opb)
    }

    fn join_horz(&mut self, op1: usize, op1b: usize, op2: usize, op2b: usize, pt: Pt, discard_left: bool) -> bool {
        let dir1 = self.op[op1].pt.x <= self.op[op1b].pt.x;
        let dir2 = self.op[op2].pt.x <= self.op[op2b].pt.x;
        if dir1 == dir2 { return false; }
        let (op1, op1b) = self.join_horz_side(op1, dir1, pt, discard_left);
        let (op2, op2b) = self.join_horz_side(op2, dir2, pt, discard_left);
        if dir1 == discard_left {
            self.op[op1].prev = op2;
            self.op[op2].next = op1;
            self.op[op1b].next = op2b;
            self.op[op2b].prev = op1b;
        } else {
            self.op[op1].next = op2;
            self.op[op2].prev = op1;
            self.op[op1b].prev = op2b;
            self.op[op2b].next = op1b;
        }
        true
    }

    fn join_points(&mut self, j: usize, r1: usize, r2: usize) -> bool {
        let (mut op1, mut op2, off) = (self.joins[j].op1, self.joins[j].op2, self.joins[j].off_pt);
        let is_horizontal = self.op[op1].pt.y == off.y;
        if is_horizontal && off == self.op[op1].pt && off == self.op[op2].pt {
            // StrictlySimple join.
            if r1 != r2 { return false; }
            let mut op1b = self.op[op1].next;
            while op1b != op1 && self.op[op1b].pt == off { op1b = self.op[op1b].next; }
            let reverse1 = self.op[op1b].pt.y > off.y;
            let mut op2b = self.op[op2].next;
            while op2b != op2 && self.op[op2b].pt == off { op2b = self.op[op2b].next; }
            let reverse2 = self.op[op2b].pt.y > off.y;
            if reverse1 == reverse2 { return false; }
            self.link_join(j, op1, op2, reverse1);
            true
        } else if is_horizontal {
            // Horizontal joins: OutPt1 and OutPt2 may be anywhere along the edge.
            let mut op1b = op1;
            while self.op[self.op[op1].prev].pt.y == self.op[op1].pt.y && self.op[op1].prev != op1b && self.op[op1].prev != op2 { op1 = self.op[op1].prev; }
            while self.op[self.op[op1b].next].pt.y == self.op[op1b].pt.y && self.op[op1b].next != op1 && self.op[op1b].next != op2 { op1b = self.op[op1b].next; }
            if self.op[op1b].next == op1 || self.op[op1b].next == op2 { return false; }
            let mut op2b = op2;
            while self.op[self.op[op2].prev].pt.y == self.op[op2].pt.y && self.op[op2].prev != op2b && self.op[op2].prev != op1b { op2 = self.op[op2].prev; }
            while self.op[self.op[op2b].next].pt.y == self.op[op2b].pt.y && self.op[op2b].next != op2 && self.op[op2b].next != op1 { op2b = self.op[op2b].next; }
            if self.op[op2b].next == op2 || self.op[op2b].next == op1 { return false; }
            let (p1, p1b, p2, p2b) = (self.op[op1].pt, self.op[op1b].pt, self.op[op2].pt, self.op[op2b].pt);
            let Some((left, right)) = get_overlap(p1.x, p1b.x, p2.x, p2b.x) else { return false };
            let (pt, discard_left) = if p1.x >= left && p1.x <= right { (p1, p1.x > p1b.x) }
                else if p2.x >= left && p2.x <= right { (p2, p2.x > p2b.x) }
                else if p1b.x >= left && p1b.x <= right { (p1b, p1b.x > p1.x) }
                else { (p2b, p2b.x > p2.x) };
            self.joins[j].op1 = op1;
            self.joins[j].op2 = op2;
            self.join_horz(op1, op1b, op2, op2b, pt, discard_left)
        } else {
            let full = self.use_full_range;
            let find = |c: &Self, op: usize, forward: bool| {
                let step = |p: usize| if forward { c.op[p].next } else { c.op[p].prev };
                let mut b = step(op);
                while c.op[b].pt == c.op[op].pt && b != op { b = step(b); }
                b
            };
            let bad = |c: &Self, op: usize, b: usize| c.op[b].pt.y > c.op[op].pt.y || !slopes_equal3(c.op[op].pt, c.op[b].pt, off, full);
            let mut op1b = find(self, op1, true);
            let reverse1 = bad(self, op1, op1b);
            if reverse1 {
                op1b = find(self, op1, false);
                if bad(self, op1, op1b) { return false; }
            }
            let mut op2b = find(self, op2, true);
            let reverse2 = bad(self, op2, op2b);
            if reverse2 {
                op2b = find(self, op2, false);
                if bad(self, op2, op2b) { return false; }
            }
            if op1b == op1 || op2b == op2 || op1b == op2b || (r1 == r2 && reverse1 == reverse2) { return false; }
            self.link_join(j, op1, op2, reverse1);
            true
        }
    }

    /// The shared tail of the non-horizontal and StrictlySimple `JoinPoints` cases.
    fn link_join(&mut self, j: usize, op1: usize, op2: usize, reverse1: bool) {
        let (op1b, op2b);
        if reverse1 {
            op1b = self.dup_out_pt(op1, false);
            op2b = self.dup_out_pt(op2, true);
            self.op[op1].prev = op2;
            self.op[op2].next = op1;
            self.op[op1b].next = op2b;
            self.op[op2b].prev = op1b;
        } else {
            op1b = self.dup_out_pt(op1, true);
            op2b = self.dup_out_pt(op2, false);
            self.op[op1].next = op2;
            self.op[op2].prev = op1;
            self.op[op1b].prev = op2b;
            self.op[op2b].next = op1b;
        }
        self.joins[j].op1 = op1;
        self.joins[j].op2 = op1b;
    }

    fn parse_first_left(&self, mut fl: Option<usize>) -> Option<usize> {
        while let Some(f) = fl.filter(|&f| self.outs[f].pts.is_none()) { fl = self.outs[f].first_left; }
        fl
    }

    fn point_in_polygon(&self, pt: Pt, start: usize) -> i32 {
        let mut result = 0;
        let mut op = start;
        loop {
            let (a, b) = (self.op[op].pt, self.op[self.op[op].next].pt);
            if b.y == pt.y && (b.x == pt.x || (a.y == pt.y && ((b.x > pt.x) == (a.x < pt.x)))) { return -1; }
            if (a.y < pt.y) != (b.y < pt.y) {
                if a.x >= pt.x {
                    if b.x > pt.x { result = 1 - result; } else {
                        let d = (a.x - pt.x) as f64 * (b.y - pt.y) as f64 - (b.x - pt.x) as f64 * (a.y - pt.y) as f64;
                        if d == 0.0 { return -1; }
                        if (d > 0.0) == (b.y > a.y) { result = 1 - result; }
                    }
                } else if b.x > pt.x {
                    let d = (a.x - pt.x) as f64 * (b.y - pt.y) as f64 - (b.x - pt.x) as f64 * (a.y - pt.y) as f64;
                    if d == 0.0 { return -1; }
                    if (d > 0.0) == (b.y > a.y) { result = 1 - result; }
                }
            }
            op = self.op[op].next;
            if op == start { return result; }
        }
    }

    fn poly2_contains_poly1(&self, pts1: usize, pts2: usize) -> bool {
        let mut op = pts1;
        loop {
            let res = self.point_in_polygon(self.op[op].pt, pts2);
            if res >= 0 { return res > 0; }
            op = self.op[op].next;
            if op == pts1 { return true; }
        }
    }

    fn fixup_first_lefts1(&mut self, old: usize, new: usize) {
        for i in 0..self.outs.len() {
            let fl = self.parse_first_left(self.outs[i].first_left);
            if let Some(pts) = self.outs[i].pts {
                if fl == Some(old) && self.poly2_contains_poly1(pts, self.outs[new].pts.unwrap()) { self.outs[i].first_left = Some(new); }
            }
        }
    }

    fn fixup_first_lefts2(&mut self, inner: usize, outer: usize) {
        let orfl = self.outs[outer].first_left;
        for i in 0..self.outs.len() {
            let Some(pts) = self.outs[i].pts else { continue };
            if i == outer || i == inner { continue; }
            let fl = self.parse_first_left(self.outs[i].first_left);
            if fl != orfl && fl != Some(inner) && fl != Some(outer) { continue; }
            if self.poly2_contains_poly1(pts, self.outs[inner].pts.unwrap()) { self.outs[i].first_left = Some(inner); }
            else if self.poly2_contains_poly1(pts, self.outs[outer].pts.unwrap()) { self.outs[i].first_left = Some(outer); }
            else if self.outs[i].first_left == Some(inner) || self.outs[i].first_left == Some(outer) { self.outs[i].first_left = orfl; }
        }
    }

    fn fixup_first_lefts3(&mut self, old: usize, new: usize) {
        for i in 0..self.outs.len() {
            let fl = self.parse_first_left(self.outs[i].first_left);
            if self.outs[i].pts.is_some() && fl == Some(old) { self.outs[i].first_left = Some(new); }
        }
    }

    fn update_out_pt_idxs(&mut self, r: usize) {
        let start = self.outs[r].pts.unwrap();
        let mut op = start;
        loop {
            self.op[op].idx = self.outs[r].idx;
            op = self.op[op].prev;
            if op == start { break; }
        }
    }

    fn join_common_edges(&mut self) {
        for j in 0..self.joins.len() {
            let r1 = self.get_out_rec(self.op[self.joins[j].op1].idx);
            let mut r2 = self.get_out_rec(self.op[self.joins[j].op2].idx);
            if self.outs[r1].pts.is_none() || self.outs[r2].pts.is_none() { continue; }
            let hole_state_rec = if r1 == r2 { r1 }
                else if self.outrec1_right_of_outrec2(r1, r2) { r2 }
                else if self.outrec1_right_of_outrec2(r2, r1) { r1 }
                else { self.get_lowermost_rec(r1, r2) };
            if !self.join_points(j, r1, r2) { continue; }
            if r1 == r2 {
                // One polygon split into two.
                self.outs[r1].pts = Some(self.joins[j].op1);
                self.outs[r1].bottom_pt = None;
                let idx = self.outs.len();
                self.outs.push(OutRec { idx: idx as i32, is_hole: false, first_left: None, poly_nd: None, pts: Some(self.joins[j].op2), bottom_pt: None });
                r2 = idx;
                self.update_out_pt_idxs(r2);
                let (p1, p2) = (self.outs[r1].pts.unwrap(), self.outs[r2].pts.unwrap());
                if self.poly2_contains_poly1(p2, p1) {
                    self.outs[r2].is_hole = !self.outs[r1].is_hole;
                    self.outs[r2].first_left = Some(r1);
                    self.fixup_first_lefts2(r2, r1);
                    if self.outs[r2].is_hole == (self.area(p2) > 0.0) { self.reverse_poly_pt_links(p2); }
                } else if self.poly2_contains_poly1(p1, p2) {
                    self.outs[r2].is_hole = self.outs[r1].is_hole;
                    self.outs[r1].is_hole = !self.outs[r2].is_hole;
                    self.outs[r2].first_left = self.outs[r1].first_left;
                    self.outs[r1].first_left = Some(r2);
                    self.fixup_first_lefts2(r1, r2);
                    if self.outs[r1].is_hole == (self.area(p1) > 0.0) { self.reverse_poly_pt_links(p1); }
                } else {
                    self.outs[r2].is_hole = self.outs[r1].is_hole;
                    self.outs[r2].first_left = self.outs[r1].first_left;
                    self.fixup_first_lefts1(r1, r2);
                }
            } else {
                // Two polygons joined.
                self.outs[r2].pts = None;
                self.outs[r2].bottom_pt = None;
                self.outs[r2].idx = self.outs[r1].idx;
                self.outs[r1].is_hole = self.outs[hole_state_rec].is_hole;
                if hole_state_rec == r2 { self.outs[r1].first_left = self.outs[r2].first_left; }
                self.outs[r2].first_left = Some(r1);
                self.fixup_first_lefts3(r2, r1);
            }
        }
    }
}
