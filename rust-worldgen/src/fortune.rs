//! Fortune's sweepline Voronoi, faithful port of ONI's `Delaunay.*`
//! namespace (Voronoi + Site + Edge + Vertex + Halfedge + EdgeList +
//! HalfedgePriorityQueue + SiteList + EdgeReorderer).
//!
//! The C# source uses object pools and mutable linked lists
//! everywhere. To match the same algorithmic behaviour while keeping
//! Rust borrows sane, everything is stored in `Vec` pools inside the
//! `Voronoi` struct and referenced by index. A sentinel `NONE =
//! usize::MAX` replaces the C# `null`.
//!
//! The algorithm produces:
//! - a list of `Edge`s (the Voronoi bisectors), each with up to two
//!   clipped endpoints and the two `Site`s it separates;
//! - per-`Site` access to its incident edges and its region polygon
//!   (computed on demand and clipped against the plot rectangle).
//!
//! Scope of this port: enough for `VoronoiTree.Diagram` (Phase 2b.2)
//! to iterate power diagrams. The `Kruskal` / `MST` helpers in
//! `DelaunayHelpers` are not ported — worldgen doesn't use them.

use crate::geom::{LineSegment, Polygon, Rect, V2, Winding};

/// Pool index type. `NONE` stands in for the C# `null`.
type Idx = usize;
const NONE: Idx = usize::MAX;

/// `Side` enum matching `Delaunay.LR.Side`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

impl Side {
    fn other(self) -> Side {
        match self {
            Side::Left => Side::Right,
            Side::Right => Side::Left,
        }
    }
    /// Matches C#'s `(int)Side` cast, used by the original object
    /// pools. Kept for parity with the decompile; Rust callers
    /// don't need it today.
    #[allow(dead_code)]
    fn as_usize(self) -> usize {
        match self {
            Side::Left => 0,
            Side::Right => 1,
        }
    }
}

/// Used by `EdgeReorderer` to choose what to chain on (site vs
/// vertex endpoints). Matches `VertexOrSite` in the source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VertexOrSite {
    Site,
    Vertex,
}

#[derive(Debug)]
#[allow(dead_code)]
struct Site {
    coord: V2,
    weight: f32,
    /// Mirrors `Site.scaled_weight`; used by PowerDiagram in Phase 2b.2.
    scaled_weight: f32,
    /// `Site._siteIndex`; may get swapped during `CompareTo` in C#.
    site_index: u32,
    color: u32,
    edges: Vec<EdgeIdx>,
    edge_orientations: Option<Vec<Side>>,
    region: Option<Vec<V2>>,
}

#[derive(Debug)]
#[allow(dead_code)]
struct Edge {
    a: f32,
    b: f32,
    c: f32,
    left_vertex: VertexIdx,
    right_vertex: VertexIdx,
    clipped_left: Option<V2>,
    clipped_right: Option<V2>,
    /// `true` once `ClipVertices` runs; equivalent to the C#
    /// `_clippedVertices != null` check via `visible`.
    clipped: bool,
    left_site: SiteIdx,
    right_site: SiteIdx,
    /// `Edge.DELETED` sentinel flag — set by `EdgeList.Remove` in the
    /// source. Kept for parity even though Rust's ownership checks
    /// don't need it.
    deleted: bool,
}

#[derive(Debug)]
#[allow(dead_code)]
struct Vertex {
    coord: V2,
    vertex_index: i32,
    /// Matches `VERTEX_AT_INFINITY`.
    at_infinity: bool,
}

#[derive(Debug)]
struct Halfedge {
    edge_list_left: HalfedgeIdx,
    edge_list_right: HalfedgeIdx,
    next_in_pq: HalfedgeIdx,
    edge: EdgeIdx,
    left_right: Option<Side>,
    vertex: VertexIdx,
    ystar: f32,
}

type SiteIdx = Idx;
type EdgeIdx = Idx;
type VertexIdx = Idx;
type HalfedgeIdx = Idx;

/// Main driver. Constructing a `Voronoi` runs the full sweepline
/// pass; inspect the result via the public methods.
pub struct Voronoi {
    plot_bounds: Rect,
    sites: Vec<Site>,
    site_order: Vec<SiteIdx>,   // index into `sites` after CompareByYThenX sort
    site_cursor: usize,         // `_currentIndex` in SiteList
    site_by_coord: std::collections::HashMap<(u32, u32), SiteIdx>, // bitcasted (x, y) for lookup
    edges: Vec<Edge>,
    vertices: Vec<Vertex>,
    halfedges: Vec<Halfedge>,
    next_vertex_index: i32,
    bottom_most_site: SiteIdx,
    weight_sum: f32,
    _min_weight: f32,
    _max_weight: f32,
}

impl Voronoi {
    pub fn new(points: &[V2], colors: &[u32], weights: &[f32], plot_bounds: Rect) -> Self {
        let mut v = Voronoi {
            plot_bounds,
            sites: Vec::new(),
            site_order: Vec::new(),
            site_cursor: 0,
            site_by_coord: std::collections::HashMap::new(),
            edges: Vec::new(),
            vertices: Vec::new(),
            halfedges: Vec::new(),
            next_vertex_index: 0,
            bottom_most_site: NONE,
            weight_sum: 0.0,
            _min_weight: f32::MAX,
            _max_weight: f32::MIN,
        };
        for i in 0..points.len() {
            let color = *colors.get(i).unwrap_or(&0);
            let weight = *weights.get(i).unwrap_or(&1.0);
            v.add_site(points[i], color, i as u32, weight);
        }
        let delta = v._max_weight - v._min_weight;
        if delta > 0.0 {
            let scale = 1.0 + delta;
            for s in v.sites.iter_mut() {
                s.scaled_weight = s.weight * scale;
            }
        }
        v.fortunes_algorithm();
        v
    }

    fn add_site(&mut self, p: V2, color: u32, index: u32, weight: f32) {
        let key = (p.x.to_bits(), p.y.to_bits());
        if self.site_by_coord.contains_key(&key) {
            return;
        }
        let idx = self.sites.len();
        self.sites.push(Site {
            coord: p,
            weight,
            scaled_weight: -1.0,
            site_index: index,
            color,
            edges: Vec::new(),
            edge_orientations: None,
            region: None,
        });
        self.site_by_coord.insert(key, idx);
        self._min_weight = self._min_weight.min(weight);
        self._max_weight = self._max_weight.max(weight);
        self.weight_sum += weight;
    }

    // ---- Public API -------------------------------------------------

    /// Return the region polygon for a site (clipped to `plot_bounds`),
    /// computed on first access and cached per site.
    pub fn region(&mut self, p: V2) -> Vec<V2> {
        let idx = match self.site_by_coord.get(&(p.x.to_bits(), p.y.to_bits())) {
            Some(&i) => i,
            None => return Vec::new(),
        };
        self.compute_region(idx)
    }

    /// Neighbouring site IDs (from `Site.color`) for the site at `p`,
    /// deduplicated in first-seen order like the game's `HashSet<uint>`.
    /// Matches `NeighborSitesIDsForSite`.
    pub fn neighbor_site_ids(&mut self, p: V2) -> Vec<u32> {
        let idx = match self.site_by_coord.get(&(p.x.to_bits(), p.y.to_bits())) {
            Some(&i) => i,
            None => return Vec::new(),
        };
        let mut ids = Vec::new();
        for i in self.neighbor_sites(idx) {
            if !ids.contains(&self.sites[i].color) { ids.push(self.sites[i].color); }
        }
        ids
    }

    /// Flat list of neighbouring site IDs in the order they come off
    /// the site's edge list. Matches `ListNeighborSitesIDsForSite`.
    pub fn list_neighbor_site_ids(&mut self, p: V2) -> Vec<u32> {
        let idx = match self.site_by_coord.get(&(p.x.to_bits(), p.y.to_bits())) {
            Some(&i) => i,
            None => return Vec::new(),
        };
        let ns = self.neighbor_sites(idx);
        ns.into_iter().map(|i| self.sites[i].color).collect()
    }

    /// All Voronoi edges produced by the sweepline (including those
    /// that are fully outside the plot bounds and thus not visible).
    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }

    /// Returns the edge endpoints (clipped) for edge `i`, or `None`
    /// if the edge is not visible.
    pub fn edge_segment(&self, i: usize) -> Option<LineSegment> {
        let e = &self.edges[i];
        if !e.clipped {
            return None;
        }
        Some(LineSegment::new(e.clipped_left, e.clipped_right))
    }

    // ---- Fortune's sweepline ---------------------------------------

    fn fortunes_algorithm(&mut self) {
        let sites_bounds = self.sort_and_get_sites_bounds();
        let sqrt_n = ((self.sites.len() as f32 + 4.0).sqrt()) as i32;
        let sqrt_n = sqrt_n.max(1);
        let mut pq = HalfedgePq::new(sites_bounds.y, sites_bounds.height, sqrt_n);
        let mut el = ELList::new(sites_bounds.x, sites_bounds.width, sqrt_n, &mut self.halfedges);

        self.bottom_most_site = self.next_site();
        let mut new_site = self.next_site();

        loop {
            let min_pt = if !pq.empty() {
                pq.min(&self.halfedges, &self.vertices)
            } else {
                V2::ZERO
            };

            if new_site != NONE
                && (pq.empty()
                    || compare_y_then_x_site_pt(&self.sites[new_site], min_pt) < 0)
            {
                // Site event.
                let site_pt = self.sites[new_site].coord;
                let lbnd = el.left_neighbor(site_pt, &self.halfedges, &self.edges, &self.sites);
                let rbnd = self.halfedges[lbnd].edge_list_right;
                let bot = self.right_region(lbnd);
                let e = self.create_bisecting_edge(bot, new_site);
                let bisector_l = self.create_halfedge(e, Side::Left);
                el.insert(lbnd, bisector_l, &mut self.halfedges);
                if let Some(v) = self.vertex_intersect(lbnd, bisector_l) {
                    pq.remove(lbnd, &mut self.halfedges);
                    self.halfedges[lbnd].vertex = v;
                    self.halfedges[lbnd].ystar =
                        self.vertices[v].coord.y + dist(self.sites[new_site].coord, self.vertices[v].coord);
                    pq.insert(lbnd, &mut self.halfedges, &self.vertices);
                }
                let bisector_r = self.create_halfedge(e, Side::Right);
                el.insert(bisector_l, bisector_r, &mut self.halfedges);
                if let Some(v) = self.vertex_intersect(bisector_r, rbnd) {
                    self.halfedges[bisector_r].vertex = v;
                    self.halfedges[bisector_r].ystar =
                        self.vertices[v].coord.y + dist(self.sites[new_site].coord, self.vertices[v].coord);
                    pq.insert(bisector_r, &mut self.halfedges, &self.vertices);
                }
                new_site = self.next_site();
            } else if pq.empty() {
                break;
            } else {
                // Circle event.
                let lbnd = pq.extract_min(&mut self.halfedges);
                let llbnd = self.halfedges[lbnd].edge_list_left;
                let rbnd = self.halfedges[lbnd].edge_list_right;
                let rrbnd = self.halfedges[rbnd].edge_list_right;
                let mut bot = self.left_region(lbnd);
                let mut top = self.right_region(rbnd);
                let v = self.halfedges[lbnd].vertex;
                self.vertices[v].vertex_index = self.next_vertex_index;
                self.next_vertex_index += 1;
                let e_lbnd = self.halfedges[lbnd].edge;
                let lbnd_side = self.halfedges[lbnd].left_right.unwrap();
                self.set_edge_vertex(e_lbnd, lbnd_side, v);
                let e_rbnd = self.halfedges[rbnd].edge;
                let rbnd_side = self.halfedges[rbnd].left_right.unwrap();
                self.set_edge_vertex(e_rbnd, rbnd_side, v);
                el.remove(lbnd, &mut self.halfedges);
                pq.remove(rbnd, &mut self.halfedges);
                el.remove(rbnd, &mut self.halfedges);
                let mut side = Side::Left;
                if self.sites[bot].coord.y > self.sites[top].coord.y {
                    std::mem::swap(&mut bot, &mut top);
                    side = Side::Right;
                }
                let e = self.create_bisecting_edge(bot, top);
                let bisector = self.create_halfedge(e, side);
                el.insert(llbnd, bisector, &mut self.halfedges);
                self.set_edge_vertex(e, side.other(), v);
                if let Some(v2) = self.vertex_intersect(llbnd, bisector) {
                    pq.remove(llbnd, &mut self.halfedges);
                    self.halfedges[llbnd].vertex = v2;
                    self.halfedges[llbnd].ystar =
                        self.vertices[v2].coord.y + dist(self.sites[bot].coord, self.vertices[v2].coord);
                    pq.insert(llbnd, &mut self.halfedges, &self.vertices);
                }
                if let Some(v2) = self.vertex_intersect(bisector, rrbnd) {
                    self.halfedges[bisector].vertex = v2;
                    self.halfedges[bisector].ystar =
                        self.vertices[v2].coord.y + dist(self.sites[bot].coord, self.vertices[v2].coord);
                    pq.insert(bisector, &mut self.halfedges, &self.vertices);
                }
            }
        }

        // Clip every visible edge against the plot rectangle.
        for i in 0..self.edges.len() {
            self.clip_vertices(i);
        }
    }

    fn sort_and_get_sites_bounds(&mut self) -> Rect {
        // Stable sort by (y, x) matching `Voronoi.CompareByYThenX`.
        // C#'s `List.Sort` also mutates site indices as it goes; we
        // replicate by permuting `site_index` in pass with the sort.
        let mut order: Vec<usize> = (0..self.sites.len()).collect();
        order.sort_by(|&a, &b| {
            let sa = &self.sites[a];
            let sb = &self.sites[b];
            match compare_y_then_x(sa, sb) {
                -1 => std::cmp::Ordering::Less,
                1 => std::cmp::Ordering::Greater,
                _ => std::cmp::Ordering::Equal,
            }
        });
        self.site_order = order;
        self.site_cursor = 0;

        if self.sites.is_empty() {
            return Rect::new(0.0, 0.0, 0.0, 0.0);
        }
        let mut x_min = f32::MAX;
        let mut x_max = f32::MIN;
        for s in &self.sites {
            if s.coord.x < x_min {
                x_min = s.coord.x;
            }
            if s.coord.x > x_max {
                x_max = s.coord.x;
            }
        }
        let first = &self.sites[self.site_order[0]];
        let last = &self.sites[*self.site_order.last().unwrap()];
        Rect::new(x_min, first.coord.y, x_max - x_min, last.coord.y - first.coord.y)
    }

    fn next_site(&mut self) -> SiteIdx {
        if self.site_cursor >= self.site_order.len() {
            return NONE;
        }
        let i = self.site_order[self.site_cursor];
        self.site_cursor += 1;
        i
    }

    fn create_bisecting_edge(&mut self, s0: SiteIdx, s1: SiteIdx) -> EdgeIdx {
        let c0 = self.sites[s0].coord;
        let c1 = self.sites[s1].coord;
        let dx = c0.x - c1.x;
        let dy = c0.y - c1.y;
        let adx = dx.abs();
        let ady = dy.abs();
        let (dx64, dy64) = (dx as f64, dy as f64);
        let mut cval = (c1.x as f64 * dx64 + c1.y as f64 * dy64
            + (dx64 * dx64 + dy64 * dy64) * 0.5) as f32;
        let (a, b);
        if adx > ady {
            a = 1.0;
            b = dy / dx;
            cval /= dx;
        } else {
            b = 1.0;
            a = dx / dy;
            cval /= dy;
        }
        let idx = self.edges.len();
        self.edges.push(Edge {
            a,
            b,
            c: cval,
            left_vertex: NONE,
            right_vertex: NONE,
            clipped_left: None,
            clipped_right: None,
            clipped: false,
            left_site: s0,
            right_site: s1,
            deleted: false,
        });
        self.sites[s0].edges.push(idx);
        self.sites[s1].edges.push(idx);
        idx
    }

    fn create_halfedge(&mut self, e: EdgeIdx, side: Side) -> HalfedgeIdx {
        let idx = self.halfedges.len();
        self.halfedges.push(Halfedge {
            edge_list_left: NONE,
            edge_list_right: NONE,
            next_in_pq: NONE,
            edge: e,
            left_right: Some(side),
            vertex: NONE,
            ystar: 0.0,
        });
        idx
    }

    #[allow(dead_code)]
    fn create_dummy_halfedge(&mut self) -> HalfedgeIdx {
        let idx = self.halfedges.len();
        self.halfedges.push(Halfedge {
            edge_list_left: NONE,
            edge_list_right: NONE,
            next_in_pq: NONE,
            edge: NONE,
            left_right: None,
            vertex: NONE,
            ystar: 0.0,
        });
        idx
    }

    fn left_region(&self, he: HalfedgeIdx) -> SiteIdx {
        let e = self.halfedges[he].edge;
        if e == NONE {
            return self.bottom_most_site;
        }
        let side = self.halfedges[he].left_right.unwrap();
        self.edge_site(e, side)
    }

    fn right_region(&self, he: HalfedgeIdx) -> SiteIdx {
        let e = self.halfedges[he].edge;
        if e == NONE {
            return self.bottom_most_site;
        }
        let side = self.halfedges[he].left_right.unwrap().other();
        self.edge_site(e, side)
    }

    fn edge_site(&self, e: EdgeIdx, side: Side) -> SiteIdx {
        match side {
            Side::Left => self.edges[e].left_site,
            Side::Right => self.edges[e].right_site,
        }
    }

    fn set_edge_vertex(&mut self, e: EdgeIdx, side: Side, v: VertexIdx) {
        match side {
            Side::Left => self.edges[e].left_vertex = v,
            Side::Right => self.edges[e].right_vertex = v,
        }
    }

    /// `Vertex.Intersect`. Returns the newly created vertex index, or
    /// `None` if the halfedges don't produce a valid intersection.
    fn vertex_intersect(&mut self, h0: HalfedgeIdx, h1: HalfedgeIdx) -> Option<VertexIdx> {
        let e0 = self.halfedges[h0].edge;
        let e1 = self.halfedges[h1].edge;
        if e0 == NONE || e1 == NONE {
            return None;
        }
        let e0_right = self.edges[e0].right_site;
        let e1_right = self.edges[e1].right_site;
        if e0_right == e1_right {
            return None;
        }
        let ea = self.edges[e0].a;
        let eb = self.edges[e0].b;
        let ec = self.edges[e0].c;
        let fa = self.edges[e1].a;
        let fb = self.edges[e1].b;
        let fc = self.edges[e1].c;
        let d = (ea as f64 * fb as f64 - eb as f64 * fa as f64) as f32;
        if d.abs() < 1e-10 {
            return None;
        }
        let x = ((ec as f64 * fb as f64 - fc as f64 * eb as f64) / d as f64) as f32;
        let y = ((fc as f64 * ea as f64 - ec as f64 * fa as f64) / d as f64) as f32;
        let (h_use, e_use);
        let r0 = &self.sites[e0_right];
        let r1 = &self.sites[e1_right];
        if compare_y_then_x(r0, r1) < 0 {
            h_use = h0;
            e_use = e0;
        } else {
            h_use = h1;
            e_use = e1;
        }
        let right_site_x = self.sites[self.edges[e_use].right_site].coord.x;
        let right_of = x >= right_site_x;
        let side = self.halfedges[h_use].left_right.unwrap();
        if (right_of && side == Side::Left) || (!right_of && side == Side::Right) {
            return None;
        }
        let idx = self.vertices.len();
        self.vertices.push(Vertex {
            coord: V2::new(x, y),
            vertex_index: -1,
            at_infinity: x.is_nan() || y.is_nan(),
        });
        Some(idx)
    }

    /// Port of `Edge.ClipVertices`. Sets `edge.clipped_left/right`
    /// and marks `edge.clipped = true` if the edge is visible.
    fn clip_vertices(&mut self, e_idx: EdgeIdx) {
        let b = self.plot_bounds;
        let x_min = b.x;
        let y_min = b.y;
        let x_max = b.x + b.width;
        let y_max = b.y + b.height;

        let (a, bb, cc) = (self.edges[e_idx].a, self.edges[e_idx].b, self.edges[e_idx].c);
        let (v0_idx, v1_idx);
        if a == 1.0 && bb >= 0.0 {
            v0_idx = self.edges[e_idx].right_vertex;
            v1_idx = self.edges[e_idx].left_vertex;
        } else {
            v0_idx = self.edges[e_idx].left_vertex;
            v1_idx = self.edges[e_idx].right_vertex;
        }

        let mut x0;
        let mut y0;
        let mut x1;
        let mut y1;
        if a == 1.0 {
            y0 = y_min;
            if v0_idx != NONE && self.vertices[v0_idx].coord.y > y_min {
                y0 = self.vertices[v0_idx].coord.y;
            }
            if y0 > y_max {
                return;
            }
            x0 = (cc as f64 - bb as f64 * y0 as f64) as f32;
            y1 = y_max;
            if v1_idx != NONE && self.vertices[v1_idx].coord.y < y_max {
                y1 = self.vertices[v1_idx].coord.y;
            }
            if y1 < y_min {
                return;
            }
            x1 = (cc as f64 - bb as f64 * y1 as f64) as f32;
            if (x0 > x_max && x1 > x_max) || (x0 < x_min && x1 < x_min) {
                return;
            }
            if x0 > x_max {
                x0 = x_max;
                y0 = ((cc as f64 - x0 as f64) / bb as f64) as f32;
            } else if x0 < x_min {
                x0 = x_min;
                y0 = ((cc as f64 - x0 as f64) / bb as f64) as f32;
            }
            if x1 > x_max {
                x1 = x_max;
                y1 = ((cc as f64 - x1 as f64) / bb as f64) as f32;
            } else if x1 < x_min {
                x1 = x_min;
                y1 = ((cc as f64 - x1 as f64) / bb as f64) as f32;
            }
        } else {
            x0 = x_min;
            if v0_idx != NONE && self.vertices[v0_idx].coord.x > x_min {
                x0 = self.vertices[v0_idx].coord.x;
            }
            if x0 > x_max {
                return;
            }
            y0 = (cc as f64 - a as f64 * x0 as f64) as f32;
            x1 = x_max;
            if v1_idx != NONE && self.vertices[v1_idx].coord.x < x_max {
                x1 = self.vertices[v1_idx].coord.x;
            }
            if x1 < x_min {
                return;
            }
            y1 = (cc as f64 - a as f64 * x1 as f64) as f32;
            if (y0 > y_max && y1 > y_max) || (y0 < y_min && y1 < y_min) {
                return;
            }
            if y0 > y_max {
                y0 = y_max;
                x0 = ((cc as f64 - y0 as f64) / a as f64) as f32;
            } else if y0 < y_min {
                y0 = y_min;
                x0 = ((cc as f64 - y0 as f64) / a as f64) as f32;
            }
            if y1 > y_max {
                y1 = y_max;
                x1 = ((cc as f64 - y1 as f64) / a as f64) as f32;
            } else if y1 < y_min {
                y1 = y_min;
                x1 = ((cc as f64 - y1 as f64) / a as f64) as f32;
            }
        }

        self.edges[e_idx].clipped = true;
        if v0_idx == self.edges[e_idx].left_vertex {
            self.edges[e_idx].clipped_left = Some(V2::new(x0, y0));
            self.edges[e_idx].clipped_right = Some(V2::new(x1, y1));
        } else {
            self.edges[e_idx].clipped_right = Some(V2::new(x0, y0));
            self.edges[e_idx].clipped_left = Some(V2::new(x1, y1));
        }
    }

    // ---- Region / neighbour extraction -----------------------------

    fn reorder_edges(&mut self, s: SiteIdx) {
        let orig = self.sites[s].edges.clone();
        let (ordered, orientations) = reorder_edges(&orig, VertexOrSite::Vertex, &self.edges, &self.vertices);
        self.sites[s].edges = ordered;
        self.sites[s].edge_orientations = Some(orientations);
    }

    fn compute_region(&mut self, s: SiteIdx) -> Vec<V2> {
        if self.sites[s].edges.is_empty() {
            return Vec::new();
        }
        if self.sites[s].edge_orientations.is_none() {
            self.reorder_edges(s);
            let region = self.clip_to_bounds(s);
            let mut poly = Polygon::new(region);
            if poly.winding() == Winding::Clockwise {
                poly.vertices.reverse();
            }
            self.sites[s].region = Some(poly.vertices);
        }
        self.sites[s].region.clone().unwrap_or_default()
    }

    fn neighbor_sites(&mut self, s: SiteIdx) -> Vec<SiteIdx> {
        if self.sites[s].edges.is_empty() {
            return Vec::new();
        }
        if self.sites[s].edge_orientations.is_none() {
            self.reorder_edges(s);
        }
        let mut out = Vec::with_capacity(self.sites[s].edges.len());
        for &e in &self.sites[s].edges.clone() {
            let n = if self.edges[e].left_site == s {
                self.edges[e].right_site
            } else if self.edges[e].right_site == s {
                self.edges[e].left_site
            } else {
                NONE
            };
            if n != NONE {
                out.push(n);
            }
        }
        out
    }

    fn clip_to_bounds(&self, s: SiteIdx) -> Vec<V2> {
        let bounds = self.plot_bounds;
        let edges = &self.sites[s].edges;
        let orients = self.sites[s].edge_orientations.as_ref().unwrap();
        let n = edges.len();
        let mut i = 0;
        while i < n && !self.edges[edges[i]].clipped {
            i += 1;
        }
        if i == n {
            return Vec::new();
        }
        let e0 = edges[i];
        let s0 = orients[i];
        let mut points: Vec<V2> = Vec::new();
        match s0 {
            Side::Left => {
                points.push(self.edges[e0].clipped_left.unwrap());
                points.push(self.edges[e0].clipped_right.unwrap());
            }
            Side::Right => {
                points.push(self.edges[e0].clipped_right.unwrap());
                points.push(self.edges[e0].clipped_left.unwrap());
            }
        }
        for j in (i + 1)..n {
            let e = edges[j];
            if self.edges[e].clipped {
                self.connect(&mut points, j, bounds, edges, orients, false);
            }
        }
        self.connect(&mut points, i, bounds, edges, orients, true);
        points
    }

    #[allow(clippy::too_many_arguments)]
    fn connect(
        &self,
        points: &mut Vec<V2>,
        j: usize,
        bounds: Rect,
        edges: &[EdgeIdx],
        orients: &[Side],
        closing_up: bool,
    ) {
        let prev = *points.last().unwrap();
        let e = edges[j];
        let side = orients[j];
        let this_end = match side {
            Side::Left => self.edges[e].clipped_left.unwrap(),
            Side::Right => self.edges[e].clipped_right.unwrap(),
        };
        if !close_enough(prev, this_end) {
            if prev.x != this_end.x && prev.y != this_end.y {
                let bc_prev = bounds_check(prev, bounds);
                let bc_this = bounds_check(this_end, bounds);
                let x_min = bounds.x;
                let y_min = bounds.y;
                let x_max = bounds.x + bounds.width;
                let y_max = bounds.y + bounds.height;
                if bc_prev & BC_RIGHT != 0 {
                    let xb = x_max;
                    if bc_this & BC_BOTTOM != 0 {
                        points.push(V2::new(xb, y_max));
                    } else if bc_this & BC_TOP != 0 {
                        points.push(V2::new(xb, y_min));
                    } else if bc_this & BC_LEFT != 0 {
                        let yb = if prev.y as f64 - bounds.y as f64 + this_end.y as f64 - bounds.y as f64 >= bounds.height as f64 {
                            y_max
                        } else {
                            y_min
                        };
                        points.push(V2::new(xb, yb));
                        points.push(V2::new(x_min, yb));
                    }
                } else if bc_prev & BC_LEFT != 0 {
                    let xb = x_min;
                    if bc_this & BC_BOTTOM != 0 {
                        points.push(V2::new(xb, y_max));
                    } else if bc_this & BC_TOP != 0 {
                        points.push(V2::new(xb, y_min));
                    } else if bc_this & BC_RIGHT != 0 {
                        let yb = if prev.y as f64 - bounds.y as f64 + this_end.y as f64 - bounds.y as f64 >= bounds.height as f64 {
                            y_max
                        } else {
                            y_min
                        };
                        points.push(V2::new(xb, yb));
                        points.push(V2::new(x_max, yb));
                    }
                } else if bc_prev & BC_TOP != 0 {
                    let yb = y_min;
                    if bc_this & BC_RIGHT != 0 {
                        points.push(V2::new(x_max, yb));
                    } else if bc_this & BC_LEFT != 0 {
                        points.push(V2::new(x_min, yb));
                    } else if bc_this & BC_BOTTOM != 0 {
                        let xb = if prev.x as f64 - bounds.x as f64 + this_end.x as f64 - bounds.x as f64 >= bounds.width as f64 {
                            x_max
                        } else {
                            x_min
                        };
                        points.push(V2::new(xb, yb));
                        points.push(V2::new(xb, y_max));
                    }
                } else if bc_prev & BC_BOTTOM != 0 {
                    let yb = y_max;
                    if bc_this & BC_RIGHT != 0 {
                        points.push(V2::new(x_max, yb));
                    } else if bc_this & BC_LEFT != 0 {
                        points.push(V2::new(x_min, yb));
                    } else if bc_this & BC_TOP != 0 {
                        let xb = if prev.x as f64 - bounds.x as f64 + this_end.x as f64 - bounds.x as f64 >= bounds.width as f64 {
                            x_max
                        } else {
                            x_min
                        };
                        points.push(V2::new(xb, yb));
                        points.push(V2::new(xb, y_min));
                    }
                }
            }
            if closing_up {
                return;
            }
            points.push(this_end);
        }
        let far = match side {
            Side::Left => self.edges[e].clipped_right.unwrap(),
            Side::Right => self.edges[e].clipped_left.unwrap(),
        };
        if !close_enough(points[0], far) {
            points.push(far);
        }
    }
}

fn close_enough(a: V2, b: V2) -> bool {
    dist(a, b) < 0.005
}

const BC_LEFT: u32 = 1;
const BC_RIGHT: u32 = 2;
const BC_TOP: u32 = 4;
const BC_BOTTOM: u32 = 8;

fn bounds_check(p: V2, b: Rect) -> u32 {
    let mut flags = 0u32;
    if p.x == b.x {
        flags |= BC_LEFT;
    }
    if p.x == b.x + b.width {
        flags |= BC_RIGHT;
    }
    if p.y == b.y {
        flags |= BC_TOP;
    }
    if p.y == b.y + b.height {
        flags |= BC_BOTTOM;
    }
    flags
}

fn compare_y_then_x(a: &Site, b: &Site) -> i32 {
    if a.coord.y < b.coord.y {
        return -1;
    }
    if a.coord.y > b.coord.y {
        return 1;
    }
    if a.coord.x < b.coord.x {
        return -1;
    }
    if a.coord.x > b.coord.x {
        return 1;
    }
    0
}

fn compare_y_then_x_site_pt(a: &Site, p: V2) -> i32 {
    if a.coord.y < p.y {
        return -1;
    }
    if a.coord.y > p.y {
        return 1;
    }
    if a.coord.x < p.x {
        return -1;
    }
    if a.coord.x > p.x {
        return 1;
    }
    0
}

/// `Vector2.Distance`: float differences, double sum and square root.
fn dist(a: V2, b: V2) -> f32 {
    let (x, y) = (a.x - b.x, a.y - b.y);
    (x as f64 * x as f64 + y as f64 * y as f64).sqrt() as f32
}

// ---- EdgeReorderer ----------------------------------------------------

fn reorder_edges(
    orig: &[EdgeIdx],
    criterion: VertexOrSite,
    edges_pool: &[Edge],
    vertex_pool: &[Vertex],
) -> (Vec<EdgeIdx>, Vec<Side>) {
    let count = orig.len();
    if count == 0 {
        return (Vec::new(), Vec::new());
    }
    let mut done = vec![false; count];
    let mut result: Vec<EdgeIdx> = Vec::new();
    let mut orientations: Vec<Side> = Vec::new();

    let idx = 0usize;
    let first = orig[idx];
    result.push(first);
    orientations.push(Side::Left);

    let (mut left_coord, mut right_coord);
    let (left_is_vertex_inf, right_is_vertex_inf);
    match criterion {
        VertexOrSite::Site => {
            left_coord = CoordRef::Site(edges_pool[first].left_site);
            right_coord = CoordRef::Site(edges_pool[first].right_site);
            left_is_vertex_inf = false;
            right_is_vertex_inf = false;
        }
        VertexOrSite::Vertex => {
            let lv = edges_pool[first].left_vertex;
            let rv = edges_pool[first].right_vertex;
            left_is_vertex_inf = lv == NONE || vertex_pool[lv].at_infinity;
            right_is_vertex_inf = rv == NONE || vertex_pool[rv].at_infinity;
            left_coord = CoordRef::Vertex(lv);
            right_coord = CoordRef::Vertex(rv);
        }
    }
    if left_is_vertex_inf || right_is_vertex_inf {
        return (Vec::new(), Vec::new());
    }
    done[idx] = true;
    let mut num_done = 1;

    while num_done < count {
        let mut progress = false;
        for i in 1..count {
            if done[i] {
                continue;
            }
            let e = orig[i];
            let (cand_left, cand_right);
            match criterion {
                VertexOrSite::Site => {
                    cand_left = CoordRef::Site(edges_pool[e].left_site);
                    cand_right = CoordRef::Site(edges_pool[e].right_site);
                }
                VertexOrSite::Vertex => {
                    let lv = edges_pool[e].left_vertex;
                    let rv = edges_pool[e].right_vertex;
                    let l_inf = lv == NONE || vertex_pool[lv].at_infinity;
                    let r_inf = rv == NONE || vertex_pool[rv].at_infinity;
                    if l_inf || r_inf {
                        return (Vec::new(), Vec::new());
                    }
                    cand_left = CoordRef::Vertex(lv);
                    cand_right = CoordRef::Vertex(rv);
                }
            }
            if cand_left == right_coord {
                right_coord = cand_right;
                orientations.push(Side::Left);
                result.push(e);
                done[i] = true;
                num_done += 1;
                progress = true;
            } else if cand_right == left_coord {
                left_coord = cand_left;
                orientations.insert(0, Side::Left);
                result.insert(0, e);
                done[i] = true;
                num_done += 1;
                progress = true;
            } else if cand_left == left_coord {
                left_coord = cand_right;
                orientations.insert(0, Side::Right);
                result.insert(0, e);
                done[i] = true;
                num_done += 1;
                progress = true;
            } else if cand_right == right_coord {
                right_coord = cand_left;
                orientations.push(Side::Right);
                result.push(e);
                done[i] = true;
                num_done += 1;
                progress = true;
            }
            let _ = idx;
        }
        if !progress {
            break;
        }
    }
    let _ = (left_is_vertex_inf, right_is_vertex_inf);
    (result, orientations)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CoordRef {
    Site(SiteIdx),
    Vertex(VertexIdx),
}

// ---- EdgeList (beachline) -------------------------------------------

struct ELList {
    xmin: f32,
    deltax: f32,
    hashsize: usize,
    hash: Vec<HalfedgeIdx>,
    left_end: HalfedgeIdx,
    right_end: HalfedgeIdx,
}

impl ELList {
    fn new(xmin: f32, deltax: f32, sqrt_n: i32, halfedges: &mut Vec<Halfedge>) -> Self {
        let hashsize = 2 * sqrt_n as usize;
        let mut hash = vec![NONE; hashsize];
        // Two dummy halfedges bracket the beachline.
        let left = halfedges.len();
        halfedges.push(Halfedge {
            edge_list_left: NONE,
            edge_list_right: NONE,
            next_in_pq: NONE,
            edge: NONE,
            left_right: None,
            vertex: NONE,
            ystar: 0.0,
        });
        let right = halfedges.len();
        halfedges.push(Halfedge {
            edge_list_left: NONE,
            edge_list_right: NONE,
            next_in_pq: NONE,
            edge: NONE,
            left_right: None,
            vertex: NONE,
            ystar: 0.0,
        });
        halfedges[left].edge_list_right = right;
        halfedges[right].edge_list_left = left;
        hash[0] = left;
        hash[hashsize - 1] = right;
        Self {
            xmin,
            deltax,
            hashsize,
            hash,
            left_end: left,
            right_end: right,
        }
    }

    fn insert(&mut self, lb: HalfedgeIdx, new_he: HalfedgeIdx, halfedges: &mut Vec<Halfedge>) {
        let lb_right = halfedges[lb].edge_list_right;
        halfedges[new_he].edge_list_left = lb;
        halfedges[new_he].edge_list_right = lb_right;
        halfedges[lb_right].edge_list_left = new_he;
        halfedges[lb].edge_list_right = new_he;
    }

    fn remove(&mut self, he: HalfedgeIdx, halfedges: &mut Vec<Halfedge>) {
        let l = halfedges[he].edge_list_left;
        let r = halfedges[he].edge_list_right;
        halfedges[l].edge_list_right = r;
        halfedges[r].edge_list_left = l;
        halfedges[he].edge = NONE; // mark deleted
        halfedges[he].edge_list_left = NONE;
        halfedges[he].edge_list_right = NONE;
    }

    fn left_neighbor(
        &mut self,
        p: V2,
        halfedges: &[Halfedge],
        edges: &[Edge],
        sites: &[Site],
    ) -> HalfedgeIdx {
        let mut bucket = ((p.x - self.xmin) / self.deltax * self.hashsize as f32) as isize;
        if bucket < 0 {
            bucket = 0;
        }
        if bucket >= self.hashsize as isize {
            bucket = self.hashsize as isize - 1;
        }
        let mut he = self.get_hash(bucket as usize, halfedges);
        if he == NONE {
            let mut k = 1isize;
            loop {
                let b_lo = bucket - k;
                let b_hi = bucket + k;
                he = if b_lo >= 0 { self.get_hash(b_lo as usize, halfedges) } else { NONE };
                if he != NONE {
                    break;
                }
                he = if b_hi < self.hashsize as isize {
                    self.get_hash(b_hi as usize, halfedges)
                } else {
                    NONE
                };
                if he != NONE {
                    break;
                }
                k += 1;
            }
        }
        if he == self.left_end
            || (he != self.right_end && is_left_of(&halfedges[he], p, edges, sites))
        {
            loop {
                he = halfedges[he].edge_list_right;
                if he == self.right_end || !is_left_of(&halfedges[he], p, edges, sites) {
                    break;
                }
            }
            he = halfedges[he].edge_list_left;
        } else {
            loop {
                he = halfedges[he].edge_list_left;
                if he == self.left_end || is_left_of(&halfedges[he], p, edges, sites) {
                    break;
                }
            }
        }
        if bucket > 0 && (bucket as usize) < self.hashsize - 1 {
            self.hash[bucket as usize] = he;
        }
        he
    }

    fn get_hash(&mut self, b: usize, halfedges: &[Halfedge]) -> HalfedgeIdx {
        if b >= self.hashsize {
            return NONE;
        }
        let he = self.hash[b];
        // Circle events remove arcs without immediately clearing every bucket.
        if he != NONE && he != self.left_end && he != self.right_end && halfedges[he].edge == NONE {
            self.hash[b] = NONE;
            return NONE;
        }
        he
    }
}

fn is_left_of(he: &Halfedge, p: V2, edges: &[Edge], sites: &[Site]) -> bool {
    let e_idx = he.edge;
    if e_idx == NONE {
        return false;
    }
    let e = &edges[e_idx];
    let topsite = &sites[e.right_site];
    let right_of_site = p.x > topsite.coord.x;
    let lr = he.left_right.unwrap();
    if right_of_site && lr == Side::Left {
        return true;
    }
    if !right_of_site && lr == Side::Right {
        return false;
    }
    let above;
    if e.a == 1.0 {
        let dyp = p.y - topsite.coord.y;
        let dxp = p.x - topsite.coord.x;
        let mut fast = false;
        let ab;
        if (!right_of_site && e.b < 0.0) || (right_of_site && e.b >= 0.0) {
            ab = dyp as f64 >= e.b as f64 * dxp as f64;
            fast = ab;
            above = ab;
        } else {
            let mut a = p.x as f64 + p.y as f64 * e.b as f64 > e.c as f64;
            if e.b < 0.0 {
                a = !a;
            }
            above = a;
            if !a {
                fast = true;
            }
        }
        if !fast {
            let dxs = topsite.coord.x - sites[e.left_site].coord.x;
            let (b, dxp, dyp, dxs) = (e.b as f64, dxp as f64, dyp as f64, dxs as f64);
            let ab2 = b * (dxp * dxp - dyp * dyp) < dxs * dyp * (1.0 + 2.0 * dxp / dxs + b * b);
            let ab2 = if e.b < 0.0 { !ab2 } else { ab2 };
            return if lr == Side::Left { ab2 } else { !ab2 };
        }
    } else {
        let yl = (e.c as f64 - e.a as f64 * p.x as f64) as f32;
        let t1 = (p.y - yl) as f64;
        let t2 = (p.x - topsite.coord.x) as f64;
        let t3 = (yl - topsite.coord.y) as f64;
        above = t1 * t1 > t2 * t2 + t3 * t3;
    }
    if lr == Side::Left { above } else { !above }
}

// ---- HalfedgePriorityQueue ----------------------------------------

struct HalfedgePq {
    hash: Vec<HalfedgeIdx>,
    count: usize,
    min_bucket: usize,
    hashsize: usize,
    ymin: f32,
    deltay: f32,
}

impl HalfedgePq {
    fn new(ymin: f32, deltay: f32, sqrt_n: i32) -> Self {
        let hashsize = 4 * sqrt_n as usize;
        Self {
            hash: vec![NONE; hashsize],
            count: 0,
            min_bucket: 0,
            hashsize,
            ymin,
            deltay,
        }
    }

    fn bucket(&self, he: &Halfedge) -> usize {
        let mut b = ((he.ystar - self.ymin) / self.deltay * self.hashsize as f32) as isize;
        if b < 0 {
            b = 0;
        }
        if b >= self.hashsize as isize {
            b = self.hashsize as isize - 1;
        }
        b as usize
    }

    fn insert(
        &mut self,
        he_idx: HalfedgeIdx,
        halfedges: &mut Vec<Halfedge>,
        vertices: &[Vertex],
    ) {
        let b = self.bucket(&halfedges[he_idx]);
        if b < self.min_bucket {
            self.min_bucket = b;
        }
        // Walk the bucket chain: head is self.hash[b] (a dummy slot
        // with .next_in_pq pointing to the first real halfedge).
        let he_y = halfedges[he_idx].ystar;
        let he_vx = if halfedges[he_idx].vertex != NONE {
            vertices[halfedges[he_idx].vertex].coord.x
        } else {
            0.0
        };
        let mut prev = NONE;
        let mut cur = self.hash[b];
        while cur != NONE {
            let cur_y = halfedges[cur].ystar;
            let cur_vx = if halfedges[cur].vertex != NONE {
                vertices[halfedges[cur].vertex].coord.x
            } else {
                0.0
            };
            if he_y > cur_y || (he_y == cur_y && he_vx > cur_vx) {
                prev = cur;
                cur = halfedges[cur].next_in_pq;
            } else {
                break;
            }
        }
        halfedges[he_idx].next_in_pq = cur;
        if prev == NONE {
            self.hash[b] = he_idx;
        } else {
            halfedges[prev].next_in_pq = he_idx;
        }
        self.count += 1;
    }

    fn remove(&mut self, he_idx: HalfedgeIdx, halfedges: &mut [Halfedge]) {
        if halfedges[he_idx].vertex == NONE {
            return;
        }
        let b = self.bucket(&halfedges[he_idx]);
        let mut prev = NONE;
        let mut cur = self.hash[b];
        while cur != NONE && cur != he_idx {
            prev = cur;
            cur = halfedges[cur].next_in_pq;
        }
        if cur != he_idx {
            return;
        }
        let next = halfedges[he_idx].next_in_pq;
        if prev == NONE {
            self.hash[b] = next;
        } else {
            halfedges[prev].next_in_pq = next;
        }
        self.count -= 1;
        halfedges[he_idx].vertex = NONE;
        halfedges[he_idx].next_in_pq = NONE;
    }

    fn empty(&self) -> bool {
        self.count == 0
    }

    fn adjust_min_bucket(&mut self) {
        while self.min_bucket < self.hashsize - 1 && self.hash[self.min_bucket] == NONE {
            self.min_bucket += 1;
        }
    }

    fn min(&mut self, halfedges: &[Halfedge], vertices: &[Vertex]) -> V2 {
        self.adjust_min_bucket();
        let he = self.hash[self.min_bucket];
        let y = halfedges[he].ystar;
        let x = if halfedges[he].vertex != NONE {
            vertices[halfedges[he].vertex].coord.x
        } else {
            0.0
        };
        V2::new(x, y)
    }

    fn extract_min(&mut self, halfedges: &mut [Halfedge]) -> HalfedgeIdx {
        self.adjust_min_bucket();
        let he = self.hash[self.min_bucket];
        let next = halfedges[he].next_in_pq;
        self.hash[self.min_bucket] = next;
        self.count -= 1;
        halfedges[he].next_in_pq = NONE;
        he
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removed_beachline_arc_is_not_reused_from_hash() {
        let mut halfedges = Vec::new();
        let mut list = ELList::new(0.0, 10.0, 4, &mut halfedges);
        let arc = halfedges.len();
        halfedges.push(Halfedge {
            edge_list_left: NONE, edge_list_right: NONE, next_in_pq: NONE,
            edge: 0, left_right: Some(Side::Left), vertex: NONE, ystar: 0.0,
        });
        list.insert(list.left_end, arc, &mut halfedges);
        list.hash[3] = arc;
        assert_eq!(list.get_hash(3, &halfedges), arc);
        list.remove(arc, &mut halfedges);
        assert_eq!(list.get_hash(3, &halfedges), NONE);
        assert_eq!(list.get_hash(0, &halfedges), list.left_end);
        assert_eq!(list.get_hash(list.hashsize - 1, &halfedges), list.right_end);
        // A query in the stale bucket must fall back to the live beachline.
        assert_eq!(list.left_neighbor(V2::new(4.0, 5.0), &halfedges, &[], &[]), list.left_end);
    }

    fn basic_sites() -> (Vec<V2>, Vec<u32>, Vec<f32>) {
        let pts: Vec<V2> = vec![
            V2::new(1.0, 1.0),
            V2::new(9.0, 1.0),
            V2::new(1.0, 9.0),
            V2::new(9.0, 9.0),
            V2::new(5.0, 5.0),
        ];
        let ids: Vec<u32> = (0..5u32).collect();
        let weights = vec![1.0f32; 5];
        (pts, ids, weights)
    }

    #[test]
    fn five_site_diagram_builds() {
        let (pts, ids, w) = basic_sites();
        let bounds = Rect::new(0.0, 0.0, 10.0, 10.0);
        let v = Voronoi::new(&pts, &ids, &w, bounds);
        assert!(v.edge_count() > 0);
    }

    /// Cross-check: every Delaunay edge produced by `miconvexhull`
    /// must appear as a Voronoi edge between the same two sites.
    /// Fortune's edges carry site indices via `left_site` /
    /// `right_site`; mapping those through `Site.color` (= input
    /// index) should give the same unordered pairs as MIConvexHull.
    #[test]
    fn topology_matches_miconvexhull() {
        use crate::miconvexhull::delaunay_2d;
        use std::collections::HashSet;
        let (pts, ids, w) = basic_sites();
        let bounds = Rect::new(0.0, 0.0, 10.0, 10.0);
        let v = Voronoi::new(&pts, &ids, &w, bounds);

        let mut fortune_pairs: HashSet<(u32, u32)> = HashSet::new();
        for e in &v.edges {
            let a = v.sites[e.left_site].color;
            let b = v.sites[e.right_site].color;
            let (lo, hi) = if a < b { (a, b) } else { (b, a) };
            fortune_pairs.insert((lo, hi));
        }

        let pts64: Vec<(f64, f64)> = pts.iter().map(|p| (p.x as f64, p.y as f64)).collect();
        let mut mi_pairs: HashSet<(u32, u32)> = HashSet::new();
        for t in delaunay_2d(&pts64) {
            for &(x, y) in &[(t[0], t[1]), (t[1], t[2]), (t[0], t[2])] {
                let (lo, hi) = if x < y { (x as u32, y as u32) } else { (y as u32, x as u32) };
                mi_pairs.insert((lo, hi));
            }
        }

        assert_eq!(
            fortune_pairs, mi_pairs,
            "fortune vs miconvexhull edge topology differs"
        );
    }

    #[test]
    fn centre_site_region_has_four_neighbours() {
        let (pts, ids, w) = basic_sites();
        let bounds = Rect::new(0.0, 0.0, 10.0, 10.0);
        let mut v = Voronoi::new(&pts, &ids, &w, bounds);
        let ids = v.neighbor_site_ids(V2::new(5.0, 5.0));
        assert_eq!(ids.len(), 4, "ids = {:?}", ids);
    }
}
