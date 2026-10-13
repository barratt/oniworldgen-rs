//! The `oniworldgen` npm package: generation-only worldgen as WebAssembly.
//!
//! One coordinate in, a `MapData`-shaped document out (typed in
//! `npm/index.d.ts`). It runs the summary tier of the generator
//! (`oni_worldgen::compact`, `Tier::Summary`): layout, biome polygons,
//! template placement, template entities, geysers and the starmap. No cell
//! grids are built for that and nothing is simulated, so ambient (critter
//! and plant) spawns are not part of the output, with one exception: ambient
//! `OilWell`s (oil reservoirs), estimated on the unsettled grid
//! (`oni_worldgen::ambient_oil`). Their positions are approximate and,
//! rarely, their count is off by one; every other entity is placed by a
//! template and is exact.
//!
//! Numbers that are `f32` in the game (polygon vertices, cell centres,
//! geyser rolls) are written widened to `f64`.

use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;

use oni_worldgen::ambient_oil::OIL_WELL;
use oni_worldgen::compact::{self, Tier, TierRun};
use oni_worldgen::data_source::DataSource;
use oni_worldgen::diagnostics::{Event, RunError};
use oni_worldgen::dlc::ActiveDlc;
use oni_worldgen::template_entities::Prefab;
use serde_json::{json, Map, Value};

#[cfg(target_arch = "wasm32")]
mod wasm;

/// The game build the embedded data comes from.
pub const GAME_BUILD: &str = "744825";
/// Package version; `version()` returns `"<GAME_BUILD>+<PACKAGE_VERSION>"`.
/// scripts/build-npm.sh sets ONIWORLDGEN_PKG_VERSION when it is given a
/// version other than the crate's.
pub const PACKAGE_VERSION: &str = match option_env!("ONIWORLDGEN_PKG_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};

pub fn version() -> String { format!("{GAME_BUILD}+{PACKAGE_VERSION}") }

/// Which template entities go into `buildings` and `other_entities`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Entities {
    /// Every template-placed building and other entity (geysers are listed
    /// in `geysers`, not in `other_entities`).
    All,
    /// Only the tags a map view needs: the points of interest, `OilWell`
    /// and `UnderwaterVent`. Much smaller JSON.
    Map,
}

/// Points of interest and map geysers (`OilWell`, `UnderwaterVent`).
pub const MAP_TAGS: &[&str] = &[
    "Headquarters", "WarpConduitSender", "WarpConduitReceiver", "WarpPortal", "WarpReceiver", "GeneShuffler",
    "MassiveHeatSink", "SapTree", "GravitasPedestal", "PropSurfaceSatellite1", "PropSurfaceSatellite2",
    "PropSurfaceSatellite3", "TemporalTearOpener", "CryoTank", "PropFacilityStatue", "GeothermalVentEntity",
    "GeothermalControllerEntity", "POICeresTechUnlock", "OilWell", "UnderwaterVent",
];

#[derive(Debug, Clone, Copy)]
pub struct Options {
    pub entities: Entities,
    /// Generate the coordinate the game uses when it would rewrite the
    /// requested one (fixed-seed clusters, mixing the game drops); otherwise
    /// such a coordinate is an error.
    pub accept_rewritten: bool,
    /// Append ambient `OilWell`s to `other_entities` (after the template
    /// entities, in spawn order). Positions are approximate.
    pub ambient_oil: bool,
}

impl Default for Options {
    fn default() -> Self { Options { entities: Entities::All, accept_rewritten: false, ambient_oil: true } }
}

/// The loaded game data.
pub struct Engine {
    pub ds: Rc<DataSource>,
    pub dlc2_installed: bool,
    pub game_build: Option<String>,
}

impl Engine {
    /// Restore a data snapshot written by `worldgen-snapshot`.
    pub fn from_snapshot(bytes: &[u8]) -> Result<Engine, String> {
        let s = oni_worldgen::snapshot::decode(bytes)?;
        Ok(Engine { ds: Rc::new(s.data), dlc2_installed: s.dlc2_installed, game_build: s.game_build })
    }
}

/// An error that is not a worldgen failure: unparseable or unknown
/// coordinate, a coordinate the game would rewrite, bad options. Returned to
/// JavaScript as `{error}`.
#[derive(Debug, Clone)]
pub struct Refused(pub String);

/// Generate `coordinate` and build the document. A worldgen failure (the
/// game would fail the same way) is a document with `failure` set and empty
/// lists, so every field is always present.
pub fn generate(engine: &Engine, coordinate: &str, options: Options) -> Result<Value, Refused> {
    let ds = &engine.ds;
    let checked = oni_worldgen::pipeline::validate(coordinate.trim(), ds, None, engine.dlc2_installed).map_err(Refused)?;
    let coordinate = checked.coordinate(options.accept_rewritten, "{acceptRewritten: true}").map_err(Refused)?.to_string();
    let cluster_id = ds.cluster_for_coord(&coordinate).map(|c| c.id.clone()).ok_or_else(|| Refused(format!("no cluster for {coordinate}")))?;
    let parsed = oni_worldgen::coord::Coord::parse(&coordinate).ok_or_else(|| Refused(format!("invalid coordinate {coordinate}")))?;
    match compact::run(&coordinate, ds, None::<ActiveDlc>, Tier::Summary) {
        Ok(mut run) => {
            if options.ambient_oil { run.estimate_ambient_oil(ds); }
            Ok(document(ds, &run, &cluster_id, &parsed.prefix, options))
        }
        Err(e) => match &e.failure {
            Some(_) => Ok(failure_document(&coordinate, parsed.seed, &cluster_id, &parsed.prefix, &e)),
            None => Err(Refused(e.text)),
        },
    }
}

/// `f32` as the double JavaScript would see from a widened value.
fn f(v: f32) -> Value { json!(v as f64) }

fn telemetry(events: &[Event]) -> Vec<Value> {
    events.iter().map(|e| {
        let message = if e.world_index >= 0 { format!("world[{}]: {}", e.world_index, e.message) } else { e.message.clone() };
        json!({"category": e.category, "message": message})
    }).collect()
}

fn failure_document(coordinate: &str, seed: i32, cluster_id: &str, prefix: &str, e: &RunError) -> Value {
    let failure = e.failure.as_ref().expect("worldgen failure");
    let mut events = telemetry(&e.telemetry);
    let at = if failure.world_index >= 0 { format!("world[{}]: ", failure.world_index) } else { String::new() };
    events.push(json!({"category": "cluster_failure", "message": format!("{at}{}: {}", failure.stage, failure.message)}));
    json!({
        "coordinate": coordinate, "seed": seed, "cluster_id": cluster_id, "coordinate_prefix": prefix,
        "starmap": [], "starmap_pois": [], "vanilla_starmap": [], "worlds": [],
        "failure": {"stage": failure.stage, "world_index": failure.world_index, "message": failure.message},
        "telemetry": events,
    })
}

fn entity(p: &Prefab) -> Value {
    json!({"tag": p.id.as_deref().unwrap_or(""), "x": p.location_x, "y": p.location_y})
}

fn is_geyser(id: Option<&str>) -> bool { id.is_some_and(|id| id == "GeyserGeneric" || id.starts_with("GeyserGeneric_")) }

/// Convert a generated summary tier. Ambient `OilWell`s are appended when
/// the run holds an estimate ([`TierRun::estimate_ambient_oil`]).
pub fn document(ds: &DataSource, run: &TierRun, cluster_id: &str, prefix: &str, options: Options) -> Value {
    let keep = |p: &&Prefab| match options.entities {
        Entities::All => true,
        Entities::Map => p.id.as_deref().is_some_and(|id| MAP_TAGS.contains(&id)),
    };
    let mut worlds = Vec::with_capacity(run.worlds.len());
    let mut events = telemetry(&run.telemetry);
    for (i, (w, s)) in run.cluster.worlds.iter().zip(&run.worlds).enumerate() {
        let biome_cells: Vec<Value> = w.cells.iter().enumerate().map(|(id, c)| {
            let zone = c.subworld.as_deref().and_then(|n| ds.subworlds.get(n)).map(|s| s.zone_type.clone());
            let poly: Vec<Value> = c.polygon.iter().flat_map(|&(x, y)| [f(x), f(y)]).collect();
            // The game's overworld node ids start at 1.
            json!({"id": id + 1, "zone_type": zone, "x": f(c.position.0), "y": f(c.position.1), "poly": poly})
        }).collect();
        // Geysers in template entity order (the order the game spawns them),
        // with the configuration rolled at their global position.
        let e = &s.template_entities;
        let mut rolled: HashMap<(i32, i32), &oni_worldgen::runtime_geysers::Geyser> = HashMap::new();
        for g in &s.runtime_geysers.geysers { rolled.insert(g.position, g); }
        let mut geysers = Vec::new();
        for p in e.buildings.iter().chain(&e.pickupables).chain(&e.elemental_ores).chain(&e.backwall_entities).chain(&e.other_entities) {
            if !is_geyser(p.id.as_deref()) { continue; }
            let Some(g) = rolled.get(&(p.location_x, p.location_y)) else { continue };
            geysers.push(json!({"tag": p.id.as_deref().unwrap_or(""), "x": p.location_x, "y": p.location_y, "type": g.type_id,
                "scaled_rate": f(g.scaled[0]), "scaled_iter_len": f(g.scaled[1]), "scaled_iter_pct": f(g.scaled[2]),
                "scaled_year_len": f(g.scaled[3]), "scaled_year_pct": f(g.scaled[4])}));
        }
        let buildings: Vec<Value> = e.buildings.iter().filter(keep).map(entity).collect();
        let mut other_entities: Vec<Value> = e.other_entities.iter().filter(|p| !is_geyser(p.id.as_deref())).filter(keep).map(entity).collect();
        // `OilWell` is one of the map tags, so both entity sets keep it.
        match run.ambient_oil.as_ref().and_then(|a| a.get(i)) {
            Some(Ok(wells)) => other_entities.extend(wells.iter().map(|&(x, y)| json!({"tag": OIL_WELL, "x": x, "y": y}))),
            Some(Err(e)) => events.push(json!({"category": "ambient_oil", "message": format!("world[{}]: ambient OilWells skipped: {e}", w.index)})),
            None => {}
        }
        worlds.push(json!({
            "name": w.name, "width": w.size_x, "height": w.size_y, "is_starting": w.index == run.starting_world,
            "world_traits": w.world_traits.clone().unwrap_or_default(),
            "biome_cells": biome_cells, "geysers": geysers, "buildings": buildings, "other_entities": other_entities,
        }));
    }
    let (mut starmap, mut starmap_pois, mut vanilla) = (Vec::new(), Vec::new(), Vec::new());
    if let Some(sm) = &run.starmap {
        // Starmap worlds are in cluster order, one per world.
        let index: BTreeMap<&str, usize> = run.cluster.worlds.iter().enumerate().map(|(i, w)| (w.name.as_str(), i)).collect();
        for (i, loc) in sm.worlds.iter().enumerate() {
            let world_index = if run.cluster.worlds.get(i).is_some_and(|w| w.name == loc.name) { i } else { index.get(loc.name.as_str()).copied().unwrap_or(i) };
            starmap.push(json!({"world_index": world_index, "q": loc.location.q, "r": loc.location.r}));
        }
        for p in &sm.pois { starmap_pois.push(json!({"poi_type": p.id, "q": p.location.q, "r": p.location.r})); }
        for d in &sm.vanilla_destinations { vanilla.push(json!({"type": d.type_id, "distance": d.distance})); }
    }
    let mut doc = Map::new();
    doc.insert("coordinate".into(), json!(run.coordinate));
    doc.insert("seed".into(), json!(run.cluster.base_seed));
    doc.insert("cluster_id".into(), json!(cluster_id));
    doc.insert("coordinate_prefix".into(), json!(prefix));
    doc.insert("starmap".into(), Value::Array(starmap));
    doc.insert("starmap_pois".into(), Value::Array(starmap_pois));
    doc.insert("vanilla_starmap".into(), Value::Array(vanilla));
    doc.insert("worlds".into(), Value::Array(worlds));
    doc.insert("failure".into(), Value::Null);
    doc.insert("telemetry".into(), Value::Array(events));
    Value::Object(doc)
}
