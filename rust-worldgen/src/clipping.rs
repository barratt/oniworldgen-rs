//! ONI Polygon.Clip: Clipper 6 integer intersection, with the game's scaling.
//! Native builds use the upstream C++ implementation through clipper-sys;
//! WASM builds (and native builds with the `rust-clipper` feature) use the Rust
//! port in `crate::clipper`, which gives the same vertices in the same order.
//! Worldgen does not load ONI assemblies or a WASM generator.
use crate::geom::{Polygon, V2};

pub fn intersect(subject: &Polygon, bounds: &Polygon) -> Option<Polygon> {
    intersect_vertices(&subject.vertices, &bounds.vertices)
}

/// [`intersect`] of the polygons with these vertex lists.
pub fn intersect_vertices(subject: &[V2], bounds: &[V2]) -> Option<Polygon> {
    fn path(vertices: &[V2], out: &mut Vec<[i64; 2]>) -> Option<()> {
        out.clear();
        if vertices.len() < 3 { return None; }
        for p in vertices {
            let (x, y) = (p.x as f64 * 10000.0, p.y as f64 * 10000.0);
            // Stay inside Clipper's signed 62-bit range; reject NaN before FFI.
            if !x.is_finite() || !y.is_finite() || x.abs() >= (1u64 << 61) as f64 || y.abs() >= (1u64 << 61) as f64 { return None; }
            out.push([x as i64, y as i64]); // C# IntPoint(double) truncates toward zero.
        }
        Some(())
    }
    // Working paths, reused per thread.
    thread_local! { static PATHS: std::cell::RefCell<(Vec<[i64; 2]>, Vec<[i64; 2]>)> = const { std::cell::RefCell::new((Vec::new(), Vec::new())) }; }
    let to_v2 = |p: &[i64; 2]| V2::new(p[0] as f32 * 0.0001, p[1] as f32 * 0.0001);
    let first = PATHS.with(|paths| {
        let (a, b) = &mut *paths.borrow_mut();
        path(subject, a)?;
        path(bounds, b)?;
        match contained_convex(a, b) {
            Some(order) => {
                let contour: Vec<V2> = order.map(|i| to_v2(&a[i])).collect();
                #[cfg(feature = "clip-check")]
                assert_eq!(clipper_first(a.clone(), b.clone()).map(|c| c.iter().map(to_v2).collect::<Vec<_>>()).as_ref(), Some(&contour), "contained_convex: {a:?} / {b:?}");
                Some(contour)
            }
            None => clipper_first(a.clone(), b.clone()).map(|c| c.iter().map(to_v2).collect()),
        }
    })?;
    if first.len() < 3 { return None; }
    Some(Polygon::new(first))
}

/// Whether `points` are a strictly convex loop whose every point is far
/// (1e-4) from the line through its neighbours, so that a convex hull of
/// them keeps them all.
pub fn is_convex_loop(points: &[V2]) -> bool {
    let n = points.len();
    if n < 3 { return false; }
    let mut sign = 0.0;
    for i in 0..n {
        let (p, q, r) = (points[(i + n - 1) % n], points[i], points[(i + 1) % n]);
        let (ux, uy) = (q.x as f64 - p.x as f64, q.y as f64 - p.y as f64);
        let (vx, vy) = (r.x as f64 - p.x as f64, r.y as f64 - p.y as f64);
        let cross = ux * vy - uy * vx;
        if i == 0 { sign = cross.signum(); }
        if !(cross * sign > 1e-4 * (vx * vx + vy * vy).sqrt()) { return false; }
    }
    true
}

/// The first contour of Clipper's intersection.
fn clipper_first(a: Vec<[i64; 2]>, b: Vec<[i64; 2]>) -> Option<Vec<[i64; 2]>> {
    #[cfg(any(target_arch = "wasm32", feature = "rust-clipper"))]
    let polygons = port_intersection(&a, &b);
    #[cfg(not(any(target_arch = "wasm32", feature = "rust-clipper")))]
    let polygons = native_intersection(a, b);
    polygons.into_iter().next()?.into_iter().next()
}

/// Clipper's intersection when it is `a` itself: `a` strictly convex without
/// horizontal edges and well inside the convex `b`. Clipper then adds
/// `a`'s vertices to one output polygon from its bottom (largest y) up both
/// sides, ends it at the top vertex (smallest y), orients it positively and
/// returns it starting after the top vertex, so the top vertex comes last.
/// Returns the indexes into `a` in output order; `None` where that does not
/// apply.
fn contained_convex(a: &[[i64; 2]], b: &[[i64; 2]]) -> Option<impl Iterator<Item = usize>> {
    // Clipper's 30-bit range: no overflow below, no full-range arithmetic.
    const RANGE: i64 = 0x3FFF_FFFF;
    let (n, m) = (a.len(), b.len());
    if n < 3 || m < 3 || a.iter().chain(b).any(|p| p[0].abs() > RANGE || p[1].abs() > RANGE) { return None; }
    let cross = |o: [i64; 2], p: [i64; 2], q: [i64; 2]| (p[0] - o[0]) * (q[1] - o[1]) - (p[1] - o[1]) * (q[0] - o[0]);
    // Every turn strictly the same way and y changing direction twice: one
    // convex loop (without horizontal edges for `a`).
    let convex = |p: &[[i64; 2]], horizontals: bool| -> Option<i64> {
        let k = p.len();
        let sign = cross(p[k - 1], p[0], p[1]).signum();
        if sign == 0 { return None; }
        let (mut turns, mut last) = (0, 0);
        for i in 0..k {
            let (q, r, s) = (p[i], p[(i + 1) % k], p[(i + 2) % k]);
            if cross(q, r, s).signum() != sign { return None; }
            let dy = (r[1] - q[1]).signum();
            if dy == 0 { if horizontals { continue } else { return None } }
            if last != 0 && dy != last { turns += 1; }
            last = dy;
        }
        let first = p.iter().zip(p.iter().cycle().skip(1)).map(|(q, r)| (r[1] - q[1]).signum()).find(|&d| d != 0)?;
        if first != last { turns += 1; }
        (turns == 2).then_some(sign)
    };
    let sign_a = convex(a, false)?;
    let sign_b = convex(b, true)?;
    for i in 0..m {
        let (p, q) = (b[i], b[(i + 1) % m]);
        if a.iter().any(|&v| cross(p, q, v).signum() != sign_b) { return None; }
    }
    // Clipper places edges at each scanbeam's y by rounding, so it only
    // keeps `a` as it is where, at every vertex y of either path, `a` is at
    // least two units wide and two units clear of `b` on both sides.
    let span = |p: &[[i64; 2]], y: i64| -> (f64, f64) {
        let (mut lo, mut hi) = (f64::MAX, f64::MIN);
        for i in 0..p.len() {
            let (q, r) = (p[i], p[(i + 1) % p.len()]);
            if (q[1] <= y && y <= r[1]) || (r[1] <= y && y <= q[1]) {
                let x = if q[1] == r[1] { lo = lo.min(q[0].min(r[0]) as f64); hi = hi.max(q[0].max(r[0]) as f64); continue }
                    else { q[0] as f64 + (r[0] - q[0]) as f64 * (y - q[1]) as f64 / (r[1] - q[1]) as f64 };
                lo = lo.min(x); hi = hi.max(x);
            }
        }
        (lo, hi)
    };
    let top = (0..n).min_by_key(|&i| a[i][1])?;
    let bottom = a.iter().map(|p| p[1]).max()?;
    for y in a.iter().chain(b).map(|p| p[1]).filter(|&y| a[top][1] <= y && y <= bottom) {
        let ((al, ar), (bl, br)) = (span(a, y), span(b, y));
        if al - bl < 2.0 || br - ar < 2.0 || (y != a[top][1] && y != bottom && ar - al < 2.0) { return None; }
    }
    // Positive area in Clipper's sense is a positive shoelace sum over the
    // returned order; `sign_a` is the shoelace sign of `a`'s own order.
    Some((1..=n).map(move |k| if sign_a > 0 { (top + k) % n } else { (top + n - k) % n }))
}

/// Even-odd intersection of two closed paths: outer contours, each followed by its holes.
type Polygons = Vec<Vec<Vec<[i64; 2]>>>;

#[cfg_attr(not(any(target_arch = "wasm32", feature = "rust-clipper", test)), allow(dead_code))]
fn port_intersection(a: &[[i64; 2]], b: &[[i64; 2]]) -> Polygons {
    use crate::clipper::{execute, ClipType, FillType};
    execute(ClipType::Intersection, a, b, FillType::EvenOdd, FillType::EvenOdd)
}

#[cfg(not(target_arch = "wasm32"))]
#[cfg_attr(feature = "rust-clipper", allow(dead_code))]
fn native_intersection(mut a: Vec<[i64; 2]>, mut b: Vec<[i64; 2]>) -> Polygons {
    use clipper_sys as ffi;
    let mut paths = [
        ffi::Path { vertices: a.as_mut_ptr(), vertices_count: a.len(), closed: 1 },
        ffi::Path { vertices: b.as_mut_ptr(), vertices_count: b.len(), closed: 1 },
    ];
    let mut polygons = [
        ffi::Polygon { paths: &mut paths[0], paths_count: 1, type_: ffi::PolyType_ptSubject },
        ffi::Polygon { paths: &mut paths[1], paths_count: 1, type_: ffi::PolyType_ptClip },
    ];
    struct Output(ffi::Polygons);
    impl Drop for Output {
        fn drop(&mut self) { unsafe { ffi::free_polygons(self.0); } }
    }
    // Input buffers outlive execute, which copies them. The returned allocation
    // is owned by Output and freed by the same library, including on unwinding.
    let output = Output(unsafe { ffi::execute(ffi::ClipType_ctIntersection,
        ffi::Polygons { polygons: polygons.as_mut_ptr(), polygons_count: 2 },
        ffi::PolyFillType_pftEvenOdd, ffi::PolyFillType_pftEvenOdd) });
    fn slice<'a, T>(ptr: *const T, n: usize) -> &'a [T] { if n == 0 { &[] } else { unsafe { std::slice::from_raw_parts(ptr, n) } } }
    slice(output.0.polygons, output.0.polygons_count).iter().map(|polygon|
        slice(polygon.paths, polygon.paths_count).iter().map(|path| slice(path.vertices, path.vertices_count).to_vec()).collect()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::Rect;
    #[test]
    fn clips_to_actual_polygon_instead_of_its_box() {
        let subject = Polygon::from_rect(Rect::new(-1.0, -1.0, 5.0, 5.0));
        let triangle = Polygon::new(vec![V2::new(0.0, 0.0), V2::new(2.0, 0.0), V2::new(0.0, 2.0)]);
        let result = intersect(&subject, &triangle).unwrap();
        assert_eq!(result.vertices.len(), 3);
        assert_eq!(result.area(), 2.0);
        assert!(result.vertices.iter().all(|p| triangle.vertices.contains(p)));
    }
    #[test]
    fn disjoint_and_nonfinite_input_have_no_polygon() {
        let a = Polygon::from_rect(Rect::new(0.0, 0.0, 1.0, 1.0));
        let b = Polygon::from_rect(Rect::new(2.0, 2.0, 1.0, 1.0));
        assert!(intersect(&a, &b).is_none());
        let mut bad = a.clone(); bad.vertices[0].x = f32::NAN;
        assert!(intersect(&bad, &a).is_none());
    }

    /// The convex containment shortcut against clipper-sys, on contained
    /// and nearly contained convex cells, with snapped coordinates that give
    /// horizontal edges, collinear points and vertices on the bounds.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn contained_convex_matches_clipper_sys() {
        let mut rng = crate::rng::KRandom::new(744825);
        let mut hits = 0;
        for round in 0..300000 {
            let scale = [4_000_000, 60, 8][round % 3];
            let convex_path = |rng: &mut crate::rng::KRandom, n: usize, cx: f64, cy: f64, r: f64| -> Vec<[i64; 2]> {
                let mut angles: Vec<f64> = (0..n).map(|_| rng.next_double() * std::f64::consts::TAU).collect();
                angles.sort_by(f64::total_cmp);
                if round % 2 == 0 { angles.reverse(); }
                let start = rng.next_max(n as i32) as usize;
                angles.rotate_left(start);
                angles.iter().map(|a| [(cx + r * a.cos()) as i64, (cy + r * a.sin()) as i64]).collect()
            };
            let (cx, cy) = (rng.next_max(scale) as f64, rng.next_max(scale) as f64);
            let r = 1.0 + rng.next_max(scale) as f64 / 2.0;
            let b = convex_path(&mut rng, 3 + round % 9, cx, cy, r);
            let (ox, oy) = ((rng.next_double() - 0.5) * r, (rng.next_double() - 0.5) * r);
            let ra = r * (0.05 + rng.next_double() * 0.9);
            let a = convex_path(&mut rng, 3 + (round / 9) % 8, cx + ox, cy + oy, ra);
            let fast = contained_convex(&a, &b).map(|order| order.map(|i| a[i]).collect::<Vec<_>>());
            let expected = native_intersection(a.clone(), b.clone()).into_iter().next().and_then(|p| p.into_iter().next());
            if let Some(contour) = fast {
                hits += 1;
                assert_eq!(Some(contour), expected, "round {round}: {a:?} / {b:?}");
            }
        }
        assert!(hits > 20000, "{hits} contained");
    }

    /// The Rust port against clipper-sys on convex cells like worldgen's, on
    /// snapped coordinates that force duplicates, collinear and horizontal
    /// edges, and on self-intersecting paths that need joins and holes.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn port_matches_clipper_sys() {
        let mut rng = crate::rng::KRandom::new(744825);
        let (mut cases, mut multi, mut holes) = (0, 0, 0);
        for round in 0..200000 {
            let kind = round % 4;
            let scale = [4_000_000, 40, 6, 1_000_000][kind];
            let mut random_path = |n: usize, convex: bool| -> Vec<[i64; 2]> {
                let (cx, cy, r) = (rng.next_max(scale) as f64, rng.next_max(scale) as f64, 1.0 + rng.next_max(scale) as f64 / 2.0);
                let mut angles: Vec<f64> = (0..n).map(|_| rng.next_double() * std::f64::consts::TAU).collect();
                if convex { angles.sort_by(f64::total_cmp); }
                angles.iter().map(|a| {
                    let rr = if convex { r } else { r * (0.2 + rng.next_double()) };
                    [(cx + rr * a.cos()) as i64, (cy + rr * a.sin()) as i64]
                }).collect()
            };
            let convex = kind != 3;
            let (na, nb) = if convex { (3 + round % 7, 3 + (round / 7) % 6) } else { (5 + round % 11, 3 + (round / 11) % 9) };
            let a = random_path(na, convex);
            let b = random_path(nb, convex || round % 8 == 3);
            let expected = native_intersection(a.clone(), b.clone());
            assert_eq!(port_intersection(&a, &b), expected, "round {round}: {a:?} / {b:?}");
            cases += usize::from(!expected.is_empty());
            multi += usize::from(expected.len() > 1);
            holes += usize::from(expected.iter().any(|p| p.len() > 1));
        }
        assert!(cases > 50000 && multi > 1000 && holes > 100, "{cases} clipped, {multi} with several outers, {holes} with holes");
    }
}
