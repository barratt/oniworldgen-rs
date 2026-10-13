//! Port of `VoronoiTree.*` — tree of Voronoi nodes with the
//! PowerDiagram iteration used by ONI's worldgen.
//!
//! Scope: the `usePD=true` path only (every ONI world's YAML sets
//! `layoutMethod: PowerTree`, so the non-PD branch is dead code for
//! our purposes). Covers:
//!
//! * `PowerDiagramSite` — a site with weight, polygon and neighbours
//! * `PowerDiagram` — builds the 3D lifted convex hull via
//!   `miconvexhull`, extracts per-site polygons from circumcentres of
//!   incident Delaunay faces, iterates weights to convergence.
//! * `Tree` / `Node` — recursive container: each node has a Voronoi
//!   sub-diagram over child sites, with neighbour lookup by site id.
//!
//! Non-goals: `Diagram` (the non-PD wrapper), `Leaf` serialization
//! hooks, debug-draw hooks, Kruskal/MST helpers.

use crate::geom::{Polygon, V2, Winding};
use crate::miconvexhull::{convex_hull_2d, delaunay_2d_with_adjacency, lower_hull_3d, DelaunayFace, NO_ADJ};

pub const FORCED_WINDING: Winding = Winding::CounterClockwise;

/// Entry of the game's `site.neighbours` during the power iterations. ComputeVD
/// stores the Delaunay cells touching a site (default-constructed sites at the
/// origin with zero weight); ComputePD stores real and external-edge sites.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PdNeighbour {
    Site(usize),
    External(usize),
    Face,
}

/// Power-diagram site. Holds the weight used in the lift
/// `(x, y, x²+y²-w)`, plus neighbour ids and its assigned polygon
/// after a `ComputeVD`/`ComputePowerDiagram` round.
#[derive(Debug, Clone)]
pub struct PowerDiagramSite {
    pub id: i32,
    pub position: V2,
    pub weight: f32,
    pub current_weight: f32,
    pub previous_weight_adaption: f32,
    pub poly: Option<Polygon>,
    /// Ids of the non-dummy entries of `pd_neighbours`, as Node.ComputeNodePD
    /// collects them (or the Fortune neighbours after `compute_node_voronoi`).
    pub neighbours: Vec<i32>,
    pub pd_neighbours: Vec<PdNeighbour>,
    /// `true` for the 8 external-edge sites PowerDiagram adds around
    /// the bounds to keep the convex hull closed.
    pub dummy: bool,
}

impl PowerDiagramSite {
    pub fn new(id: i32, position: V2, weight: f32) -> Self {
        Self {
            id,
            position,
            weight,
            current_weight: weight,
            // Real sites use the game's (id, position, weight) constructor.
            previous_weight_adaption: 0.0,
            poly: None,
            neighbours: Vec::new(),
            pd_neighbours: Vec::new(),
            dummy: false,
        }
    }

    fn dummy(position: V2) -> Self {
        Self {
            id: -1,
            position,
            weight: f32::from_bits(1),
            current_weight: f32::from_bits(1),
            previous_weight_adaption: 1.0,
            poly: None,
            neighbours: Vec::new(),
            pd_neighbours: Vec::new(),
            dummy: true,
        }
    }

    fn lifted(&self) -> (f64, f64, f64) {
        // Mono retains expression precision until DualSite3d stores Vector3.
        let xf = self.position.x;
        let yf = self.position.y;
        let zf = (xf as f64 * xf as f64 + yf as f64 * yf as f64 - self.current_weight as f64) as f32;
        (xf as f64, yf as f64, zf as f64)
    }
}

/// `VoronoiTree.PowerDiagram`.
pub struct PowerDiagram {
    pub bounds: Polygon,
    pub sites: Vec<PowerDiagramSite>,
    pub external_edge_points: Vec<PowerDiagramSite>,
    pub weight_sum: f32,
    pub completed_iterations: u32,
    /// `DualSite2d.visited`; the dual sites live as long as the diagram.
    vd_visited: Vec<bool>,
}

impl PowerDiagram {
    pub fn new(mut bounds: Polygon, new_sites: Vec<PowerDiagramSite>) -> Self {
        bounds.force_winding(FORCED_WINDING);
        let mut pd = Self {
            bounds,
            sites: Vec::new(),
            external_edge_points: Vec::new(),
            weight_sum: 0.0,
            completed_iterations: 0,
            vd_visited: Vec::new(),
        };
        for s in new_sites {
            if pd.bounds.contains(s.position) {
                pd.weight_sum += s.weight;
                pd.sites.push(s);
            }
        }
        // Mirror PowerDiagram ctor: for each bounds edge, add 2 dummy
        // sites far outside, so the 3D convex hull encloses the real
        // sites without any hull-boundary artefacts near them.
        let mut centroid_poly = pd.bounds.clone();
        let centre = centroid_poly.centroid();
        let verts = pd.bounds.vertices.clone();
        for i in 0..verts.len() {
            let a = verts[i];
            let b = verts[(i + 1) % verts.len()];
            let outward = {
                let d = a.sub(centre);
                let len = ((d.x as f64 * d.x as f64 + d.y as f64 * d.y as f64).sqrt()) as f32;
                if len > 1e-5 { V2::new(d.x / len * 1000.0, d.y / len * 1000.0) } else { V2::ZERO }
            };
            pd.external_edge_points.push(PowerDiagramSite::dummy(a.add(outward)));
            let mid = V2::new(
                (b.x - a.x) * 0.5 + b.x - centre.x,
                (b.y - a.y) * 0.5 + b.y - centre.y,
            );
            let len = ((mid.x as f64 * mid.x as f64 + mid.y as f64 * mid.y as f64).sqrt()) as f32;
            let outward_mid = if len > 1e-5 { V2::new(mid.x / len * 1000.0, mid.y / len * 1000.0) } else { V2::ZERO };
            pd.external_edge_points.push(PowerDiagramSite::dummy(b.add(outward_mid)));
        }
        pd.vd_visited = vec![false; pd.sites.len() + pd.external_edge_points.len()];
        pd
    }

    /// `PowerDiagram.ComputeVD`: unweighted Delaunay over the dual sites. Each
    /// real site gets the clipped hull of its touching cells' circumcentres, and
    /// those cells as its neighbours. `Err` where the game throws.
    pub fn compute_vd(&mut self) -> Result<(), String> {
        let n_real = self.sites.len();
        let pts2d: Vec<(f64, f64)> = self.sites.iter().chain(&self.external_edge_points)
            .map(|s| (s.position.x as f64, s.position.y as f64)).collect();
        let faces = delaunay_2d_with_adjacency(&pts2d);
        let circumcentres: Vec<V2> = faces.iter().map(|f| circumcentre_2d(&pts2d, f)).collect();
        for fi in 0..faces.len() {
            for v in faces[fi].vertices {
                if self.vd_visited[v] { continue; }
                self.vd_visited[v] = true;
                if v >= n_real { continue; }
                let touching = touching_faces(&faces, v, fi, false);
                self.sites[v].pd_neighbours = vec![PdNeighbour::Face; touching.len()];
                self.sites[v].neighbours.clear();
                if !touching.is_empty() {
                    let points: Vec<V2> = touching.iter().map(|&i| circumcentres[i]).collect();
                    self.sites[v].poly = clipped_cell(&points, &self.bounds, self.sites[v].id)?;
                }
            }
        }
        Ok(())
    }

    /// `PowerDiagram.ComputePD`: lower hull of the lifted sites. Sites without
    /// a lower face keep their previous polygon and neighbours, as do clipped
    /// polygons that vanish. `Err` where the game throws, leaving the sites
    /// already visited updated.
    pub fn compute_pd(&mut self) -> Result<(), String> {
        let n_real = self.sites.len();
        let lifted: Vec<(f64, f64, f64)> = self.sites.iter().chain(&self.external_edge_points)
            .map(|s| s.lifted()).collect();
        let faces = lower_hull_3d(&lifted);
        let dual_points: Vec<V2> = faces.iter().map(|f| power_centre_2d(&lifted, f)).collect();
        let mut visited = vec![false; lifted.len()];
        // Working lists, reused for every site; a site's neighbour lists keep
        // their capacity between iterations.
        let (mut touching, mut around, mut stack, mut popped, mut ids, mut points) = (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
        for fi in 0..faces.len() {
            for v in faces[fi].vertices {
                if v >= n_real || visited[v] { continue; }
                visited[v] = true;
                touching_faces_into(&faces, v, fi, true, &mut touching, &mut stack);
                generate_neighbours_into(&faces, v, fi, &mut around, &mut popped, &mut stack);
                ids.clear();
                for &n in &around {
                    if n < n_real && !ids.contains(&self.sites[n].id) { ids.push(self.sites[n].id); }
                }
                let site = &mut self.sites[v];
                site.pd_neighbours.clear();
                site.pd_neighbours.extend(around.iter().map(|&n| if n < n_real { PdNeighbour::Site(n) } else { PdNeighbour::External(n - n_real) }));
                site.neighbours.clear();
                site.neighbours.extend_from_slice(&ids);
                points.clear();
                points.extend(touching.iter().map(|&i| dual_points[i]));
                if let Some(clipped) = clipped_cell(&points, &self.bounds, site.id)? {
                    site.poly = Some(clipped);
                }
            }
        }
        Ok(())
    }

    /// Initial unweighted Fortune pass performed by Node.ComputeNode. This is
    /// distinct from PowerDiagram.ComputeVD, and recentres the input sites.
    pub fn compute_node_voronoi(&mut self) {
        let b = self.bounds.bounds;
        let mut points: Vec<V2> = self.sites.iter().map(|s| s.position).collect();
        let count = points.len();
        if count == 1 {
            self.sites[0].poly = Some(self.bounds.clone());
            self.recenter();
            return;
        }
        points.extend([
            V2::new(b.x - 500.0, b.y + b.height / 2.0),
            V2::new(b.x + b.width + 500.0, b.y + b.height / 2.0),
            V2::new(b.x + b.width / 2.0, b.y - 500.0),
            V2::new(b.x + b.width / 2.0, b.y + b.height + 500.0),
        ]);
        let mut weights: Vec<f32> = self.sites.iter().map(|s| s.weight).collect();
        weights.extend([1.0; 4]);
        let ids: Vec<u32> = (0..points.len() as u32).collect();
        let mut diagram = crate::fortune::Voronoi::new(&points, &ids, &weights,
            crate::geom::Rect::new(b.x - 500.0, b.y - 500.0, b.width + 500.0, b.height + 500.0));
        for i in 0..count {
            let polygon = Polygon::new(diagram.region(points[i]));
            self.sites[i].poly = crate::clipping::intersect(&polygon, &self.bounds);
            self.sites[i].neighbours = diagram.neighbor_site_ids(points[i]).into_iter()
                .filter(|&n| (n as usize) < count).map(|n| self.sites[n as usize].id).collect();
        }
        self.recenter();
    }

    pub fn recenter(&mut self) {
        for site in &mut self.sites {
            if let Some(poly) = site.poly.as_mut() { site.position = poly.centroid(); }
        }
    }

    /// Node.FilterNeighbours removes triangulation neighbors whose clipped
    /// polygons no longer share an edge (especially along the world boundary).
    /// Call after the power iterations, not during their weight adaptation.
    pub fn filter_neighbours(&mut self) {
        let filtered: Vec<Vec<i32>> = self.sites.iter().map(|s| s.neighbours.iter().copied().filter(|id| {
            let Some(other) = self.sites.iter().find(|n| n.id == *id) else { return false; };
            match (&s.poly, &other.poly) {
                (Some(a), Some(b)) => s.id != other.id && a.shares_edge(b).0 == crate::geom::Commonality::Edge,
                _ => false,
            }
        }).collect()).collect();
        for (site, neighbors) in self.sites.iter_mut().zip(filtered) { site.neighbours = neighbors; }
    }

    /// `ComputePowerDiagram(maxIterations, threshold)`. Iterates
    /// `UpdateWeights` + `ComputePD` until relative area error is
    /// below `threshold` or `max_iters` rounds are reached. A site
    /// without a polygon is an error before the loop; inside it, the
    /// game catches the exception and stops iterating.
    pub fn compute_power_diagram(&mut self, max_iters: u32, threshold: f32) -> Result<(), String> {
        self.completed_iterations = 0;
        for site in &mut self.sites {
            let Some(p) = site.poly.as_mut() else {
                return Err(format!("site poly is null for [{}]", site.id));
            };
            site.position = p.centroid();
        }
        for i in 0..=max_iters {
            if let Err(e) = self.update_weights().and_then(|_| self.compute_pd()) {
                eprintln!("power diagram iteration {}/{} stopped: {}", self.completed_iterations, max_iters, e);
                return Ok(());
            }
            let bounds_area = self.bounds.area();
            let mut worst = 0.0f32;
            for s in &self.sites {
                let a = s.poly.as_ref().map(|p| p.area()).unwrap_or(0.1);
                let target = (s.weight as f64 / self.weight_sum as f64 * bounds_area as f64) as f32;
                let rel = (a - target).abs() / target;
                if rel > worst {
                    worst = rel;
                }
            }
            if worst < threshold {
                self.completed_iterations = i;
                return Ok(());
            }
            self.completed_iterations = i + 1;
        }
        Ok(())
    }

    fn update_weights(&mut self) -> Result<(), String> {
        // Recentre each site on its polygon centroid, clamp weight.
        for s in &mut self.sites {
            let Some(p) = s.poly.as_mut() else {
                return Err(format!("site poly is null for [{}]", s.id));
            };
            s.position = p.centroid();
            s.current_weight = s.current_weight.max(1.0);
        }
        let bounds_area = self.bounds.area();
        let mut lowest = 0.0f32;
        for s in &mut self.sites {
            // C# `((site.poly == null) ? 0.1f : site.poly.Area())`.
            let area = s.poly.as_ref().map(|p| p.area()).unwrap_or(0.1);
            let target = (s.weight as f64 / self.weight_sum as f64 * bounds_area as f64) as f32;
            let r_actual = (area / std::f32::consts::PI).sqrt();
            let r_target = (target / std::f32::consts::PI).sqrt();
            let r_delta = r_actual - r_target;
            let mut factor = target / area;
            if (factor as f64 > 1.1 && (s.previous_weight_adaption as f64) < 0.9)
                || ((factor as f64) < 0.9 && s.previous_weight_adaption as f64 > 1.1)
            {
                factor = factor.sqrt();
            }
            if (factor as f64) < 1.1 && factor as f64 > 0.9 && s.current_weight != 1.0 {
                factor = factor.sqrt();
            }
            if s.current_weight < 10.0 {
                factor *= factor;
            }
            if s.current_weight > 10.0 {
                factor = factor.sqrt();
            }
            s.previous_weight_adaption = factor;
            s.current_weight *= factor;
            if s.current_weight < 1.0 {
                let delta = s.current_weight.sqrt() - r_delta;
                if delta < 0.0 {
                    s.current_weight = -(delta * delta);
                    if s.current_weight < lowest {
                        lowest = s.current_weight;
                    }
                }
            }
        }
        if lowest < 0.0 {
            let bump = -(lowest as f64) + 1.0;
            for s in &mut self.sites {
                s.current_weight = (s.current_weight as f64 + bump) as f32;
            }
        }
        // Neighbour limiter over the game's neighbour objects: after ComputeVD
        // these are the touching Delaunay cells (origin, weight 0); after
        // ComputePD they include the external-edge sites. Neighbour sites are
        // read live, which needs no snapshot since nothing here writes them.
        let mut limiter = 1.0f32;
        for s in &self.sites {
            for n in &s.pd_neighbours {
                let (position, weight) = match *n {
                    PdNeighbour::Site(j) => (self.sites[j].position, self.sites[j].current_weight),
                    PdNeighbour::External(j) => (self.external_edge_points[j].position, self.external_edge_points[j].current_weight),
                    PdNeighbour::Face => (V2::ZERO, 0.0),
                };
                let d = s.position.sub(position);
                let sq = (d.x as f64 * d.x as f64 + d.y as f64 * d.y as f64) as f32;
                let denom = (s.current_weight - weight).abs() as f64 + 1.0;
                let r = (sq as f64 / denom) as f32;
                if r < limiter {
                    limiter = r;
                }
            }
        }
        for s in &mut self.sites {
            s.current_weight *= limiter;
        }
        Ok(())
    }
}

/// Power-diagram dual point for a triangle of lifted (x, y, z)
/// vertices, where z = x² + y² - weight. Mirrors the exact form from
/// C# `PowerDiagram.ConvexFaceExt.GetDualPoint`: f32 sums per vertex,
/// the divide promoted to double, then cast back to f32 for storage.
fn power_centre_2d(pts: &[(f64, f64, f64)], face: &DelaunayFace) -> V2 {
    let (x0, y0, z0) = pts[face.vertices[0]];
    let (x1, y1, z1) = pts[face.vertices[1]];
    let (x2, y2, z2) = pts[face.vertices[2]];
    // Each coordinate has already been quantized to Vector3 precision.
    // GetDualPoint assigns these expressions to double locals in Mono.
    let nx = y0 * (z1 - z2) + y1 * (z2 - z0) + y2 * (z0 - z1);
    let ny = z0 * (x1 - x2) + z1 * (x2 - x0) + z2 * (x0 - x1);
    let scale = -0.5 / (x0 * (y1 - y2) + x1 * (y2 - y0) + x2 * (y0 - y1));
    V2::new((nx * scale) as f32, (ny * scale) as f32)
}
/// `PowerDiagramSite.GetCircumcenter`: three 3x3 determinants in double,
/// each product kept in the game's order. An algebraically equal closed
/// form rounds differently and occasionally lands one float ulp away.
fn circumcentre_2d(pts: &[(f64, f64)], face: &DelaunayFace) -> V2 {
    let p = [pts[face.vertices[0]], pts[face.vertices[1]], pts[face.vertices[2]]];
    let det = |m: [[f64; 3]; 3]| m[0][0] * (m[1][1] * m[2][2] - m[2][1] * m[1][2])
        - m[0][1] * (m[1][0] * m[2][2] - m[2][0] * m[1][2])
        + m[0][2] * (m[1][0] * m[2][1] - m[2][0] * m[1][1]);
    let len2 = |q: (f64, f64)| 0.0 + q.0 * q.0 + q.1 * q.1;
    let scale = -1.0 / (2.0 * det([[p[0].0, p[0].1, 1.0], [p[1].0, p[1].1, 1.0], [p[2].0, p[2].1, 1.0]]));
    let nx = 0.0 - det([[len2(p[0]), p[0].1, 1.0], [len2(p[1]), p[1].1, 1.0], [len2(p[2]), p[2].1, 1.0]]);
    let ny = det([[len2(p[0]), p[0].0, 1.0], [len2(p[1]), p[1].0, 1.0], [len2(p[2]), p[2].0, 1.0]]);
    V2::new((scale * nx) as f32, (scale * ny) as f32)
}

fn contains_vert(faces: &[DelaunayFace], face: usize, site: usize) -> bool {
    face != NO_ADJ && faces[face].vertices.contains(&site)
}

/// `TouchingFaces`: depth-first walk over the faces containing `site`. The 3D
/// overload also skips pushing faces already collected.
fn touching_faces(faces: &[DelaunayFace], site: usize, start: usize, skip_listed: bool) -> Vec<usize> {
    let mut list = Vec::new();
    touching_faces_into(faces, site, start, skip_listed, &mut list, &mut Vec::new());
    list
}

/// [`touching_faces`] into `list`, with `stack` as working space.
fn touching_faces_into(faces: &[DelaunayFace], site: usize, start: usize, skip_listed: bool, list: &mut Vec<usize>, stack: &mut Vec<usize>) {
    list.clear();
    stack.clear();
    stack.push(start);
    while let Some(f) = stack.pop() {
        if !contains_vert(faces, f, site) || list.contains(&f) { continue; }
        list.push(f);
        for a in faces[f].adjacent {
            if contains_vert(faces, a, site) && !(skip_listed && list.contains(&a)) { stack.push(a); }
        }
    }
}

/// `GenerateNeighbors`: the site across each shared edge, in push order,
/// external-edge sites included.
fn generate_neighbours_into(faces: &[DelaunayFace], site: usize, start: usize, list: &mut Vec<usize>, popped: &mut Vec<usize>, stack: &mut Vec<usize>) {
    list.clear();
    popped.clear();
    stack.clear();
    stack.push(start);
    while let Some(f) = stack.pop() {
        popped.push(f);
        for a in faces[f].adjacent {
            if contains_vert(faces, a, site) && !popped.contains(&a) {
                // GetEdge: shared vertices in this face's order.
                let mut shared = faces[f].vertices.iter().copied().filter(|v| faces[a].vertices.contains(v));
                let first = shared.next().unwrap();
                let other = if first == site { shared.next().unwrap() } else { first };
                list.push(other);
                stack.push(a);
            }
        }
    }
}

/// `PolyForRandomPoints(points).Clip(bounds)`. Where the points are already
/// a well-separated convex loop (see [`crate::clipping::is_convex_loop`]),
/// the hull would hold exactly those points, and the clip of a convex
/// polygon does not depend on where its vertex list starts or on its
/// winding, so the loop is clipped as it is.
fn clipped_cell(points: &[V2], bounds: &Polygon, id: i32) -> Result<Option<Polygon>, String> {
    if crate::clipping::is_convex_loop(points) {
        let clipped = crate::clipping::intersect_vertices(points, &bounds.vertices);
        #[cfg(feature = "clip-check")]
        {
            let poly = poly_for_random_points(points.to_vec()).expect("hull of a convex loop");
            assert_eq!(poly.vertices.len(), points.len(), "hull dropped loop points: {points:?}");
            assert_eq!(crate::clipping::intersect(&poly, bounds).map(|p| p.vertices), clipped.as_ref().map(|p| p.vertices.clone()), "convex loop clip: {points:?} / {:?}", bounds.vertices);
        }
        return Ok(clipped);
    }
    let poly = poly_for_random_points(points.to_vec()).ok_or_else(|| format!("PolyForRandomPoints failed for site [{id}]"))?;
    Ok(crate::clipping::intersect(&poly, bounds))
}

/// `PolyForRandomPoints`: 2D convex hull of the points in the game's
/// order, wound counter-clockwise.
fn poly_for_random_points(points: Vec<V2>) -> Option<Polygon> {
    let coords: Vec<(f64, f64)> = points.iter().map(|p| (p.x as f64, p.y as f64)).collect();
    let mut polygon = Polygon::new(convex_hull_2d(&coords)?.into_iter().map(|i| points[i]).collect());
    polygon.force_winding(FORCED_WINDING);
    Some(polygon)
}

// ---- Tree + Node skeletons ----------------------------------------

/// Minimal `VoronoiTree.Node` port. Stores a site (position + poly),
/// computes child sub-diagrams via [`PowerDiagram`]. Tree-recursive
/// `GenerateChildren` logic is deferred to Phase 2c, where the
/// SubWorld filter chain drives subdivision. For now we provide just
/// enough surface to feed `WorldLayout.GenerateOverworld`.
#[derive(Debug, Clone)]
pub struct Node {
    pub id: u32,
    pub position: V2,
    pub poly: Option<Polygon>,
    pub weight: f32,
    pub tags: Vec<String>,
    pub children: Vec<Node>,
    pub subworld_type: Option<String>,
}

impl Node {
    pub fn new(id: u32, position: V2, weight: f32) -> Self {
        Self {
            id,
            position,
            poly: None,
            weight,
            tags: Vec::new(),
            children: Vec::new(),
            subworld_type: None,
        }
    }
}

/// `VoronoiTree.Tree`. Holds a root node and its children; exposes
/// `compute_node_pd` which builds a PowerDiagram over the child
/// sites and fills each child's `poly` / `neighbours`.
pub struct Tree {
    pub root: Node,
    pub children: Vec<Node>,
    pub seed: i32,
}

impl Tree {
    pub fn new(root: Node, seed: i32) -> Self {
        Self {
            root,
            children: Vec::new(),
            seed,
        }
    }

    pub fn add_child(&mut self, n: Node) {
        self.children.push(n);
    }

    /// `Tree.ComputeNodePD` analogue: build a PowerDiagram over the
    /// tree's direct children, within the root's polygon, and fold
    /// the resulting polygons / neighbour sets back into the children.
    pub fn compute_node_pd(&mut self, max_iters: u32, threshold: f32) -> bool {
        let Some(root_poly) = self.root.poly.clone() else {
            return false;
        };
        let mut pd_sites: Vec<PowerDiagramSite> = self
            .children
            .iter()
            .enumerate()
            .map(|(i, c)| PowerDiagramSite::new(i as i32, c.position, c.weight))
            .collect();
        let mut pd = PowerDiagram::new(root_poly, pd_sites.drain(..).collect());
        if pd.compute_vd().and_then(|_| pd.compute_power_diagram(max_iters, threshold)).is_err() {
            return false;
        }
        for (i, c) in self.children.iter_mut().enumerate() {
            if i < pd.sites.len() {
                c.poly = pd.sites[i].poly.clone();
                c.position = pd
                    .sites[i]
                    .poly
                    .as_ref()
                    .map(|p| p.clone().centroid())
                    .unwrap_or(c.position);
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::Rect;

    #[test]
    fn single_site_claims_entire_bounds() {
        let bounds = Polygon::from_rect(Rect::new(0.0, 0.0, 10.0, 10.0));
        let sites = vec![PowerDiagramSite::new(1, V2::new(5.0, 5.0), 1.0)];
        let mut pd = PowerDiagram::new(bounds, sites);
        pd.compute_vd().unwrap();
        assert!(pd.sites[0].poly.is_some());
    }

    #[test]
    fn four_site_diagram_builds() {
        let bounds = Polygon::from_rect(Rect::new(0.0, 0.0, 100.0, 100.0));
        let sites = vec![
            PowerDiagramSite::new(1, V2::new(25.0, 25.0), 1.0),
            PowerDiagramSite::new(2, V2::new(75.0, 25.0), 1.0),
            PowerDiagramSite::new(3, V2::new(25.0, 75.0), 1.0),
            PowerDiagramSite::new(4, V2::new(75.0, 75.0), 1.0),
        ];
        let mut pd = PowerDiagram::new(bounds, sites);
        pd.compute_vd().unwrap();
        for s in &pd.sites {
            assert!(s.poly.is_some(), "site {} has no poly", s.id);
        }
    }

    #[test]
    fn tree_compute_node_pd_populates_children() {
        let mut root = Node::new(0, V2::new(50.0, 50.0), 1.0);
        root.poly = Some(Polygon::from_rect(Rect::new(0.0, 0.0, 100.0, 100.0)));
        let mut tree = Tree::new(root, 42);
        tree.add_child(Node::new(1, V2::new(25.0, 25.0), 1.0));
        tree.add_child(Node::new(2, V2::new(75.0, 25.0), 1.0));
        tree.add_child(Node::new(3, V2::new(25.0, 75.0), 1.0));
        tree.add_child(Node::new(4, V2::new(75.0, 75.0), 1.0));
        let ok = tree.compute_node_pd(50, 0.2);
        assert!(ok);
        for c in &tree.children {
            assert!(c.poly.is_some());
        }
    }
}
