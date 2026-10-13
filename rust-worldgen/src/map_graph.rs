//! MapGraph: Voronoi graph of biome cells built from Poisson disk
//! sample points via Delaunay triangulation.
//!
//! Follows the shape of ONI's `ProcGen.Map.MapGraph`
//! (Cell / Edge / Corner), but simplified to only what worldgen
//! reads.
//!
//! Uses [`crate::miconvexhull::delaunay_2d`] under the hood, so the
//! triangulation is bit-exact with the game's `MIConvexHull` on
//! well-conditioned inputs. Voronoi corners are the circumcentres of
//! each Delaunay triangle; each MapGraph edge between two site cells
//! borrows the circumcentres of the two triangles that share the
//! corresponding Delaunay edge.
//!
//! Hull edges (Delaunay edges that bound only a single triangle) are
//! dropped from the MapGraph. ONI's code clips unbounded Voronoi
//! regions against the world rectangle during `WorldLayout`; that
//! clipping is done at a later stage.

use std::collections::HashMap;

use crate::miconvexhull::delaunay_2d;

/// Biome-cell seed point. Owned by the graph; indexed by `usize`.
#[derive(Debug, Clone, Copy)]
pub struct Cell {
    pub id: usize,
    pub x: f32,
    pub y: f32,
    pub subworld_idx: Option<usize>,
}

/// Edge between two cells. `corner0` / `corner1` index into `corners`.
#[derive(Debug, Clone, Copy)]
pub struct Edge {
    pub cell0: usize,
    pub cell1: usize,
    pub corner0: usize,
    pub corner1: usize,
}

/// Voronoi corner shared by 2+ cells. Always the circumcentre of a
/// Delaunay triangle in this build.
#[derive(Debug, Clone, Copy)]
pub struct Corner {
    pub x: f32,
    pub y: f32,
}

#[derive(Debug, Default)]
pub struct MapGraph {
    pub cells: Vec<Cell>,
    pub edges: Vec<Edge>,
    pub corners: Vec<Corner>,
    /// cells[i] -> list of edge indices touching it
    pub cell_edges: Vec<Vec<usize>>,
}

/// Circumcentre of a triangle in f64. Undefined for collinear input;
/// our sampler never produces that.
fn circumcentre(a: (f64, f64), b: (f64, f64), c: (f64, f64)) -> (f64, f64) {
    let d = 2.0 * (a.0 * (b.1 - c.1) + b.0 * (c.1 - a.1) + c.0 * (a.1 - b.1));
    let a2 = a.0 * a.0 + a.1 * a.1;
    let b2 = b.0 * b.0 + b.1 * b.1;
    let c2 = c.0 * c.0 + c.1 * c.1;
    let ux = (a2 * (b.1 - c.1) + b2 * (c.1 - a.1) + c2 * (a.1 - b.1)) / d;
    let uy = (a2 * (c.0 - b.0) + b2 * (a.0 - c.0) + c2 * (b.0 - a.0)) / d;
    (ux, uy)
}

impl MapGraph {
    /// Build a MapGraph from a list of site points (e.g. Poisson-disk
    /// output). Cells are created in the same order as `points` to
    /// keep IDs stable across runs.
    pub fn from_points(points: &[[f32; 2]]) -> Self {
        let mut g = MapGraph::default();
        g.cells.reserve(points.len());
        for (i, p) in points.iter().enumerate() {
            g.cells.push(Cell {
                id: i,
                x: p[0],
                y: p[1],
                subworld_idx: None,
            });
        }
        g.cell_edges = vec![Vec::new(); points.len()];

        if points.len() < 3 {
            return g;
        }

        let pts_f64: Vec<(f64, f64)> = points
            .iter()
            .map(|p| (p[0] as f64, p[1] as f64))
            .collect();
        let triangles = delaunay_2d(&pts_f64);

        // One Voronoi corner per Delaunay triangle = triangle's
        // circumcentre. Indexing: corner `ti` <-> triangle `ti`.
        g.corners.reserve(triangles.len());
        for t in &triangles {
            let (cx, cy) = circumcentre(pts_f64[t[0]], pts_f64[t[1]], pts_f64[t[2]]);
            g.corners.push(Corner {
                x: cx as f32,
                y: cy as f32,
            });
        }

        // For each Delaunay edge, remember the 1 or 2 triangles that
        // contain it. Key = (min_site, max_site).
        let mut edge_to_tris: HashMap<(usize, usize), (usize, Option<usize>)> =
            HashMap::with_capacity(triangles.len() * 3);
        for (ti, t) in triangles.iter().enumerate() {
            for &(x, y) in &[(t[0], t[1]), (t[1], t[2]), (t[0], t[2])] {
                let key = if x < y { (x, y) } else { (y, x) };
                edge_to_tris
                    .entry(key)
                    .and_modify(|e| {
                        if e.1.is_none() {
                            e.1 = Some(ti);
                        }
                    })
                    .or_insert((ti, None));
            }
        }

        // Stable ordering: iterate triangles and emit each edge once
        // (only when hit as the "first" triangle). This keeps edge
        // order deterministic for equal inputs even across HashMap
        // rehashes.
        let mut seen: HashMap<(usize, usize), usize> = HashMap::new();
        for (ti, t) in triangles.iter().enumerate() {
            for &(x, y) in &[(t[0], t[1]), (t[1], t[2]), (t[0], t[2])] {
                let key = if x < y { (x, y) } else { (y, x) };
                if seen.contains_key(&key) {
                    continue;
                }
                let (t0, t1_opt) = edge_to_tris[&key];
                let Some(t1) = t1_opt else {
                    // Hull edge; one-triangle only. Dropped until
                    // WorldLayout's clipping stage exists.
                    continue;
                };
                let _ = ti;
                let (a, b) = key;
                let idx = g.edges.len();
                g.edges.push(Edge {
                    cell0: a,
                    cell1: b,
                    corner0: t0,
                    corner1: t1,
                });
                g.cell_edges[a].push(idx);
                g.cell_edges[b].push(idx);
                seen.insert(key, idx);
            }
        }

        g
    }

    pub fn cell_count(&self) -> usize {
        self.cells.len()
    }
    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }

    /// Nearest cell to a given point. O(n); good enough for the
    /// ~hundreds of cells ONI worldgen produces per world.
    pub fn nearest_cell(&self, x: f32, y: f32) -> Option<usize> {
        let mut best: Option<(usize, f32)> = None;
        for c in &self.cells {
            let dx = c.x - x;
            let dy = c.y - y;
            let d = dx * dx + dy * dy;
            match best {
                Some((_, bd)) if bd <= d => {}
                _ => best = Some((c.id, d)),
            }
        }
        best.map(|(i, _)| i)
    }

    /// Neighbour cells via graph edges.
    pub fn neighbors(&self, cell: usize) -> Vec<usize> {
        self.cell_edges[cell]
            .iter()
            .map(|&e| {
                let ed = &self.edges[e];
                if ed.cell0 == cell { ed.cell1 } else { ed.cell0 }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sampler::{poisson_disk_rectangle, SeededRandom};

    #[test]
    fn small_graph_from_square() {
        let points = vec![[0.0, 0.0], [2.0, 0.0], [0.0, 2.0], [2.0, 2.0], [1.0, 1.0]];
        let g = MapGraph::from_points(&points);
        assert_eq!(g.cell_count(), 5);
        // Centre cell neighbours all four corners.
        let centre_neighbours = g.neighbors(4);
        assert_eq!(centre_neighbours.len(), 4);
    }

    #[test]
    fn poisson_integration() {
        let mut rnd = SeededRandom::new(42);
        let pts = poisson_disk_rectangle(&mut rnd, [0.0, 0.0], [256.0, 384.0], 30.0, 30);
        assert!(pts.len() > 20, "too few points: {}", pts.len());
        let g = MapGraph::from_points(&pts);
        assert_eq!(g.cell_count(), pts.len());
        let inner_cells = (0..g.cell_count()).filter(|&i| !g.neighbors(i).is_empty());
        assert!(inner_cells.count() > g.cell_count() / 2);
    }

    #[test]
    fn nearest_cell_works() {
        let points = vec![[0.0, 0.0], [10.0, 0.0], [0.0, 10.0], [10.0, 10.0]];
        let g = MapGraph::from_points(&points);
        assert_eq!(g.nearest_cell(1.0, 1.0), Some(0));
        assert_eq!(g.nearest_cell(9.0, 9.0), Some(3));
        assert_eq!(g.nearest_cell(10.0, 1.0), Some(1));
    }

    /// Same input must always produce the same triangulation and the
    /// same edge ordering (important for downstream biome tagging).
    #[test]
    fn deterministic_build() {
        let mut rnd = SeededRandom::new(42);
        let pts = poisson_disk_rectangle(&mut rnd, [0.0, 0.0], [256.0, 384.0], 30.0, 30);
        let a = MapGraph::from_points(&pts);
        let b = MapGraph::from_points(&pts);
        assert_eq!(a.cell_count(), b.cell_count());
        assert_eq!(a.edge_count(), b.edge_count());
        for i in 0..a.edge_count() {
            assert_eq!(a.edges[i].cell0, b.edges[i].cell0);
            assert_eq!(a.edges[i].cell1, b.edges[i].cell1);
            assert_eq!(a.edges[i].corner0, b.edges[i].corner0);
            assert_eq!(a.edges[i].corner1, b.edges[i].corner1);
        }
        assert_eq!(a.corners.len(), b.corners.len());
        for i in 0..a.corners.len() {
            assert_eq!(a.corners[i].x.to_bits(), b.corners[i].x.to_bits());
            assert_eq!(a.corners[i].y.to_bits(), b.corners[i].y.to_bits());
        }
    }
}
