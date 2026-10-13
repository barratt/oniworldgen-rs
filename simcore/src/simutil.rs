//! Shared cell helpers (simutil.cpp): adding mass with temperature mixing
//! and displacing a cell's contents into a neighbour. Sim thread (FTZ).
use crate::events::Events;
use crate::fp::F;
use crate::gas::combined_clamp;
use crate::germs;
use crate::sim::SimData;
use crate::state::Cells;
use crate::tables::Disease;

/// AddMassAndUpdateTemperature on grid `c`.
#[allow(clippy::too_many_arguments)]
pub fn add_mass_and_update_temperature(dis: &[Disease], c: &mut Cells, i: usize, mass: F, temp: F, disease: u8, count: i32) {
    let old = F(c.mass[i]);
    let nm = old + mass;
    if nm > 0.0 {
        let t_old = F(c.temperature[i]);
        let v = (old * t_old + temp * mass) / nm;
        c.mass[i] = nm.v();
        c.temperature[i] = combined_clamp(v, t_old, temp).v();
        germs::add(dis, c, i, disease, count);
    } else {
        c.temperature[i] = 0.0;
        germs::clear(c, i);
    }
}

/// DoDisplacement on `updated`: move `from`'s contents into `to`, clear `from`.
pub fn do_displacement(d: &mut SimData, dis: &[Disease], ev: &mut Events, from: usize, to: usize) {
    let u = &mut d.updated;
    let (m, t, di, dc) = (F(u.mass[from]), F(u.temperature[from]), u.disease[from], u.disease_count[from]);
    add_mass_and_update_temperature(dis, u, to, m, t, di, dc);
    u.element[to] = u.element[from];
    d.clear_cell(from);
    ev.change_substance(d, from);
    ev.change_substance(d, to);
}
