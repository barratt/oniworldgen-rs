//! Gas pressure flow between neighbouring gas/vacuum cells (UpdateData
//! L335-460 and UpdatePressure). Sim thread: FTZ arithmetic.
use crate::events::Events;
use crate::fp::{cvttss2si, fmax, fmin, F};
use crate::germs;
use crate::phases::Region;
use crate::sim::SimData;
use crate::tables::{Disease, Element};

/// Per-element data used here (native ElementPressureData).
#[derive(Clone, Copy, Debug, Default)]
pub struct PressureData {
    pub state: u8,
    pub flow: f32,
}

pub fn build_table(elements: &[Element]) -> Vec<PressureData> {
    elements.iter().map(|e| PressureData { state: e.state, flow: e.flow }).collect()
}

struct CellInfo {
    cell: usize,
    impermeable: bool,
    element: u16,
    state: u8,
    flow: f32,
}

fn info(d: &SimData, p: &[PressureData], cell: usize) -> CellInfo {
    let e = d.cells.element[cell];
    let pd = p[e as usize];
    CellInfo { cell, impermeable: d.cells.properties[cell] & 1 != 0, element: e, state: pd.state & 3, flow: pd.flow }
}

fn valid(d: &SimData, a: &CellInfo) -> bool {
    !a.impermeable && d.updated.element[a.cell] == a.element && a.state <= 1
}

/// The pair lambda (L399-409): flow between `a` and `b` if both are valid
/// gas/vacuum cells holding the same element or different states.
fn try_pair(d: &mut SimData, p: &[PressureData], dis: &[Disease], ev: &mut Events, a: &CellInfo, b: usize) -> F {
    if !valid(d, a) {
        return F(0.0);
    }
    let eb = d.cells.element[b];
    let sb = p[eb as usize].state & 3;
    if d.cells.properties[b] & 1 != 0 || d.updated.element[b] != eb || sb > 1 {
        return F(0.0);
    }
    if a.element != eb && a.state == sb {
        return F(0.0);
    }
    update_pressure(d, p, dis, ev, a.cell, a.element, a.state, a.flow, b)
}

/// UpdatePressure: move mass from the denser to the lighter cell. Reads
/// masses and germ counts from `cells`, writes `updated`. Returns the
/// signed, clamped amount (positive: a to b).
#[allow(clippy::too_many_arguments)]
pub fn update_pressure(d: &mut SimData, p: &[PressureData], dis: &[Disease], ev: &mut Events, a: usize, ea: u16, sa: u8, flow_a: f32, b: usize) -> F {
    let eb = d.cells.element[b];
    let sb = p[eb as usize].state & 3;
    let ma = F(d.cells.mass[a]);
    let mb = F(d.cells.mass[b]);
    let diff = ma - mb;
    let flow = if sa == 1 { flow_a } else { p[eb as usize].flow };
    let mut amount = F(flow) * diff;
    amount = fmax(fmin(amount, ma * 0.125), mb * -0.125);
    let (src, dst, src_elem, dst_elem, dst_state, src_mass) =
        if amount >= 0.0 { (a, b, ea, eb, sb, ma) } else { (b, a, eb, ea, sa, mb) };
    let mut moved = amount.abs();
    if !(src_mass >= moved) {
        moved = src_mass;
    }
    let frac = fmax(fmin(moved / src_mass, F(1.0)), F(0.0));
    let germs_moved = cvttss2si((frac * F::from_i32(d.cells.disease_count[src])).v());
    let u = &mut d.updated;
    let disease_idx = u.disease[src];
    let src_temp = F(u.temperature[src]);
    // FastAddMassAndUpdateTemperature(dst, moved, src_temp, germs)
    let dst_mass = F(u.mass[dst]);
    let new_mass = moved + dst_mass;
    let t = if new_mass > 0.0 {
        let dst_temp = F(u.temperature[dst]);
        let combined = (dst_temp * dst_mass + src_temp * moved) / new_mass;
        combined_clamp(combined, dst_temp, src_temp)
    } else {
        F(0.0)
    };
    u.mass[dst] = new_mass.v();
    u.temperature[dst] = t.v();
    germs::add(dis, u, dst, disease_idx, germs_moved);
    let left = F(u.mass[src]) - moved;
    u.mass[src] = if left > 0.0 { left.v() } else { 0.0 };
    u.disease_count[src] = u.disease_count[src].wrapping_sub(germs_moved);
    if u.disease_count[src] <= 0 {
        germs::clear(u, src);
    }
    if dst_state == 0 {
        if dst_elem != d.void_element {
            d.updated.element[dst] = src_elem;
            ev.change_substance(d, dst);
        } else {
            d.updated.mass[dst] = 0.0;
            d.updated.temperature[dst] = 0.0;
            germs::clear(&mut d.updated, dst);
        }
    }
    amount
}

/// FastCalculateCombinedTemperature's clamp of `v` between the two input
/// temperatures (std::clamp with the larger as upper bound).
pub fn combined_clamp(v: F, dst_t: F, src_t: F) -> F {
    let (lo, hi) = if dst_t > src_t { (src_t, dst_t) } else { (dst_t, src_t) };
    if hi < v {
        hi
    } else if v >= lo {
        v
    } else {
        lo
    }
}

/// The gas flow phase over one active region.
pub fn flow_phase(d: &mut SimData, p: &[PressureData], dis: &[Disease], ev: &mut Events, r: Region) {
    let w = d.width;
    for y in r.y0..r.y1 {
        let row = w * y;
        let dir = d.iterate_direction;
        let (mut cur, end) = if dir >= 0 { (row + r.x0, row + r.x1) } else { (row + r.x1, row + r.x0) };
        while cur != end {
            let a = info(d, p, cur as usize);
            if valid(d, &a) {
                let b = (cur + dir) as usize;
                let amount = try_pair(d, p, dis, ev, &a, b);
                let fd = F::from_i32(dir) * amount;
                let fl = &mut d.flow;
                fl[a.cell][1] = (fd + fl[a.cell][1]).v();
                fl[b][0] = (F(fl[b][0]) - fd).v();
                let b2 = (cur + w) as usize;
                let amount2 = try_pair(d, p, dis, ev, &a, b2);
                let fl = &mut d.flow;
                fl[a.cell][2] = (amount2 + fl[a.cell][2]).v();
                fl[b2][3] = (F(fl[b2][3]) - amount2).v();
                let s1 = p[d.cells.element[b2] as usize].state;
                let s2 = p[d.cells.element[(cur + dir) as usize] as usize].state;
                if (s1 | s2) & 3 != 3 {
                    let b3 = (w + cur + dir) as usize;
                    try_pair(d, p, dis, ev, &a, b3);
                }
            }
            cur += dir;
        }
    }
}

/// DoGasPressureDisplacement: a heavier gas at `cell` pushes a different,
/// lighter gas at `n1` on into `n2` and flows into the vacated cell.
fn do_gas_pressure_displacement(d: &mut SimData, p: &[PressureData], dis: &[Disease], ev: &mut Events, e: u16, cell: usize, n1: usize, n2: usize) -> F {
    let e1 = d.cells.element[n1];
    if p[e1 as usize].state & 3 != 1 {
        return F(0.0);
    }
    let mc = F(d.cells.mass[cell]);
    if !(mc > d.cells.mass[n1]) {
        return F(0.0);
    }
    if d.updated.element[cell] != e || d.updated.element[n1] != e1 {
        return F(0.0);
    }
    if 0.0 >= d.updated.mass[n1] {
        return F(0.0);
    }
    let e2 = d.updated.element[n2];
    if (e2 != e1 && e2 != d.vacuum_element) || d.updated.properties[n2] & 1 != 0 {
        return F(0.0);
    }
    crate::simutil::do_displacement(d, dis, ev, n1, n2);
    let amt = fmin(F(d.updated.mass[cell]), mc * 0.125);
    d.updated.mass[n1] = (amt + d.updated.mass[n1]).v();
    d.updated.temperature[n1] = d.cells.temperature[cell];
    d.updated.element[n1] = e;
    ev.change_substance(d, n1);
    let left = F(d.updated.mass[cell]) - amt;
    d.updated.mass[cell] = if left > 0.0 { left.v() } else { 0.0 };
    let g = cvttss2si((F::from_i32(d.cells.disease_count[cell]) * 0.125).v());
    let di = d.cells.disease[cell];
    germs::add(dis, &mut d.updated, n1, di, g);
    germs::modify(&mut d.updated, cell, g.wrapping_neg());
    amt
}

/// The gas pressure displacement phase (L462-515). The `cells <- updated`
/// copy that follows it belongs to the next phase boundary.
pub fn displacement_phase(d: &mut SimData, p: &[PressureData], dis: &[Disease], ev: &mut Events, r: Region) {
    let w = d.width;
    let x0 = r.x0.max(3);
    let x1 = r.x1.min(w - 3);
    // The native code takes the minimum here as well.
    let y0 = r.y0.min(3);
    let y1 = r.y1.min(d.height - 3);
    let count = (r.x1 - r.x0).min(x1 - x0);
    for y in y0..y1 {
        let start = y * w + x0;
        for cell in start..start + count {
            let c = cell as usize;
            let e = d.cells.element[c];
            if p[e as usize].state & 3 != 1 {
                continue;
            }
            let dir = d.iterate_direction;
            for (n1, n2) in [(cell - w, cell - 2 * w), (cell - dir, cell - 2 * dir), (cell + dir, cell + 2 * dir), (cell + w, cell + 2 * w)] {
                let (n1, n2) = (n1 as usize, n2 as usize);
                let en = d.cells.element[n1];
                if en == e || d.cells.properties[n1] & 1 != 0 || p[en as usize].state & 3 != 1 {
                    continue;
                }
                let amt = do_gas_pressure_displacement(d, p, dis, ev, e, c, n1, n2);
                d.flow[c][3] = (F(d.flow[c][3]) - amt).v();
                d.flow[n1][2] = (amt + d.flow[n1][2]).v();
            }
        }
    }
}
