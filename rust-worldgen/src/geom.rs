//! Port of the `Delaunay.Geo` polygon primitives used by
//! `WorldLayout` and `VoronoiTree`. Only the subset actually
//! reachable from worldgen is ported; Unity-specific debug draws,
//! `ClipperLib`-backed intersection, and the SAT clip variant are
//! intentionally omitted until a caller needs them.
//!
//! Conventions follow the C# source: f32 throughout (Unity `Vector2`
//! is f32), 2D only, ccw / cw winding as computed from the signed
//! double area.

use std::cmp::Ordering;

/// 2D point in the same coordinate space ONI uses for a world (cells).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct V2 {
    pub x: f32,
    pub y: f32,
}

impl V2 {
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    pub const ZERO: V2 = V2 { x: 0.0, y: 0.0 };

    pub fn sub(self, o: V2) -> V2 {
        V2::new(self.x - o.x, self.y - o.y)
    }
    pub fn add(self, o: V2) -> V2 {
        V2::new(self.x + o.x, self.y + o.y)
    }
    pub fn mul(self, k: f32) -> V2 {
        V2::new(self.x * k, self.y * k)
    }
    pub fn dot(self, o: V2) -> f32 {
        self.x * o.x + self.y * o.y
    }
    pub fn cross(self, o: V2) -> f32 {
        self.x * o.y - self.y * o.x
    }
    pub fn len(self) -> f32 {
        (self.x * self.x + self.y * self.y).sqrt()
    }
    pub fn distance(self, o: V2) -> f32 {
        self.sub(o).len()
    }
}

/// Axis-aligned rect, analogue of Unity's `Rect`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Rect {
    pub const fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self { x, y, width, height }
    }
    pub fn min_max(x_min: f32, y_min: f32, x_max: f32, y_max: f32) -> Self {
        Self::new(x_min, y_min, x_max - x_min, y_max - y_min)
    }
    pub fn contains(self, p: V2) -> bool {
        p.x >= self.x
            && p.x <= self.x + self.width
            && p.y >= self.y
            && p.y <= self.y + self.height
    }
}

/// Matches the C# `Winding` enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Winding {
    None,
    Clockwise,
    CounterClockwise,
}

/// Matches the C# `Polygon.Commonality` enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Commonality {
    None,
    Point,
    Edge,
}

/// Line segment with optional endpoints. The C# source uses
/// `Vector2?` — we use `Option<V2>` with the same nullability.
#[derive(Debug, Clone, Copy)]
pub struct LineSegment {
    pub p0: Option<V2>,
    pub p1: Option<V2>,
}

impl LineSegment {
    pub const fn new(p0: Option<V2>, p1: Option<V2>) -> Self {
        Self { p0, p1 }
    }

    pub fn from_pts(p0: V2, p1: V2) -> Self {
        Self::new(Some(p0), Some(p1))
    }

    pub fn direction(&self) -> V2 {
        match (self.p0, self.p1) {
            (Some(a), Some(b)) => b.sub(a),
            _ => V2::ZERO,
        }
    }

    pub fn center(&self) -> Option<V2> {
        match (self.p0, self.p1) {
            (Some(a), Some(b)) => Some(a.add(b.sub(a).mul(0.5))),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        }
    }

    /// Port of `LineSegment.Intersection(a1, a2, b1, b2)` returning
    /// 0, 1, or 2 points. Matches the C# branching exactly so
    /// numerical results are equivalent.
    pub fn intersection_pts(a1: V2, a2: V2, b1: V2, b2: V2) -> Vec<V2> {
        let eps = f32::EPSILON;
        if a1 == a2 && b1 == b2 {
            if a1 == b1 {
                return vec![a1];
            }
            return vec![];
        }
        if b1 == b2 {
            if point_on_line(b1, a1, a2) {
                return vec![b1];
            }
            return vec![];
        }
        if a1 == a2 {
            if point_on_line(a1, b1, b2) {
                return vec![a1];
            }
            return vec![];
        }
        let num = (b2.x - b1.x) * (a1.y - b1.y) - (b2.y - b1.y) * (a1.x - b1.x);
        let num2 = (a2.x - a1.x) * (a1.y - b1.y) - (a2.y - a1.y) * (a1.x - b1.x);
        let num3 = (b2.y - b1.y) * (a2.x - a1.x) - (b2.x - b1.x) * (a2.y - a1.y);
        if !(-eps < num3 && num3 < eps) {
            let u = num / num3;
            let v = num2 / num3;
            if 0.0 <= u && u <= 1.0 && 0.0 <= v && v <= 1.0 {
                return vec![V2::new(
                    a1.x + u * (a2.x - a1.x),
                    a1.y + u * (a2.y - a1.y),
                )];
            }
            return vec![];
        }
        if (-eps < num && num < eps) || (-eps < num2 && num2 < eps) {
            if a1 == a2 {
                return one_d_intersection(b1, b2, a1, a2);
            }
            return one_d_intersection(a1, a2, b1, b2);
        }
        vec![]
    }

    pub fn does_intersect(a: &LineSegment, b: &LineSegment) -> bool {
        match (a.p0, a.p1, b.p0, b.p1) {
            (Some(a0), Some(a1), Some(b0), Some(b1)) => {
                !Self::intersection_pts(a0, a1, b0, b1).is_empty()
            }
            _ => false,
        }
    }
}

fn overlap_intervals(ub1: f32, ub2: f32) -> Vec<f32> {
    let lo = ub1.min(ub2);
    let hi = ub1.max(ub2);
    let a = lo.max(0.0);
    let b = hi.min(1.0);
    if a > b {
        return vec![];
    }
    if a != b {
        return vec![a, b];
    }
    vec![a]
}

fn one_d_intersection(a1: V2, a2: V2, b1: V2, b2: V2) -> Vec<V2> {
    let dx = a2.x - a1.x;
    let dy = a2.y - a1.y;
    let (ub1, ub2) = if dx.abs() > dy.abs() {
        ((b1.x - a1.x) / dx, (b2.x - a1.x) / dx)
    } else {
        ((b1.y - a1.y) / dy, (b2.y - a1.y) / dy)
    };
    overlap_intervals(ub1, ub2)
        .into_iter()
        .map(|t| V2::new(a2.x * t + a1.x * (1.0 - t), a2.y * t + a1.y * (1.0 - t)))
        .collect()
}

fn point_on_line(p: V2, a1: V2, a2: V2) -> bool {
    dist_from_seg(p, a1, a2) < f32::EPSILON as f64
}

fn dist_from_seg(p: V2, q0: V2, q1: V2) -> f64 {
    let dx = (q1.x - q0.x) as f64;
    let dy = (q1.y - q0.y) as f64;
    let px = (q0.x - p.x) as f64;
    let py = (q0.y - p.y) as f64;
    let len = (dx * dx + dy * dy).sqrt();
    if len < f32::EPSILON as f64 {
        panic!("dist_from_seg expects a real segment, not a point");
    }
    (dx * py - px * dy).abs() / len
}

/// Port of `Delaunay.Geo.Polygon`. `vertices` is a closed loop
/// (last vertex connects back to first). Winding is computed from
/// signed area and can be forced cw/ccw.
#[derive(Debug, Clone)]
pub struct Polygon {
    pub vertices: Vec<V2>,
    pub bounds: Rect,
    centroid: Option<V2>,
}

impl Polygon {
    pub fn new(verts: Vec<V2>) -> Self {
        let mut p = Self {
            vertices: verts,
            bounds: Rect::new(0.0, 0.0, 0.0, 0.0),
            centroid: None,
        };
        p.refresh_bounds();
        p
    }

    /// `new Polygon(Rect)` in C# — builds a 4-vertex rectangle
    /// polygon in (x, y) to (x+w, y+h) order, ccw.
    pub fn from_rect(r: Rect) -> Self {
        Self::new(vec![
            V2::new(r.x, r.y),
            V2::new(r.x + r.width, r.y),
            V2::new(r.x + r.width, r.y + r.height),
            V2::new(r.x, r.y + r.height),
        ])
    }

    pub fn add(&mut self, v: V2) {
        self.vertices.push(v);
        self.centroid = None;
    }

    pub fn refresh_bounds(&mut self) {
        let mut min = V2::new(f32::MAX, f32::MAX);
        let mut max = V2::new(f32::MIN, f32::MIN);
        for v in &self.vertices {
            if v.x < min.x {
                min.x = v.x;
            }
            if v.y < min.y {
                min.y = v.y;
            }
            if v.x > max.x {
                max.x = v.x;
            }
            if v.y > max.y {
                max.y = v.y;
            }
        }
        self.bounds = Rect::min_max(min.x, min.y, max.x, max.y);
    }

    pub fn signed_double_area(&self) -> f32 {
        let n = self.vertices.len();
        let mut acc = 0.0f32;
        for i in 0..n {
            let j = (i + 1) % n;
            let a = self.vertices[i];
            let b = self.vertices[j];
            // ONI's Mono evaluates products/subtraction at double precision,
            // then rounds the running float accumulator after each edge.
            acc = (acc as f64 + (a.x as f64 * b.y as f64 - b.x as f64 * a.y as f64)) as f32;
        }
        acc
    }

    pub fn area(&self) -> f32 {
        (self.signed_double_area() * 0.5).abs()
    }

    pub fn winding(&self) -> Winding {
        let s = self.signed_double_area();
        match s.partial_cmp(&0.0) {
            Some(Ordering::Less) => Winding::Clockwise,
            Some(Ordering::Greater) => Winding::CounterClockwise,
            _ => Winding::None,
        }
    }

    pub fn force_winding(&mut self, w: Winding) {
        if self.winding() != w {
            self.vertices.reverse();
            self.centroid = None;
        }
    }

    pub fn centroid(&mut self) -> V2 {
        if let Some(c) = self.centroid {
            return c;
        }
        let mut c = V2::ZERO;
        if self.vertices.len() > 1 {
            let a = self.area();
            let mut j = 1usize;
            for i in 0..self.vertices.len() {
                let vi = self.vertices[i];
                let vj = self.vertices[j];
                let cross = (vi.x as f64 * vj.y as f64 - vj.x as f64 * vi.y as f64) as f32;
                c = c.add(V2::new(
                    ((vi.x as f64 + vj.x as f64) * cross as f64) as f32,
                    ((vi.y as f64 + vj.y as f64) * cross as f64) as f32,
                ));
                j = (j + 1) % self.vertices.len();
            }
            c = V2::new(c.x / (6.0 * a), c.y / (6.0 * a));
        }
        self.centroid = Some(c);
        c
    }

    pub fn contains(&self, p: V2) -> bool {
        self.point_in_polygon(p)
    }

    pub fn point_in_polygon(&self, p: V2) -> bool {
        if !self.bounds.contains(p) {
            return false;
        }
        let n = self.vertices.len();
        let mut j = n - 1;
        let mut flag = false;
        for i in 0..n {
            let vi = self.vertices[i];
            let vj = self.vertices[j];
            // Mono evaluates the crossing and the comparison in double precision;
            // the intermediate is never stored, so it is not rounded to float.
            if ((vi.y <= p.y && p.y < vj.y) || (vj.y <= p.y && p.y < vi.y))
                && (p.x as f64) < (vj.x as f64 - vi.x as f64) * (p.y as f64 - vi.y as f64)
                    / (vj.y as f64 - vi.y as f64) + vi.x as f64
            {
                flag = !flag;
            }
            j = i;
        }
        flag
    }

    pub fn get_edge(&self, edge_index: usize) -> LineSegment {
        LineSegment::from_pts(
            self.vertices[edge_index],
            self.vertices[(edge_index + 1) % self.vertices.len()],
        )
    }

    /// False when [`shares_edge`](Self::shares_edge) cannot report an edge:
    /// the bounds are more than a margin apart, so no pair of edges has
    /// intersecting bounds. The margin covers the float step `unity_bounds`
    /// can move an edge's box by.
    pub fn may_share_edge(&self, other: &Polygon) -> bool {
        const MARGIN: f32 = 0.01;
        let (a, b) = (self.bounds, other.bounds);
        a.x - MARGIN <= b.x + b.width && b.x - MARGIN <= a.x + a.width
            && a.y - MARGIN <= b.y + b.height && b.y - MARGIN <= a.y + a.height
    }

    /// Port of `Polygon.SharesEdge(other, ref edgeIdx, out overlap)`.
    ///
    /// Returns the edge-sharing relationship between `self` and
    /// `other`, the index of `self`'s edge that overlaps (when the
    /// result is `Edge`), and the overlap segment itself.
    ///
    /// The algorithm walks edge pairs, rejects pairs whose bounding
    /// boxes don't intersect, and accepts pairs whose endpoints are
    /// collinear (cross-products near zero). It preserves the exact
    /// branching, tolerance (0.001) and ordering tricks of the C#.
    pub fn shares_edge(&self, other: &Polygon) -> (Commonality, Option<usize>, Option<LineSegment>) {
        let mut result = Commonality::None;
        let mut num = self.vertices.len() - 1;
        for num2 in 0..self.vertices.len() {
            let v0 = self.vertices[num];
            let v1 = self.vertices[num2];
            let (bmin, bmax) = unity_bounds(v0, v1);
            let mut index = other.vertices.len() - 1;
            for num3 in 0..other.vertices.len() {
                let o0 = other.vertices[index];
                let o1 = other.vertices[num3];

                let endpoint_matches = (if o1.distance(v1) < 0.001 { 1 } else { 0 })
                    + (if o1.distance(v0) < 0.001 { 1 } else { 0 })
                    + (if o0.distance(v1) < 0.001 { 1 } else { 0 })
                    + (if o0.distance(v0) < 0.001 { 1 } else { 0 });
                if endpoint_matches == 1 {
                    result = Commonality::Point;
                }

                let (bmin2, bmax2) = unity_bounds(o0, o1);
                if !aabb_intersect(bmin, bmax, bmin2, bmax2) {
                    index = num3;
                    continue;
                }

                // Collinearity via two cross products.
                let f1 = ((v1.x as f64 - v0.x as f64) * (o0.y as f64 - v0.y as f64) - (o0.x as f64 - v0.x as f64) * (v1.y as f64 - v0.y as f64)) as f32;
                let f2 = ((o1.x as f64 - o0.x as f64) * (v0.y as f64 - o0.y as f64) - (v0.x as f64 - o0.x as f64) * (o1.y as f64 - o0.y as f64)) as f32;
                if !(f1.abs() < 0.001 && f2.abs() < 0.001) {
                    index = num3;
                    continue;
                }

                // Choose the "smaller" endpoint on each of the two
                // segments in the same lex order the C# uses.
                let (a0, a1) = lex_sort(v0, v1);
                let (c0, c1) = lex_sort(o0, o1);
                let (_p0_near, p0_far, p1_near, p1_far) =
                    if a0.x < c0.x || (a0.x == c0.x && a0.y < c0.y) {
                        (a0, a1, c0, c1)
                    } else {
                        (c0, c1, a0, a1)
                    };

                if p0_far.distance(p1_near) < 0.001 {
                    result = Commonality::Point;
                } else if p0_far.x - p1_near.x > 0.0
                    || (p0_far.x - p1_near.x == 0.0 && p0_far.y - p1_near.y > 0.0)
                {
                    let overlap = if p0_far.x - p1_far.x > 0.0
                        || (p0_far.x - p1_far.x == 0.0 && p0_far.y - p1_near.y > 0.0)
                    {
                        LineSegment::from_pts(p1_near, p0_far)
                    } else {
                        LineSegment::from_pts(p1_near, p1_far)
                    };
                    return (Commonality::Edge, Some(num), Some(overlap));
                }

                index = num3;
            }
            num = num2;
        }
        (result, None, None)
    }
}

/// `new Bounds(a, Vector3.zero).Encapsulate(b)` as min/max. Unity stores
/// centre and extents (`SetMinMax`: extents = (max - min) * 0.5, centre =
/// min + extents) and `Bounds.Intersects` reads min/max back as centre -/+
/// extents, so the edges can move by a float step. That decides whether
/// regions touching at an axis-aligned boundary count as sharing an edge.
fn unity_bounds(a: V2, b: V2) -> (V2, V2) {
    let (min, max) = (V2::new(a.x.min(b.x), a.y.min(b.y)), V2::new(a.x.max(b.x), a.y.max(b.y)));
    let extents = V2::new((max.x - min.x) * 0.5, (max.y - min.y) * 0.5);
    let centre = V2::new(min.x + extents.x, min.y + extents.y);
    (V2::new(centre.x - extents.x, centre.y - extents.y), V2::new(centre.x + extents.x, centre.y + extents.y))
}

fn aabb_intersect(amin: V2, amax: V2, bmin: V2, bmax: V2) -> bool {
    amin.x <= bmax.x && amax.x >= bmin.x && amin.y <= bmax.y && amax.y >= bmin.y
}

fn lex_sort(a: V2, b: V2) -> (V2, V2) {
    if a.x < b.x || (a.x == b.x && a.y < b.y) {
        (a, b)
    } else {
        (b, a)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square() -> Polygon {
        Polygon::from_rect(Rect::new(0.0, 0.0, 10.0, 10.0))
    }

    #[test]
    fn rect_polygon_has_ccw_winding_and_area_100() {
        let p = square();
        assert_eq!(p.winding(), Winding::CounterClockwise);
        assert!((p.area() - 100.0).abs() < 1e-4);
    }

    #[test]
    fn force_winding_reverses_cw() {
        let mut p = square();
        p.force_winding(Winding::Clockwise);
        assert_eq!(p.winding(), Winding::Clockwise);
        // Area unchanged by reversal.
        assert!((p.area() - 100.0).abs() < 1e-4);
    }

    #[test]
    fn centroid_of_square() {
        let mut p = square();
        let c = p.centroid();
        assert!((c.x - 5.0).abs() < 1e-4);
        assert!((c.y - 5.0).abs() < 1e-4);
    }

    #[test]
    fn centroid_matches_live_mono_polygon() {
        // Build 744825, seed 42, TundraMoonlet site 2 after initial Voronoi.
        let mut p = Polygon::new(vec![
            V2::new(25.47760009765625, 80.00180053710938),
            V2::new(26.5447998046875, 95.89889526367188),
            V2::new(20.16390037536621, 102.92949676513672),
            V2::new(13.019999504089355, 101.18719482421875),
            V2::new(5.81279993057251, 82.17440032958984),
            V2::new(18.25389862060547, 73.57540130615234),
        ]);
        let c = p.centroid();
        assert_eq!(c.x.to_bits(), 1099608333);
        assert_eq!(c.y.to_bits(), 1118875485);
    }

    #[test]
    fn contains_inside_and_outside() {
        let p = square();
        assert!(p.contains(V2::new(5.0, 5.0)));
        assert!(!p.contains(V2::new(-1.0, 5.0)));
        assert!(!p.contains(V2::new(11.0, 5.0)));
    }

    #[test]
    fn near_collinear_live_regions_do_not_share_an_edge() {
        // Build 744825, seed 44, IdealLandingSite regions 13 and 38.
        // Float32 intermediate cross products incorrectly introduce an edge.
        let a = Polygon::new(vec![
            V2::new(60.726898193359375, 83.94229888916016),
            V2::new(61.228797912597656, 85.32170104980469),
            V2::new(57.66489791870117, 95.6865005493164),
            V2::new(47.4484977722168, 92.3158950805664),
            V2::new(44.77069854736328, 85.94210052490234),
            V2::new(46.815399169921875, 82.29869842529297),
        ]);
        let b = Polygon::new(vec![
            V2::new(54.0802001953125, 69.35359954833984),
            V2::new(46.53999710083008, 82.78929901123047),
            V2::new(41.93769836425781, 76.21419525146484),
            V2::new(44.69089889526367, 68.36399841308594),
            V2::new(51.84700012207031, 66.01840209960938),
        ]);
        assert_ne!(b.shares_edge(&a).0, Commonality::Edge);
    }

    /// Two adjacent unit squares share one full edge.
    #[test]
    fn shares_edge_adjacent_squares() {
        let a = Polygon::from_rect(Rect::new(0.0, 0.0, 1.0, 1.0));
        let b = Polygon::from_rect(Rect::new(1.0, 0.0, 1.0, 1.0));
        let (c, _, seg) = a.shares_edge(&b);
        assert_eq!(c, Commonality::Edge);
        let seg = seg.expect("overlap segment");
        // The shared edge runs from (1,0) to (1,1) (or reversed).
        let p0 = seg.p0.unwrap();
        let p1 = seg.p1.unwrap();
        assert!((p0.x - 1.0).abs() < 1e-3 && (p1.x - 1.0).abs() < 1e-3);
    }

    /// Two squares touching only at a corner share a point, not an edge.
    #[test]
    fn shares_point_at_corner() {
        let a = Polygon::from_rect(Rect::new(0.0, 0.0, 1.0, 1.0));
        let b = Polygon::from_rect(Rect::new(1.0, 1.0, 1.0, 1.0));
        let (c, _, _) = a.shares_edge(&b);
        assert_eq!(c, Commonality::Point);
    }

    /// Disjoint squares share nothing.
    #[test]
    fn shares_nothing_disjoint() {
        let a = Polygon::from_rect(Rect::new(0.0, 0.0, 1.0, 1.0));
        let b = Polygon::from_rect(Rect::new(5.0, 5.0, 1.0, 1.0));
        let (c, _, _) = a.shares_edge(&b);
        assert_eq!(c, Commonality::None);
    }

    #[test]
    fn line_segment_intersection_midpoint() {
        let pts = LineSegment::intersection_pts(
            V2::new(0.0, 0.0),
            V2::new(2.0, 2.0),
            V2::new(0.0, 2.0),
            V2::new(2.0, 0.0),
        );
        assert_eq!(pts.len(), 1);
        assert!((pts[0].x - 1.0).abs() < 1e-4);
        assert!((pts[0].y - 1.0).abs() < 1e-4);
    }

    #[test]
    fn line_segment_no_intersection() {
        let pts = LineSegment::intersection_pts(
            V2::new(0.0, 0.0),
            V2::new(1.0, 0.0),
            V2::new(0.0, 1.0),
            V2::new(1.0, 1.0),
        );
        assert!(pts.is_empty());
    }
}
