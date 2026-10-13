//! Heat exchange between neighbouring cells and temperature-driven state
//! transitions (the temperature task of a step). Runs, natively, on the
//! worker thread, which does not flush denormals.
use crate::events::Events;
use crate::fp::{expf, logf, maxsd, maxss, minsd, minss};
use crate::sim::SimData;
use crate::tables::{state, Element};

/// Per-element data used here (native ElementTemperatureData).
#[derive(Clone, Debug, Default)]
pub struct TemperatureData {
    pub state: u8,
    pub low_temp_transition_idx: u16,
    pub high_temp_transition_idx: u16,
    pub low_temp_transition_ore_idx: u16,
    pub high_temp_transition_ore_idx: u16,
    pub specific_heat_capacity: f32,
    pub thermal_conductivity: f32,
    pub default_mass: f32,
    pub mass_area_scale: f32,
    pub gas_surface_area_multiplier: f32,
    pub liquid_surface_area_multiplier: f32,
    pub solid_surface_area_multiplier: f32,
    pub low_temp: f32,
    pub high_temp: f32,
    pub low_temp_transition_ore_mass_conversion: f32,
    pub high_temp_transition_ore_mass_conversion: f32,
}

const VACUUM_HASH: u32 = 0x2d39bf75;

pub fn build_table(elements: &[Element]) -> Vec<TemperatureData> {
    let index_of = |id: u32| -> u16 {
        if id == VACUUM_HASH {
            return 0xffff;
        }
        elements.iter().position(|e| e.id == id).map(|i| i as u16).unwrap_or(0xffff)
    };
    elements
        .iter()
        .map(|e| TemperatureData {
            state: e.state,
            low_temp_transition_idx: e.low_temp_transition_idx,
            high_temp_transition_idx: e.high_temp_transition_idx,
            low_temp_transition_ore_idx: index_of(e.low_temp_transition_ore_id),
            high_temp_transition_ore_idx: index_of(e.high_temp_transition_ore_id),
            specific_heat_capacity: e.specific_heat_capacity,
            thermal_conductivity: e.thermal_conductivity,
            default_mass: e.default_mass,
            mass_area_scale: if e.phase() == state::GAS { 1.0 } else { 0.001 },
            gas_surface_area_multiplier: e.gas_surface_area_multiplier,
            liquid_surface_area_multiplier: e.liquid_surface_area_multiplier,
            solid_surface_area_multiplier: e.solid_surface_area_multiplier,
            low_temp: e.low_temp,
            high_temp: e.high_temp,
            low_temp_transition_ore_mass_conversion: e.low_temp_transition_ore_mass_conversion,
            high_temp_transition_ore_mass_conversion: e.high_temp_transition_ore_mass_conversion,
        })
        .collect()
}

impl TemperatureData {
    /// Surface-area multiplier this element presents to a neighbour in state `other`.
    fn surface_area_multiplier(&self, other_state: u8) -> f32 {
        match other_state & state::MASK {
            state::GAS => self.gas_surface_area_multiplier,
            state::LIQUID => self.liquid_surface_area_multiplier,
            state::SOLID => self.solid_surface_area_multiplier,
            _ => 1.0,
        }
    }
}

/// CellSOA::GetInsulationValue: (ins^2) * float(1/65025).
fn insulation(v: u8) -> f32 {
    let f = v as i32 as f32;
    (f * f) * f32::from_bits(0x37810182)
}

/// details::CalculateTemperatureExchange_precise in double precision, with
/// the operation order of the native build. Returns (hot', cold').
fn exchange_precise(k: f64, dt: f64, t_cold: f64, c_cold: f64, t_hot: f64, c_hot: f64) -> (f32, f32) {
    exchange_precise_q(0.25, k, dt, t_cold, c_cold, t_hot, c_hot)
}

#[allow(clippy::too_many_arguments)]
fn exchange_precise_q(f: f64, k: f64, dt: f64, t_cold: f64, c_cold: f64, t_hot: f64, c_hot: f64) -> (f32, f32) {
    let dtemp = t_hot - t_cold;
    let t_eq = (t_cold * c_cold + t_hot * c_hot) / (c_cold + c_hot);
    let q = dtemp * f;
    let candidates = [dtemp * k * dt, q * c_cold, q * c_hot];
    let mut e = candidates[0];
    for &c in &candidates[1..] {
        // std::min_element with comisd/cmovb: takes `c` when below or unordered.
        if !(c >= e) {
            e = c;
        }
    }
    let cold = minsd(e / c_cold + t_cold, t_eq) as f32;
    let hot = maxsd((-1.0 / c_hot) * e + t_hot, t_eq) as f32;
    (hot, cold)
}

/// UpdateTemperature(a, b): reads `cells`, accumulates into `updated`.
pub fn update_temperature(d: &mut SimData, td: &[TemperatureData], a: usize, b: usize) {
    let c = &d.cells;
    let (ea, eb) = (&td[c.element[a] as usize], &td[c.element[b] as usize]);
    let ins_a = insulation(c.insulation[a]);
    let ka = ins_a * ea.thermal_conductivity;
    let ins_b = insulation(c.insulation[b]);
    let kb = ins_b * eb.thermal_conductivity;
    let sa = eb.surface_area_multiplier(ea.state) * ea.surface_area_multiplier(eb.state);
    let k = if ins_a >= 1.0 && ins_b >= 1.0 { expf((logf(kb) + logf(ka)) * 0.5) } else { minss(ka, kb) };
    let (ta, tb) = (c.temperature[a], c.temperature[b]);
    let ca = (c.mass[a] * ea.specific_heat_capacity) as f64;
    let cb = (c.mass[b] * eb.specific_heat_capacity) as f64;
    let kk = (k * sa) as f64;
    let dt = 0.2f32 as f64;
    let (na, nb) = if ta > tb {
        let (hot, cold) = exchange_precise(kk, dt, tb as f64, cb, ta as f64, ca);
        (hot, cold)
    } else {
        let (hot, cold) = exchange_precise(kk, dt, ta as f64, ca, tb as f64, cb);
        (cold, hot)
    };
    let u = &mut d.updated;
    u.temperature[a] += na - ta;
    u.temperature[b] += nb - tb;
    u.temperature[a] = maxss(minss(u.temperature[a], 10000.0), 1.0);
    u.temperature[b] = maxss(minss(u.temperature[b], 10000.0), 1.0);
}

/// CalculateTemperatureExchange (float wrapper): orders the pair by
/// temperature, returns (new temperature of b, new temperature of a).
fn exchange(q: f32, k: f32, dt: f32, ta: f32, ca: f32, tb: f32, cb: f32) -> (f32, f32) {
    let (q, k, dt) = (q as f64, k as f64, dt as f64);
    if ta > tb {
        let (hot, cold) = exchange_precise_q(q, k, dt, tb as f64, cb as f64, ta as f64, ca as f64);
        (cold, hot)
    } else {
        exchange_precise_q(q, k, dt, ta as f64, ca as f64, tb as f64, cb as f64)
    }
}

/// UpdateTemperatureForBackwallExchange: the cell exchanges heat with its
/// backwall; reads `cells`, accumulates into `updated`, writes the backwall.
fn update_temperature_backwall(d: &mut SimData, td: &[TemperatureData], ev: &mut Events, i: usize) {
    let (ec, eb) = (d.cells.element[i], d.backwall.element[i]);
    if ec == d.vacuum_element || eb == d.vacuum_element || ec == d.unobtanium_element || eb == d.unobtanium_element {
        return;
    }
    let (tc, tb) = (&td[ec as usize], &td[eb as usize]);
    let ins = insulation(d.cells.insulation[i]);
    let kc = ins * tc.thermal_conductivity;
    let kb = tb.thermal_conductivity;
    let ins2 = insulation(d.cells.insulation[i]);
    let sa = tc.surface_area_multiplier(tb.state) * tb.surface_area_multiplier(tc.state);
    let k = if ins2 >= 1.0 { expf((logf(kb) + logf(kc)) * 0.5) } else { minss(kb, kc) };
    let (new_b, new_c) = exchange(
        0.01,
        k * sa,
        0.2,
        d.cells.temperature[i],
        d.cells.mass[i] * tc.specific_heat_capacity,
        d.backwall.temperature[i],
        d.backwall.mass[i] * tb.specific_heat_capacity,
    );
    d.updated.temperature[i] += new_c - d.cells.temperature[i];
    d.backwall.temperature[i] = new_b;
    let bt = d.backwall.temperature[i];
    if !(bt >= tb.low_temp - 3.0) || bt > tb.high_temp + 3.0 {
        ev.backwall_should_transition(d, i);
    }
}

/// The temperature task over rows [y0, y1) and columns [x0, x1).
pub fn run_task(d: &mut SimData, td: &[TemperatureData], ev: &mut Events, x0: i32, y0: i32, x1: i32, y1: i32) -> Result<(), String> {
    let w = d.width;
    for y in y0..y1 {
        let row_end = x1 + y * w;
        for cell in (x0 + y * w)..row_end {
            let i = cell as usize;
            if !(0.0 >= d.cells.mass[i]) && !(0.0 >= d.backwall.mass[i]) {
                update_temperature_backwall(d, td, ev, i);
            }
            if cell < row_end - 1 {
                let j = i + 1;
                try_exchange(d, td, i, j);
            }
            if y < y1 - 1 {
                let j = i + w as usize;
                try_exchange(d, td, i, j);
            }
        }
    }
    for y in y0..y1 {
        for cell in (x0 + y * w)..(x1 + y * w) {
            let i = cell as usize;
            let e = d.cells.element[i] as usize;
            do_state_transition(d, td, ev, i, e);
        }
    }
    Ok(())
}

fn try_exchange(d: &mut SimData, td: &[TemperatureData], i: usize, j: usize) {
    let c = &d.cells;
    let diff = (c.temperature[i] - c.temperature[j]).abs();
    if !(diff >= 1.0) {
        return;
    }
    let (a, b) = (&td[c.element[i] as usize], &td[c.element[j] as usize]);
    if 0.0 >= a.thermal_conductivity || 0.0 >= b.thermal_conductivity {
        return;
    }
    if (a.state | b.state) & state::TEMPERATURE_INSULATED != 0 {
        return;
    }
    update_temperature(d, td, i, j);
}

/// DoStateTransition on `updated`, with `t` the element data of the cell's
/// pre-step element. Returns whether a transition happened.
pub fn do_state_transition(d: &mut SimData, td: &[TemperatureData], ev: &mut Events, i: usize, e: usize) -> bool {
    let t = &td[e];
    if 0.0 >= d.updated.mass[i] {
        return false;
    }
    let temp = d.updated.temperature[i];
    if !(temp >= t.low_temp - 3.0) && t.low_temp_transition_idx != 0xffff {
        let nt = temp + 1.5;
        d.updated.temperature[i] = if nt > 0.0 { nt } else { 0.0 };
        let ore = t.low_temp_transition_ore_idx;
        if ore != 0xffff {
            let ore_mass = t.low_temp_transition_ore_mass_conversion * d.updated.mass[i];
            if ore_mass > 0.001 {
                let germs = crate::fp::cvttss2si(d.updated.disease_count[i] as f32 * t.low_temp_transition_ore_mass_conversion);
                if ev.spawn_ore(d, i, ore_mass, true) {
                    d.updated.mass[i] -= ore_mass;
                    d.modify_disease_count(i, germs.wrapping_neg());
                }
            }
        }
        let target = t.low_temp_transition_idx;
        let tt = &td[target as usize];
        if tt.state & state::MASK == state::SOLID {
            let m = d.updated.mass[i];
            if !(m / tt.default_mass > 0.8) && ev.spawn_ore(d, i, m, false) {
                d.clear_cell(i);
                ev.change_substance(d, i);
                return true;
            }
            d.updated.element[i] = target;
        } else {
            // SpawnFallingLiquid never fires in headless mode.
            if !d.headless {
                panic!("non-headless falling liquid is not ported");
            }
            d.updated.element[i] = target;
        }
        ev.change_substance(d, i);
        return true;
    }
    if temp > t.high_temp + 3.0 && t.high_temp_transition_idx != 0xffff {
        let nt = temp - 1.5;
        d.updated.temperature[i] = if nt > 0.0 { nt } else { 0.0 };
        d.updated.element[i] = t.high_temp_transition_idx;
        let ore = t.high_temp_transition_ore_idx;
        if ore != 0xffff {
            let ore_mass = t.high_temp_transition_ore_mass_conversion * d.updated.mass[i];
            if ore_mass > 0.001 {
                let germs = crate::fp::cvttss2si(d.updated.disease_count[i] as f32 * t.high_temp_transition_ore_mass_conversion);
                if ev.spawn_ore(d, i, ore_mass, true) {
                    d.updated.mass[i] -= ore_mass;
                    d.modify_disease_count(i, germs.wrapping_neg());
                }
            }
        }
        if d.updated.properties[i] & 0x40 != 0 {
            ev.cell_melted(d, i);
            d.stable_ticks[i] = 0x1f;
        }
        ev.change_substance(d, i);
        return true;
    }
    false
}
