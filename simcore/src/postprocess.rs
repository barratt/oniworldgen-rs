//! Per-cell post-processing (UpdateData L873-938, PostProcessCell): unstable
//! falls, density displacement, offgassing and sublimation, partial melting
//! and heat transitions, pressure breaks, evaporation. Sim thread: FTZ.
//!
//! The native ElementPostProcessData table is a verbatim copy of element
//! record fields (CreateElementsTable L230-242), so this module reads the
//! element records directly.
use crate::events::Events;
use crate::fp::{cvttss2si, maxss, minss, F};
use crate::germs;
use crate::liquid::{self, displace_gas};
use crate::phases::Region;
use crate::sim::SimData;
use crate::simutil::add_mass_and_update_temperature;
use crate::state::swap_cells;
use crate::tables::{element_hash, state, Element};
use crate::Sim;

struct Cx<'a> {
    el: &'a [Element],
    lq: liquid::Ctx<'a>,
    void_hash: u32,
}

impl Cx<'_> {
    fn e(&self, i: u16) -> &Element {
        &self.el[i as usize]
    }
    fn st(&self, i: u16) -> u8 {
        self.el[i as usize].state & state::MASK
    }
}

/// SimData::GetRandom01.
pub fn rand01(d: &mut SimData) -> F {
    d.random_seed = d.random_seed.wrapping_mul(0x343fd).wrapping_add(0x269ec3);
    let r = (d.random_seed >> 16) & 0x7fff;
    F(r as f32) * f32::from_bits(0x38000100)
}

/// SimData::GetStableTicksRemaining: a fresh counter (all five bits set)
/// draws 3 to 5 ticks; otherwise the counter counts down to zero.
fn stable_ticks_remaining(d: &mut SimData, i: usize) -> u8 {
    let b = d.stable_ticks[i];
    let t = b & 0x1f;
    if t == 0x1f {
        d.random_seed = d.random_seed.wrapping_mul(0x343fd).wrapping_add(0x269ec3);
        let r = (d.random_seed >> 16) & 0x7fff;
        let v = (cvttss2si((F(r as f32) * f32::from_bits(0x38c00180)).v()) as u8).wrapping_add(3);
        d.stable_ticks[i] = ((v ^ b) & 0x1f) ^ b;
        return v;
    }
    if t != 0 {
        let v = t - 1;
        d.stable_ticks[i] = ((v ^ b) & 0x1f) ^ b;
        return v;
    }
    0
}

/// Evaporate: clear the cell and report the substance change.
fn evaporate(d: &mut SimData, ev: &mut Events, i: usize) {
    d.clear_cell(i);
    ev.change_substance(d, i);
}

fn zero_flow(d: &mut SimData, i: usize) {
    d.flow[i] = [0.0; 4];
}

/// FastCalculateCombinedTemperature(m1, t1, m2, t2): mass-weighted mean
/// clamped to [t1, t2] (std::clamp; callers pass t1 <= t2).
fn fast_combined_temperature(m1: F, t1: F, m2: F, t2: F) -> F {
    let v = (m1 * t1 + m2 * t2) / (m1 + m2);
    if !(t2 >= v) {
        t2
    } else if v >= t1 {
        v
    } else {
        t1
    }
}

/// CalculateCombinedTemperature(m1, t1, m2, t2) as inlined in the offgas
/// lambda: zero for non-positive total mass, else the mean clamped between
/// the two temperatures.
fn combined_temperature(m1: F, t1: F, m2: F, t2: F) -> F {
    let total = m1 + m2;
    if !(total > 0.0) {
        return F(0.0);
    }
    let v = (t2 * m2 + m1 * t1) / total;
    let (lo, hi) = if t1 > t2 { (t2, t1) } else { (t1, t2) };
    if !(hi >= v) {
        hi
    } else if v >= lo {
        v
    } else {
        lo
    }
}

/// CellAccessor::ModifyDiseaseCount on `updated`.
fn modify_germs(d: &mut SimData, i: usize, delta: i32) {
    germs::modify(&mut d.updated, i, delta);
}

/// DoDensityDisplacement: swap with the cell below when it holds a lighter
/// element of the same state (with probability 1 - `prob`), or, for equal
/// elements with a warmer cell below, mix temperatures (liquids) or swap.
fn density_displacement(d: &mut SimData, cx: &Cx, ev: &mut Events, cell: usize, ppd: &Element, prob: f32) -> bool {
    let below = cell - d.width as usize;
    let eb = cx.e(d.updated.element[below]);
    if ppd.state != eb.state {
        return false;
    }
    if !(rand01(d) > prob) {
        return false;
    }
    if ppd.molar_mass > eb.molar_mass {
        swap_cells(&mut d.updated, cell, below);
        ev.change_substance(d, cell);
        ev.change_substance(d, below);
        return true;
    }
    let u = &d.updated;
    if u.element[cell] != u.element[below] || !(u.temperature[below] > u.temperature[cell]) {
        return false;
    }
    if ppd.state & state::MASK == state::LIQUID {
        let t = fast_combined_temperature(F(u.mass[cell]), F(u.temperature[cell]), F(u.mass[below]), F(u.temperature[below]));
        d.updated.temperature[cell] = t.v();
        d.updated.temperature[below] = t.v();
    } else {
        swap_cells(&mut d.updated, cell, below);
    }
    true
}

/// DoPressureBreak: whether the over-pressured liquid at `cell` breaks the
/// one or two breakable solid cells in direction `dir`. Only world-damage
/// events result, which have no cell effect.
fn pressure_break(d: &SimData, cx: &Cx, cell: usize, pressure: F, dir: i32) -> bool {
    let u = &d.updated;
    let mut resistance = F(1.0);
    let mut count = 0;
    let mut c = cell as i32;
    let mut last;
    let mut i = 0;
    loop {
        c += dir;
        last = c as usize;
        let e = cx.e(u.element[last]);
        if e.state & state::MASK != state::SOLID {
            break;
        }
        if e.state & state::UNBREAKABLE != 0 || u.properties[last] & 8 != 0 {
            return false;
        }
        count += 1;
        let s = u.strength[last];
        let hi = F((s >> 7) as i32 as f32);
        let lo = F((s & 0x7f) as i32 as f32);
        let bonus = if hi >= 1.0 { F(0.0) } else { F(f32::from_bits(0x3dccccd0)) };
        let frac = F(u.mass[last]) / e.max_mass;
        let lerp = frac * hi + (F(1.0) - hi);
        resistance = resistance + ((((lerp * e.strength) * lo) * 0.25) + bonus);
        if resistance > pressure {
            return false;
        }
        i += 1;
        if i >= 3 {
            break;
        }
    }
    if !(count == 1 || count == 2) || !(pressure > resistance) {
        return false;
    }
    let mut p = pressure;
    let e = cx.e(u.element[last]);
    if e.state & state::MASK == state::LIQUID && u.mass[last] > 0.0 {
        p = p - F(u.mass[last]) / e.max_mass;
    }
    p > resistance
}

/// DisplaceLiquidSimple: spread the liquid at `cell` evenly over those of
/// `nbrs` that hold `elem` or vacuum, then clear `cell`.
fn displace_liquid_simple(d: &mut SimData, cx: &Cx, ev: &mut Events, cell: usize, elem: u16, nbrs: &[usize; 4]) -> bool {
    if 0.01 > d.updated.mass[cell] {
        return false;
    }
    let mut valid = [0usize; 4];
    let mut n = 0;
    for &nb in nbrs {
        let e = d.updated.element[nb];
        if (e == elem || e == d.vacuum_element) && d.updated.properties[nb] & 2 == 0 {
            valid[n] = nb;
            n += 1;
        }
    }
    if n == 0 {
        return false;
    }
    let per_mass = F(d.updated.mass[cell]) / F::from_i32(n as i32);
    let per_germs = d.updated.disease_count[cell].wrapping_sub(1).wrapping_add(n as i32) / n as i32;
    for &nb in &valid[..n] {
        let (t, di) = (F(d.updated.temperature[cell]), d.updated.disease[cell]);
        add_mass_and_update_temperature(cx.lq.dis, &mut d.updated, nb, per_mass, t, di, per_germs);
        d.updated.element[nb] = elem;
        ev.change_substance(d, nb);
    }
    d.clear_cell(cell);
    ev.change_substance(d, cell);
    true
}

/// DisplaceLiquid on `updated`: move the liquid at `cell` into its
/// neighbours, displacing a neighbouring gas first if needed.
fn displace_liquid(d: &mut SimData, cx: &Cx, ev: &mut Events, cell: usize, elem: u16) -> bool {
    if 0.0 >= d.updated.mass[cell] {
        return false;
    }
    if cx.st(d.updated.element[cell]) != state::LIQUID {
        return false;
    }
    let w = d.width as usize;
    let nbrs = [cell + 1, cell - 1, cell + w, cell - w];
    if displace_liquid_simple(d, cx, ev, cell, elem, &nbrs) {
        return true;
    }
    for i in 0..4 {
        let n = nbrs[(d.tick_count as usize + i) & 3];
        let en = d.updated.element[n];
        if cx.st(en) == state::GAS && displace_gas(d, &cx.lq, ev, n, en) {
            return displace_liquid_simple(d, cx, ev, cell, elem, &nbrs);
        }
    }
    false
}

/// SimEvents::SpawnFallingLiquid: emits nothing and fails in headless mode.
fn spawn_falling_liquid(d: &SimData) -> bool {
    assert!(d.headless, "falling-liquid entities are not modelled outside headless mode");
    false
}

/// DoPartialHeatTransition: the liquid at `cell` boils 5 kg off into a gas
/// using heat from the hotter neighbour `other`.
fn partial_heat_transition(d: &mut SimData, cx: &Cx, ev: &mut Events, cell: usize, other: usize) -> bool {
    if 5.0 > d.updated.mass[cell] {
        return false;
    }
    let ec = cx.e(d.updated.element[cell]);
    let eo = cx.e(d.updated.element[other]);
    let hi = F(ec.high_temp);
    let hot = hi + 3.0;
    let tgt = ec.high_temp_transition_idx;
    if tgt == 0xffff {
        return false;
    }
    let to = F(d.updated.temperature[other]);
    if !(to > hot) {
        return false;
    }
    if !(to >= F(eo.low_temp) + 13.0) {
        return false;
    }
    let tc = d.updated.temperature[cell];
    if tc >= (hi - 3.0).v() {
        return false;
    }
    let heat = ((hot - tc) * ec.specific_heat_capacity) * 5.0;
    let cap = F(d.updated.mass[other]) * eo.specific_heat_capacity;
    if !((cap * 10.0) >= heat) {
        return false;
    }
    let new_to = (to * cap - heat) / cap;
    let g = cvttss2si((F::from_i32(d.updated.disease_count[cell]) * (F(5.0) / d.updated.mass[cell])).v());
    let w = d.width as usize;
    match cx.st(tgt) {
        state::GAS => {
            d.updated.mass[cell] = (F(d.updated.mass[cell]) - 5.0).v();
            modify_germs(d, cell, g.wrapping_neg());
            d.updated.temperature[other] = new_to.v();
            let above = cell + w;
            let ea = d.updated.element[above];
            if cx.st(ea) == state::GAS && displace_gas(d, &cx.lq, ev, above, ea) {
                d.updated.mass[above] = 5.0;
                d.updated.temperature[above] = hot.v();
                d.updated.element[above] = tgt;
                ev.change_substance(d, above);
                return true;
            }
            let ecell = d.updated.element[cell];
            if displace_liquid(d, cx, ev, cell, ecell) {
                d.updated.mass[cell] = 5.0;
                d.updated.temperature[cell] = hot.v();
                d.updated.element[cell] = tgt;
                ev.change_substance(d, cell);
                return true;
            }
            d.updated.mass[cell] = (F(d.updated.mass[cell]) + 5.0).v();
            modify_germs(d, cell, g);
            d.updated.temperature[other] = to.v();
            false
        }
        state::LIQUID => {
            d.updated.mass[cell] = (F(d.updated.mass[cell]) - 5.0).v();
            modify_germs(d, cell, g.wrapping_neg());
            d.updated.temperature[other] = new_to.v();
            let ecell = d.updated.element[cell];
            if displace_liquid(d, cx, ev, cell, ecell) {
                if spawn_falling_liquid(d) {
                    return true;
                }
                d.updated.mass[cell] = 5.0;
                d.updated.temperature[cell] = hot.v();
                d.updated.element[cell] = tgt;
                ev.change_substance(d, cell);
                return true;
            }
            d.updated.mass[cell] = (F(d.updated.mass[cell]) + 5.0).v();
            modify_germs(d, cell, g);
            d.updated.temperature[other] = to.v();
            false
        }
        _ => false,
    }
}

/// DoPartialMelt: the gas or vacuum at `cell` melts 5 kg off the solid
/// neighbour `other` into a liquid that takes the cell's place.
fn partial_melt(d: &mut SimData, cx: &Cx, ev: &mut Events, cell: usize, other: usize) -> bool {
    let eo = cx.e(d.updated.element[other]);
    let ec = cx.e(d.updated.element[cell]);
    if ec.state & state::MASK > state::GAS || eo.state & state::MASK != state::SOLID {
        return false;
    }
    if 5.0 >= d.updated.mass[other] {
        return false;
    }
    if d.updated.properties[other] & 0x48 != 0 {
        return false;
    }
    let hi = F(eo.high_temp);
    let hot = hi + 3.0;
    let tc = F(d.updated.temperature[cell]);
    if !(tc > hot) {
        return false;
    }
    let to = d.updated.temperature[other];
    if to >= (hi - 3.0).v() {
        return false;
    }
    let tgt = eo.high_temp_transition_idx;
    if tgt == 0xffff {
        return false;
    }
    let heat = ((hot - to) * cx.e(tgt).specific_heat_capacity) * 5.0;
    let cap = F(d.updated.mass[cell]) * ec.specific_heat_capacity;
    let avail = (tc - (F(ec.low_temp) + 6.0)) * cap;
    if !(avail >= heat) {
        return false;
    }
    if cx.st(tgt) != state::LIQUID {
        return false;
    }
    let new_tc = (cap * tc - heat) / cap;
    let _germs = cvttss2si(((F(5.0) / d.updated.mass[other]) * F::from_i32(d.updated.disease_count[other])).v());
    if spawn_falling_liquid(d) {
        unreachable!();
    }
    let ecell = d.updated.element[cell];
    if !displace_gas(d, &cx.lq, ev, cell, ecell) {
        return false;
    }
    d.updated.temperature[cell] = new_tc.v();
    d.updated.mass[cell] = 5.0;
    d.updated.temperature[cell] = hot.v();
    d.updated.element[cell] = tgt;
    d.updated.mass[other] = (F(d.updated.mass[other]) - 5.0).v();
    if !(f32::MIN_POSITIVE < d.updated.mass[other]) {
        evaporate(d, ev, other);
    }
    ev.change_substance(d, cell);
    true
}

/// HeadlessUnstableFallSimpleSwap: in headless mode an unstable solid drops
/// straight down, swapping through everything until it rests on a solid.
fn headless_fall(d: &mut SimData, cx: &Cx, ev: &mut Events, cell: usize) {
    let w = d.width as i32;
    let mut cur = cell as i32 - w;
    while cur >= 0 {
        let above = (cur + w) as usize;
        if cx.st(d.updated.element[cur as usize]) == state::SOLID {
            ev.change_substance(d, above);
            return;
        }
        swap_cells(&mut d.updated, above, cur as usize);
        ev.change_substance(d, above);
        cur -= w;
    }
}

/// DoUnstableCheckBasic.
fn unstable_check_basic(d: &mut SimData, cx: &Cx, ev: &mut Events, cell: usize) {
    let below = cell - d.width as usize;
    let eb = cx.e(d.updated.element[below]);
    if eb.state & state::MASK >= state::SOLID || eb.id == cx.void_hash || d.updated.properties[below] & 4 != 0 {
        d.stable_ticks[cell] |= 0x1f;
        return;
    }
    if stable_ticks_remaining(d, cell) != 0 {
        return;
    }
    assert!(d.headless, "unstable falling entities are not modelled outside headless mode");
    headless_fall(d, cx, ev, cell);
}

/// DoSublimation: solid neighbours of the gas or vacuum `cell` sublimate
/// into it. `cell_elem` and `ppd` are the cell's element as of entry; the
/// native code keeps using them after the cell changes.
fn sublimation(d: &mut SimData, cx: &Cx, ev: &mut Events, cell: usize, ppd: &Element) -> bool {
    if !(1.8 > d.updated.mass[cell]) {
        return false;
    }
    let w = d.width as usize;
    let nbrs = [cell + w, cell + 1, cell - 1, cell - w];
    let cell_elem = d.updated.element[cell];
    let mut result = false;
    for &n in &nbrs {
        let p = cx.e(d.updated.element[n]);
        if p.state & state::MASK != state::SOLID || p.sublimate_index == 0xffff {
            continue;
        }
        if rand01(d) > p.sublimate_probability {
            continue;
        }
        let rate = F(p.sublimate_rate) * 0.2;
        let mn = F(d.updated.mass[n]);
        let amt = F(minss(rate.v(), mn.v()));
        if !((mn - amt) >= rate) {
            d.updated.element[n] = p.sublimate_index;
            d.updated.mass[n] = (mn * p.sublimate_efficiency).v();
            ev.change_substance(d, n);
            result = true;
            continue;
        }
        let count = F::from_i32(d.updated.disease_count[n]);
        let mut g = cvttss2si(((amt / mn) * count).v());
        let moved;
        if cell_elem == p.sublimate_index {
            let mut add = amt * p.sublimate_efficiency;
            let room = F(1.8) - d.updated.mass[cell];
            let mut m = amt;
            if add > room {
                m = (room / add) * amt;
                add = room;
                g = cvttss2si(((m / mn) * count).v());
            }
            let (t, di) = (F(d.updated.temperature[n]), d.updated.disease[n]);
            add_mass_and_update_temperature(cx.lq.dis, &mut d.updated, cell, add, t, di, g);
            moved = m;
            result = true;
        } else {
            let mut into_vacuum = ppd.state & state::MASK == state::VACUUM;
            if !into_vacuum {
                let ec = d.updated.element[cell];
                if displace_gas(d, &cx.lq, ev, cell, ec) {
                    into_vacuum = true;
                } else if !result {
                    continue;
                }
            }
            if into_vacuum {
                let u = &mut d.updated;
                u.element[cell] = p.sublimate_index;
                u.mass[cell] = (amt * p.sublimate_efficiency + u.mass[cell]).v();
                u.temperature[cell] = u.temperature[n];
                u.disease[cell] = u.disease[n];
                u.disease_count[cell] = g;
                ev.change_substance(d, cell);
                result = true;
            }
            // Otherwise (gas that cannot be displaced, after an earlier
            // success) the mass leaves `n` without arriving anywhere.
            moved = amt;
        }
        d.updated.mass[n] = (F(d.updated.mass[n]) - moved).v();
        modify_germs(d, n, g.wrapping_neg());
        d.accumulated_flow[n] = (moved + d.accumulated_flow[n]).v();
        if !(1e-9f32 < d.updated.mass[n]) {
            evaporate(d, ev, n);
        }
    }
    result
}

/// The offgassing lambda (L157-187): move `amount` from `src` into `dst`,
/// which becomes (or already is) the element's sublimate gas.
fn offgas_transfer(d: &mut SimData, ev: &mut Events, ppd: &Element, src: usize, dst: usize, amount: F, transfer_germs: bool) {
    let m = F(d.updated.mass[src]);
    d.updated.mass[src] = (m - amount).v();
    let mut g = 0;
    if transfer_germs {
        g = cvttss2si((F::from_i32(d.updated.disease_count[src]) * (amount / m)).v());
        modify_germs(d, src, g.wrapping_neg());
        if d.updated.disease[dst] != d.updated.disease[src] {
            germs::clear(&mut d.updated, dst);
            d.updated.disease[dst] = d.updated.disease[src];
        }
    }
    if d.updated.element[dst] != ppd.sublimate_index {
        d.updated.element[dst] = ppd.sublimate_index;
        ev.change_substance(d, dst);
        germs::clear(&mut d.updated, dst);
        d.updated.mass[dst] = 0.0;
    }
    let add = amount * ppd.sublimate_efficiency;
    let u = &d.updated;
    let t = combined_temperature(F(u.mass[dst]), F(u.temperature[dst]), add, F(u.temperature[src]));
    d.updated.temperature[dst] = t.v();
    d.updated.mass[dst] = (add + d.updated.mass[dst]).v();
    modify_germs(d, dst, g);
}

fn liquid_cell(d: &mut SimData, cx: &Cx, ev: &mut Events, cell: usize, ppd: &Element) {
    let w = d.width as usize;
    if 0.01 > d.updated.mass[cell] {
        zero_flow(d, cell);
        evaporate(d, ev, cell);
        return;
    }
    if density_displacement(d, cx, ev, cell, ppd, 0.3) {
        return;
    }
    let m = F(d.updated.mass[cell]);
    if m > ppd.max_mass {
        let p = m / ppd.max_mass;
        for dir in [-(d.width), -1, 1, d.width] {
            if pressure_break(d, cx, cell, p, dir) {
                return;
            }
        }
        let m = F(d.updated.mass[cell]);
        if m > (F(ppd.max_mass) * 1.5) && overflow_up(d, cx, ev, cell, m, ppd.max_mass) {
            return;
        }
    }
    for other in [cell - w, cell - 1, cell + 1, cell + w] {
        if partial_heat_transition(d, cx, ev, cell, other) {
            return;
        }
    }
    offgas(d, cx, ev, cell, ppd);
}

/// L104-131: an over-full liquid pushes almost half its mass into the
/// liquid above after displacing that liquid sideways.
fn overflow_up(d: &mut SimData, cx: &Cx, ev: &mut Events, cell: usize, m: F, max_mass: f32) -> bool {
    let above = cell + d.width as usize;
    let ma = F(d.updated.mass[above]);
    if !(m > ma) {
        return false;
    }
    let ea = d.updated.element[above];
    if cx.lq.l[ea as usize].state & 3 != state::LIQUID {
        return false;
    }
    let max_liquid = maxss((ma * 1.01f32).v(), max_mass);
    if !(m > max_liquid) {
        return false;
    }
    if !displace_liquid(d, cx, ev, above, ea) {
        return false;
    }
    let u = &mut d.updated;
    let m3 = F(u.mass[cell]);
    let amt = m3 * f32::from_bits(0x3efeb9f3);
    u.mass[above] = amt.v();
    u.temperature[above] = u.temperature[cell];
    u.element[above] = u.element[cell];
    let g = cvttss2si((F::from_i32(u.disease_count[cell]) * (amt / m3)).v());
    u.disease[above] = u.disease[cell];
    u.disease_count[above] = g;
    u.infestation_ticks[above] = 0;
    u.growth_error[above] = 0.0;
    ev.change_substance(d, above);
    d.updated.mass[cell] = (F(d.updated.mass[cell]) - amt).v();
    modify_germs(d, cell, g.wrapping_neg());
    true
}

/// L147-232: offgassing of a liquid into the gas or vacuum above.
fn offgas(d: &mut SimData, cx: &Cx, ev: &mut Events, cell: usize, ppd: &Element) {
    let w = d.width as usize;
    let above = cell + w;
    if !(1.8 > d.updated.mass[above]) || ppd.sublimate_index == 0xffff {
        return;
    }
    if cx.st(d.updated.element[above]) > state::GAS {
        return;
    }
    let r = rand01(d);
    if d.updated.properties[above] & 1 != 0 || r >= ppd.sublimate_probability {
        return;
    }
    let below = cell - w;
    let m = F(d.updated.mass[cell]);
    let mut amount = F(minss((F(ppd.off_gas_percentage) * m).v(), 1.0));
    let mut from_below = false;
    if d.updated.element[below] == d.updated.element[cell] && d.updated.mass[below] > m.v() {
        from_below = true;
        amount = F(minss((F(ppd.off_gas_percentage) * d.updated.mass[below]).v(), 1.0));
    }
    if (m - amount) >= 0.01 {
        if !(amount > 0.0) {
            return;
        }
        let ea = d.updated.element[above];
        if ea != ppd.sublimate_index && ea != d.vacuum_element && !displace_gas(d, &cx.lq, ev, above, ea) {
            return;
        }
        let add = amount * ppd.sublimate_efficiency;
        let room = F(1.8) - d.updated.mass[above];
        if add > room {
            amount = amount * (room / add);
        }
        let u = &d.updated;
        let germs = u.disease[cell] == u.disease[above] || u.disease[above] == 0xff;
        offgas_transfer(d, ev, ppd, cell, above, amount, germs);
        d.accumulated_flow[cell] = (amount + d.accumulated_flow[cell]).v();
        return;
    }
    // Nearly everything would offgas: the cell turns into the gas in place,
    // topped up from a heavier cell of the same liquid below.
    offgas_transfer(d, ev, ppd, cell, cell, m, true);
    let rest = amount - m;
    if rest > 0.0 && from_below {
        let u = &d.updated;
        let germs = u.disease[below] == u.disease[cell] || u.disease[cell] == 0xff;
        offgas_transfer(d, ev, ppd, below, cell, rest, germs);
    }
}

fn gas_cell(d: &mut SimData, cx: &Cx, ev: &mut Events, cell: usize, ppd: &Element) {
    let w = d.width as usize;
    let dd = d.displacement_direction;
    d.displacement_direction = dd.wrapping_neg();
    let m = d.updated.mass[cell];
    let mut evap = false;
    if !(m >= 1e-9f32) {
        evap = true;
    } else if !(m >= 0.001f32) {
        // A trace of gas next to a denser gas evaporates.
        let c = cell as i32;
        for n in [c + dd, c - dd, c - w as i32, c + w as i32] {
            let n = n as usize;
            if !(1.0 > d.updated.mass[n]) && cx.st(d.updated.element[n]) == state::GAS {
                evap = true;
                break;
            }
        }
    }
    if evap {
        zero_flow(d, cell);
        evaporate(d, ev, cell);
    }
    if !density_displacement(d, cx, ev, cell, ppd, 0.99) {
        gas_after_density(d, cx, ev, cell, ppd);
    }
    sublimation(d, cx, ev, cell, ppd);
}

/// L275-332: partial melting of solid neighbours, then a random swap with a
/// cooler gas neighbour (heavier gases only for the cell below).
fn gas_after_density(d: &mut SimData, cx: &Cx, ev: &mut Events, cell: usize, ppd: &Element) {
    let w = d.width as usize;
    if d.updated.properties[cell] & 2 == 0 {
        for other in [cell - w, cell - 1, cell + 1, cell + w] {
            if partial_melt(d, cx, ev, cell, other) {
                return;
            }
        }
    }
    if !(rand01(d) > 0.9) {
        return;
    }
    let dd = d.displacement_direction;
    let c = cell as i32;
    let nbrs = [(c + dd) as usize, (c - dd) as usize, cell - w];
    let check_molar = [false, false, true];
    let mut best: Option<usize> = None;
    let mut best_t = f32::MAX;
    for k in 0..3 {
        let n = nbrs[k];
        let t = d.updated.temperature[n];
        if t > best_t {
            continue;
        }
        if !(rand01(d) > 0.5) {
            continue;
        }
        let e = cx.e(d.updated.element[n]);
        if e.state & state::MASK == state::GAS && (!check_molar[k] || ppd.molar_mass > e.molar_mass) {
            best = Some(n);
            best_t = t;
        }
    }
    if let Some(b) = best {
        swap_cells(&mut d.updated, b, cell);
        ev.change_substance(d, b);
        ev.change_substance(d, cell);
    }
}

/// PostProcessCell.
fn post_process_cell(d: &mut SimData, cx: &Cx, ev: &mut Events, cell: usize) {
    let ppd = cx.e(d.updated.element[cell]).clone();
    match ppd.state & state::MASK {
        state::VACUUM => {
            if d.updated.properties[cell] & 1 == 0 {
                sublimation(d, cx, ev, cell, &ppd);
            }
        }
        state::GAS => gas_cell(d, cx, ev, cell, &ppd),
        state::LIQUID => liquid_cell(d, cx, ev, cell, &ppd),
        _ => {
            if ppd.state & 0xb == 0xb {
                // DoUnstableCheckWithDiagonals needs the saved option
                // ENABLE_DIAGONAL_FALLING_SAND, which no settle sets.
                assert!(d.saved_options & 1 == 0, "diagonal falling is not ported");
                unstable_check_basic(d, cx, ev, cell);
            }
        }
    }
}

/// The post-process phase: the cell box derived from the active region
/// (32-cell chunks), world-zone space mass loss (only with world zones,
/// which a settle never has), then PostProcessCell over the box.
pub fn phase(sim: &mut Sim, r: Region) -> Result<(), String> {
    let d = sim.data.as_mut().ok_or("not initialized")?;
    let cx = Cx {
        el: &sim.elements,
        lq: liquid::Ctx { l: &sim.liquid_table, p: &sim.pressure_table, dis: &sim.diseases },
        void_hash: element_hash("Void"),
    };
    let b = post_process_box(d, r);
    let w = d.width;
    for y in b.y0..b.y1 {
        for x in b.x0..b.x1 {
            post_process_cell(d, &cx, &mut sim.events, (y * w + x) as usize);
        }
    }
    Ok(())
}

/// UpdateData L873-879: the region widened to whole 32-cell chunks (offset
/// by the border) and clipped to the grid interior, as [x0, x1) x [y0, y1).
pub fn post_process_box(d: &SimData, r: Region) -> Region {
    let (bx0, by0) = ((r.x0 - 1) / 32, (r.y0 - 1) / 32);
    let (bx1, by1) = ((r.x1 + 30) / 32, (r.y1 + 30) / 32);
    Region {
        x0: r.x0.max(bx0 * 32 + 1),
        x1: (r.x1 + 1).min(bx1 * 32 + 1).min(d.width - 1),
        y0: r.y0.max(by0 * 32 + 1),
        y1: (r.y1 + 1).min(by1 * 32 + 1).min(d.height - 1),
    }
}
