//! Poisson-disk point sampler + higher-level `PointGenerator`.
//! Port of ONI's `UniformPoissonDiskSampler` (Bridson's algorithm)
//! plus the `PointGenerator` dispatcher that worldgen actually calls,
//! with the `UniformHex` / `PoissonDisk` behaviours implemented.
//! Other `SampleBehaviour` variants (`UniformSquare`, `UniformSpiral`,
//! `UniformCircle`) are not reached by ONI's worldgen pipeline and
//! are left out until a caller needs them.
//!
//! Used during worldgen to place subworld site points before biome
//! classification.

use crate::rng::KRandom;
use crate::geom::{Polygon, V2};

/// Matches ONI's `SeededRandom` — thin wrapper around `KRandom` with
/// game-flavored methods.
pub struct SeededRandom { rng: KRandom }

impl SeededRandom {
    pub fn state(&self)->crate::rng::RandomState {self.rng.state()}
    pub fn from_state(state:&crate::rng::RandomState)->Result<Self,String> {Ok(Self {rng:KRandom::from_state(state)?})}
    pub fn new(seed: i32) -> Self {
        // ONI coerces int.MinValue to 0; we're safe because KRandom::new
        // also handles int::MIN, but match the game's explicit check.
        let s = if seed == i32::MIN { 0 } else { seed };
        Self { rng: KRandom::new(s) }
    }
    pub fn random_value(&mut self) -> f32 { self.rng.next_double() as f32 }
    pub fn next_double(&mut self) -> f64 { self.rng.next_double() }
    pub fn random_range_f(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (self.rng.next_double() * (hi - lo) as f64) as f32
    }
    pub fn random_range_i(&mut self, lo: i32, hi: i32) -> i32 {
        let range = hi - lo;
        lo + (self.rng.next_double() * range as f64) as i32
    }
}

/// `PointGenerator.SampleBehaviour`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleBehaviour {
    UniformSquare,
    UniformHex,
    UniformScaledHex,
    UniformSpiral,
    UniformCircle,
    PoissonDisk,
    StdRand,
}

impl SampleBehaviour {
    pub fn from_str(s: &str) -> Self {
        match s {
            "PoissonDisk" => SampleBehaviour::PoissonDisk,
            "UniformSquare" => SampleBehaviour::UniformSquare,
            "UniformScaledHex" => SampleBehaviour::UniformScaledHex,
            "UniformSpiral" => SampleBehaviour::UniformSpiral,
            "UniformCircle" => SampleBehaviour::UniformCircle,
            "StdRand" => SampleBehaviour::StdRand,
            _ => SampleBehaviour::UniformHex, // game default
        }
    }
}

/// Port of `PointGenerator.GetRandomPoints(boundingRect, density,
/// avoidRadius, avoidPoints, behaviour, testInsideBounds, rnd,
/// doShuffle)`. We only implement the behaviours ONI's worldgen
/// reaches (`UniformHex`, `PoissonDisk`); others panic.
#[allow(clippy::too_many_arguments)]
pub fn get_random_points(
    rnd: &mut SeededRandom,
    bounds_min: [f32; 2],
    bounds_max: [f32; 2],
    density: f32,
    avoid_radius: f32,
    avoid_points: &[[f32; 2]],
    behaviour: SampleBehaviour,
    test_inside_bounds: bool,
    do_shuffle: bool,
) -> Vec<[f32; 2]> {
    let centre = [(bounds_min[0] + bounds_max[0]) * 0.5, (bounds_min[1] + bounds_max[1]) * 0.5];
    generate_points(rnd, bounds_min, bounds_max, centre, density, avoid_radius,
        avoid_points, behaviour, do_shuffle, |p| !test_inside_bounds ||
            (p[0] >= bounds_min[0] && p[0] <= bounds_max[0] && p[1] >= bounds_min[1] && p[1] <= bounds_max[1]))
}

/// Subworld sampling uses the polygon centroid and rejects points outside its
/// actual outline, rather than accepting every point in its bounding rectangle.
#[allow(clippy::too_many_arguments)]
pub fn get_random_points_in_polygon(
    rnd: &mut SeededRandom, polygon: &mut Polygon, density: f32,
    avoid_radius: f32, avoid_points: &[[f32; 2]], behaviour: SampleBehaviour,
    test_inside_bounds: bool, do_shuffle: bool, test_avoid_points: bool,
) -> Vec<[f32; 2]> {
    let b = polygon.bounds;
    let c = polygon.centroid();
    generate_points(rnd, [b.x, b.y], [b.x + b.width, b.y + b.height], [c.x, c.y],
        density, avoid_radius, if test_avoid_points { avoid_points } else { &[] },
        behaviour, do_shuffle, |p| !test_inside_bounds || polygon.contains(V2::new(p[0], p[1])))
}

#[allow(clippy::too_many_arguments)]
fn generate_points(
    rnd: &mut SeededRandom, bounds_min: [f32; 2], bounds_max: [f32; 2], centre: [f32; 2],
    density: f32, avoid_radius: f32, avoid_points: &[[f32; 2]], behaviour: SampleBehaviour,
    do_shuffle: bool, inside: impl Fn([f32; 2]) -> bool,
) -> Vec<[f32; 2]> {
    let width = bounds_max[0] - bounds_min[0];
    let height = bounds_max[1] - bounds_min[1];
    let half_w = width * 0.5;
    let half_h = height * 0.5;

    let mut raw: Vec<[f32; 2]> = Vec::new();
    match behaviour {
        SampleBehaviour::PoissonDisk => {
            raw = sample(rnd, bounds_min, bounds_max, None, density, 20);
        }
        SampleBehaviour::UniformHex => {
            // Matches C# line 45-52. num5 = floor(W*H / density),
            // num6 = floor(sqrt(num5)). Two nested loops of `num6`
            // produce the hex grid.
            let num5 = ((width * height) / density).floor() as i64;
            let num6 = (num5 as f32).sqrt() as u32;
            for num10 in 0..num6 {
                for num11 in 0..num6 {
                    // Mono evaluates these float expressions in double precision and
                    // rounds once when constructing the Vector2 (V-FRST-C seed 1:
                    // 102.916664, where f32 steps give 102.91667).
                    let dx = (-(half_w as f64)
                        + (0.5 + num10 as f64) / (num6 as f64) * width as f64) as f32;
                    let dy = (-(half_h as f64)
                        + (0.25
                            + 0.5 * ((num10 % 2) as f64)
                            + num11 as f64)
                            / (num6 as f64)
                            * height as f64) as f32;
                    raw.push([centre[0] + dx, centre[1] + dy]);
                }
            }
        }
        SampleBehaviour::UniformSquare => {
            let mut y = -half_h + density;
            while y < half_h - density {
                let mut x = -half_w + density;
                while x < half_w - density {
                    raw.push([centre[0] + x, centre[1] + y]);
                    x += density;
                }
                y += density;
            }
        }
        _ => panic!("unsupported sample behaviour {:?}", behaviour),
    }

    let mut filtered = Vec::with_capacity(raw.len());
    for p in raw {
        if !inside(p) {
            continue;
        }
        if !avoid_points.iter().any(|ap| avoid_distance(*ap, p) < avoid_radius) {
            filtered.push(p);
        }
    }
    if do_shuffle {
        shuffle_seeded(&mut filtered, rnd);
    }
    filtered
}

/// `(avoidPoint - point).magnitude`: float differences, then Mono sums the
/// squares and takes the root in double, rounding once to float. All-f32
/// arithmetic flips the avoid-radius test when the distance lies within an
/// ulp of the radius (CER-C-332-0-0-0: f32 gives 72.0 and keeps the point;
/// the game gets 71.99999 and rejects it).
fn avoid_distance(avoid_point: [f32; 2], point: [f32; 2]) -> f32 {
    let dx = avoid_point[0] - point[0];
    let dy = avoid_point[1] - point[1];
    (dx as f64 * dx as f64 + dy as f64 * dy as f64).sqrt() as f32
}

/// Fisher-Yates on a SeededRandom source, matching C# `ShuffleSeeded`.
pub fn shuffle_seeded<T>(v: &mut [T], rnd: &mut SeededRandom) {
    for i in (1..v.len()).rev() {
        let j = rnd.random_range_i(0, i as i32 + 1) as usize;
        v.swap(i, j.min(i));
    }
}

/// Sample a rectangle using Bridson's Poisson-disk. Matches
/// `UniformPoissonDiskSampler.SampleRectangle`.
pub fn poisson_disk_rectangle(
    rnd: &mut SeededRandom,
    top_left: [f32; 2],
    lower_right: [f32; 2],
    minimum_distance: f32,
    points_per_iteration: usize,
) -> Vec<[f32; 2]> {
    sample(rnd, top_left, lower_right, None, minimum_distance, points_per_iteration)
}

/// Sample a circle. Matches `SampleCircle`.
pub fn poisson_disk_circle(
    rnd: &mut SeededRandom,
    center: [f32; 2],
    radius: f32,
    minimum_distance: f32,
    points_per_iteration: usize,
) -> Vec<[f32; 2]> {
    let tl = [center[0] - radius, center[1] - radius];
    let br = [center[0] + radius, center[1] + radius];
    sample(rnd, tl, br, Some(radius), minimum_distance, points_per_iteration)
}

fn sample(
    rnd: &mut SeededRandom,
    top_left: [f32; 2],
    lower_right: [f32; 2],
    rejection_distance: Option<f32>,
    minimum_distance: f32,
    points_per_iteration: usize,
) -> Vec<[f32; 2]> {
    let dims = [lower_right[0] - top_left[0], lower_right[1] - top_left[1]];
    let center = [(top_left[0] + lower_right[0]) * 0.5, (top_left[1] + lower_right[1]) * 0.5];
    let sqrt2: f32 = 2f32.sqrt();
    let cell_size = minimum_distance / sqrt2;
    let grid_w = (dims[0] / cell_size) as usize + 1;
    let grid_h = (dims[1] / cell_size) as usize + 1;
    let rejection_sq: Option<f32> = rejection_distance.map(|d| d * d);

    let mut grid: Vec<Option<[f32; 2]>> = vec![None; grid_w * grid_h];
    let mut active: Vec<[f32; 2]> = Vec::new();
    let mut points: Vec<[f32; 2]> = Vec::new();

    // Add first point.
    loop {
        let num = dims[0].min(dims[1]) / 2.0;
        let v = [rnd.random_value(), rnd.random_value()];
        let p = [center[0] + v[0] * num, center[1] + v[1] * num];
        if rejection_sq.map(|r| (center[0] - p[0]).powi(2) + (center[1] - p[1]).powi(2) <= r).unwrap_or(true) {
            let d = denormalize(p, top_left, cell_size);
            let idx = d.1 * grid_w + d.0;
            if idx < grid.len() {
                grid[idx] = Some(p);
                active.push(p);
                points.push(p);
                break;
            }
        }
    }

    while !active.is_empty() {
        let i = rnd.random_range_i(0, active.len() as i32 - 1) as usize;
        let point = active[i.min(active.len() - 1)];
        let mut any = false;
        for _ in 0..points_per_iteration {
            let v = generate_random_around(rnd, point, minimum_distance);
            if v[0] < top_left[0] || v[0] >= lower_right[0]
                || v[1] <= top_left[1] || v[1] >= lower_right[1] {
                continue;
            }
            if let Some(rsq) = rejection_sq {
                let dx = center[0] - v[0]; let dy = center[1] - v[1];
                if dx*dx + dy*dy > rsq { continue; }
            }
            let d = denormalize(v, top_left, cell_size);
            let mut clash = false;
            let x0 = (d.0 as i32 - 2).max(0);
            let y0 = (d.1 as i32 - 2).max(0);
            let x1 = ((d.0 as i32) + 3).min(grid_w as i32);
            let y1 = ((d.1 as i32) + 3).min(grid_h as i32);
            for xi in x0..x1 {
                if clash { break; }
                for yj in y0..y1 {
                    let idx = yj as usize * grid_w + xi as usize;
                    if let Some(q) = grid[idx] {
                        // Vector2.Distance: float differences, then Mono sums
                        // the squares and takes the root in double, rounding
                        // once. All-f32 arithmetic flips the test when the
                        // distance lies within an ulp of minimum_distance.
                        let dx = q[0] - v[0]; let dy = q[1] - v[1];
                        if ((dx as f64 * dx as f64 + dy as f64 * dy as f64).sqrt() as f32) < minimum_distance {
                            clash = true; break;
                        }
                    }
                }
            }
            if !clash {
                active.push(v);
                points.push(v);
                let idx = d.1 * grid_w + d.0;
                if idx < grid.len() { grid[idx] = Some(v); }
                any = true;
            }
        }
        if !any {
            active.remove(i.min(active.len() - 1));
        }
    }
    points
}

fn generate_random_around(rnd: &mut SeededRandom, center: [f32; 2], minimum_distance: f32) -> [f32; 2] {
    let r1 = rnd.random_value();
    let r2 = rnd.random_value();
    point_around(center, minimum_distance, r1, r2)
}

fn point_around(center: [f32; 2], minimum_distance: f32, r1: f32, r2: f32) -> [f32; 2] {
    // ONI's Mono runtime retains the radius local at double precision. Live
    // GenerateRandomAround traces confirm that neither the multiply nor add
    // rounds here; the displacement rounds before adding the Vector2 centre.
    let radius = minimum_distance as f64 + minimum_distance as f64 * r1 as f64;
    let theta = std::f32::consts::PI * 2.0 * r2;
    // Unity Mathf evaluates System.Math's double trig, then casts to float.
    // Native sinf/cosf can differ by an ULP and accumulate across sampled sites.
    [center[0] + (radius * (theta as f64).sin() as f32 as f64) as f32,
     center[1] + (radius * (theta as f64).cos() as f32 as f64) as f32]
}

fn denormalize(p: [f32; 2], origin: [f32; 2], cell_size: f32) -> (usize, usize) {
    // Subtraction is float; only the division promotes to double.
    let x = ((p[0] - origin[0]) as f64 / cell_size as f64) as i32;
    let y = ((p[1] - origin[1]) as f64 / cell_size as f64) as i32;
    (x.max(0) as usize, y.max(0) as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sampler_matches_live_mono_arithmetic() {
        // From GenerateRandomAround in game build 744825, seed 42's
        // WarpOilySwamp and TundraMoonlet. Rounding radius to f32 fails both.
        let cases = [
            ([124.77760314941406, 81.06996154785156], 18.0,
             0.6205217838287354, 0.516291618347168,
             [121.79693603515625, 52.05325698852539]),
            ([32.67721939086914, 71.11943054199219], 15.0,
             0.8953485488891602, 0.7388114929199219,
             [4.31721305847168, 69.1224594116211]),
        ];
        for (center, distance, r1, r2, expected) in cases {
            assert_eq!(point_around(center, distance, r1, r2), expected);
        }
    }

    #[test]
    fn avoid_radius_test_matches_mono_near_tie() {
        // CER-C-332-0-0-0 CeresSpacedOutAsteroid, build 744825: the start site
        // and the fourth GenerateRandomAround result of the game's sampler
        // trace. The game's initial sites omit this point (61 sites).
        let start = [80.0, f32::from_bits(0x42fc147b)];
        let point = [f32::from_bits(0x431140d3), f32::from_bits(0x431c7874)];
        assert_eq!(avoid_distance(start, point).to_bits(), 0x428fffff); // 71.99999
        let (dx, dy) = (start[0] - point[0], start[1] - point[1]);
        assert_eq!((dx * dx + dy * dy).sqrt(), 72.0);
        // A unit hex grid of density 1 yields the single point centre + (0, -0.25).
        let filter = |avoid: &[[f32; 2]]| generate_points(&mut SeededRandom::new(1), [0.0, 0.0], [1.0, 1.0],
            [point[0], point[1] + 0.25], 1.0, 72.0, avoid, SampleBehaviour::UniformHex, false, |_| true);
        assert_eq!(filter(&[]), vec![point]);
        assert!(filter(&[start]).is_empty());
    }

    #[test]
    fn deterministic_same_seed() {
        let mut a = SeededRandom::new(42);
        let mut b = SeededRandom::new(42);
        let pa = poisson_disk_rectangle(&mut a, [0.0, 0.0], [100.0, 100.0], 5.0, 30);
        let pb = poisson_disk_rectangle(&mut b, [0.0, 0.0], [100.0, 100.0], 5.0, 30);
        assert_eq!(pa.len(), pb.len());
        for (p, q) in pa.iter().zip(pb.iter()) {
            assert_eq!(p, q);
        }
    }

    #[test]
    fn different_seed_different_output() {
        let mut a = SeededRandom::new(42);
        let mut b = SeededRandom::new(43);
        let pa = poisson_disk_rectangle(&mut a, [0.0, 0.0], [100.0, 100.0], 5.0, 30);
        let pb = poisson_disk_rectangle(&mut b, [0.0, 0.0], [100.0, 100.0], 5.0, 30);
        assert_ne!(pa.first(), pb.first());
    }

    #[test]
    fn reasonable_point_count() {
        let mut rnd = SeededRandom::new(1_000_172_956);
        let pts = poisson_disk_rectangle(&mut rnd, [0.0, 0.0], [256.0, 384.0], 10.0, 30);
        assert!(pts.len() > 100, "expected many points, got {}", pts.len());
        assert!(pts.len() < 5000, "expected reasonable count, got {}", pts.len());
    }
}
