//! The steppable simulation: message handling, initialization and the
//! per-tick driver.
use crate::io::Reader;
use crate::messages::{self, Stream};
use crate::snapshot::Snapshot;
use crate::state::{Backwall, Cells};
use crate::tables::{self, Disease, Element};

pub struct Sim {
    pub(crate) stream: Stream,
    pub(crate) next_message: usize,
    pub(crate) tick: i32,
    /// Step-time accumulator (SimBase elapsedSeconds).
    pub(crate) elapsed: f32,
    /// The managed "last substance change" grid.
    pub managed: Option<crate::driver::Managed>,
    pub elements: Vec<Element>,
    pub diseases: Vec<Disease>,
    pub data: Option<SimData>,
    started: bool,
    pub temperature_table: Vec<crate::temperature::TemperatureData>,
    pub pressure_table: Vec<crate::gas::PressureData>,
    pub liquid_table: Vec<crate::liquid::LiquidData>,
    pub radiation_table: Vec<crate::radiation::RadiationData>,
    pub events: crate::events::Events,
}

/// Native SimData: padded grid (one border cell on every side).
pub struct SimData {
    pub width: i32,
    pub height: i32,
    pub num_game_cells: i32,
    pub random_seed: u32,
    pub iterate_direction: i32,
    pub displacement_direction: i32,
    pub saved_options: u8,
    pub void_element: u16,
    pub vacuum_element: u16,
    pub unobtanium_element: u16,
    pub cells: Cells,
    pub updated: Cells,
    pub backwall: Backwall,
    /// Per-cell stable-tick counters (5-bit field of SimData::Timers).
    pub stable_ticks: Vec<u8>,
    pub tick_count: u16,
    pub headless: bool,
    pub radiation_enabled: bool,
    /// Visibility per game cell (from prepare payloads; read by SpawnOre).
    pub visible: Vec<u8>,
    /// Per-cell flow vectors (x: from left, y: right, z: up, w: from below),
    /// cleared at the start of every native iteration.
    pub flow: Vec<[f32; 4]>,
    pub accumulated_flow: Vec<f32>,
    /// Cosmic-radiation occlusion; allocated (filled with 1) by the first
    /// radiation pass and kept between steps.
    pub radiation_occlusion: Vec<f32>,
}

impl SimData {
    pub fn len(&self) -> usize {
        (self.width * self.height) as usize
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// SimData::SimCellToGameCell.
    pub fn game_cell(&self, i: usize) -> i32 {
        let w = self.width as usize;
        let (x, y) = (i % w, i / w);
        (y as i32 - 1) * (self.width - 2) + x as i32 - 1
    }

    /// SimData::ClearCell on `updated`.
    pub fn clear_cell(&mut self, i: usize) {
        let u = &mut self.updated;
        u.element[i] = self.vacuum_element;
        u.mass[i] = 0.0;
        u.temperature[i] = 0.0;
        u.disease[i] = 0xff;
        u.disease_count[i] = 0;
        u.infestation_ticks[i] = 0;
        u.growth_error[i] = 0.0;
    }

    /// CellAccessor::ModifyDiseaseCount on `updated`.
    pub fn modify_disease_count(&mut self, i: usize, delta: i32) {
        let u = &mut self.updated;
        u.disease_count[i] = u.disease_count[i].wrapping_add(delta);
        if u.disease_count[i] <= 0 {
            u.disease[i] = 0xff;
            u.disease_count[i] = 0;
            u.infestation_ticks[i] = 0;
            u.growth_error[i] = 0.0;
        }
    }

    fn boundary_cell(c: &mut Cells, i: usize, element: u16) {
        c.element[i] = element;
        c.mass[i] = 9999.0;
        c.temperature[i] = 0.0;
        c.disease_count[i] = 0;
        c.disease[i] = 0xff;
    }

    /// SimData::InitializeBoundary: unobtanium on the bottom row and the side
    /// columns, vacuum on the top row, in both grids; backwall cleared.
    pub fn initialize_boundary(&mut self) {
        let (w, h) = (self.width as usize, self.height as usize);
        let (unob, vac) = (self.unobtanium_element, self.vacuum_element);
        for x in 0..w {
            let b = x;
            Self::boundary_cell(&mut self.cells, b, unob);
            Self::boundary_cell(&mut self.updated, b, unob);
            self.backwall.clear(b);
            let t = (h - 1) * w + x;
            Self::boundary_cell(&mut self.cells, t, vac);
            Self::boundary_cell(&mut self.updated, t, vac);
            self.backwall.clear(t);
        }
        for y in 1..h - 1 {
            for i in [y * w, y * w + w - 1] {
                Self::boundary_cell(&mut self.cells, i, unob);
                Self::boundary_cell(&mut self.updated, i, unob);
                self.backwall.clear(i);
            }
        }
    }
}

fn count_cells(w: i32, h: i32) -> usize {
    (w as usize) * (h as usize)
}

fn element_index(elements: &[Element], name: &str) -> Result<u16, String> {
    let hash = tables::element_hash(name);
    elements.iter().position(|e| e.id == hash).map(|i| i as u16).ok_or_else(|| format!("element table lacks {name}"))
}

impl Sim {
    pub fn new(stream: &Stream) -> Result<Self, String> {
        let mut sim = Sim { stream: stream.clone(), next_message: 0, tick: 0, elapsed: 0.0, managed: None, elements: Vec::new(), diseases: Vec::new(), data: None, started: false, temperature_table: Vec::new(), pressure_table: Vec::new(), liquid_table: Vec::new(), radiation_table: Vec::new(), events: Default::default() };
        // Messages delivered before the first frame (tables, cells, Start).
        while sim.next_message < sim.stream.messages.len() && sim.stream.messages[sim.next_message].frame == 0 {
            let m = sim.stream.messages[sim.next_message].clone();
            sim.handle(m.id, &m.payload)?;
            sim.next_message += 1;
        }
        if !sim.started {
            return Err("stream does not start the simulation".into());
        }
        Ok(sim)
    }

    fn handle(&mut self, id: i32, payload: &[u8]) -> Result<(), String> {
        match id {
            messages::ELEMENTS => {
                self.elements = tables::parse_elements(payload)?;
                self.temperature_table = crate::temperature::build_table(&self.elements);
                self.pressure_table = crate::gas::build_table(&self.elements);
                self.liquid_table = crate::liquid::build_table(&self.elements);
                self.radiation_table = crate::radiation::build_table(&self.elements);
            }
            messages::DISEASES => self.diseases = tables::parse_diseases(payload, self.elements.len())?,
            messages::INITIALIZE_CELLS => {
                self.initialize_from_cells(payload)?;
                self.managed = Some(crate::driver::Managed::from_initialize(payload)?);
            }
            messages::START => self.start()?,
            _ => return Err(format!("unsupported message {id}")),
        }
        Ok(())
    }

    /// SimData::InitializeFromCells: the input grid goes into `updated` at the
    /// padded index; `cells` keeps constructor defaults until Start.
    fn initialize_from_cells(&mut self, payload: &[u8]) -> Result<(), String> {
        let mut r = Reader::new(payload);
        let (w, h) = (r.i32()?, r.i32()?);
        if w <= 0 || h <= 0 || (w as i64) * (h as i64) > 10_000_000 {
            return Err("invalid grid size".into());
        }
        let seed = r.u32()?;
        let radiation = r.u8()? != 0;
        let headless = r.u8()? != 0;
        let (pw, ph) = (w + 2, h + 2);
        let n = (pw * ph) as usize;
        let vacuum = element_index(&self.elements, "Vacuum")?;
        let mut d = SimData {
            width: pw,
            height: ph,
            num_game_cells: w * h,
            random_seed: seed,
            iterate_direction: -1,
            displacement_direction: -1,
            saved_options: 0,
            void_element: element_index(&self.elements, "Void")?,
            vacuum_element: vacuum,
            unobtanium_element: element_index(&self.elements, "Unobtanium")?,
            cells: Cells::new(n),
            updated: Cells::new(n),
            backwall: Backwall { vacuum_element: vacuum, element: vec![0; n], mass: vec![0.0; n], temperature: vec![0.0; n] },
            stable_ticks: vec![0x1f; n],
            tick_count: 0,
            headless,
            radiation_enabled: radiation,
            visible: vec![0; count_cells(w, h)],
            flow: vec![[0.0; 4]; n],
            accumulated_flow: vec![0.0; n],
            radiation_occlusion: Vec::new(),
        };
        let count = (w * h) as usize;
        let cells = r.take(16 * count)?;
        let disease = r.take(12 * count)?;
        let backwall = r.take(12 * count)?;
        let pad = |gi: usize| ((gi / w as usize) + 1) * pw as usize + (gi % w as usize) + 1;
        for gi in 0..count {
            let i = pad(gi);
            let c = &cells[16 * gi..16 * gi + 16];
            let u = &mut d.updated;
            u.element[i] = u16::from_le_bytes([c[0], c[1]]);
            u.properties[i] = c[2];
            u.insulation[i] = c[3];
            u.strength[i] = c[4];
            u.temperature[i] = f32::from_le_bytes(c[8..12].try_into().unwrap());
            u.mass[i] = f32::from_le_bytes(c[12..16].try_into().unwrap());
            u.radiation[i] = 0.0;
            let dc = &disease[12 * gi..12 * gi + 12];
            u.disease[i] = dc[0];
            u.infestation_ticks[i] = dc[1];
            u.disease_count[i] = i32::from_le_bytes(dc[4..8].try_into().unwrap());
            u.growth_error[i] = 0.0;
            let b = &backwall[12 * gi..12 * gi + 12];
            d.backwall.element[i] = u16::from_le_bytes([b[0], b[1]]);
            d.backwall.mass[i] = f32::from_le_bytes(b[4..8].try_into().unwrap());
            d.backwall.temperature[i] = f32::from_le_bytes(b[8..12].try_into().unwrap());
        }
        // SettleThermalBoundaries reads `cells`, which holds no elements yet: no effect.
        d.initialize_boundary();
        self.data = Some(d);
        Ok(())
    }

    fn start(&mut self) -> Result<(), String> {
        let d = self.data.as_mut().ok_or("Start before cell initialization")?;
        d.initialize_boundary();
        let (cells, updated) = (&mut d.cells, &d.updated);
        cells.copy_from(updated);
        self.started = true;
        Ok(())
    }

    pub fn frame_message(&self) -> &[u8] {
        &self.stream.frame
    }

    pub fn tick(&self) -> i32 {
        self.tick
    }

    pub fn snapshot(&self) -> Snapshot {
        let d = self.data.as_ref().expect("initialized");
        let mut s = Snapshot::new(d.width, d.height, self.tick);
        s.scalar("num_game_cells", d.num_game_cells as i64);
        s.scalar("random_seed", d.random_seed as i64);
        s.scalar("iterate_direction", d.iterate_direction as i64);
        s.scalar("displacement_direction", d.displacement_direction as i64);
        s.scalar("saved_options", d.saved_options as i64);
        s.scalar("void_element", d.void_element as i64);
        s.scalar("vacuum_element", d.vacuum_element as i64);
        s.scalar("unobtanium_element", d.unobtanium_element as i64);
        s.scalar("tick_count", d.tick_count as i64);
        s.scalar("headless", d.headless as i64);
        s.scalar("radiation_enabled", d.radiation_enabled as i64);
        s.scalar("backwall_vacuum_element", d.backwall.vacuum_element as i64);
        let c = &d.updated;
        s.field("element", &c.element);
        s.field("temperature", &c.temperature);
        s.field("mass", &c.mass);
        s.field("properties", &c.properties);
        s.field("insulation", &c.insulation);
        s.field("strength", &c.strength);
        s.field("disease", &c.disease);
        s.field("disease_count", &c.disease_count);
        s.field("infestation_ticks", &c.infestation_ticks);
        s.field("growth_error", &c.growth_error);
        s.field("radiation", &c.radiation);
        s.field("backwall_element", &d.backwall.element);
        s.field("backwall_mass", &d.backwall.mass);
        s.field("backwall_temperature", &d.backwall.temperature);
        s.field("stable_ticks", &d.stable_ticks);
        s
    }

    /// `snapshot` plus scratch grid and visibility, matching the harness's
    /// phase snapshots (flow and occlusion are not modelled yet).
    pub fn snapshot_full(&self) -> Snapshot {
        let mut s = self.snapshot();
        let d = self.data.as_ref().expect("initialized");
        let c = &d.cells;
        s.field("scratch_element", &c.element);
        s.field("scratch_temperature", &c.temperature);
        s.field("scratch_mass", &c.mass);
        s.field("scratch_properties", &c.properties);
        s.field("scratch_insulation", &c.insulation);
        s.field("scratch_strength", &c.strength);
        s.field("scratch_disease", &c.disease);
        s.field("scratch_disease_count", &c.disease_count);
        s.field("scratch_infestation_ticks", &c.infestation_ticks);
        s.field("scratch_growth_error", &c.growth_error);
        s.field("scratch_radiation", &c.radiation);
        s.field("visible", &d.visible);
        let flow: Vec<f32> = d.flow.iter().flat_map(|v| v.iter().copied()).collect();
        s.field("flow", &flow);
        s.field("accumulated_flow", &d.accumulated_flow);
        s.field("radiation_occlusion", &d.radiation_occlusion);
        s
    }

    /// SIM_BeginSave: SIMSAVE v15 bytes (without the managed length prefix).
    /// Like the native save, rewrites unset (0xFFFF) backwall cells to vacuum.
    pub fn save(&mut self, x: i32, y: i32) -> Vec<u8> {
        let d = self.data.as_mut().expect("initialized");
        let n = d.len();
        let mut out = Vec::with_capacity(29 + 36 * n);
        out.extend_from_slice(b"SIMSAVE\0");
        out.extend(15_u32.to_le_bytes());
        out.extend(d.width.to_le_bytes());
        out.extend(d.height.to_le_bytes());
        out.extend(x.to_le_bytes());
        out.extend(y.to_le_bytes());
        out.push(d.saved_options);
        let c = &d.updated;
        for i in 0..n {
            out.extend(self.elements[c.element[i] as usize].id.to_le_bytes());
            out.extend(c.temperature[i].to_le_bytes());
            out.extend(c.mass[i].to_le_bytes());
            out.extend(c.radiation[i].to_le_bytes());
        }
        for i in 0..n {
            let hash = if c.disease[i] == 0xff { 0 } else { self.diseases[c.disease[i] as usize].hash };
            out.extend(hash.to_le_bytes());
            out.extend(c.disease_count[i].to_le_bytes());
        }
        for i in 0..n {
            if d.backwall.element[i] == 0xffff {
                d.backwall.clear(i);
            }
            out.extend(self.elements[d.backwall.element[i] as usize].id.to_le_bytes());
            out.extend(d.backwall.mass[i].to_le_bytes());
            out.extend(d.backwall.temperature[i].to_le_bytes());
        }
        out
    }
}
