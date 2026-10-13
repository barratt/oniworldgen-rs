//! Radiation (UpdateData L649-737): cosmic-ray occlusion from the top down,
//! lingering decay, emission by radioactive elements and germs, and the
//! cosmic contribution. Runs only when radiation is enabled. Sim thread: FTZ.
use crate::fp::{cvttss2si, fmax, fmin, F};
use crate::phases::Region;
use crate::sim::SimData;
use crate::tables::{element_hash, Disease, Element};

/// Constants the native SimData constructor sets (no message changes them
/// during a settle).
pub const LINGER_RATE: f32 = 1.1;
pub const MAX_MASS: f32 = 2000.0;
pub const BASE_WEIGHT: f32 = 0.3;
pub const DENSITY_WEIGHT: f32 = 0.7;
pub const CONSTRUCTED_FACTOR: f32 = 0.8;
const MIN_RADIATION: f32 = 0.0;
const MAX_RADIATION: f32 = 9_000_000.0;

/// Per-element data used here (native ElementRadiationData).
#[derive(Clone, Copy, Debug, Default)]
pub struct RadiationData {
    pub absorption_factor: f32,
    pub per_1000_mass: f32,
}

pub fn build_table(elements: &[Element]) -> Vec<RadiationData> {
    elements.iter().map(|e| RadiationData { absorption_factor: e.radiation_absorption_factor, per_1000_mass: e.radiation_per_1000_mass }).collect()
}

/// Emission stencil: rows dy = -2..=2, columns dx = -2..=2, weighted by the
/// cell's ring around the emitter.
fn stencil() -> [(f32, f32, f32); 25] {
    let weight = |dx: i32, dy: i32| -> f32 {
        let (a, b) = (dx.abs().max(dy.abs()), dx.abs().min(dy.abs()));
        match (a, b) {
            (0, 0) => 1.0,
            (1, 0) => 0.75,
            (1, 1) => 0.5,
            (2, 0) => 0.25,
            (2, 1) => 0.15,
            _ => 0.1,
        }
    };
    let mut out = [(0.0, 0.0, 0.0); 25];
    for dy in -2..=2 {
        for dx in -2..=2 {
            out[((dy + 2) * 5 + dx + 2) as usize] = (dx as f32, dy as f32, weight(dx, dy));
        }
    }
    out
}

/// RadiationAbsorption for `updated[i]`, clamped to [0, 1].
fn absorption(d: &SimData, rad: &[RadiationData], i: usize) -> F {
    let u = &d.updated;
    let a = F(rad[u.element[i] as usize].absorption_factor);
    let v = if (u.properties[i] as i8) < 0 {
        a * CONSTRUCTED_FACTOR
    } else {
        ((F(u.mass[i]) / MAX_MASS) * a) * DENSITY_WEIGHT + a * BASE_WEIGHT
    };
    fmax(fmin(v, F(1.0)), F(0.0))
}

pub fn phase(d: &mut SimData, rad: &[RadiationData], dis: &[Disease], cosmic: f32, r: Region) {
    let sick = element_hash("RadiationSickness");
    let sick_idx = dis.iter().position(|x| x.hash == sick).map(|i| i as u8).unwrap_or(0xff);
    let w = d.width;
    let n = (d.width * d.height) as usize;
    if d.radiation_occlusion.len() < n {
        d.radiation_occlusion.resize(n, 1.0);
    }
    let span = r.x1 - r.x0;
    // Pass 1: rows from the region's top (inclusive) down to its bottom.
    let mut y = r.y1;
    while y >= r.y0 {
        let start = (y * w + r.x0) as usize;
        for cell in start..start + span as usize {
            let (cy, cx) = (cell as i32 / w, cell as i32 % w);
            let above = if cy + 1 < r.y1 { F(d.radiation_occlusion[((cy + 1) * w + cx) as usize]) } else { F(1.0) };
            let mut v = (F(1.0) - absorption(d, rad, cell)) * above;
            if !(v >= 0.01) {
                v = F(0.0);
            }
            d.radiation_occlusion[cell] = if v > 0.0 { v.v() } else { 0.0 };
            let cur = F(d.updated.radiation[cell]);
            let q = cur / LINGER_RATE;
            let next = if q == 0.0 || q.v().is_nan() { fmax(cur - 1.0, F(0.0)) } else { cur - q };
            d.updated.radiation[cell] = next.v();
        }
        y -= 1;
    }
    // Pass 2: emission and cosmic radiation, bottom-up.
    let kernel = stencil();
    let cosmic_rate = F(cosmic) / LINGER_RATE;
    for y in r.y0..r.y1 {
        let start = (y * w + r.x0) as usize;
        for cell in start..start + span as usize {
            let e = d.updated.element[cell] as usize;
            let per1000 = rad[e].per_1000_mass;
            if !(0.0 >= per1000) {
                let base = F(d.updated.mass[cell]) * 0.001;
                let (cx, cy) = (cell as i32 % w, cell as i32 / w);
                for &(dx, dy, wt) in &kernel {
                    let tx = cvttss2si(dx) + cx;
                    let ty = cvttss2si(dy) + cy;
                    if tx < r.x0 || ty < r.y0 || tx > r.x1 || ty > r.y1 {
                        continue;
                    }
                    let t = (ty * w + tx) as usize;
                    d.updated.radiation[t] = ((base * per1000) * wt + d.updated.radiation[t]).v();
                }
            }
            if d.updated.disease[cell] == sick_idx {
                d.updated.radiation[cell] = (F::from_i32(d.updated.disease_count[cell]) * 0.001 + d.updated.radiation[cell]).v();
            }
            let occ = F(d.radiation_occlusion[cell]);
            if occ > 0.0 {
                d.updated.radiation[cell] = (cosmic_rate * occ + d.updated.radiation[cell]).v();
            }
            let v = d.updated.radiation[cell];
            d.updated.radiation[cell] = if v >= 0.0 { if v > MAX_RADIATION { MAX_RADIATION } else { v } } else { MIN_RADIATION };
            if 0.01 >= d.updated.radiation[cell] {
                d.updated.radiation[cell] = 0.0;
            }
        }
    }
}
