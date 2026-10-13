//! The per-tick driver (Sim::Main / SimBase::Step / SimFrameManager): frame
//! delivery, per-frame messages (template cell writes), the UpdateData
//! prologue and epilogue, and the managed "last substance change" view.
use crate::fp::{cvttss2si, F};
use crate::messages::{self, Message};
use crate::phases::PHASES;
use crate::snapshot::Snapshot;
use crate::Sim;

/// The managed foreground grid as the game's settle keeps it: the initial
/// cells, updated only for cells named in substance-change notifications.
/// This is the grid ambient (critter/plant) spawning reads.
#[derive(Clone, Debug, Default)]
pub struct Managed {
    pub element: Vec<u16>,
    pub mass: Vec<f32>,
    pub temperature: Vec<f32>,
    pub disease: Vec<u8>,
    pub disease_count: Vec<i32>,
    /// Notifications applied so far (each cell counted once per tick).
    pub applied: u64,
    /// Per game cell: the last tick (plus one) it was applied in, so a tick's
    /// repeated notifications for a cell count once without allocating.
    stamp: Vec<u32>,
}

impl Managed {
    pub fn from_initialize(payload: &[u8]) -> Result<Self, String> {
        let w = i32::from_le_bytes(payload[0..4].try_into().unwrap()) as usize;
        let h = i32::from_le_bytes(payload[4..8].try_into().unwrap()) as usize;
        let n = w * h;
        if payload.len() < 14 + 40 * n {
            return Err("invalid initialization grid length".into());
        }
        let mut m = Managed { stamp: vec![0; n], ..Default::default() };
        for i in 0..n {
            let c = &payload[14 + 16 * i..14 + 16 * (i + 1)];
            m.element.push(u16::from_le_bytes([c[0], c[1]]));
            m.temperature.push(f32::from_le_bytes(c[8..12].try_into().unwrap()));
            m.mass.push(f32::from_le_bytes(c[12..16].try_into().unwrap()));
            let d = &payload[14 + 16 * n + 12 * i..14 + 16 * n + 12 * (i + 1)];
            m.disease.push(d[0]);
            m.disease_count.push(i32::from_le_bytes(d[4..8].try_into().unwrap()));
        }
        Ok(m)
    }

    pub fn snapshot(&self, width: i32, height: i32, tick: i32) -> Snapshot {
        // `stamp` is bookkeeping, not state, and is left out.
        let mut s = Snapshot::new(width, height, tick);
        s.scalar("applied_changes", self.applied as i64);
        s.field("element", &self.element);
        s.field("mass", &self.mass);
        s.field("temperature", &self.temperature);
        s.field("disease", &self.disease);
        s.field("disease_count", &self.disease_count);
        s
    }
}

/// One CellModification message (28 bytes).
struct CellModification {
    game_cell: i32,
    temperature: f32,
    mass: f32,
    disease_count: i32,
    element: u16,
    replace_type: u8,
    disease: u8,
}

fn parse_modification(p: &[u8]) -> Result<CellModification, String> {
    if p.len() < 25 {
        return Err("short cell modification".into());
    }
    let f = |o: usize| f32::from_le_bytes(p[o..o + 4].try_into().unwrap());
    let i = |o: usize| i32::from_le_bytes(p[o..o + 4].try_into().unwrap());
    Ok(CellModification {
        game_cell: i(0),
        temperature: f(8),
        mass: f(12),
        disease_count: i(16),
        element: u16::from_le_bytes([p[20], p[21]]),
        replace_type: p[22],
        disease: p[23],
    })
}

impl Sim {
    /// SimData::GameCellToSimCell.
    fn sim_cell(&self, game: i32) -> usize {
        let d = self.data.as_ref().unwrap();
        let gw = d.width - 2;
        (((game / gw) + 1) * d.width + game % gw + 1) as usize
    }

    /// ProcessCellModifications for one message (replace type 1 only, which
    /// is what template cells use).
    fn apply_cell_modification(&mut self, payload: &[u8]) -> Result<(), String> {
        let m = parse_modification(payload)?;
        let (mut t, mut mass) = (m.temperature, m.mass);
        let report = if t > 0.0 { t > 10000.0 } else { mass > 0.0 };
        if report {
            t = crate::fp::minss(t, 10000.0);
            if !(t > 0.0) {
                t = 0.0;
            }
            if !(mass > 0.0) {
                mass = 0.0;
                t = 0.0;
            }
        }
        if m.replace_type != 1 {
            return Err(format!("cell modification replace type {} is not modelled", m.replace_type));
        }
        let cell = self.sim_cell(m.game_cell);
        let vacuum = self.elements[m.element as usize].phase() == 0;
        let d = self.data.as_mut().unwrap();
        // ReplaceElement.
        let u = &mut d.updated;
        u.element[cell] = m.element;
        u.temperature[cell] = if vacuum { 0.0 } else { t };
        u.mass[cell] = if vacuum { 0.0 } else { mass };
        self.events.change_substance(d, cell);
        let u = &mut d.updated;
        u.disease[cell] = m.disease;
        u.disease_count[cell] = m.disease_count;
        u.infestation_ticks[cell] = 0;
        u.growth_error[cell] = 0.0;
        Ok(())
    }

    /// ModifyBackwallData: written raw into the backwall layer.
    fn apply_backwall(&mut self, p: &[u8]) -> Result<(), String> {
        if p.len() < 16 {
            return Err("short backwall message".into());
        }
        let cell = self.sim_cell(i32::from_le_bytes(p[0..4].try_into().unwrap()));
        let d = self.data.as_mut().unwrap();
        d.backwall.element[cell] = u16::from_le_bytes([p[4], p[5]]);
        d.backwall.mass[cell] = f32::from_le_bytes(p[8..12].try_into().unwrap());
        d.backwall.temperature[cell] = f32::from_le_bytes(p[12..16].try_into().unwrap());
        Ok(())
    }

    /// Everything from the end of the previous tick up to the first phase
    /// boundary of this one: flow reset, visibility, the frame's queued
    /// messages, the step-time accumulator and the UpdateData prologue.
    /// Returns how many UpdateData calls this frame needs.
    pub(crate) fn begin_frame(&mut self) -> Result<u32, String> {
        let k = self.tick;
        let pending: Vec<Message> = self.take_frame_messages(k);
        {
            let d = self.data.as_mut().ok_or("not initialized")?;
            for f in d.flow.iter_mut() {
                *f = [0.0; 4];
            }
            // The visibility grid a frame sees comes from the prepare two sim
            // iterations earlier: nothing on the first frame, then the
            // all-visible grids the settle sends.
            let v = if k == 0 { 0 } else { 255 };
            d.visible.iter_mut().for_each(|x| *x = v);
        }
        self.events.clear();
        // ProcessFrame order: cell modifications, then (with element chunks)
        // backwall data.
        for m in pending.iter().filter(|m| m.id == messages::MODIFY_CELL) {
            self.apply_cell_modification(&m.payload)?;
        }
        for m in pending.iter().filter(|m| m.id == messages::BACKWALL) {
            self.apply_backwall(&m.payload)?;
        }
        if let Some(m) = pending.iter().find(|m| m.id != messages::MODIFY_CELL && m.id != messages::BACKWALL) {
            return Err(format!("per-frame message {} is not modelled", m.id));
        }
        let dt = F(f32::from_le_bytes(self.frame_message()[0..4].try_into().unwrap()));
        if !(dt > 0.0) {
            return Err("frames without elapsed time are not modelled".into());
        }
        let acc = dt + self.elapsed;
        let n = cvttss2si((acc * 5.0).v()).max(1);
        let left = acc - F::from_i32(n) * 0.2;
        self.elapsed = if left > 0.0 { left.v() } else { 0.0 };
        Ok(n as u32)
    }

    /// UpdateData prologue (L276-296).
    pub(crate) fn update_prologue(&mut self) {
        let d = self.data.as_mut().unwrap();
        d.iterate_direction = -d.iterate_direction;
        let (cells, updated) = (&mut d.cells, &d.updated);
        cells.copy_from(updated);
    }

    fn take_frame_messages(&mut self, frame: i32) -> Vec<Message> {
        let mut out = Vec::new();
        while self.next_message < self.stream.messages.len() && self.stream.messages[self.next_message].frame as i32 == frame {
            out.push(self.stream.messages[self.next_message].clone());
            self.next_message += 1;
        }
        out
    }

    /// Run one tick: one frame message and the UpdateData calls it drives.
    pub fn step(&mut self) -> Result<(), String> {
        let n = self.begin_frame()?;
        for _ in 0..n {
            self.update_prologue();
            for phase in &PHASES[1..PHASES.len() - 1] {
                self.run_phase(phase)?;
            }
            let d = self.data.as_mut().unwrap();
            d.tick_count = d.tick_count.wrapping_add(1);
        }
        self.apply_managed();
        self.tick += 1;
        Ok(())
    }

    /// Copy the cells named by this tick's substance-change notifications
    /// into the managed view (as the game does from GameData each prepare).
    fn apply_managed(&mut self) {
        let Some(m) = self.managed.as_mut() else { return };
        let d = self.data.as_ref().unwrap();
        let stamp = self.tick as u32 + 1;
        for &g in &self.events.substance_changes {
            if m.stamp[g as usize] == stamp {
                continue;
            }
            m.stamp[g as usize] = stamp;
            let gw = d.width - 2;
            let i = (((g / gw) + 1) * d.width + g % gw + 1) as usize;
            let g = g as usize;
            m.element[g] = d.updated.element[i];
            m.mass[g] = d.updated.mass[i];
            m.temperature[g] = d.updated.temperature[i];
            m.disease[g] = d.updated.disease[i];
            m.disease_count[g] = d.updated.disease_count[i];
            m.applied += 1;
        }
    }

    /// Advance until `tick` frames have been processed.
    pub fn advance(&mut self, tick: i32) -> Result<(), String> {
        while self.tick < tick {
            self.step()?;
        }
        Ok(())
    }

    /// The managed view snapshot (game-sized grid).
    pub fn managed_snapshot(&self) -> Option<Snapshot> {
        let d = self.data.as_ref()?;
        self.managed.as_ref().map(|m| m.snapshot(d.width - 2, d.height - 2, self.tick))
    }
}

impl Sim {
    /// Position the driver after `tick` processed frames (to resume from a
    /// saved state).
    pub fn set_tick(&mut self, tick: i32) {
        self.tick = tick;
        self.next_message = self.stream.messages.iter().position(|m| m.frame as i32 >= tick.max(1)).unwrap_or(self.stream.messages.len());
    }

    /// From the end of one tick to the first phase boundary of the next.
    pub fn run_frame_start(&mut self) -> Result<(), String> {
        let n = self.begin_frame()?;
        if n != 1 {
            return Err(format!("{n} UpdateData calls in one frame are not checked"));
        }
        self.update_prologue();
        Ok(())
    }
}
