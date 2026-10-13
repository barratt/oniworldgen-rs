//! The phases of one UpdateData call, individually runnable so each can be
//! verified against the native state at its boundaries.
use crate::snapshot::{Kind, Snapshot};
use crate::Sim;

/// Phase names in execution order; a phase transforms the state at
/// the previous boundary into the state at its own boundary.
pub const PHASES: &[&str] = &[
    "start",
    "temperature",
    "gas_flow",
    "gas_displacement",
    "liquid",
    "disease_diffusion",
    "radiation",
    "components",
    "post_process",
    "disease_post_process",
    "end",
];

/// Active region in padded coordinates: [x0, x1) x [y0, y1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Region {
    pub x0: i32,
    pub y0: i32,
    pub x1: i32,
    pub y1: i32,
}

fn get<T: Copy>(s: &Snapshot, name: &str, out: &mut Vec<T>, kind: Kind, conv: impl Fn(u32) -> T) -> Result<(), String> {
    let f = s.get(name).ok_or_else(|| format!("snapshot lacks {name}"))?;
    if f.kind != kind {
        return Err(format!("{name} has kind {:?}", f.kind));
    }
    *out = (0..f.len()).map(|i| conv(f.bits(i))).collect();
    Ok(())
}

impl Sim {
    /// Replace the simulation state with a native phase snapshot (teacher
    /// forcing): both grids, backwall, timers, visibility and step scalars.
    pub fn load_state(&mut self, s: &Snapshot) -> Result<(), String> {
        let d = self.data.as_mut().ok_or("not initialized")?;
        if s.width != d.width || s.height != d.height {
            return Err("snapshot dimensions differ".into());
        }
        for (prefix, c) in [("", &mut d.updated), ("scratch_", &mut d.cells)] {
            let n = |x: &str| format!("{prefix}{x}");
            get(s, &n("element"), &mut c.element, Kind::U16, |v| v as u16)?;
            get(s, &n("temperature"), &mut c.temperature, Kind::F32, f32::from_bits)?;
            get(s, &n("mass"), &mut c.mass, Kind::F32, f32::from_bits)?;
            get(s, &n("properties"), &mut c.properties, Kind::U8, |v| v as u8)?;
            get(s, &n("insulation"), &mut c.insulation, Kind::U8, |v| v as u8)?;
            get(s, &n("strength"), &mut c.strength, Kind::U8, |v| v as u8)?;
            get(s, &n("disease"), &mut c.disease, Kind::U8, |v| v as u8)?;
            get(s, &n("disease_count"), &mut c.disease_count, Kind::I32, |v| v as i32)?;
            get(s, &n("infestation_ticks"), &mut c.infestation_ticks, Kind::U8, |v| v as u8)?;
            get(s, &n("growth_error"), &mut c.growth_error, Kind::F32, f32::from_bits)?;
            get(s, &n("radiation"), &mut c.radiation, Kind::F32, f32::from_bits)?;
        }
        get(s, "backwall_element", &mut d.backwall.element, Kind::U16, |v| v as u16)?;
        get(s, "backwall_mass", &mut d.backwall.mass, Kind::F32, f32::from_bits)?;
        get(s, "backwall_temperature", &mut d.backwall.temperature, Kind::F32, f32::from_bits)?;
        get(s, "stable_ticks", &mut d.stable_ticks, Kind::U8, |v| v as u8)?;
        get(s, "visible", &mut d.visible, Kind::U8, |v| v as u8)?;
        let mut flow: Vec<f32> = Vec::new();
        get(s, "flow", &mut flow, Kind::F32, f32::from_bits)?;
        d.flow = flow.chunks_exact(4).map(|c| [c[0], c[1], c[2], c[3]]).collect();
        get(s, "accumulated_flow", &mut d.accumulated_flow, Kind::F32, f32::from_bits)?;
        get(s, "radiation_occlusion", &mut d.radiation_occlusion, Kind::F32, f32::from_bits)?;
        let scalar = |n: &str| s.scalars.iter().find(|(k, _)| k == n).map(|x| x.1).ok_or_else(|| format!("snapshot lacks {n}"));
        d.random_seed = scalar("random_seed")? as u32;
        d.iterate_direction = scalar("iterate_direction")? as i32;
        d.displacement_direction = scalar("displacement_direction")? as i32;
        d.tick_count = scalar("tick_count")? as u16;
        Ok(())
    }

    /// The active region from the frame message (NewGameFrame), padded.
    pub fn region(&self) -> Region {
        let f = &self.frame_message();
        let r = |o: usize| i32::from_le_bytes(f[o..o + 4].try_into().unwrap());
        Region { x0: r(4) + 1, y0: r(8) + 1, x1: r(12) + 1, y1: r(16) + 1 }
    }

    /// Run one phase on the current state.
    pub fn run_phase(&mut self, name: &str) -> Result<(), String> {
        let region = self.region();
        match name {
            "temperature" => {
                let d = self.data.as_mut().ok_or("not initialized")?;
                crate::temperature::run_task(d, &self.temperature_table, &mut self.events, region.x0, region.y0, region.x1, region.y1)
            }
            "gas_flow" => {
                let d = self.data.as_mut().ok_or("not initialized")?;
                // The temperature boundary sits just before `cells <- updated` (L328).
                let (cells, updated) = (&mut d.cells, &d.updated);
                cells.copy_from(updated);
                crate::gas::flow_phase(d, &self.pressure_table, &self.diseases, &mut self.events, region);
                Ok(())
            }
            "gas_displacement" => {
                let d = self.data.as_mut().ok_or("not initialized")?;
                crate::gas::displacement_phase(d, &self.pressure_table, &self.diseases, &mut self.events, region);
                Ok(())
            }
            "liquid" => {
                let d = self.data.as_mut().ok_or("not initialized")?;
                let cx = crate::liquid::Ctx { l: &self.liquid_table, p: &self.pressure_table, dis: &self.diseases };
                crate::liquid::phase(d, &cx, &mut self.events, region);
                Ok(())
            }
            "disease_diffusion" => {
                let d = self.data.as_mut().ok_or("not initialized")?;
                let pp = &self.pressure_table;
                let state = |e: u16| pp[e as usize].state;
                crate::disease::diffusion_phase(&self.diseases, &state, d, region);
                Ok(())
            }
            "radiation" => {
                let cosmic = f32::from_le_bytes(self.frame_message()[24..28].try_into().unwrap());
                let d = self.data.as_mut().ok_or("not initialized")?;
                if d.radiation_enabled {
                    crate::radiation::phase(d, &self.radiation_table, &self.diseases, cosmic, region);
                }
                Ok(())
            }
            // SimData::UpdateComponents: no building, emitter or chunk is ever
            // registered during a settle, so every component list is empty.
            "components" => Ok(()),
            "end" => {
                let d = self.data.as_mut().ok_or("not initialized")?;
                d.tick_count = d.tick_count.wrapping_add(1);
                Ok(())
            }
            "post_process" => crate::postprocess::phase(self, region),
            "disease_post_process" => crate::disease::post_process_phase(self, region),
            _ => Err(format!("phase {name} is not ported")),
        }
    }
}
