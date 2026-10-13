//! Liquid flow (UpdateLiquid: falling, spreading, overflow, diagonal swaps)
//! and liquid pressure displacement (UpdateData L528-616). Sim thread: FTZ.
use crate::events::Events;
use crate::fp::{cvttss2si, fmax, fmin, F};
use crate::gas::PressureData;
use crate::germs;
use crate::phases::Region;
use crate::sim::SimData;
use crate::simutil::{add_mass_and_update_temperature, do_displacement};
use crate::state::Cells;
use crate::tables::{Disease, Element};

/// Per-element data used here (native ElementLiquidData).
#[derive(Clone, Copy, Debug, Default)]
pub struct LiquidData {
    pub state: u8,
    pub flow: f32,
    pub viscosity: f32,
    pub min_horizontal_flow: f32,
    pub min_vertical_flow: f32,
    pub max_mass: f32,
}

pub fn build_table(elements: &[Element]) -> Vec<LiquidData> {
    elements
        .iter()
        .map(|e| LiquidData {
            state: e.state,
            flow: e.flow,
            viscosity: e.viscosity,
            min_horizontal_flow: e.min_horizontal_flow,
            min_vertical_flow: e.min_vertical_flow,
            max_mass: e.max_mass,
        })
        .collect()
}

pub struct Ctx<'a> {
    pub l: &'a [LiquidData],
    pub p: &'a [PressureData],
    pub dis: &'a [Disease],
}

impl Ctx<'_> {
    fn st(&self, e: u16) -> u8 {
        self.l[e as usize].state & 3
    }
}

/// IsLiquidPermeable: not liquid-impermeable and vacuum or gas.
fn permeable(cx: &Ctx, c: &Cells, i: usize) -> bool {
    c.properties[i] & 2 == 0 && cx.st(c.element[i]) < 2
}

/// IsSolid (simutil.h): solid element or liquid-impermeable cell.
fn is_solid(cx: &Ctx, c: &Cells, i: usize) -> bool {
    cx.st(c.element[i]) == 3 || c.properties[i] & 2 != 0
}

/// `std::min(m, viscosity)` as compiled: keeps viscosity when it is below m.
fn min_visc(visc: f32, m: F) -> F {
    if visc < m.v() { F(visc) } else { m }
}

/// DisplaceGas on `updated`: push the gas at `cell` into a free neighbour.
pub fn displace_gas(d: &mut SimData, cx: &Ctx, ev: &mut Events, cell: usize, elem: u16) -> bool {
    let u = &d.updated;
    if 0.0 >= u.mass[cell] {
        return false;
    }
    if cx.l[u.element[cell] as usize].state & 3 != 1 {
        return false;
    }
    let w = d.width as usize;
    let tc = d.tick_count as usize;
    let around = [cell + w, cell - 1, cell + 1, cell - w];
    for i in 0..4 {
        let n = around[(i + tc) & 3];
        let en = d.updated.element[n];
        if (en == elem || en == d.vacuum_element) && d.updated.properties[n] & 1 == 0 {
            do_displacement(d, cx.dis, ev, cell, n);
            return true;
        }
    }
    let diag = [cell + w - 1, cell + w + 1];
    let side = [cell - 1, cell + 1];
    for i in 0..2 {
        let k = (i + tc) & 1;
        let n = diag[k];
        if d.updated.element[n] == elem && cx.st(d.updated.element[side[k]]) != 3 && d.updated.properties[n] & 1 == 0 {
            do_displacement(d, cx.dis, ev, cell, n);
            return true;
        }
    }
    false
}

/// UpdateNeighbourLiquidMass: move `amt` of element `e` from `cell` into `nb`.
#[allow(clippy::too_many_arguments)]
fn update_neighbour(d: &mut SimData, cx: &Ctx, ev: &mut Events, cell: usize, e: u16, nb: usize, amt: F, disease: u8, germs_moved: i32) -> bool {
    let src_t = F(d.cells.temperature[cell]);
    let en = d.updated.element[nb];
    if e == en {
        add_mass_and_update_temperature(cx.dis, &mut d.updated, nb, amt, src_t, disease, germs_moved);
        return true;
    }
    if en == d.void_element {
        d.updated.mass[nb] = 0.0;
        return true;
    }
    match cx.st(en) {
        2 => return false,
        1 => {
            if !displace_gas(d, cx, ev, nb, en) {
                return false;
            }
        }
        _ => {}
    }
    d.updated.element[nb] = e;
    add_mass_and_update_temperature(cx.dis, &mut d.updated, nb, amt, src_t, disease, germs_moved);
    ev.change_substance(d, nb);
    true
}

fn spawn_falling(d: &SimData) -> bool {
    // SimEvents::SpawnFallingLiquid emits nothing in headless mode.
    assert!(d.headless, "falling-liquid entities are not modelled outside headless mode");
    false
}

/// After a successful transfer out of `cell`: germ bookkeeping and mass.
fn took(d: &mut SimData, cell: usize, amt: F, germs_moved: i32, m: &mut F) {
    if d.updated.disease[cell] == d.cells.disease[cell] {
        germs::modify(&mut d.updated, cell, germs_moved.wrapping_neg());
    }
    *m = *m - amt;
    d.updated.mass[cell] = (F(d.updated.mass[cell]) - amt).v();
}

/// UpdateLiquid for one liquid cell.
pub fn update_liquid(d: &mut SimData, cx: &Ctx, ev: &mut Events, cell: usize) {
    let w = d.width as usize;
    let e = d.cells.element[cell];
    let ld = cx.l[e as usize];
    let mut m = F(d.cells.mass[cell]);
    let mut dc = d.cells.disease_count[cell];
    let below = cell - w;
    let ebc = d.cells.element[below];
    let ebu = d.updated.element[below];
    if cx.st(ebu) != 3 && d.cells.properties[below] & 2 == 0 {
        let stb = cx.st(ebc);
        if e == ebc {
            let bm = if stb == 2 { F(d.cells.mass[below]) } else { F(0.0) };
            let maxm = fmax(m * 1.01f32, F(ld.max_mass));
            let half = fmax(maxm - bm, F(0.0)) * 0.5;
            let amt = fmin(min_visc(ld.viscosity, m), half);
            let blocked = m > ld.min_vertical_flow && !(amt >= ld.min_vertical_flow);
            if !blocked && amt > 0.0 {
                let g = cvttss2si(((amt / m) * F::from_i32(dc)).v());
                let di = d.cells.disease[cell];
                if update_neighbour(d, cx, ev, cell, e, below, amt, di, g) {
                    dc = dc.wrapping_sub(g);
                    took(d, cell, amt, g, &mut m);
                    d.flow[cell][2] = (amt + d.flow[cell][2]).v();
                }
            }
        } else if stb <= 1 {
            if ebc == d.void_element {
                d.updated.element[cell] = d.vacuum_element;
                d.updated.mass[cell] = 0.0;
                d.updated.temperature[cell] = 0.0;
                germs::clear(&mut d.updated, cell);
                ev.change_substance(d, cell);
                return;
            }
            let u = &d.updated;
            let a = permeable(cx, u, cell - 1);
            let fall = (a && !permeable(cx, u, below - 1)) || {
                let c = permeable(cx, u, cell + 1);
                c && !permeable(cx, u, below + 1)
            };
            if fall {
                if spawn_falling(d) {
                    unreachable!();
                }
                return;
            }
            crate::state::swap_cells(&mut d.updated, cell, below);
            ev.change_substance(d, cell);
            ev.change_substance(d, below);
            return;
        }
    }
    // Horizontal spreading: left (flow.x) then right (flow.y).
    for (nb, slot) in [(cell - 1, 0usize), (cell + 1, 1usize)] {
        if !(m > 0.0) {
            return;
        }
        let en = d.cells.element[nb];
        let enu = d.updated.element[nb];
        if (e != en && cx.st(en) > 1) || cx.st(enu) == 3 || d.cells.properties[nb] & 2 != 0 {
            continue;
        }
        let nm = if cx.st(en) == 2 { F(d.cells.mass[nb]) } else { F(0.0) };
        let x = (m - nm) * 0.25;
        let amt = fmin(min_visc(ld.viscosity, m), x);
        if !(amt >= ld.min_horizontal_flow) || !(amt > 0.0) {
            continue;
        }
        let g = cvttss2si(((amt / m) * F::from_i32(dc)).v());
        let mut ok = false;
        if is_solid(cx, &d.cells, cell - w) && permeable(cx, &d.updated, nb - w) {
            ok = spawn_falling(d);
        }
        if !ok {
            let di = d.cells.disease[cell];
            ok = update_neighbour(d, cx, ev, cell, e, nb, amt, di, g);
        }
        if ok {
            dc = dc.wrapping_sub(g);
            took(d, cell, amt, g, &mut m);
            d.flow[cell][slot] = (amt + d.flow[cell][slot]).v();
        }
    }
    if !(m > 0.0) {
        return;
    }
    // Overflow upwards (flow.w).
    let above = cell + w;
    let ea = d.cells.element[above];
    let eau = d.updated.element[above];
    if !((e != ea && cx.st(ea) > 1) || cx.st(eau) == 3 || d.cells.properties[above] & 2 != 0) {
        let am = if cx.st(ea) == 2 { F(d.cells.mass[above]) } else { F(0.0) };
        let maxm = fmax(am * 1.01f32, F(ld.max_mass));
        let x = fmax(m - maxm, F(0.0)) * 0.5;
        let amt = if m >= maxm + maxm { x } else { fmin(min_visc(ld.viscosity, m), x) };
        if amt > 0.01 {
            let g = cvttss2si(((amt / m) * F::from_i32(dc)).v());
            let di = d.cells.disease[cell];
            if update_neighbour(d, cx, ev, cell, e, above, amt, di, g) {
                took(d, cell, amt, g, &mut m);
                d.flow[cell][3] = (amt + d.flow[cell][3]).v();
            }
        }
    }
    if !(m > 0.0) {
        return;
    }
    // Swap with gas diagonally below (right first, then left).
    let bw = cell - w;
    let u = &d.updated;
    let solid = |i: usize| cx.st(u.element[i]) == 3;
    let gas = |i: usize| cx.st(u.element[i]) == 1;
    let target = if gas(bw + 1)
        && (solid(cell + 1) || u.properties[cell + 1] & 1 != 0)
        && !solid(bw)
        && u.properties[cell] & 1 == 0
        && u.properties[bw + 1] & 2 == 0
        && u.properties[bw] & 3 == 0
    {
        bw + 1
    } else {
        if !gas(bw - 1) || !(solid(cell - 1) || u.properties[cell - 1] & 1 != 0) || solid(bw) {
            return;
        }
        if u.properties[cell] & 1 != 0 || u.properties[bw - 1] & 2 != 0 || u.properties[bw] & 3 != 0 {
            return;
        }
        bw - 1
    };
    crate::state::swap_cells(&mut d.updated, cell, target);
    ev.change_substance(d, cell);
    ev.change_substance(d, target);
}

/// DisplaceLiquidDirectional on `updated`.
fn displace_liquid_directional(d: &mut SimData, cx: &Ctx, ev: &mut Events, from: usize, to: usize) -> bool {
    if 0.0 >= d.updated.mass[from] || d.updated.properties[to] & 2 != 0 {
        return false;
    }
    let et = d.updated.element[to];
    match cx.st(et) {
        0 | 1 => {
            if !displace_gas(d, cx, ev, to, et) {
                return false;
            }
            crate::state::swap_cells(&mut d.updated, from, to);
            ev.change_substance(d, from);
            ev.change_substance(d, to);
            true
        }
        2 => {
            if d.updated.element[from] != et {
                return false;
            }
            let u = &mut d.updated;
            let (m, t, di, dc) = (F(u.mass[from]), F(u.temperature[from]), u.disease[from], u.disease_count[from]);
            add_mass_and_update_temperature(cx.dis, u, to, m, t, di, dc);
            d.clear_cell(from);
            ev.change_substance(d, from);
            true
        }
        _ => false,
    }
}

/// DoLiquidPressureDisplacement: a pressured liquid at `cell` pushes a
/// different liquid at `n1` on into `n2` and flows into the vacated cell.
fn do_liquid_pressure_displacement(d: &mut SimData, cx: &Ctx, ev: &mut Events, e: u16, cell: usize, n1: usize, n2: usize) -> F {
    if d.updated.properties[n1] & 2 != 0 {
        return F(0.0);
    }
    let e1 = d.cells.element[n1];
    if e1 == e || d.updated.element[cell] != e || d.updated.element[n1] != e1 {
        return F(0.0);
    }
    if cx.p[e1 as usize].state & 3 != 2 {
        return F(0.0);
    }
    let fl = d.flow[cell];
    let pressure = F(d.cells.mass[cell]) - (((F(fl[1]) + fl[0]) + fl[2]) + fl[3]);
    if !(pressure > (F(d.cells.mass[n2]) + d.cells.mass[n1])) {
        return F(0.0);
    }
    if !displace_liquid_directional(d, cx, ev, n1, n2) {
        return F(0.0);
    }
    let amt = pressure * 0.125;
    d.updated.mass[n1] = (amt + d.updated.mass[n1]).v();
    d.updated.temperature[n1] = d.cells.temperature[cell];
    d.updated.element[n1] = e;
    ev.change_substance(d, n1);
    let left = F(d.updated.mass[cell]) - amt;
    d.updated.mass[cell] = if left > 0.0 { left.v() } else { 0.0 };
    let g = cvttss2si((F::from_i32(d.cells.disease_count[cell]) * 0.125).v());
    let di = d.cells.disease[cell];
    germs::add(cx.dis, &mut d.updated, n1, di, g);
    germs::modify(&mut d.updated, cell, g.wrapping_neg());
    amt
}

/// The liquid phase: `cells <- updated`, UpdateLiquid over the region, then
/// liquid pressure displacement over the inset region.
pub fn phase(d: &mut SimData, cx: &Ctx, ev: &mut Events, r: Region) {
    let (cells, updated) = (&mut d.cells, &d.updated);
    cells.copy_from(updated);
    let w = d.width;
    if r.x1 != r.x0 {
        for y in r.y0..r.y1 {
            let start = y * w + r.x0;
            for cell in start..start + (r.x1 - r.x0) {
                let c = cell as usize;
                if cx.st(d.cells.element[c]) == 2 && !(0.0 >= d.cells.mass[c]) {
                    update_liquid(d, cx, ev, c);
                }
            }
        }
    }
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
            if cx.p[e as usize].state & 3 != 2 {
                continue;
            }
            let dir = d.iterate_direction;
            let n1 = (cell - dir) as usize;
            if d.cells.element[n1] != e {
                let amt = do_liquid_pressure_displacement(d, cx, ev, e, c, n1, (cell - 2 * dir) as usize);
                d.flow[c][0] = (amt + d.flow[c][0]).v();
                d.flow[c - 1][1] = (F(d.flow[c - 1][1]) - amt).v();
            }
            let dir = d.iterate_direction;
            let n1 = (cell + dir) as usize;
            if d.cells.element[n1] != e {
                let amt = do_liquid_pressure_displacement(d, cx, ev, e, c, n1, (cell + 2 * dir) as usize);
                d.flow[c][0] = (F(d.flow[c][0]) - amt).v();
                d.flow[c - 1][1] = (amt + d.flow[c - 1][1]).v();
            }
            let up = c + w as usize;
            if d.cells.element[up] != e {
                let max_mass = cx.l[d.updated.element[c] as usize].max_mass;
                if !(max_mass > d.updated.mass[c]) {
                    let amt = do_liquid_pressure_displacement(d, cx, ev, e, c, up, up + w as usize);
                    d.flow[c][3] = (amt + d.flow[c][3]).v();
                    d.flow[up][2] = (F(d.flow[up][2]) - amt).v();
                }
            }
        }
    }
}
