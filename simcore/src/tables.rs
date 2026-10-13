//! Static tables delivered by messages: the element table (164-byte
//! `Sim.Element` records serialized by the managed ElementLoader) and the
//! disease table. Everything here is a function of game data.
use crate::io::Reader;

pub const ELEMENT_RECORD: usize = 164;

pub mod state {
    pub const VACUUM: u8 = 0;
    pub const GAS: u8 = 1;
    pub const LIQUID: u8 = 2;
    pub const SOLID: u8 = 3;
    pub const MASK: u8 = 3;
    pub const UNBREAKABLE: u8 = 4;
    pub const UNSTABLE: u8 = 8;
    pub const TEMPERATURE_INSULATED: u8 = 16;
}

#[derive(Clone, Debug, Default)]
pub struct Element {
    pub id: u32,
    pub low_temp_transition_idx: u16,
    pub high_temp_transition_idx: u16,
    pub elements_table_idx: u16,
    /// State in the low two bits plus `state::` flags.
    pub state: u8,
    pub specific_heat_capacity: f32,
    pub thermal_conductivity: f32,
    pub molar_mass: f32,
    pub solid_surface_area_multiplier: f32,
    pub liquid_surface_area_multiplier: f32,
    pub gas_surface_area_multiplier: f32,
    pub flow: f32,
    pub viscosity: f32,
    pub min_horizontal_flow: f32,
    pub min_vertical_flow: f32,
    pub max_mass: f32,
    pub low_temp: f32,
    pub high_temp: f32,
    pub strength: f32,
    pub low_temp_transition_ore_id: u32,
    pub low_temp_transition_ore_mass_conversion: f32,
    pub high_temp_transition_ore_id: u32,
    pub high_temp_transition_ore_mass_conversion: f32,
    pub sublimate_index: u16,
    pub convert_index: u16,
    pub material_properties: u32,
    pub sublimate_fx: i32,
    pub sublimate_rate: f32,
    pub sublimate_efficiency: f32,
    pub sublimate_probability: f32,
    pub off_gas_percentage: f32,
    pub light_absorption_factor: f32,
    pub radiation_absorption_factor: f32,
    pub radiation_per_1000_mass: f32,
    pub default_temperature: f32,
    pub default_mass: f32,
    pub default_pressure: f32,
}

impl Element {
    pub fn phase(&self) -> u8 {
        self.state & state::MASK
    }
}

fn f(b: &[u8], o: usize) -> f32 {
    f32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
fn u(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
fn h(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes(b[o..o + 2].try_into().unwrap())
}

/// Parse the elements message: count, records, then names (ignored).
pub fn parse_elements(payload: &[u8]) -> Result<Vec<Element>, String> {
    let mut r = Reader::new(payload);
    let count = r.u32()? as usize;
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let b = r.take(ELEMENT_RECORD)?;
        out.push(Element {
            id: u(b, 0x00),
            low_temp_transition_idx: h(b, 0x04),
            high_temp_transition_idx: h(b, 0x06),
            elements_table_idx: h(b, 0x08),
            state: b[0x0a],
            specific_heat_capacity: f(b, 0x0c),
            thermal_conductivity: f(b, 0x10),
            molar_mass: f(b, 0x14),
            solid_surface_area_multiplier: f(b, 0x18),
            liquid_surface_area_multiplier: f(b, 0x1c),
            gas_surface_area_multiplier: f(b, 0x20),
            flow: f(b, 0x24),
            viscosity: f(b, 0x28),
            min_horizontal_flow: f(b, 0x2c),
            min_vertical_flow: f(b, 0x30),
            max_mass: f(b, 0x34),
            low_temp: f(b, 0x38),
            high_temp: f(b, 0x3c),
            strength: f(b, 0x40),
            low_temp_transition_ore_id: u(b, 0x44),
            low_temp_transition_ore_mass_conversion: f(b, 0x48),
            high_temp_transition_ore_id: u(b, 0x4c),
            high_temp_transition_ore_mass_conversion: f(b, 0x50),
            sublimate_index: h(b, 0x54),
            convert_index: h(b, 0x56),
            material_properties: u(b, 0x58),
            sublimate_fx: u(b, 0x78) as i32,
            sublimate_rate: f(b, 0x7c),
            sublimate_efficiency: f(b, 0x80),
            sublimate_probability: f(b, 0x84),
            off_gas_percentage: f(b, 0x88),
            light_absorption_factor: f(b, 0x8c),
            radiation_absorption_factor: f(b, 0x90),
            radiation_per_1000_mass: f(b, 0x94),
            default_temperature: f(b, 0x98),
            default_mass: f(b, 0x9c),
            default_pressure: f(b, 0xa0),
        });
    }
    Ok(out)
}

/// Element hash of a name as the game computes it (SDBM over lower case).
pub fn element_hash(name: &str) -> u32 {
    let mut hash: u32 = 0;
    for c in name.to_lowercase().bytes() {
        hash = (c as u32).wrapping_add(hash << 6).wrapping_add(hash << 16).wrapping_sub(hash);
    }
    hash
}

#[derive(Clone, Copy, Debug, Default)]
pub struct RangeInfo {
    pub min_viable: f32,
    pub min_growth: f32,
    pub max_growth: f32,
    pub max_viable: f32,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ElemGrowth {
    pub under_population_death_rate: f32,
    pub population_half_life: f32,
    pub over_population_half_life: f32,
    pub diffusion_scale: f32,
    pub min_count_per_kg: f32,
    pub max_count_per_kg: f32,
    pub min_diffusion_count: i32,
    pub min_diffusion_infestation_tick_count: u8,
}

#[derive(Clone, Debug, Default)]
pub struct Disease {
    pub name: String,
    pub hash: u32,
    pub strength: f32,
    pub temperature_range: RangeInfo,
    pub temperature_half_lives: RangeInfo,
    pub pressure_range: RangeInfo,
    pub pressure_half_lives: RangeInfo,
    pub radiation_kill_rate: f32,
    /// Indexed by element index.
    pub growth: Vec<ElemGrowth>,
}

fn range(r: &mut Reader) -> Result<RangeInfo, String> {
    Ok(RangeInfo { min_viable: r.f32()?, min_growth: r.f32()?, max_growth: r.f32()?, max_viable: r.f32()? })
}

/// Parse the diseases message (SimMessages.CreateDiseaseTable).
pub fn parse_diseases(payload: &[u8], element_count: usize) -> Result<Vec<Disease>, String> {
    let mut r = Reader::new(payload);
    let count = r.u32()? as usize;
    if r.u32()? as usize != element_count {
        return Err("disease table element count differs".into());
    }
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let n = r.i32()? as usize;
        let name = String::from_utf8_lossy(r.take(n)?).into_owned();
        let mut d = Disease { name, hash: r.u32()?, strength: r.f32()?, ..Default::default() };
        d.temperature_range = range(&mut r)?;
        d.temperature_half_lives = range(&mut r)?;
        d.pressure_range = range(&mut r)?;
        d.pressure_half_lives = range(&mut r)?;
        d.radiation_kill_rate = r.f32()?;
        for _ in 0..element_count {
            d.growth.push(ElemGrowth {
                under_population_death_rate: r.f32()?,
                population_half_life: r.f32()?,
                over_population_half_life: r.f32()?,
                diffusion_scale: r.f32()?,
                min_count_per_kg: r.f32()?,
                max_count_per_kg: r.f32()?,
                min_diffusion_count: r.i32()?,
                min_diffusion_infestation_tick_count: r.u8()?,
            });
        }
        out.push(d);
    }
    if !r.is_empty() {
        return Err(format!("{} unread bytes in disease table", r.remaining()));
    }
    Ok(out)
}
