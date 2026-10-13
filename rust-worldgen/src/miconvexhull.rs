//! Port of the 2D Delaunay triangulation inside ONI's MIConvexHull.
//!
//! The C# source is the `MIConvexHull` namespace shipped with
//! `Assembly-CSharp.dll`. The algorithm is Quickhull: lift 2D points
//! to 3D via the paraboloid `(x, y) -> (x, y, x^2 + y^2)`, compute
//! the 3D convex hull, then discard the "upper" faces. The remaining
//! "lower" faces project back to the Delaunay triangles of the 2D
//! input.
//!
//! This port keeps the f64 arithmetic, the same control flow, the
//! same tie-breaking, and the same lifted shift-and-scale. Two
//! deliberate simplifications:
//!
//! * `DetermineDimension` in the source samples 10 random vertices to
//!   validate uniform dimensionality. We fix the dimension at 2 on
//!   the caller side (via [`delaunay_2d`]) and skip the random check.
//! * `RandomOffsetToLift` in the source uses a non-seeded
//!   `System.Random` to perturb the lifted Z when the input is truly
//!   degenerate (e.g. the four corners of an axis-aligned square are
//!   exactly cocircular, so their lifted images lie on a common
//!   plane). That branch is inherently nondeterministic in the game.
//!   We substitute a deterministic `KRandom` seeded from a hash of
//!   the input coordinates so our output is reproducible; on
//!   well-conditioned inputs the branch is never hit.

use crate::hash::sdbm_lower;
use crate::rng::KRandom;

const CONNECTOR_BUCKETS: usize = 2017;
const TOL: f64 = 1e-10;

/// Hull face index into `ConvexHullAlgorithm.face_pool`.
type FaceId = usize;
/// Sentinel for "no face". Matches C#'s `-1`.
const NO_FACE: FaceId = usize::MAX;

/// 3D Delaunay face used by `PowerDiagram`. Vertices reference
/// input point indices; `adjacent` gives the neighbour triangle
/// across each edge (opposite the vertex at the same index),
/// `NO_ADJ` when the edge is a hull edge.
///
/// Produced by [`delaunay_2d_with_adjacency`].
#[derive(Debug, Clone, Copy)]
pub struct DelaunayFace {
    pub vertices: [usize; 3],
    pub adjacent: [usize; 3],
    pub is_normal_flipped: bool,
}

pub const NO_ADJ: usize = usize::MAX;

/// Same as [`delaunay_2d`] but also returns face-to-face adjacency
/// and the normal-flip flag per triangle, i.e. the full triangulation
/// graph rather than just the set of triangles.
pub fn delaunay_2d_with_adjacency(points: &[(f64, f64)]) -> Vec<DelaunayFace> {
    if points.len() < 3 {
        return Vec::new();
    }
    ConvexHullAlgorithm::with(points, true, TOL, |algo| {
        algo.get_convex_hull();
        algo.remove_upper_faces();
        export_faces(algo, &algo.convex_faces)
    })
}

/// Lower hull of explicitly lifted weighted points. Unlike ordinary Delaunay,
/// changing z changes triangle connectivity, not just the dual coordinates.
pub fn lower_hull_3d(points: &[(f64, f64, f64)]) -> Vec<DelaunayFace> {
    if points.len() < 4 { return Vec::new(); }
    let xy: Vec<_> = points.iter().map(|p| (p.0, p.1)).collect();
    ConvexHullAlgorithm::with(&xy, true, TOL, |algo| {
        // Reuse the 3D working buffers, but this is a genuine 3D hull: the supplied
        // z values must not be replaced or rescaled as Delaunay paraboloid lifts.
        algo.is_lifted = false;
        algo.positions.clear();
        algo.positions.extend(points.iter().flat_map(|p| [p.0, p.1, p.2]));
        algo.get_convex_hull();
        let lower: Vec<_> = algo.convex_faces.iter().copied()
            .filter(|&f| algo.face_pool[f].normal[2] < -(f32::from_bits(1) as f64)).collect();
        export_faces(algo, &lower)
    })
}

/// `ConvexHull.Create` on 2D points, which returns `Return2DResultInOrder`:
/// hull vertex indices walked edge to edge. `None` where the C# throws (too
/// few points, a duplicate edge key or a broken edge chain).
pub fn convex_hull_2d(points: &[(f64, f64)]) -> Option<Vec<usize>> {
    if points.len() <= 2 { return None; }
    let faces: Vec<[usize; 2]> = ConvexHullAlgorithm::with(points, false, TOL, |algo| {
        algo.get_convex_hull();
        // GetConvexFaces swaps vertices 0 and NumOfDimensions-1 on flipped faces.
        algo.convex_faces.iter().map(|&f| {
            let face = &algo.face_pool[f];
            if face.is_normal_flipped { [face.vertices[1], face.vertices[0]] } else { [face.vertices[0], face.vertices[1]] }
        }).collect()
    });
    let mut by_second = vec![usize::MAX; points.len()];
    for (i, f) in faces.iter().enumerate() {
        if std::mem::replace(&mut by_second[f[1]], i) != usize::MAX { return None; }
    }
    let start = faces[0][1];
    let mut current = faces[0][0];
    let mut list = vec![start];
    let (mut lowest, mut count) = (0usize, 0usize);
    while current != start {
        if list.len() > faces.len() { return None; }
        list.push(current);
        let face = by_second[current];
        if face == usize::MAX { return None; }
        // The game records the index of the previously added vertex here.
        let (p, q) = (points[current], points[list[lowest]]);
        if p.0 < q.0 || (p.0 == q.0 && p.1 <= q.1) { lowest = count; }
        count += 1;
        current = faces[face][0];
    }
    if list.len() != faces.len() { return None; }
    Some((0..faces.len()).map(|j| list[(j + lowest) % faces.len()]).collect())
}

fn export_faces(algo: &ConvexHullAlgorithm, selected: &[FaceId]) -> Vec<DelaunayFace> {

    // Map face_pool indices to compact triangle indices.
    let mut remap = vec![NO_ADJ; algo.face_pool.len()];
    for (i, &fid) in selected.iter().enumerate() {
        remap[fid] = i;
    }

    let mut out = Vec::with_capacity(selected.len());
    for &fid in selected {
        let f = &algo.face_pool[fid];
        let (v0, v1, v2) = (f.vertices[0], f.vertices[1], f.vertices[2]);
        let verts = if f.is_normal_flipped { [v2, v1, v0] } else { [v0, v1, v2] };
        let mut adj = [NO_ADJ; 3];
        for (i, &a) in f.adjacent_faces.iter().take(3).enumerate() {
            adj[i] = if a == NO_FACE || a >= remap.len() { NO_ADJ } else { remap[a] };
        }
        // Mirror the C# source's 0/NumOfDimensions-1 swap on flip.
        let adjacency = if f.is_normal_flipped {
            [adj[2], adj[1], adj[0]]
        } else {
            adj
        };
        out.push(DelaunayFace {
            vertices: verts,
            adjacent: adjacency,
            is_normal_flipped: f.is_normal_flipped,
        });
    }
    out
}

/// 2D Delaunay triangulation of the input points.
///
/// Returns a list of triangles, each `[a, b, c]` a triple of indices
/// into `points`. Vertex winding within a triangle follows the
/// source algorithm (no renormalisation).
pub fn delaunay_2d(points: &[(f64, f64)]) -> Vec<[usize; 3]> {
    if points.len() < 3 {
        return Vec::new();
    }
    // 2D input, lifted to 3D paraboloid -> work in 3 dimensions.
    let mut algo = ConvexHullAlgorithm::new(points, true, TOL);
    algo.get_convex_hull();
    algo.remove_upper_faces();
    let mut out = Vec::with_capacity(algo.convex_faces.len());
    for &fid in &algo.convex_faces {
        let f = &algo.face_pool[fid];
        // Two verts come directly, third flipped if normal was flipped.
        let v0 = f.vertices[0];
        let v1 = f.vertices[1];
        let v2 = f.vertices[2];
        let tri = if f.is_normal_flipped {
            [v2, v1, v0]
        } else {
            [v0, v1, v2]
        };
        out.push(tri);
    }
    out
}

/// Fixed-capacity list of at most three entries (one per hull dimension),
/// so faces and connectors need no heap allocation.
#[derive(Debug, Clone, Copy)]
struct Small<T: Copy> { data: [T; 3], len: u8 }

impl<T: Copy> Small<T> {
    fn filled(value: T, len: usize) -> Self {
        debug_assert!(len <= 3);
        Self { data: [value; 3], len: len as u8 }
    }
}

impl<T: Copy> std::ops::Deref for Small<T> {
    type Target = [T];
    fn deref(&self) -> &[T] { &self.data[..self.len as usize] }
}

impl<T: Copy> std::ops::DerefMut for Small<T> {
    fn deref_mut(&mut self) -> &mut [T] { &mut self.data[..self.len as usize] }
}

impl<'a, T: Copy> IntoIterator for &'a Small<T> {
    type Item = &'a T;
    type IntoIter = std::slice::Iter<'a, T>;
    fn into_iter(self) -> Self::IntoIter { self.iter() }
}

struct FaceData {
    adjacent_faces: Small<FaceId>,
    furthest_vertex: usize,
    in_list: bool,
    is_normal_flipped: bool,
    next: Option<FaceId>,
    previous: Option<FaceId>,
    normal: Small<f64>,
    offset: f64,
    vertices: Small<usize>,
    vertices_beyond: Vec<usize>,
    tag: i32,
}

impl FaceData {
    fn new(dim: usize) -> Self {
        Self {
            adjacent_faces: Small::filled(NO_FACE, dim),
            furthest_vertex: 0,
            in_list: false,
            is_normal_flipped: false,
            next: None,
            previous: None,
            normal: Small::filled(0.0, dim),
            offset: 0.0,
            vertices: Small::filled(0, dim),
            vertices_beyond: Vec::new(),
            tag: 0,
        }
    }
}

#[derive(Default, Clone, Copy)]
struct DeferredFace {
    face_idx: FaceId,
    pivot_idx: FaceId,
    old_face_idx: FaceId,
    face_index: usize,
    pivot_index: usize,
}

struct FaceConnector {
    edge_index: usize,
    face_idx: FaceId,
    hash_code: u32,
    vertices: Small<usize>,
    next: Option<usize>,
    previous: Option<usize>,
}

impl FaceConnector {
    fn new(dim: usize) -> Self {
        Self {
            edge_index: 0,
            face_idx: NO_FACE,
            hash_code: 0,
            vertices: Small::filled(0, dim - 1),
            next: None,
            previous: None,
        }
    }
}

struct ConvexHullAlgorithm {
    // Config
    num_dim: usize,
    is_lifted: bool,
    plane_distance_tolerance: f64,

    // Input (lifted positions packed row-major: v0.x, v0.y, v0.z, v1.x, ...).
    vertices_count: usize,
    positions: Vec<f64>,

    // Working
    vertex_visited: Vec<bool>,
    face_pool: Vec<FaceData>,
    affected_face_flags: Vec<bool>,
    convex_hull_size: i32,

    // Linked list of unprocessed faces, ordered by verticesBeyond.Count desc.
    unprocessed_first: Option<FaceId>,
    unprocessed_last: Option<FaceId>,
    convex_faces: Vec<FaceId>,

    // Per-step state
    current_vertex: usize,
    max_distance: f64,
    furthest_vertex: usize,
    center: Small<f64>,
    update_buffer: Small<FaceId>,
    update_indices: Small<usize>,
    traverse_stack: Vec<FaceId>,
    /// Placeholder held by faces that currently have no "beyond" list.
    /// Matches `EmptyBuffer` in the source; kept so that the face's
    /// `vertices_beyond` is never observably-null.
    #[allow(dead_code)]
    empty_buffer: Vec<usize>,
    beyond_buffer: Vec<usize>,
    affected_face_buffer: Vec<FaceId>,
    cone_face_buffer: Vec<DeferredFace>,
    /// Membership flags of the source's `singularVertices` set.
    singular_vertices: Vec<bool>,

    // Pooled FaceConnectors.
    connector_pool: Vec<FaceConnector>,
    connector_free_head: Option<usize>,
    /// Bucket heads of the connector hash table. Connectors only match
    /// connectors with the same hash, which share a bucket for any bucket
    /// count, and each bucket is a LIFO list, so the bucket count does not
    /// change which faces get connected.
    connector_table_first: Vec<Option<usize>>,

    // Bounding box.
    bounding_box_points: Vec<Vec<usize>>,
    index_of_dim_with_least_extremes: usize,
    minima: Small<f64>,
    maxima: Small<f64>,

    // ObjectManager state (sans ConnectorStack; above).
    free_face_indices: Vec<FaceId>,
    empty_buffer_stack: Vec<Vec<usize>>,
    deferred_face_stack: Vec<DeferredFace>,

    // Math temporaries.
    nt_x: Small<f64>,
    nt_y: Small<f64>,
    /// Reserved for the 4D+ normal computation in the C# source; our
    /// Delaunay path only hits the 2D/3D specialisations but we keep
    /// the field so the port stays line-for-line comparable.
    #[allow(dead_code)]
    nt_z: Small<f64>,
}

impl ConvexHullAlgorithm {
    fn new(points: &[(f64, f64)], lift: bool, tol: f64) -> Self {
        let mut algo = Self {
            num_dim: 0, is_lifted: false, plane_distance_tolerance: 0.0, vertices_count: 0,
            positions: Vec::new(), vertex_visited: Vec::new(), face_pool: Vec::new(), affected_face_flags: Vec::new(),
            convex_hull_size: 0, unprocessed_first: None, unprocessed_last: None, convex_faces: Vec::new(),
            current_vertex: 0, max_distance: f64::NEG_INFINITY, furthest_vertex: 0,
            center: Small::filled(0.0, 0), update_buffer: Small::filled(NO_FACE, 0), update_indices: Small::filled(0, 0),
            traverse_stack: Vec::new(), empty_buffer: Vec::new(), beyond_buffer: Vec::new(), affected_face_buffer: Vec::new(),
            cone_face_buffer: Vec::new(), singular_vertices: Vec::new(), connector_pool: Vec::new(), connector_free_head: None,
            connector_table_first: Vec::new(), bounding_box_points: Vec::new(), index_of_dim_with_least_extremes: 0,
            minima: Small::filled(0.0, 0), maxima: Small::filled(0.0, 0), free_face_indices: Vec::new(),
            empty_buffer_stack: Vec::new(), deferred_face_stack: Vec::new(),
            nt_x: Small::filled(0.0, 0), nt_y: Small::filled(0.0, 0), nt_z: Small::filled(0.0, 0),
        };
        algo.reset(points, lift, tol);
        algo
    }

    /// Run `f` on an algorithm set up for `points` as [`new`](Self::new)
    /// does, reusing a thread's earlier instance and its buffers.
    fn with<R>(points: &[(f64, f64)], lift: bool, tol: f64, f: impl FnOnce(&mut Self) -> R) -> R {
        thread_local! { static POOL: std::cell::RefCell<Vec<ConvexHullAlgorithm>> = const { std::cell::RefCell::new(Vec::new()) }; }
        let mut algo = match POOL.with(|p| p.borrow_mut().pop()) {
            Some(mut algo) => { algo.reset(points, lift, tol); algo }
            None => Self::new(points, lift, tol),
        };
        let out = f(&mut algo);
        POOL.with(|p| p.borrow_mut().push(algo));
        out
    }

    /// The state `new` starts from, keeping allocated buffers. Spare beyond
    /// buffers go to the empty-buffer stack, where they stand in for the new
    /// empty buffers `get_face` would otherwise create.
    fn reset(&mut self, points: &[(f64, f64)], lift: bool, tol: f64) {
        let base_dim = 2usize;
        let num_dim = if lift { base_dim + 1 } else { base_dim };
        let n = points.len();
        assert!(n > num_dim, "too few vertices for dimension");
        self.num_dim = num_dim;
        self.is_lifted = lift;
        self.plane_distance_tolerance = tol;
        self.vertices_count = n;
        // Serialise into the positions array. For lifted 2D input
        // paraboloid the third coordinate is x^2 + y^2.
        self.positions.clear();
        self.positions.resize(n * num_dim, 0.0);
        let positions = &mut self.positions;
        if lift {
            for (i, &(x, y)) in points.iter().enumerate() {
                let base = i * num_dim;
                positions[base] = x;
                positions[base + 1] = y;
                positions[base + 2] = x * x + y * y;
            }
        } else {
            for (i, &(x, y)) in points.iter().enumerate() {
                let base = i * num_dim;
                positions[base] = x;
                positions[base + 1] = y;
            }
        }
        self.vertex_visited.clear();
        self.vertex_visited.resize(n, false);
        for buffer in &mut self.empty_buffer_stack { buffer.clear(); }
        for face in self.face_pool.drain(..) {
            if self.empty_buffer_stack.len() >= 256 { break; }
            let mut buffer = face.vertices_beyond;
            buffer.clear();
            self.empty_buffer_stack.push(buffer);
        }
        self.face_pool.clear();
        self.affected_face_flags.clear();
        self.convex_hull_size = 0;
        self.unprocessed_first = None;
        self.unprocessed_last = None;
        self.convex_faces.clear();
        self.current_vertex = 0;
        self.max_distance = f64::NEG_INFINITY;
        self.furthest_vertex = 0;
        self.center = Small::filled(0.0, num_dim);
        self.update_buffer = Small::filled(NO_FACE, num_dim);
        self.update_indices = Small::filled(0, num_dim);
        self.traverse_stack.clear();
        self.empty_buffer.clear();
        self.beyond_buffer.clear();
        self.affected_face_buffer.clear();
        self.cone_face_buffer.clear();
        self.singular_vertices.clear();
        self.singular_vertices.resize(n, false);
        self.connector_pool.clear();
        self.connector_free_head = None;
        self.connector_table_first.clear();
        self.connector_table_first.resize((4 * n).clamp(31, CONNECTOR_BUCKETS), None);
        self.bounding_box_points.resize_with(num_dim, Vec::new);
        self.bounding_box_points.truncate(num_dim);
        for points in &mut self.bounding_box_points { points.clear(); }
        self.index_of_dim_with_least_extremes = 0;
        self.minima = Small::filled(f64::INFINITY, num_dim);
        self.maxima = Small::filled(f64::NEG_INFINITY, num_dim);
        self.free_face_indices.clear();
        self.deferred_face_stack.clear();
        self.nt_x = Small::filled(0.0, num_dim);
        self.nt_y = Small::filled(0.0, num_dim);
        self.nt_z = Small::filled(0.0, num_dim);
    }

    // ---- Math helpers (ported from MathHelper.cs) ----------------

    #[inline]
    fn get_coordinate(&self, v: usize, dim: usize) -> f64 {
        self.positions[v * self.num_dim + dim]
    }

    fn vector_between_vertices_new(&self, to_idx: usize, from_idx: usize) -> Small<f64> {
        let mut v = Small::filled(0.0, self.num_dim);
        let a = to_idx * self.num_dim;
        let b = from_idx * self.num_dim;
        for i in 0..self.num_dim {
            v[i] = self.positions[a + i] - self.positions[b + i];
        }
        v
    }

    /// Fills `normal` with the plane normal of the face's vertices.
    fn find_normal_vector(&mut self, vertices: &[usize], normal: &mut [f64]) {
        let dim = self.num_dim;
        // Fill temporaries directly without calling self-methods to
        // avoid borrow conflicts.
        match dim {
            2 => {
                let a = vertices[1] * dim;
                let b = vertices[0] * dim;
                for i in 0..dim {
                    self.nt_x[i] = self.positions[a + i] - self.positions[b + i];
                }
                let nx = -self.nt_x[1];
                let ny = self.nt_x[0];
                let len = (nx * nx + ny * ny).sqrt();
                let inv = 1.0 / len;
                normal[0] = inv * nx;
                normal[1] = inv * ny;
            }
            3 => {
                let (a1, b1) = (vertices[1] * dim, vertices[0] * dim);
                let (a2, b2) = (vertices[2] * dim, vertices[1] * dim);
                for i in 0..dim {
                    self.nt_x[i] = self.positions[a1 + i] - self.positions[b1 + i];
                    self.nt_y[i] = self.positions[a2 + i] - self.positions[b2 + i];
                }
                let nx = self.nt_x[1] * self.nt_y[2] - self.nt_x[2] * self.nt_y[1];
                let ny = self.nt_x[2] * self.nt_y[0] - self.nt_x[0] * self.nt_y[2];
                let nz = self.nt_x[0] * self.nt_y[1] - self.nt_x[1] * self.nt_y[0];
                let len = (nx * nx + ny * ny + nz * nz).sqrt();
                let inv = 1.0 / len;
                normal[0] = inv * nx;
                normal[1] = inv * ny;
                normal[2] = inv * nz;
            }
            _ => {
                // ONI's Delaunay is always 2D input lifted to 3D, so we
                // only need the 3D specialisation here. Higher-dim
                // LUFactor path is in the C# source but is never
                // exercised by ONI's worldgen.
                panic!("only 2D or 3D (lifted) supported");
            }
        }
    }

    fn calculate_face_plane(&mut self, face_idx: FaceId) -> bool {
        // We mutate face.normal and face.offset via index accesses to
        // avoid borrow conflicts with self for the math helpers.
        let vertices = self.face_pool[face_idx].vertices;
        let mut normal = Small::filled(0.0, self.num_dim);
        self.find_normal_vector(&vertices, &mut normal);
        if normal[0].is_nan() {
            return false;
        }
        let mut num = 0.0;
        let mut num2 = 0.0;
        let base = vertices[0] * self.num_dim;
        for i in 0..self.num_dim {
            let n = normal[i];
            num += n * self.positions[base + i];
            num2 += n * self.center[i];
        }
        let mut offset = -num;
        num2 -= num;
        let mut flipped = false;
        if num2 > 0.0 {
            for n in normal.iter_mut() {
                *n = -*n;
            }
            offset = num;
            flipped = true;
        }
        let face = &mut self.face_pool[face_idx];
        face.normal.copy_from_slice(&normal);
        face.offset = offset;
        face.is_normal_flipped = flipped;
        true
    }

    fn get_vertex_distance(&self, v: usize, face_idx: FaceId) -> f64 {
        let face = &self.face_pool[face_idx];
        let base = v * self.num_dim;
        let mut sum = face.offset;
        for i in 0..self.num_dim {
            sum += face.normal[i] * self.positions[base + i];
        }
        sum
    }

    /// |det(edge_vectors[0..=last])| with rows beyond `last_index`
    /// filled with `(-1)^row * row / big_number` placeholders.
    fn get_simplex_volume(&self, edges: &[Small<f64>], last_index: usize, big_number: f64) -> f64 {
        let dim = self.num_dim;
        let mut storage = [0.0f64; 9];
        let a = &mut storage[..dim * dim];
        let mut idx = 0;
        for i in 0..dim {
            for j in 0..dim {
                if i <= last_index {
                    a[idx] = edges[i][j];
                    idx += 1;
                } else {
                    let sign = if idx % 2 == 0 { 1.0 } else { -1.0 };
                    a[idx] = sign * (idx as f64) / big_number;
                    idx += 1;
                }
            }
        }
        determinant_destructive(a, dim).abs()
    }

    // ---- Main algorithm ------------------------------------------

    fn get_convex_hull(&mut self) {
        self.find_bounding_box_points();
        self.shift_and_scale_positions();
        self.create_initial_simplex();
        while let Some(first) = self.unprocessed_first {
            let furthest = self.face_pool[first].furthest_vertex;
            self.current_vertex = furthest;
            self.update_center();
            self.tag_affected_faces(first);
            let singular = self.singular_vertices[self.current_vertex];
            if !singular && self.create_cone() {
                self.commit_cone();
            } else {
                self.handle_singular();
            }
            let count = self.affected_face_buffer.len();
            for i in 0..count {
                self.affected_face_flags[self.affected_face_buffer[i]] = false;
            }
        }
    }

    fn find_bounding_box_points(&mut self) {
        let mut least = usize::MAX;
        self.index_of_dim_with_least_extremes = 0;
        for i in 0..self.num_dim {
            let mut mins: Vec<usize> = Vec::new();
            let mut maxs: Vec<usize> = Vec::new();
            let mut cur_min = f64::INFINITY;
            let mut cur_max = f64::NEG_INFINITY;
            for j in 0..self.vertices_count {
                let c = self.get_coordinate(j, i);

                let d = cur_min - c;
                if d >= self.plane_distance_tolerance {
                    cur_min = c;
                    mins.clear();
                    mins.push(j);
                } else if d > 0.0 {
                    cur_min = c;
                    let cur = cur_min;
                    let tol = self.plane_distance_tolerance;
                    let positions = &self.positions;
                    let num_dim = self.num_dim;
                    mins.retain(|&idx| cur - positions[idx * num_dim + i] <= tol);
                    mins.push(j);
                } else if d > -self.plane_distance_tolerance {
                    mins.push(j);
                }

                let d2 = c - cur_max;
                if d2 >= self.plane_distance_tolerance {
                    cur_max = c;
                    maxs.clear();
                    maxs.push(j);
                } else if d2 > 0.0 {
                    cur_max = c;
                    // The C# source has a known oddity here: its
                    // retain lambda tests `min - GetCoordinate(...)`
                    // instead of `max - ...`. We mirror that exactly
                    // so the output matches ONI's behaviour.
                    let cur = cur_min;
                    let tol = self.plane_distance_tolerance;
                    let positions = &self.positions;
                    let num_dim = self.num_dim;
                    maxs.retain(|&idx| cur - positions[idx * num_dim + i] <= tol);
                    maxs.push(j);
                } else if d2 > -self.plane_distance_tolerance {
                    maxs.push(j);
                }
            }
            self.minima[i] = cur_min;
            self.maxima[i] = cur_max;
            let mut combined = mins;
            combined.extend(maxs);
            if combined.len() < least {
                least = combined.len();
                self.index_of_dim_with_least_extremes = i;
            }
            self.bounding_box_points[i] = combined;
        }
    }

    fn shift_and_scale_positions(&mut self) {
        let n = self.positions.len();
        if self.is_lifted {
            let top = self.num_dim - 1;
            let sum_abs = self.minima.iter().map(|x| x.abs()).sum::<f64>()
                + self.maxima.iter().map(|x| x.abs()).sum::<f64>()
                - self.maxima[top].abs()
                - self.minima[top].abs();
            let scale = 2.0 / sum_abs;
            self.minima[top] *= scale;
            self.maxima[top] *= scale;
            let mut k = top;
            while k < n {
                self.positions[k] *= scale;
                k += self.num_dim;
            }
        }
        let mut shift = Small::filled(0.0f64, self.num_dim);
        for d in 0..self.num_dim {
            if self.maxima[d] == self.minima[d] {
                shift[d] = 0.0;
            } else {
                shift[d] = self.maxima[d] - self.minima[d] - self.minima[d];
            }
        }
        for i in 0..n {
            self.positions[i] += shift[i % self.num_dim];
        }
    }

    fn create_initial_simplex(&mut self) {
        let initial = self.find_initial_points();
        let mut simplex_face_ids = vec![NO_FACE; self.num_dim + 1];
        for i in 0..=self.num_dim {
            let mut verts = Small::filled(0usize, self.num_dim);
            let mut k = 0;
            for j in 0..=self.num_dim {
                if i != j {
                    verts[k] = initial[j];
                    k += 1;
                }
            }
            verts.sort_unstable();
            let fid = self.get_face();
            self.face_pool[fid].vertices = verts;
            self.calculate_face_plane(fid);
            simplex_face_ids[i] = fid;
        }
        for k in 0..self.num_dim {
            for l in (k + 1)..=self.num_dim {
                self.update_adjacency(simplex_face_ids[k], simplex_face_ids[l]);
            }
        }
        for &fid in &simplex_face_ids.clone() {
            self.find_beyond_vertices_all(fid);
            if self.face_pool[fid].vertices_beyond.is_empty() {
                self.convex_faces.push(fid);
            } else {
                self.unprocessed_add(fid);
            }
        }
        for &v in &initial {
            self.vertex_visited[v] = false;
        }
    }

    fn find_initial_points(&mut self) -> Vec<usize> {
        let big_number = self.maxima.iter().sum::<f64>()
            * (self.num_dim as f64)
            * (self.vertices_count as f64);

        // Take first+last from the "least-extreme" dimension.
        let dim = self.index_of_dim_with_least_extremes;
        assert!(self.bounding_box_points[dim].len() >= 2, "degenerate input");
        let first = self.bounding_box_points[dim][0];
        let last = *self.bounding_box_points[dim].last().unwrap();
        // Remove first, then last (in that order to match C#).
        self.bounding_box_points[dim].remove(0);
        self.bounding_box_points[dim].pop();
        let mut list = vec![first, last];
        self.vertex_visited[first] = true;
        self.vertex_visited[last] = true;
        self.current_vertex = first;
        self.update_center();
        self.current_vertex = last;
        self.update_center();

        let mut edges: Vec<Small<f64>> = vec![Small::filled(0.0, 0); self.num_dim];
        edges[0] = self.vector_between_vertices_new(last, first);

        // All bounding box points flattened (preserves per-dim order).
        let mut pool: Vec<usize> = self
            .bounding_box_points
            .iter()
            .flatten()
            .copied()
            .collect();

        let mut n3 = 1usize;
        while n3 < self.num_dim && !pool.is_empty() {
            let mut pick: i64 = -1;
            let mut pick_edge: Small<f64> = Small::filled(0.0, 0);
            let mut best_vol = 0.0;
            let mut idx = pool.len();
            while idx > 0 {
                idx -= 1;
                let v = pool[idx];
                if list.contains(&v) {
                    pool.remove(idx);
                } else {
                    let cand = self.vector_between_vertices_new(v, first);
                    edges[n3] = cand;
                    let vol = self.get_simplex_volume(&edges, n3, big_number);
                    if best_vol < vol {
                        best_vol = vol;
                        pick = v as i64;
                        pick_edge = cand;
                    }
                }
            }
            if pick >= 0 {
                let pv = pick as usize;
                if let Some(pos) = pool.iter().position(|&x| x == pv) {
                    pool.remove(pos);
                }
                list.push(pv);
                edges[n3] = pick_edge;
                n3 += 1;
                self.current_vertex = pv;
                self.update_center();
            } else {
                break;
            }
        }

        // Fallback 1: scan all vertices.
        if list.len() <= self.num_dim {
            let mut pool2: Vec<usize> = (0..self.vertices_count).collect();
            while n3 < self.num_dim && !pool2.is_empty() {
                let mut pick: i64 = -1;
                let mut pick_edge: Small<f64> = Small::filled(0.0, 0);
                let mut best_vol = 0.0;
                let mut idx = pool2.len();
                while idx > 0 {
                    idx -= 1;
                    let v = pool2[idx];
                    if list.contains(&v) {
                        pool2.remove(idx);
                    } else {
                        let cand = self.vector_between_vertices_new(v, first);
                        edges[n3] = cand;
                        let vol = self.get_simplex_volume(&edges, n3, big_number);
                        if best_vol < vol {
                            best_vol = vol;
                            pick = v as i64;
                            pick_edge = cand;
                        }
                    }
                }
                if pick >= 0 {
                    let pv = pick as usize;
                    if let Some(pos) = pool2.iter().position(|&x| x == pv) {
                        pool2.remove(pos);
                    }
                    list.push(pv);
                    edges[n3] = pick_edge;
                    n3 += 1;
                    self.current_vertex = pv;
                    self.update_center();
                } else {
                    break;
                }
            }
        }

        if list.len() <= self.num_dim && self.is_lifted {
            // Degenerate fallback (e.g. cocircular input). Mirrors
            // `RandomOffsetToLift` in the C# source, but with a
            // deterministic `KRandom` seeded from the input so our
            // output is reproducible.
            let mut pool3: Vec<usize> = (0..self.vertices_count).collect();
            let mut k_rng = self.degenerate_rng();
            while n3 < self.num_dim && !pool3.is_empty() {
                let mut pick: i64 = -1;
                let mut pick_edge: Small<f64> = Small::filled(0.0, 0);
                let mut best_vol = 0.0;
                let mut idx = pool3.len();
                while idx > 0 {
                    idx -= 1;
                    let v = pool3[idx];
                    if list.contains(&v) {
                        pool3.remove(idx);
                    } else {
                        self.random_offset_to_lift(v, &mut k_rng);
                        let cand = self.vector_between_vertices_new(v, first);
                        edges[n3] = cand;
                        let vol = self.get_simplex_volume(&edges, n3, big_number);
                        if best_vol < vol {
                            best_vol = vol;
                            pick = v as i64;
                            pick_edge = cand;
                        }
                    }
                }
                if pick >= 0 {
                    let pv = pick as usize;
                    if let Some(pos) = pool3.iter().position(|&x| x == pv) {
                        pool3.remove(pos);
                    }
                    list.push(pv);
                    edges[n3] = pick_edge;
                    n3 += 1;
                    self.current_vertex = pv;
                    self.update_center();
                } else {
                    break;
                }
            }
        }
        if list.len() <= self.num_dim {
            panic!("input is degenerate for the requested dimension");
        }
        list
    }

    /// Seed a `KRandom` from a hash of the input positions. Same
    /// input → same sequence, so the degenerate branch stays
    /// reproducible.
    fn degenerate_rng(&self) -> KRandom {
        let mut seed: i32 = 0x1234_5678u32 as i32;
        for &p in &self.positions {
            let bits = p.to_bits();
            let lo = (bits & 0xFFFF_FFFF) as u32;
            let hi = (bits >> 32) as u32;
            let s = format!("{:08x}{:08x}", hi, lo);
            seed = seed.wrapping_add(sdbm_lower(&s) as i32);
        }
        KRandom::new(seed)
    }

    fn random_offset_to_lift(&mut self, index: usize, rng: &mut KRandom) {
        let pos = index * self.num_dim + self.num_dim - 1;
        let n = rng.next_double();
        self.positions[pos] += self.positions[pos] * n;
    }

    fn update_adjacency(&mut self, l: FaceId, r: FaceId) {
        let v_l = self.face_pool[l].vertices;
        let v_r = self.face_pool[r].vertices;
        for &v in &v_l {
            self.vertex_visited[v] = false;
        }
        for &v in &v_r {
            self.vertex_visited[v] = true;
        }
        let mut i = 0usize;
        while i < v_l.len() && self.vertex_visited[v_l[i]] {
            i += 1;
        }
        if i == self.num_dim {
            return;
        }
        for j in (i + 1)..v_l.len() {
            if !self.vertex_visited[v_l[j]] {
                return;
            }
        }
        self.face_pool[l].adjacent_faces[i] = r;
        for &v in &v_l {
            self.vertex_visited[v] = false;
        }
        let mut k = 0usize;
        while k < v_r.len() && !self.vertex_visited[v_r[k]] {
            k += 1;
        }
        self.face_pool[r].adjacent_faces[k] = l;
    }

    fn find_beyond_vertices_all(&mut self, face_idx: FaceId) {
        self.max_distance = f64::NEG_INFINITY;
        self.furthest_vertex = 0;
        let mut vb = std::mem::take(&mut self.face_pool[face_idx].vertices_beyond);
        for i in 0..self.vertices_count {
            if !self.vertex_visited[i] {
                self.is_beyond(face_idx, &mut vb, i);
            }
        }
        let fv = self.furthest_vertex;
        self.face_pool[face_idx].vertices_beyond = vb;
        self.face_pool[face_idx].furthest_vertex = fv;
    }

    fn tag_affected_faces(&mut self, face_idx: FaceId) {
        self.affected_face_buffer.clear();
        self.affected_face_buffer.push(face_idx);
        self.traverse_affected_faces(face_idx);
    }

    fn traverse_affected_faces(&mut self, start: FaceId) {
        self.traverse_stack.clear();
        self.traverse_stack.push(start);
        if start >= self.affected_face_flags.len() {
            self.affected_face_flags.resize(start + 1, false);
        }
        self.affected_face_flags[start] = true;
        while let Some(fid) = self.traverse_stack.pop() {
            for i in 0..self.num_dim {
                let n = self.face_pool[fid].adjacent_faces[i];
                if n == NO_FACE {
                    continue;
                }
                if n >= self.affected_face_flags.len() {
                    self.affected_face_flags.resize(n + 1, false);
                }
                if !self.affected_face_flags[n]
                    && self.get_vertex_distance(self.current_vertex, n)
                        >= self.plane_distance_tolerance
                {
                    self.affected_face_buffer.push(n);
                    self.affected_face_flags[n] = true;
                    self.traverse_stack.push(n);
                }
            }
        }
    }

    fn create_cone(&mut self) -> bool {
        let current_vertex = self.current_vertex;
        self.cone_face_buffer.clear();
        for i in 0..self.affected_face_buffer.len() {
            let num = self.affected_face_buffer[i];
            let mut num2 = 0usize;
            for j in 0..self.num_dim {
                let adj = self.face_pool[num].adjacent_faces[j];
                let flagged = adj != NO_FACE && adj < self.affected_face_flags.len()
                    && self.affected_face_flags[adj];
                if !flagged {
                    self.update_buffer[num2] = adj;
                    self.update_indices[num2] = j;
                    num2 += 1;
                }
            }
            for k in 0..num2 {
                let pivot_id = self.update_buffer[k];
                let mut pivot_index = 0usize;
                if pivot_id != NO_FACE {
                    for (l, &aj) in self.face_pool[pivot_id].adjacent_faces.iter().enumerate() {
                        if aj == num {
                            pivot_index = l;
                            break;
                        }
                    }
                }
                let num4 = self.update_indices[k];
                let face = self.get_face();
                let mut vertices = self.face_pool[num].vertices;
                let num5 = vertices[num4];
                let num6;
                if current_vertex < num5 {
                    let mut n6 = 0usize;
                    let mut n7 = num4 as isize - 1;
                    while n7 >= 0 {
                        if vertices[n7 as usize] > current_vertex {
                            vertices[(n7 + 1) as usize] = vertices[n7 as usize];
                            n7 -= 1;
                            continue;
                        }
                        n6 = (n7 + 1) as usize;
                        break;
                    }
                    num6 = n6;
                } else {
                    let mut n6 = self.num_dim - 1;
                    for n in (num4 + 1)..self.num_dim {
                        if vertices[n] < current_vertex {
                            vertices[n - 1] = vertices[n];
                            continue;
                        }
                        n6 = n - 1;
                        break;
                    }
                    num6 = n6;
                }
                vertices[num6] = current_vertex;
                self.face_pool[face].vertices = vertices;
                if !self.calculate_face_plane(face) {
                    return false;
                }
                let df = DeferredFace {
                    face_idx: face,
                    pivot_idx: pivot_id,
                    old_face_idx: num,
                    face_index: num6,
                    pivot_index,
                };
                self.cone_face_buffer.push(df);
            }
        }
        true
    }

    fn commit_cone(&mut self) {
        let n_cone = self.cone_face_buffer.len();
        for i in 0..n_cone {
            let df = self.cone_face_buffer[i];
            let face_id = df.face_idx;
            let pivot_id = df.pivot_idx;
            let old_id = df.old_face_idx;
            let face_index = df.face_index;
            let pivot_index = df.pivot_index;

            if pivot_id != NO_FACE {
                self.face_pool[face_id].adjacent_faces[face_index] = pivot_id;
                self.face_pool[pivot_id].adjacent_faces[pivot_index] = face_id;
            } else {
                self.face_pool[face_id].adjacent_faces[face_index] = NO_FACE;
            }

            for j in 0..self.num_dim {
                if j != face_index {
                    self.connect_face_via_connector(face_id, j);
                }
            }

            // Distribute beyond-vertex work between pivot's and
            // old face's beyond-buffers, same branch structure as C#.
            let pivot_beyond_empty = pivot_id == NO_FACE
                || self.face_pool[pivot_id].vertices_beyond.is_empty();
            if pivot_beyond_empty {
                // Use old face's beyond-list only.
                self.find_beyond_from_one(face_id, old_id);
            } else {
                let pivot_len = self.face_pool[pivot_id].vertices_beyond.len();
                let old_len = self.face_pool[old_id].vertices_beyond.len();
                if pivot_len < old_len {
                    self.find_beyond_from_two(face_id, pivot_id, old_id);
                } else {
                    self.find_beyond_from_two(face_id, old_id, pivot_id);
                }
            }

            if self.face_pool[face_id].vertices_beyond.is_empty() {
                self.convex_faces.push(face_id);
                self.unprocessed_remove(face_id);
                let vb = std::mem::take(&mut self.face_pool[face_id].vertices_beyond);
                self.empty_buffer_stack.push(vb);
                // Face's vertices_beyond stays empty Vec.
            } else {
                self.unprocessed_add(face_id);
            }
        }
        // DeferredFaces are dropped (pool rebuilt next round).
        self.deferred_face_stack.append(&mut self.cone_face_buffer);

        // Recycle affected faces.
        let affected = std::mem::take(&mut self.affected_face_buffer);
        for fid in &affected {
            self.unprocessed_remove(*fid);
            self.deposit_face(*fid);
        }
        self.affected_face_buffer = affected;
    }

    fn handle_singular(&mut self) {
        self.rollback_center();
        self.singular_vertices[self.current_vertex] = true;
        for i in 0..self.affected_face_buffer.len() {
            let fid = self.affected_face_buffer[i];
            let vb_len = self.face_pool[fid].vertices_beyond.len();
            for j in 0..vb_len {
                let v = self.face_pool[fid].vertices_beyond[j];
                self.singular_vertices[v] = true;
            }
            self.convex_faces.push(fid);
            self.unprocessed_remove(fid);
            let vb = std::mem::take(&mut self.face_pool[fid].vertices_beyond);
            self.empty_buffer_stack.push(vb);
        }
    }

    fn is_beyond(&mut self, face_idx: FaceId, beyond: &mut Vec<usize>, v: usize) {
        let d = self.get_vertex_distance(v, face_idx);
        if !(d >= self.plane_distance_tolerance) {
            return;
        }
        if d > self.max_distance {
            if d - self.max_distance < self.plane_distance_tolerance {
                if self.lex_compare(v, self.furthest_vertex) > 0 {
                    self.max_distance = d;
                    self.furthest_vertex = v;
                }
            } else {
                self.max_distance = d;
                self.furthest_vertex = v;
            }
        }
        beyond.push(v);
    }

    fn lex_compare(&self, u: usize, v: usize) -> i32 {
        let a = u * self.num_dim;
        let b = v * self.num_dim;
        for i in 0..self.num_dim {
            let x = self.positions[a + i];
            let y = self.positions[b + i];
            if x < y {
                return -1;
            }
            if x > y {
                return 1;
            }
        }
        0
    }

    fn find_beyond_from_one(&mut self, face_idx: FaceId, source_face: FaceId) {
        let mut out = std::mem::take(&mut self.beyond_buffer);
        out.clear();
        self.max_distance = f64::NEG_INFINITY;
        self.furthest_vertex = 0;
        // is_beyond reads only the new face, so the source list can be
        // borrowed out of the pool instead of copied.
        let source = std::mem::take(&mut self.face_pool[source_face].vertices_beyond);
        out.reserve(source.len());
        for &v in &source {
            if v != self.current_vertex {
                self.is_beyond(face_idx, &mut out, v);
            }
        }
        self.face_pool[source_face].vertices_beyond = source;
        let fv = self.furthest_vertex;
        let old = std::mem::replace(&mut self.face_pool[face_idx].vertices_beyond, out);
        self.face_pool[face_idx].furthest_vertex = fv;
        // Recycle old buffer.
        self.beyond_buffer = old;
        self.beyond_buffer.clear();
    }

    fn find_beyond_from_two(&mut self, face_idx: FaceId, beyond: FaceId, beyond1: FaceId) {
        let mut out = std::mem::take(&mut self.beyond_buffer);
        out.clear();
        self.max_distance = f64::NEG_INFINITY;
        self.furthest_vertex = 0;
        // Mark beyond1's vertices as visited, then walk `beyond`.
        let b1 = std::mem::take(&mut self.face_pool[beyond1].vertices_beyond);
        for &v in &b1 {
            self.vertex_visited[v] = true;
        }
        self.vertex_visited[self.current_vertex] = false;
        let b0 = std::mem::take(&mut self.face_pool[beyond].vertices_beyond);
        out.reserve(b0.len() + b1.len());
        for &v in &b0 {
            if v != self.current_vertex {
                self.vertex_visited[v] = false;
                self.is_beyond(face_idx, &mut out, v);
            }
        }
        for &v in &b1 {
            if self.vertex_visited[v] {
                self.is_beyond(face_idx, &mut out, v);
            }
        }
        self.face_pool[beyond].vertices_beyond = b0;
        self.face_pool[beyond1].vertices_beyond = b1;
        let fv = self.furthest_vertex;
        let old = std::mem::replace(&mut self.face_pool[face_idx].vertices_beyond, out);
        self.face_pool[face_idx].furthest_vertex = fv;
        self.beyond_buffer = old;
        self.beyond_buffer.clear();
    }

    fn update_center(&mut self) {
        for i in 0..self.num_dim {
            self.center[i] *= self.convex_hull_size as f64;
        }
        self.convex_hull_size += 1;
        let inv = 1.0 / self.convex_hull_size as f64;
        let base = self.current_vertex * self.num_dim;
        for j in 0..self.num_dim {
            self.center[j] = inv * (self.center[j] + self.positions[base + j]);
        }
    }

    fn rollback_center(&mut self) {
        for i in 0..self.num_dim {
            self.center[i] *= self.convex_hull_size as f64;
        }
        self.convex_hull_size -= 1;
        let inv = if self.convex_hull_size > 0 {
            1.0 / self.convex_hull_size as f64
        } else {
            0.0
        };
        let base = self.current_vertex * self.num_dim;
        for j in 0..self.num_dim {
            self.center[j] = inv * (self.center[j] - self.positions[base + j]);
        }
    }

    // ---- Face pool / unprocessed list / connector pool ----------

    fn get_face(&mut self) -> FaceId {
        if let Some(fid) = self.free_face_indices.pop() {
            let f = &mut self.face_pool[fid];
            for a in f.adjacent_faces.iter_mut() {
                *a = NO_FACE;
            }
            f.in_list = false;
            f.next = None;
            f.previous = None;
            f.furthest_vertex = 0;
            f.tag = 0;
            f.is_normal_flipped = false;
            f.vertices_beyond = self
                .empty_buffer_stack
                .pop()
                .unwrap_or_default();
            return fid;
        }
        let fid = self.face_pool.len();
        let mut face = FaceData::new(self.num_dim);
        face.vertices_beyond = self
            .empty_buffer_stack
            .pop()
            .unwrap_or_default();
        self.face_pool.push(face);
        self.affected_face_flags.push(false);
        fid
    }

    fn deposit_face(&mut self, fid: FaceId) {
        for a in self.face_pool[fid].adjacent_faces.iter_mut() {
            *a = NO_FACE;
        }
        self.free_face_indices.push(fid);
    }

    fn unprocessed_add(&mut self, fid: FaceId) {
        // If face already in list, move it to the front if
        // its beyond-count now exceeds the current head's.
        let face_count = self.face_pool[fid].vertices_beyond.len();
        if self.face_pool[fid].in_list {
            if let Some(first) = self.unprocessed_first {
                if self.face_pool[first].vertices_beyond.len() < face_count {
                    self.unprocessed_remove(fid);
                    self.unprocessed_add_first(fid);
                }
            }
            return;
        }
        self.face_pool[fid].in_list = true;
        if let Some(first) = self.unprocessed_first {
            if self.face_pool[first].vertices_beyond.len() < face_count {
                self.face_pool[first].previous = Some(fid);
                self.face_pool[fid].next = Some(first);
                self.face_pool[fid].previous = None;
                self.unprocessed_first = Some(fid);
                return;
            }
        }
        if let Some(last) = self.unprocessed_last {
            self.face_pool[last].next = Some(fid);
        }
        self.face_pool[fid].previous = self.unprocessed_last;
        self.face_pool[fid].next = None;
        self.unprocessed_last = Some(fid);
        if self.unprocessed_first.is_none() {
            self.unprocessed_first = Some(fid);
        }
    }

    fn unprocessed_add_first(&mut self, fid: FaceId) {
        self.face_pool[fid].in_list = true;
        if let Some(first) = self.unprocessed_first {
            self.face_pool[first].previous = Some(fid);
            self.face_pool[fid].next = Some(first);
        } else {
            self.face_pool[fid].next = None;
            self.unprocessed_last = Some(fid);
        }
        self.face_pool[fid].previous = None;
        self.unprocessed_first = Some(fid);
    }

    fn unprocessed_remove(&mut self, fid: FaceId) {
        if !self.face_pool[fid].in_list {
            return;
        }
        self.face_pool[fid].in_list = false;
        let prev = self.face_pool[fid].previous;
        let next = self.face_pool[fid].next;
        match prev {
            Some(p) => self.face_pool[p].next = next,
            None => self.unprocessed_first = next,
        }
        match next {
            Some(n) => self.face_pool[n].previous = prev,
            None => self.unprocessed_last = prev,
        }
        self.face_pool[fid].next = None;
        self.face_pool[fid].previous = None;
    }

    // ---- Connector pool / table ---------------------------------

    fn get_connector(&mut self) -> usize {
        if let Some(head) = self.connector_free_head {
            self.connector_free_head = self.connector_pool[head].next;
            self.connector_pool[head].next = None;
            self.connector_pool[head].previous = None;
            self.connector_pool[head].face_idx = NO_FACE;
            return head;
        }
        let id = self.connector_pool.len();
        self.connector_pool.push(FaceConnector::new(self.num_dim));
        id
    }

    fn deposit_connector(&mut self, cid: usize) {
        self.connector_pool[cid].next = self.connector_free_head;
        self.connector_pool[cid].previous = None;
        self.connector_pool[cid].face_idx = NO_FACE;
        self.connector_free_head = Some(cid);
    }

    fn connect_face_via_connector(&mut self, face_id: FaceId, edge_index: usize) {
        let cid = self.get_connector();
        // Update connector (mirrors FaceConnector.Update).
        let dim = self.num_dim;
        let verts = self.face_pool[face_id].vertices;
        {
            let c = &mut self.connector_pool[cid];
            c.face_idx = face_id;
            c.edge_index = edge_index;
            let mut hash: u32 = 23;
            let mut idx = 0;
            for i in 0..edge_index {
                c.vertices[idx] = verts[i];
                idx += 1;
                hash = hash.wrapping_mul(31).wrapping_add(verts[i] as u32);
            }
            for i in (edge_index + 1)..verts.len() {
                c.vertices[idx] = verts[i];
                idx += 1;
                hash = hash.wrapping_mul(31).wrapping_add(verts[i] as u32);
            }
            c.hash_code = hash;
        }
        self.connect_face(cid, dim);
    }

    fn connect_face(&mut self, cid: usize, dim: usize) {
        let _ = dim;
        let bucket = (self.connector_pool[cid].hash_code as usize) % self.connector_table_first.len();
        // Walk the bucket looking for a matching edge.
        let mut cur = self.connector_table_first[bucket];
        while let Some(other) = cur {
            if self.are_connectable(cid, other) {
                // Connect the two faces via the shared edge.
                let fa = self.connector_pool[other].face_idx;
                let ea = self.connector_pool[other].edge_index;
                let fb = self.connector_pool[cid].face_idx;
                let eb = self.connector_pool[cid].edge_index;
                self.face_pool[fa].adjacent_faces[ea] = fb;
                self.face_pool[fb].adjacent_faces[eb] = fa;

                // Remove `other` from bucket list.
                let prev = self.connector_pool[other].previous;
                let next = self.connector_pool[other].next;
                match prev {
                    Some(p) => self.connector_pool[p].next = next,
                    None => self.connector_table_first[bucket] = next,
                }
                if let Some(n) = next {
                    self.connector_pool[n].previous = prev;
                }
                self.connector_pool[other].next = None;
                self.connector_pool[other].previous = None;
                self.connector_pool[other].face_idx = NO_FACE;
                self.connector_pool[cid].face_idx = NO_FACE;
                self.deposit_connector(other);
                self.deposit_connector(cid);
                return;
            }
            cur = self.connector_pool[other].next;
        }
        // No match: add to bucket.
        self.connector_pool[cid].previous = None;
        self.connector_pool[cid].next = self.connector_table_first[bucket];
        if let Some(head) = self.connector_table_first[bucket] {
            self.connector_pool[head].previous = Some(cid);
        }
        self.connector_table_first[bucket] = Some(cid);
    }

    fn are_connectable(&self, a: usize, b: usize) -> bool {
        let ca = &self.connector_pool[a];
        let cb = &self.connector_pool[b];
        if ca.hash_code != cb.hash_code {
            return false;
        }
        if ca.vertices.len() != cb.vertices.len() {
            return false;
        }
        for i in 0..ca.vertices.len() {
            if ca.vertices[i] != cb.vertices[i] {
                return false;
            }
        }
        true
    }

    // ---- Upper-face removal (Delaunay-specific) -----------------

    fn remove_upper_faces(&mut self) {
        let top = self.num_dim - 1;
        let mut i = self.convex_faces.len();
        while i > 0 {
            i -= 1;
            let fid = self.convex_faces[i];
            if self.face_pool[fid].normal[top] >= 0.0 {
                // Disconnect neighbours.
                let adj = self.face_pool[fid].adjacent_faces;
                for &a in &adj {
                    if a == NO_FACE {
                        continue;
                    }
                    for j in 0..self.face_pool[a].adjacent_faces.len() {
                        if self.face_pool[a].adjacent_faces[j] == fid {
                            self.face_pool[a].adjacent_faces[j] = NO_FACE;
                        }
                    }
                }
                // swap-remove.
                let last = self.convex_faces.len() - 1;
                self.convex_faces.swap(i, last);
                self.convex_faces.pop();
            }
        }
    }
}

/// Destructive determinant using the LU factorisation from the C#
/// source. Only the 2x2 and 3x3 cases are reachable for our
/// dim in {2, 3}.
fn determinant_destructive(a: &mut [f64], dim: usize) -> f64 {
    match dim {
        0 => 0.0,
        1 => a[0],
        2 => a[0] * a[3] - a[1] * a[2],
        3 => {
            a[0] * a[4] * a[8]
                + a[1] * a[5] * a[6]
                + a[2] * a[3] * a[7]
                - a[0] * a[5] * a[7]
                - a[1] * a[3] * a[8]
                - a[2] * a[4] * a[6]
        }
        _ => panic!("dim > 3 not needed for MIConvexHull Delaunay"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weighted_lower_hull_changes_connectivity() {
        let a = lower_hull_3d(&[(0.0,0.0,-1.0), (2.0,0.0,4.0), (2.0,2.0,8.0), (0.0,2.0,4.0)]);
        let b = lower_hull_3d(&[(0.0,0.0,0.0), (2.0,0.0,3.0), (2.0,2.0,8.0), (0.0,2.0,4.0)]);
        assert_eq!(tri_set(&a.iter().map(|f| f.vertices).collect::<Vec<_>>()), tri_set(&[[0,1,2], [0,2,3]]));
        assert_eq!(tri_set(&b.iter().map(|f| f.vertices).collect::<Vec<_>>()), tri_set(&[[0,1,3], [1,2,3]]));
    }

    fn tri_set(tris: &[[usize; 3]]) -> std::collections::HashSet<[usize; 3]> {
        let mut s = std::collections::HashSet::new();
        for t in tris {
            let mut t = *t;
            t.sort_unstable();
            s.insert(t);
        }
        s
    }

    /// A unit square should Delaunay-triangulate into two triangles.
    #[test]
    fn unit_square_two_triangles() {
        let pts = vec![(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)];
        let tris = delaunay_2d(&pts);
        assert_eq!(tris.len(), 2);
        let set = tri_set(&tris);
        // Either diagonal {0,1,2}+{0,2,3} or {0,1,3}+{1,2,3}.
        let diag_a = [[0usize, 1, 2], [0, 2, 3]];
        let diag_b = [[0usize, 1, 3], [1, 2, 3]];
        let set_a = tri_set(&diag_a);
        let set_b = tri_set(&diag_b);
        assert!(set == set_a || set == set_b);
    }

    #[test]
    fn five_points_covers_all() {
        let pts = vec![
            (0.0, 0.0),
            (2.0, 0.0),
            (2.0, 2.0),
            (0.0, 2.0),
            (1.0, 1.0),
        ];
        let tris = delaunay_2d(&pts);
        // 5 points with the 5th at the centre => 4 triangles fanned
        // around index 4.
        assert_eq!(tris.len(), 4);
        let set = tri_set(&tris);
        for t in set {
            assert!(t.contains(&4));
        }
    }

    #[test]
    fn deterministic_same_input() {
        let pts = vec![
            (1.3, 2.7), (4.1, 0.2), (3.6, 3.9), (0.5, 4.4),
            (2.2, 1.1), (4.7, 2.8), (0.9, 0.6), (3.0, 0.8),
        ];
        let a = delaunay_2d(&pts);
        let b = delaunay_2d(&pts);
        assert_eq!(a, b);
    }

    #[test]
    fn tolerant_to_wobble() {
        // Nudge the square vertices by a tiny amount; still two tris.
        let pts = vec![
            (0.000000001, 0.0),
            (1.0, 0.000000002),
            (1.0, 1.0),
            (0.0, 1.0),
        ];
        let tris = delaunay_2d(&pts);
        assert_eq!(tris.len(), 2);
    }
}
