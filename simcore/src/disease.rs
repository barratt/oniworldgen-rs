//! Germ phases: diffusion between neighbours (Disease::UpdateCells) and
//! growth/death (Disease::PostProcess). Parameters come from the diseases
//! message (see `disease_table` for how the product builds it).
use crate::fp::{cvttss2si, powf, F};
use crate::germs;
use crate::phases::Region;
use crate::sim::SimData;
use crate::tables::{Disease, RangeInfo};

/// Disease::GetDiffusionScale for the germs in `cells[i]`.
fn diffusion_scale(dis: &[Disease], d: &SimData, i: usize) -> F {
    let c = &d.cells;
    let g = &dis[c.disease[i] as usize].growth[c.element[i] as usize];
    if c.disease_count[i] < g.min_diffusion_count || c.infestation_ticks[i] < g.min_diffusion_infestation_tick_count {
        return F(0.0);
    }
    F(g.diffusion_scale)
}

/// Disease::UpdateCells for the pair (a, b): reads `cells`, adds to `updated`.
fn update_cells(dis: &[Disease], state: &dyn Fn(u16) -> u8, d: &mut SimData, a: usize, b: usize) {
    let c = &d.cells;
    if (state(c.element[a]) ^ state(c.element[b])) & 3 != 0 {
        return;
    }
    let (da, db) = (c.disease[a], c.disease[b]);
    // (germ index, amount) added to a and to b.
    let (ia, ta, ib, tb);
    if da == db {
        if da == 0xff {
            return;
        }
        let diff = c.disease_count[b].wrapping_sub(c.disease_count[a]);
        let src = if diff > 0 { b } else { a };
        let t = cvttss2si((diffusion_scale(dis, d, src) * (F::from_i32(diff) * 0.125)).v());
        (ia, ta, ib, tb) = (da, t, db, t.wrapping_neg());
    } else if da == 0xff {
        let t = cvttss2si(((F::from_i32(c.disease_count[b]) * 0.125) * diffusion_scale(dis, d, b)).v());
        (ia, ta, ib, tb) = (db, t, db, t.wrapping_neg());
    } else if db == 0xff {
        let t = cvttss2si(((F::from_i32(c.disease_count[a]) * 0.125) * diffusion_scale(dis, d, a)).v());
        (ia, ta, ib, tb) = (da, t.wrapping_neg(), da, t);
    } else {
        let sa = F::from_i32(c.disease_count[a]) * dis[da as usize].strength;
        let sb = F::from_i32(c.disease_count[b]) * dis[db as usize].strength;
        if sa > sb {
            let t = cvttss2si(((F::from_i32(c.disease_count[a]) * diffusion_scale(dis, d, a)) * 0.125).v());
            (ia, ta, ib, tb) = (da, t.wrapping_neg(), da, t);
        } else {
            let t = cvttss2si(((F::from_i32(c.disease_count[b]) * diffusion_scale(dis, d, b)) * 0.125).v());
            (ia, ta, ib, tb) = (db, t, db, t.wrapping_neg());
        }
    }
    if ta != 0 {
        germs::add(dis, &mut d.updated, a, ia, ta);
    }
    if tb != 0 {
        germs::add(dis, &mut d.updated, b, ib, tb);
    }
}

/// The diffusion phase (UpdateData L616-647): `cells <- updated`, then each
/// cell exchanges germs with its right and upper neighbours.
pub fn diffusion_phase(dis: &[Disease], state: &dyn Fn(u16) -> u8, d: &mut SimData, r: Region) {
    let (cells, updated) = (&mut d.cells, &d.updated);
    cells.copy_from(updated);
    let w = d.width as usize;
    for y in r.y0..r.y1 {
        let start = (y * d.width + r.x0) as usize;
        for cell in start..start + (r.x1 - r.x0) as usize {
            if d.cells.disease[cell] != 0xff || d.cells.disease[cell + 1] != 0xff {
                update_cells(dis, state, d, cell, cell + 1);
            }
            if d.cells.disease[cell] != 0xff || d.cells.disease[cell + w] != 0xff {
                update_cells(dis, state, d, cell, cell + w);
            }
        }
    }
}

/// HalfLifeToGrowthRate with the step's dt (0.2 s) folded in: the factor a
/// population with this half-life changes by per step. Zero (and NaN) half
/// lives kill everything; infinite ones mean no change.
fn growth_rate(half_life: F) -> F {
    if !(half_life != 0.0) {
        return F(0.0);
    }
    if half_life == f32::INFINITY {
        return F(1.0);
    }
    F(powf(2.0, (F(-0.2) / half_life).v()))
}

/// CalculateRangeHalfLife followed by HalfLifeToGrowthRate: the growth rate
/// at `v` for a range (min viable, min growth, max growth, max viable) and
/// the half lives at those points, interpolated linearly between them.
/// Inside the growth band the rate is 1.
fn range_growth_rate(v: F, range: &RangeInfo, half_lives: &RangeInfo) -> F {
    let r = [range.min_viable, range.min_growth, range.max_growth, range.max_viable];
    let h = [half_lives.min_viable, half_lives.min_growth, half_lives.max_growth, half_lives.max_viable];
    let (mut below, mut idx, mut hi) = (3i64, 3, 3usize);
    for (i, &bound) in r.iter().enumerate() {
        if !(v > bound) {
            below = i as i64 - 1;
            idx = i;
            hi = i;
            break;
        }
    }
    let lo = if below >= 0 { below as usize } else { hi };
    if lo == 1 && idx == 2 {
        return F(1.0);
    }
    let (h_lo, h_hi) = (F(h[lo]), F(h[hi]));
    // ucomiss/je: infinite or NaN half lives give rate 1.
    if h_lo.v() == f32::INFINITY || h_lo.v().is_nan() || h_hi.v() == f32::INFINITY || h_hi.v().is_nan() {
        return F(1.0);
    }
    let r_lo = F(r[lo]);
    let span = F(r[hi]) - r_lo;
    let t = if span > 0.0 { (v - r_lo) / span } else { F(0.0) };
    growth_rate((F(1.0) - t) * h_lo + t * h_hi)
}

/// Disease::PostProcess over [x0, x1) x [y0, y1): germ growth and death
/// from temperature, population density and radiation (with the fractional
/// remainder carried in the growth error), then clean-up of empty cells and
/// the infestation tick count of every cell.
fn grow(dis: &[Disease], d: &mut SimData, b: Region) {
    let w = d.width;
    let radiation = d.radiation_enabled;
    let u = &mut d.updated;
    for y in b.y0..b.y1 {
        for x in b.x0..b.x1 {
            let i = (y * w + x) as usize;
            if u.disease[i] == 0xff {
                continue;
            }
            let ds = &dis[u.disease[i] as usize];
            let rate = range_growth_rate(F(u.temperature[i]), &ds.temperature_range, &ds.temperature_half_lives);
            let count = F::from_i32(u.disease_count[i]);
            let mut acc = ((count * rate) + u.growth_error[i]) - count;
            let g = &ds.growth[u.element[i] as usize];
            let mass = F(u.mass[i]);
            let min_count = mass * g.min_count_per_kg;
            let delta = if min_count > count || count > (mass * g.max_count_per_kg) {
                if count >= min_count {
                    (growth_rate(F(g.over_population_half_life)) - 1.0) * count
                } else {
                    F(g.under_population_death_rate) * -0.2
                }
            } else {
                (growth_rate(F(g.population_half_life)) - 1.0) * count
            };
            acc = acc + delta;
            if radiation {
                acc = acc - F(u.radiation[i]) * ds.radiation_kill_rate;
            }
            let whole = cvttss2si(acc.v());
            u.growth_error[i] = (acc - F::from_i32(whole)).v();
            u.disease_count[i] = u.disease_count[i].wrapping_add(whole);
        }
    }
    for y in b.y0..b.y1 {
        for x in b.x0..b.x1 {
            let i = (y * w + x) as usize;
            if u.disease_count[i] <= 0 {
                germs::clear(u, i);
            }
            u.infestation_ticks[i] = (u.infestation_ticks[i] as u32 + 1).min(0xfe) as u8;
        }
    }
}

/// Germ growth and death (Disease::PostProcess, UpdateData L946), over the
/// same cell box as the post-process phase.
pub fn post_process_phase(sim: &mut crate::Sim, r: Region) -> Result<(), String> {
    let d = sim.data.as_mut().ok_or("not initialized")?;
    let b = crate::postprocess::post_process_box(d, r);
    grow(&sim.diseases, d, b);
    Ok(())
}
