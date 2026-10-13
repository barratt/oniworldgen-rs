//! Settle generated worlds with the Rust port of the game's cell simulation
//! (`simcore`) and place ambient mobs from the settled state, in pure Rust.
//!
//! Inputs come straight from the generator: the cluster run (world grids and
//! template cell writes), the element table built from the game's element
//! YAML and the transcribed germ table.
use oni_worldgen::dlc::ActiveDlc;
use oni_worldgen::sim_transport::{self, Prediction, Profile};
use serde_json::{json, Value};
use simcore::messages::{Message, Stream};
use std::collections::BTreeMap;

pub const GAME_VERSION: &str = "744825";
/// Ticks in the worldgen settle (WorldGenSimUtil.DoSettleSim).
pub const SETTLE_TICKS: i32 = 500;

/// Static simulation tables for one DLC set.
pub struct Tables {
    pub entries: Vec<simcore::element_yaml::Entry>,
    pub elements_payload: Vec<u8>,
    pub diseases_payload: Vec<u8>,
    pub radiation: bool,
    pub active_dlc_ids: Vec<String>,
}

impl Tables {
    /// `element_yaml`: the texts of StreamingAssets `elements/*.yaml`, in file
    /// name order. Radiation (and its germ) exists only with Spaced Out.
    pub fn new(element_yaml: &[&str], scope: &ActiveDlc) -> Result<Self, String> {
        let ids = scope.ids();
        let mut dlcs: Vec<&str> = vec![""];
        dlcs.extend(ids.iter().map(String::as_str));
        let entries = simcore::element_yaml::collect(element_yaml, &dlcs)?;
        let radiation = scope.expansion1;
        Ok(Tables {
            elements_payload: simcore::element_yaml::payload(&entries),
            diseases_payload: simcore::disease_table::payload(&entries, radiation),
            entries,
            radiation,
            active_dlc_ids: ids.iter().map(|s| s.to_string()).collect(),
        })
    }

    /// The table description `sim_transport::build` consumes.
    fn profile(&self) -> Profile {
        let element_table = self.entries.iter().enumerate().map(|(i, e)| (i.to_string(), e.id.clone())).collect();
        let disease_table = simcore::disease_table::DISEASES
            .iter()
            .filter(|d| self.radiation || d.id != "RadiationSickness")
            .enumerate()
            .map(|(i, d)| (i.to_string(), d.id.to_string()))
            .collect();
        Profile {
            schema_version: 1,
            game_version: GAME_VERSION.into(),
            active_dlc_ids: self.active_dlc_ids.clone(),
            radiation: self.radiation,
            mxcsr: 0x1f80,
            element_table,
            disease_table,
            elements_payload: sim_transport::encode_hex(&self.elements_payload),
            diseases_payload: sim_transport::encode_hex(&self.diseases_payload),
        }
    }

    pub fn element_name(&self, idx: u16) -> &str {
        &self.entries[idx as usize].id
    }
}

/// Read `elements/*.yaml` from a StreamingAssets directory, in name order.
#[cfg(not(target_arch = "wasm32"))]
pub fn element_yaml_from_assets(assets: &std::path::Path) -> Result<Vec<String>, String> {
    let dir = assets.join("elements");
    let mut paths: Vec<_> = std::fs::read_dir(&dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "yaml") && !p.file_name().unwrap().to_string_lossy().starts_with('.'))
        .collect();
    paths.sort();
    paths.iter().map(|p| std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))).collect()
}

/// `elements/*.yaml` from a bundle's file map (the WASM data source).
pub fn element_yaml_from_files(files: &BTreeMap<String, String>) -> Vec<String> {
    files.iter().filter(|(k, _)| k.starts_with("elements/") && k.ends_with(".yaml") && !k[9..].starts_with('.')).map(|(_, v)| v.clone()).collect()
}

/// One world's simulation input.
pub struct WorldInput {
    pub name: String,
    pub width: i32,
    pub height: i32,
    pub world_seed: i32,
    pub save_x: i32,
    pub save_y: i32,
    pub stream: Stream,
}

/// Build every world's message stream from the generator's output
/// (`simulate::prediction` of a cluster run) and the tables.
pub fn world_inputs(prediction: &Value, tables: &Tables) -> Result<Vec<WorldInput>, String> {
    use serde::Deserialize;
    let inputs = Prediction::deserialize(prediction).map_err(|e| e.to_string())?;
    let transport = sim_transport::build(&tables.profile(), &inputs)?;
    transport
        .worlds
        .iter()
        .map(|w| {
            let messages = w
                .messages
                .iter()
                .map(|m| Ok(Message { frame: m.frame, id: m.id, payload: sim_transport::decode_hex(&m.payload)? }))
                .collect::<Result<Vec<_>, String>>()?;
            Ok(WorldInput {
                name: w.name.clone(),
                width: w.width,
                height: w.height,
                world_seed: w.world_seed,
                save_x: w.save_x,
                save_y: w.save_y,
                stream: Stream { width: w.width, height: w.height, frame: sim_transport::decode_hex(&w.frame_message)?, messages },
            })
        })
        .collect()
}

/// Steppable settle of a whole cluster.
pub struct Settler {
    pub worlds: Vec<WorldInput>,
    pub sims: Vec<simcore::Sim>,
}

impl Settler {
    pub fn new(worlds: Vec<WorldInput>) -> Result<Self, String> {
        let sims = worlds.iter().map(|w| simcore::Sim::new(&w.stream)).collect::<Result<Vec<_>, _>>()?;
        Ok(Settler { worlds, sims })
    }
    /// Processed ticks (equal for every world).
    pub fn tick(&self) -> i32 {
        self.sims.first().map(|s| s.tick()).unwrap_or(0)
    }
    /// Advance every world to `tick` (at most the settle length).
    pub fn advance(&mut self, tick: i32) -> Result<(), String> {
        let tick = tick.min(SETTLE_TICKS);
        for s in &mut self.sims {
            s.advance(tick)?;
        }
        Ok(())
    }
    /// Unpadded live cells of world `i` at the current tick.
    pub fn cells(&self, i: usize) -> Cells {
        Cells::from_sim(&self.sims[i])
    }
    /// The managed (substance-change) element indices of world `i`.
    pub fn managed_elements(&self, i: usize) -> Vec<u16> {
        self.sims[i].managed.as_ref().map(|m| m.element.clone()).unwrap_or_default()
    }
}

/// Unpadded per-cell state of one world (row-major, game cell order).
#[derive(Clone, Debug, Default)]
pub struct Cells {
    pub width: i32,
    pub height: i32,
    pub element: Vec<u16>,
    pub mass: Vec<f32>,
    pub temperature: Vec<f32>,
    /// 255 = no germs.
    pub disease: Vec<u8>,
    pub disease_count: Vec<i32>,
}

impl Cells {
    pub fn from_sim(sim: &simcore::Sim) -> Cells {
        let d = sim.data.as_ref().expect("initialized");
        let (pw, w, h) = (d.width as usize, d.width as usize - 2, d.height as usize - 2);
        let mut c = Cells { width: w as i32, height: h as i32, ..Default::default() };
        let n = w * h;
        c.element.reserve(n);
        for y in 0..h {
            let row = (y + 1) * pw + 1;
            c.element.extend_from_slice(&d.updated.element[row..row + w]);
            c.mass.extend_from_slice(&d.updated.mass[row..row + w]);
            c.temperature.extend_from_slice(&d.updated.temperature[row..row + w]);
            c.disease.extend_from_slice(&d.updated.disease[row..row + w]);
            c.disease_count.extend_from_slice(&d.updated.disease_count[row..row + w]);
        }
        c
    }
}

/// One settled world.
pub struct SettledWorld {
    pub name: String,
    pub world_seed: i32,
    pub width: i32,
    pub height: i32,
    pub offset: (i32, i32),
    /// SIMSAVE bytes with the managed i32 length prefix (what the game saves and loads).
    pub saved: Vec<u8>,
    /// Final cells (decoded from the save, padding removed).
    pub cells: Cells,
    /// Managed view element indices at the observation endpoint (for ambient spawning).
    pub observed_elements: Vec<u16>,
}

/// Settle one world: advance to the observation endpoint, record the
/// managed view, finish the settle and save.
pub fn settle_world(w: WorldInput, frames: i32) -> Result<SettledWorld, String> {
    let mut sim = simcore::Sim::new(&w.stream)?;
    sim.advance(frames)?;
    let observed_elements = sim.managed.as_ref().map(|m| m.element.clone()).unwrap_or_default();
    sim.advance(SETTLE_TICKS)?;
    let save = sim.save(w.save_x, w.save_y);
    let mut saved = (save.len() as i32).to_le_bytes().to_vec();
    saved.extend_from_slice(&save);
    Ok(SettledWorld {
        name: w.name,
        world_seed: w.world_seed,
        width: w.width,
        height: w.height,
        offset: (w.save_x, w.save_y),
        cells: Cells::from_sim(&sim),
        saved,
        observed_elements,
    })
}

/// Run the full settle. `frames` is the ambient observation endpoint (499 or
/// 500 processed frames). Worlds are
/// independent, so native builds settle them on parallel threads (results do
/// not depend on scheduling); WASM settles them in turn.
pub fn settle(worlds: Vec<WorldInput>, frames: i32) -> Result<Vec<SettledWorld>, String> {
    if !matches!(frames, 499 | 500) {
        return Err("--frames must be 499 or 500".into());
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1).max(1);
        // Largest worlds first so the longest settles start early.
        let mut order: Vec<usize> = (0..worlds.len()).collect();
        order.sort_by_key(|&i| std::cmp::Reverse(worlds[i].width * worlds[i].height));
        let jobs = std::sync::Mutex::new(order.into_iter());
        let slots: Vec<std::sync::Mutex<Option<WorldInput>>> = worlds.into_iter().map(|w| std::sync::Mutex::new(Some(w))).collect();
        let results: Vec<std::sync::Mutex<Option<Result<SettledWorld, String>>>> = slots.iter().map(|_| std::sync::Mutex::new(None)).collect();
        std::thread::scope(|scope| {
            for _ in 0..threads.min(slots.len()) {
                scope.spawn(|| loop {
                    let Some(i) = jobs.lock().unwrap().next() else { break };
                    let w = slots[i].lock().unwrap().take().unwrap();
                    *results[i].lock().unwrap() = Some(settle_world(w, frames));
                });
            }
        });
        results.into_iter().map(|r| r.into_inner().unwrap().unwrap()).collect()
    }
    #[cfg(target_arch = "wasm32")]
    {
        worlds.into_iter().map(|w| settle_world(w, frames)).collect()
    }
}

/// Struct-of-arrays grid with name palettes (palettes sorted by name;
/// `disease` null where a cell has no germs).
pub fn cells_json(cells: &Cells, tables: &Tables) -> Value {
    let mut names: Vec<&str> = cells.element.iter().map(|&e| tables.element_name(e)).collect();
    names.sort_unstable();
    names.dedup();
    let index: BTreeMap<&str, usize> = names.iter().enumerate().map(|(i, n)| (*n, i)).collect();
    let disease_ids: Vec<&str> = simcore::disease_table::DISEASES.iter().filter(|d| tables.radiation || d.id != "RadiationSickness").map(|d| d.id).collect();
    let mut dnames: Vec<&str> = cells.disease.iter().filter(|&&d| d != 255).map(|&d| disease_ids[d as usize]).collect();
    dnames.sort_unstable();
    dnames.dedup();
    let dindex: BTreeMap<&str, usize> = dnames.iter().enumerate().map(|(i, n)| (*n, i)).collect();
    json!({
        "elements": names,
        "element": cells.element.iter().map(|&e| index[tables.element_name(e)]).collect::<Vec<_>>(),
        "mass": cells.mass,
        "temperature": cells.temperature,
        "diseases": dnames,
        "disease": cells.disease.iter().map(|&d| if d == 255 { Value::Null } else { json!(dindex[disease_ids[d as usize]]) }).collect::<Vec<_>>(),
        "disease_count": cells.disease_count,
    })
}

/// Ambient (critter and plant) placement from the settled managed views,
/// through rust-worldgen's ambient pipeline.
pub fn ambient(
    ds: &oni_worldgen::data_source::DataSource,
    coordinate: &str,
    explicit_dlc: Option<ActiveDlc>,
    prediction: &Value,
    settled: &[SettledWorld],
    tables: &Tables,
) -> Result<Vec<oni_worldgen::ambient_pipeline::Output>, String> {
    let observed: Vec<_> = settled
        .iter()
        .map(|w| oni_worldgen::ambient_pipeline::Observed {
            name: w.name.clone(),
            world_seed: w.world_seed,
            width: w.width,
            height: w.height,
            elements: w.observed_elements.iter().map(|&e| tables.element_name(e).to_string()).collect(),
        })
        .collect();
    oni_worldgen::ambient_pipeline::generate_cluster(ds, coordinate, explicit_dlc, Some(prediction), &observed)
}

/// The document's `simulation` section (the engine fields name this crate).
#[allow(clippy::too_many_arguments)]
pub fn simulation_section(
    coordinate: &str,
    frames: i32,
    settled: &[SettledWorld],
    ambient: &[oni_worldgen::ambient_pipeline::Output],
    tables: &Tables,
    milliseconds: Value,
) -> Value {
    let worlds: Vec<Value> = settled
        .iter()
        .map(|w| {
            json!({"name": w.name, "world_seed": w.world_seed, "width": w.width, "height": w.height, "offset": [w.offset.0, w.offset.1],
                "saved_bytes": {"length": w.saved.len(), "sha256": oni_worldgen::sha256::hex(&w.saved)}, "cells": cells_json(&w.cells, tables)})
        })
        .collect();
    json!({
        "frames": frames, "settleFrames": SETTLE_TICKS,
        "observationEndpoint": if frames == 499 { "managed_spawn_state_499" } else { "managed_spawn_state" },
        "engine": {"name": "simcore", "gameVersion": GAME_VERSION, "activeDlcIds": tables.active_dlc_ids, "radiation": tables.radiation},
        "scope": "Rust port of the game's cell simulation replayed from Rust-built inputs; cells are decoded from each world's saved simulation bytes (padding border removed).",
        "worlds": worlds,
        "ambient": oni_worldgen::ambient_pipeline::document(coordinate, frames as u32, ambient),
        "milliseconds": milliseconds,
    })
}
