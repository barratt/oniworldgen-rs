//! Summary and compact output tiers, next to the full verification dump of
//! [`crate::pipeline`].
//!
//! * Summary: the fields a seed search indexes (traits, biomes, geysers,
//!   templates, POIs, entity counts, starmap), without cell grids. It skips
//!   every stage those fields cannot depend on: noise is evaluated only where
//!   the world border's vacuum test reads a noise-dependent element, the heat
//!   field, backwalls, starting resources and the per-stage grid copies not
//!   at all (see [`summary_terrain`]).
//! * Compact: the summary plus each world's final pre-settle grid (terrain
//!   after starting resources with template cells written over it) as
//!   indexed planes, and the template entity records.
//!
//! [`run`] mirrors [`crate::pipeline::run_cluster`] stage by stage: it
//! shares the pipeline's preparation, template placement (story traits and
//! deferred layout failures included), telemetry and starmap, so the same
//! failures come out as the same [`RunError`] (text, failure record and
//! warnings) in the same order; the tier fields equal the corresponding
//! full-dump fields (checked by the zz-tier-check differential example over
//! the verification coordinates). [`run_selected`] is the partial run of
//! [`crate::pipeline::run_partial`].
use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::data_source::DataSource;
use crate::diagnostics::{Event, RunError};
use crate::dlc::ActiveDlc;
use crate::generate::{ClusterOutput, Selection, WorldOutput, WorldSelector};
use crate::pipeline::Templates;
use crate::name::Name;
use crate::terrain::MaterialCell;
use crate::terrain_render::{Background, TerrainRender};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier { Summary, Compact }

/// Every tier stage's result for one cluster.
pub struct TierRun {
    pub tier: Tier,
    pub coordinate: String,
    pub explicit_dlc: Option<ActiveDlc>,
    pub scope: ActiveDlc,
    pub quality: BTreeMap<String, String>,
    pub teleporters: bool,
    pub starting_world: usize,
    /// `Some(templates)` for a partial run ([`run_selected`]).
    pub partial: Option<bool>,
    pub cluster: ClusterOutput,
    /// One per generated world, in `cluster.worlds` order.
    pub worlds: Vec<TierWorld>,
    /// `None` for a partial run.
    pub starmap: Option<crate::starmap::Starmap>,
    /// Warnings the game logs during this worldgen, as `ClusterRun::telemetry`.
    pub telemetry: Vec<Event>,
    /// Estimated ambient `OilWell`s per world, in `cluster.worlds` order:
    /// `None` until [`TierRun::estimate_ambient_oil`] runs (`crate::ambient_oil`).
    pub ambient_oil: Option<Vec<Result<Vec<(i32, i32)>, String>>>,
}

pub struct TierWorld {
    pub world_border: crate::world_border::WorldBorder,
    pub template_placement: crate::template_placement::Output,
    pub template_cell_writes: Vec<crate::template_cells::Write>,
    pub template_entities: crate::template_entities::Output,
    pub runtime_geysers: crate::runtime_geysers::Runtime,
    pub story_traits: Vec<String>,
    /// Compact tier only.
    pub grid: Option<Grid>,
}

/// A world's final pre-settle cells. `cells` is the full dump's
/// `terrain_after_starting_adjustments` with each `template_cell_writes`
/// entry written over its cell as given (the simulation later normalizes
/// some template masses and temperatures by element limits).
pub struct Grid {
    pub cells: Vec<MaterialCell>,
    pub backwall: Vec<MaterialCell>,
    /// Index into `subworlds` of the terrain region that rendered each cell
    /// (the last region whose polygon holds it), `u16::MAX` for none.
    pub subworld_idx: Vec<u16>,
    pub subworlds: Vec<String>,
}

/// Buildings and props the seed browser lists as points of interest.
pub const POI_IDS: &[&str] = &[
    "HeadquartersComplete", "WarpConduitSender", "WarpConduitReceiver", "WarpPortal", "WarpReceiver",
    "GeneShuffler", "MassiveHeatSink", "SapTree", "GravitasPedestal", "PropSurfaceSatellite1",
    "PropSurfaceSatellite2", "PropSurfaceSatellite3", "TemporalTearOpener", "CryoTank", "PropFacilityStatue",
    "GeothermalVentEntity", "GeothermalControllerEntity", "POICeresTechUnlock", "VendingMachine",
];

/// Run the stages `tier` needs for `coordinate`; see [`crate::pipeline::run_cluster`].
pub fn run(coordinate: &str, ds: &DataSource, explicit_dlc: Option<ActiveDlc>, tier: Tier) -> Result<TierRun, RunError> {
    run_selected(coordinate, ds, explicit_dlc, tier, None)
}

/// [`run`] for some of the worlds, as [`crate::pipeline::run_partial`] does:
/// `partial` is the world selectors and whether template rules run. Story
/// traits and the starmap are skipped.
pub fn run_selected(coordinate: &str, ds: &DataSource, explicit_dlc: Option<ActiveDlc>, tier: Tier, partial: Option<(&[WorldSelector], bool)>)
    -> Result<TierRun, RunError> {
    let (selection, templates) = match partial {
        None => (Selection::All, Templates::WithStories),
        Some((selectors, true)) => (Selection::Worlds(selectors), Templates::RulesOnly),
        Some((selectors, false)) => (Selection::Worlds(selectors), Templates::None),
    };
    let prepared = crate::pipeline::prepare(coordinate, ds, explicit_dlc, selection)?;
    let generated = &prepared.generated;
    let starting_world = usize::try_from(prepared.cluster.start_world_index).map_err(|_| "invalid starting world index")?;
    // Failures before template placement carry the layout's warnings, as in
    // `Prepared::render`.
    let so_far = |e: String| RunError::from(e).after(generated.telemetry.clone());
    let size = |w: &WorldOutput| (w.size_x as usize, w.size_y as usize);
    let rendered = match tier {
        Tier::Compact => {
            let noise = map_worlds(&generated.worlds, |w| crate::noise_map::generate(ds, &w.cells, size(w).0, size(w).1, w.world_seed)).into_iter().collect::<Result<Vec<_>, _>>().map_err(so_far)?;
            let heat = map_worlds(&generated.worlds, |w| crate::noise_map::heat(ds, size(w).0, size(w).1, w.world_seed)).into_iter().collect::<Result<Vec<_>, _>>().map_err(so_far)?;
            let inputs: Vec<_> = generated.worlds.iter().zip(noise.iter().zip(&heat)).collect();
            map_worlds(&inputs, |&(w, (noise, heat))| {
                let mut terrain = crate::terrain_render::render_with(ds, w, noise, heat)?;
                crate::biome_borders::render(ds, w, &mut terrain)?;
                let border = crate::world_border::render(ds, w, &mut terrain)?;
                Ok((terrain, border))
            }).into_iter().collect::<Result<Vec<_>, String>>().map_err(so_far)?
        }
        Tier::Summary => {
            // The full pipeline's noise and heat failures that do not depend
            // on sample values, for every world before any rendering.
            for w in &generated.worlds { crate::noise_map::generate_partial(ds, &w.cells, size(w).0, size(w).1, w.world_seed, None).map_err(so_far)?; }
            for w in &generated.worlds { crate::noise_map::heat_check(ds, size(w).0, size(w).1, w.world_seed).map_err(so_far)?; }
            map_worlds(&generated.worlds, |w| {
                let mut terrain = summary_terrain(ds, w)?;
                crate::biome_borders::render(ds, w, &mut terrain)?;
                let border = crate::world_border::render(ds, w, &mut terrain)?;
                Ok((terrain, border))
            }).into_iter().collect::<Result<Vec<_>, String>>().map_err(so_far)?
        }
    };
    let poi_bounds: Vec<_> = rendered.iter().map(|r| r.1.poi_bounds.clone()).collect();
    let placed = prepared.place_templates(ds, coordinate, templates, &poi_bounds)?;
    let telemetry = prepared.telemetry(&placed);
    let world_stages = || -> Result<Vec<TierWorld>, String> {
        let mut worlds = Vec::with_capacity(generated.worlds.len());
        for ((w, (terrain, world_border)), (templates, story_traits)) in generated.worlds.iter().zip(rendered).zip(placed) {
            let template_cell_writes = crate::template_cells::writes(ds, w, &templates.placements)?;
            let template_entities = crate::template_entities::generate_with(ds, w, &templates.placements, &world_border.cells, prepared.teleporters)?;
            let runtime_geysers = crate::runtime_geysers::generate(w, &template_entities, prepared.runtime_seed, prepared.expansion1)?;
            let grid = match tier {
                Tier::Summary => {
                    // Starting resources only change masses; keep their settings
                    // failure. Their region claims repeat the render's raster.
                    if w.index == starting_world { crate::starting_elements::settings(ds, w)?; }
                    None
                }
                Tier::Compact => {
                    let TerrainRender { mut cells, backwall_cells, regions, .. } = terrain;
                    if w.index == starting_world {
                        let indices = crate::starting_elements::region_cells(w)?;
                        let settings = crate::starting_elements::settings(ds, w)?;
                        crate::starting_elements::apply(&mut cells, &indices, &settings)?;
                    }
                    let mut cells = cells.into_iter().map(|c| c.ok_or("missing terrain cell")).collect::<Result<Vec<_>, _>>()?;
                    let mut backwall = backwall_cells;
                    // RenderOffline: the world border's managed backwall is reset to Vacuum.
                    for &c in &world_border.cells { backwall[c] = MaterialCell { element: "Vacuum".into(), ..Default::default() }; }
                    for write in &template_cell_writes {
                        let cell = &mut cells[write.cell];
                        *cell = MaterialCell { element: write.element.as_str().into(), mass: write.mass, temperature: write.temperature,
                            disease: write.disease.as_deref().map(Into::into), disease_count: write.disease_count };
                        // A template cell without backwallElement sends element index 0xFFFF
                        // (default SimHashes), which the sim keeps as an unset backwall and saves
                        // as Vacuum with zero mass and temperature: it clears terrain backwalls.
                        backwall[write.cell] = match &write.backwall_element {
                            Some(element) => MaterialCell { element: element.as_str().into(), mass: write.backwall_mass, temperature: write.backwall_temperature, ..Default::default() },
                            None => MaterialCell { element: "Vacuum".into(), ..Default::default() },
                        };
                    }
                    let mut subworlds: Vec<String> = Vec::new();
                    let mut subworld_idx = vec![u16::MAX; cells.len()];
                    for (region, trace) in w.terrain_regions.iter().zip(&regions) {
                        let index = match subworlds.iter().position(|s| *s == region.subworld) {
                            Some(i) => i,
                            None => { subworlds.push(region.subworld.clone()); subworlds.len() - 1 }
                        } as u16;
                        for &c in &trace.spawn_cells { subworld_idx[c as usize] = index; }
                    }
                    Some(Grid { cells, backwall, subworld_idx, subworlds })
                }
            };
            worlds.push(TierWorld { world_border, template_placement: templates, template_cell_writes, template_entities, runtime_geysers, story_traits, grid });
        }
        Ok(worlds)
    };
    let worlds = world_stages().map_err(|e| RunError::from(e).after(telemetry.clone()))?;
    let starmap = match partial { None => Some(prepared.starmap(ds, &telemetry)?), Some(_) => None };
    let crate::pipeline::Prepared { scope, quality, teleporters, generated, .. } = prepared;
    Ok(TierRun { tier, coordinate: coordinate.to_string(), explicit_dlc, scope, quality, teleporters, starting_world, partial: partial.map(|p| p.1),
        cluster: generated, worlds, starmap, telemetry, ambient_oil: None })
}

/// `f` over the worlds in order. Natively the worlds run on scoped threads:
/// the stages mapped are independent per world and deterministic, and the
/// caller takes the first error in world order, as a serial run reports it.
pub(crate) fn map_worlds<T: Sync, R: Send>(items: &[T], f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    #[cfg(not(target_arch = "wasm32"))]
    if items.len() > 1 {
        return std::thread::scope(|scope| {
            let handles: Vec<_> = items.iter().map(|item| scope.spawn(|| f(item))).collect();
            handles.into_iter().map(|h| h.join().unwrap_or_else(|panic| std::panic::resume_unwind(panic))).collect()
        });
    }
    items.iter().map(f).collect()
}

/// The rendered terrain of `w` as far as the world border reads it: exact
/// foreground features, RNG streams, priority claims and region traces, and
/// exact elements at the cells [`crate::world_border::band`] tests for vacuum.
/// Background cells elsewhere, masses, temperatures and backwalls are not
/// computed. Noise is evaluated only for trees covering band cells whose
/// element depends on it; where a region's band table could fail outside the
/// band, the world is rendered in full so the failure is the same.
///
/// Not checked here, unlike the full pipeline: non-finite noise or heat
/// samples outside the evaluated trees and a constant heat field. Those are
/// generator sanity checks the game does not have, and the noise primitives
/// in use are bounded.
pub fn summary_terrain(ds: &DataSource, w: &WorldOutput) -> Result<TerrainRender, String> {
    let (width, height) = (w.size_x as usize, w.size_y as usize);
    let full = || -> Result<TerrainRender, String> {
        let noise = crate::noise_map::generate(ds, &w.cells, width, height, w.world_seed)?;
        let heat = crate::noise_map::heat(ds, width, height, w.world_seed)?;
        crate::terrain_render::render_with(ds, w, &noise, &heat)
    };
    let (Ok(_), Ok(_), Some(count)) = (i32::try_from(width), i32::try_from(height), width.checked_mul(height)) else { return full() };
    let mask = crate::world_border::band(ds, w).unwrap_or_else(|| vec![false; count]);
    let zero = crate::noise_map::NoiseMaps { base_noise: vec![0.0; count], override_noise: vec![0.0; count], density_noise: vec![0.0; count], backwall_noise: vec![0.0; count] };
    let heat = vec![0.0; count];
    let mut needs = vec![0u8; count];
    match crate::terrain_render::render_impl(ds, w, &zero, &heat, Background::Masked(&mask, Some(&mut needs))) {
        Err(e) if e == crate::terrain_render::MASKED_FALLBACK => full(),
        Err(e) => Err(e),
        Ok(terrain) if needs.iter().all(|&b| b == 0) => Ok(terrain),
        Ok(_) => {
            let noise = crate::noise_map::generate_partial(ds, &w.cells, width, height, w.world_seed, Some(Some(&needs)))?;
            crate::terrain_render::render_impl(ds, w, &noise, &heat, Background::Masked(&mask, None))
        }
    }
}

fn to_value(v: &impl serde::Serialize) -> Value { serde_json::to_value(v).expect("serializable stage output") }

impl TierRun {
    /// The summary document (also the metadata part of the compact tier),
    /// without `coordinateCheck`, `gameVersion` and `provenance`. `telemetry`
    /// is the run's warnings. A partial run has no `starmap` and adds the
    /// partial document's `partial`, `templates`, `storyTraitsSkipped`,
    /// `starmapSkipped`, `placements`, `mixingApplied` and `absent`; each
    /// world's `index` is its placement index.
    pub fn summary(&self, ds: &DataSource) -> Value {
        let prefix = &crate::coord::Coord::parse(&self.coordinate).map(|c| c.prefix.clone()).unwrap_or_default();
        let mut worlds: Vec<Value> = self.cluster.worlds.iter().zip(&self.worlds).map(|(w, s)| self.world_summary(ds, w.index, w, s)).collect();
        // Only when estimated, so the summary is otherwise unchanged.
        for (world, wells) in worlds.iter_mut().zip(self.ambient_oil.iter().flatten()) {
            match wells {
                Ok(wells) => world["ambient_oil_wells"] = wells.iter().map(|&(x, y)| json!({"x": x, "y": y})).collect(),
                Err(e) => { world["ambient_oil_wells"] = Value::Null; world["ambient_oil_error"] = json!(e); }
            }
        }
        let mut doc = json!({
            "schemaVersion": 1, "tier": match self.tier { Tier::Summary => "summary", Tier::Compact => "compact" },
            "coordinate": self.coordinate, "coordinatePrefix": prefix, "seed": self.cluster.base_seed,
            "activeDlcIds": self.explicit_dlc.map(|d| d.ids()),
            "dlcScope": {"ids": self.scope.ids(), "expansion1": self.scope.expansion1, "dlc2": self.scope.dlc2, "pureVanilla": self.scope.is_pure_vanilla()},
            "settings": {"quality": self.quality, "teleportersEnabled": self.teleporters},
            "startWorldIndex": self.starting_world,
            "mixing": to_value(&self.cluster.mixing),
            "worlds": worlds, "failure": null, "telemetry": to_value(&self.telemetry),
        });
        match (&self.starmap, self.partial) {
            (Some(starmap), _) => doc["starmap"] = to_value(starmap),
            (None, templates) => {
                doc["partial"] = json!(true);
                doc["templates"] = json!(if templates == Some(false) { "notemplates" } else { "templates" });
                doc["storyTraitsSkipped"] = json!(true);
                doc["starmapSkipped"] = json!(true);
                doc["placements"] = json!(self.cluster.placements);
                doc["mixingApplied"] = json!(self.cluster.substitutions.iter().map(|(slot, from, to)| json!({"slot": slot, "from": from, "to": to})).collect::<Vec<_>>());
                doc["absent"] = json!(self.cluster.absent);
            }
        }
        doc
    }

    fn world_summary(&self, ds: &DataSource, index: usize, w: &WorldOutput, s: &TierWorld) -> Value {
        // Overworld cells per zone type and subworld, with polygon areas.
        let mut zones: BTreeMap<String, (usize, f64)> = BTreeMap::new();
        let mut subworlds: BTreeMap<String, (String, usize, f64)> = BTreeMap::new();
        let mut biome_cells = Vec::with_capacity(w.cells.len());
        for (id, c) in w.cells.iter().enumerate() {
            let subworld = c.subworld.clone().unwrap_or_default();
            let zone = c.subworld.as_deref().and_then(|n| ds.subworlds.get(n)).map(|s| s.zone_type.clone()).unwrap_or_default();
            let area = crate::geom::Polygon::new(c.polygon.iter().map(|&(x, y)| crate::geom::V2::new(x, y)).collect()).area() as f64;
            let z = zones.entry(zone.clone()).or_default(); z.0 += 1; z.1 += area;
            let e = subworlds.entry(subworld.clone()).or_insert((zone.clone(), 0, 0.0)); e.1 += 1; e.2 += area;
            biome_cells.push(json!({"id": id, "subworld": subworld, "zone_type": zone, "x": c.position.0, "y": c.position.1,
                "poly": c.polygon.iter().flat_map(|&(x, y)| [x, y]).collect::<Vec<_>>(),
                "tags": w.parent_metadata.get(id).map(|m| m.tags.clone()).unwrap_or_default()}));
        }
        let e = &s.template_entities;
        let start = s.template_placement.placements.iter()
            .find(|p| p.template.rsplit("::").next().unwrap_or("").starts_with("bases/"));
        let mut pois: Vec<Value> = start.iter().map(|p| json!({"id": "HeadquartersComplete", "x": p.position.0, "y": p.position.1})).collect();
        for p in e.buildings.iter().chain(&e.other_entities).chain(&e.pickupables) {
            if let Some(id) = p.id.as_deref().filter(|id| POI_IDS.contains(id)) { pois.push(json!({"id": id, "x": p.location_x, "y": p.location_y})); }
        }
        let oil_wells: Vec<Value> = e.buildings.iter().chain(&e.other_entities).filter(|p| p.id.as_deref() == Some("OilWell"))
            .map(|p| json!({"x": p.location_x, "y": p.location_y})).collect();
        let counts = |list: &[crate::template_entities::Prefab]| {
            let mut m: BTreeMap<&str, usize> = BTreeMap::new();
            for p in list { *m.entry(p.id.as_deref().unwrap_or("")).or_default() += 1; }
            to_value(&m)
        };
        json!({
            "index": index, "name": w.name, "width": w.size_x, "height": w.size_y, "offset": w.offset, "world_seed": w.world_seed,
            "is_starting": index == self.starting_world,
            "start_position": start.map(|p| [p.position.0, p.position.1]),
            "world_traits": w.world_traits, "story_traits": s.story_traits, "mixing": to_value(&w.mixing),
            "zones": zones.into_iter().map(|(z, (n, a))| json!({"zone_type": z, "cells": n, "area": a})).collect::<Vec<_>>(),
            "subworlds": subworlds.into_iter().map(|(n, (z, c, a))| json!({"subworld": n, "zone_type": z, "cells": c, "area": a})).collect::<Vec<_>>(),
            "biome_cells": biome_cells,
            "geysers": to_value(&s.runtime_geysers.summaries()),
            "oil_wells": oil_wells,
            "template_placements": to_value(&s.template_placement.placements),
            "pois": pois,
            "entity_counts": {"buildings": counts(&e.buildings), "pickupables": counts(&e.pickupables), "elemental_ores": counts(&e.elemental_ores),
                "backwall_entities": counts(&e.backwall_entities), "other_entities": counts(&e.other_entities)},
        })
    }
}

/// The compact tier's planes for one world, with element and disease names
/// indexed into the cluster tables.
pub struct Planes {
    pub width: usize, pub height: usize,
    pub element_idx: Vec<u16>, pub mass: Vec<f32>, pub temperature: Vec<f32>,
    pub disease_idx: Vec<u8>, pub disease_count: Vec<i32>,
    pub backwall_element_idx: Vec<u16>, pub backwall_mass: Vec<f32>, pub backwall_temperature: Vec<f32>,
    pub subworld_idx: Vec<u16>,
}

/// Cluster-wide element and disease name tables (sorted) and each world's planes.
pub fn planes(run: &TierRun) -> Result<(Vec<String>, Vec<String>, Vec<Planes>), String> {
    // Distinct names, then sorted tables. Neighbouring cells mostly share
    // their names, so each lookup first tries the previous cell's.
    let mut element_set = crate::fxhash::FxHashSet::default();
    let mut disease_set = crate::fxhash::FxHashSet::default();
    for g in run.worlds.iter().filter_map(|w| w.grid.as_ref()) {
        let mut last: Option<Name> = None;
        for c in g.cells.iter().chain(&g.backwall) {
            if last != Some(c.element) { element_set.insert(c.element); last = Some(c.element); }
            if let Some(d) = c.disease { disease_set.insert(d); }
        }
    }
    let mut elements: Vec<Name> = element_set.into_iter().collect();
    let mut diseases: Vec<Name> = disease_set.into_iter().collect();
    elements.sort_unstable();
    diseases.sort_unstable();
    if elements.len() > u16::MAX as usize || diseases.len() >= u8::MAX as usize { return Err("compact palette overflow".into()); }
    fn indexer(table: &[Name]) -> impl FnMut(Name) -> usize + '_ {
        let mut last = (Name::default(), usize::MAX);
        move |name| {
            if last.1 == usize::MAX || last.0 != name { last = (name, table.binary_search(&name).expect("name in table")); }
            last.1
        }
    }
    let mut out = Vec::new();
    for (w, s) in run.cluster.worlds.iter().zip(&run.worlds) {
        let g = s.grid.as_ref().ok_or("compact planes need the compact tier")?;
        let (mut element_index, mut disease_index) = (indexer(&elements), indexer(&diseases));
        let element_idx = g.cells.iter().map(|c| element_index(c.element) as u16).collect();
        let backwall_element_idx = g.backwall.iter().map(|c| element_index(c.element) as u16).collect();
        out.push(Planes {
            width: w.size_x as usize, height: w.size_y as usize,
            element_idx,
            mass: g.cells.iter().map(|c| c.mass).collect(),
            temperature: g.cells.iter().map(|c| c.temperature).collect(),
            disease_idx: g.cells.iter().map(|c| c.disease.map_or(u8::MAX, |d| disease_index(d) as u8)).collect(),
            disease_count: g.cells.iter().map(|c| c.disease_count).collect(),
            backwall_element_idx,
            backwall_mass: g.backwall.iter().map(|c| c.mass).collect(),
            backwall_temperature: g.backwall.iter().map(|c| c.temperature).collect(),
            subworld_idx: g.subworld_idx.clone(),
        });
    }
    Ok((elements.into_iter().map(String::from).collect(), diseases.into_iter().map(String::from).collect(), out))
}

/// The compact document's JSON part: [`summary`] plus the element and disease
/// tables and, per world, the subworld table, the template entity records
/// and the template cell writes.
pub fn compact_metadata(run: &TierRun, ds: &DataSource, elements: &[String], diseases: &[String]) -> Value {
    let mut doc = run.summary(ds);
    doc["element_table"] = json!(elements);
    doc["disease_table"] = json!(diseases);
    for (w, s) in doc["worlds"].as_array_mut().expect("worlds").iter_mut().zip(&run.worlds) {
        let e = &s.template_entities;
        if let Some(g) = &s.grid {
            w["subworld_table"] = json!(g.subworlds);
            w["subworld_zone"] = json!(g.subworlds.iter().map(|n| ds.subworlds.get(n).map(|s| s.zone_type.clone())).collect::<Vec<_>>());
        }
        w["buildings"] = to_value(&e.buildings); w["pickupables"] = to_value(&e.pickupables);
        w["elemental_ores"] = to_value(&e.elemental_ores); w["backwall_entities"] = to_value(&e.backwall_entities);
        w["other_entities"] = to_value(&e.other_entities);
    }
    doc
}

/// Plane names, in blob order, with their element size in bytes.
pub const PLANES: [(&str, usize); 9] = [("element_idx", 2), ("mass", 4), ("temperature", 4), ("disease_idx", 1), ("disease_count", 4),
    ("backwall_element_idx", 2), ("backwall_mass", 4), ("backwall_temperature", 4), ("subworld_idx", 2)];

/// One world's grid blob: the [`PLANES`] in order, each little-endian with
/// its bytes split into byte planes (all first bytes, then all second
/// bytes...), concatenated and raw-deflate compressed (`DecompressionStream
/// ("deflate-raw")` in browsers).
pub fn encode_world(p: &Planes) -> Vec<u8> {
    let n = p.width * p.height;
    let mut raw = Vec::with_capacity(n * 27);
    fn shuffle<const W: usize>(raw: &mut Vec<u8>, values: impl Iterator<Item = [u8; W]> + Clone) {
        for b in 0..W { raw.extend(values.clone().map(|v| v[b])); }
    }
    shuffle(&mut raw, p.element_idx.iter().map(|v| v.to_le_bytes()));
    shuffle(&mut raw, p.mass.iter().map(|v| v.to_le_bytes()));
    shuffle(&mut raw, p.temperature.iter().map(|v| v.to_le_bytes()));
    raw.extend_from_slice(&p.disease_idx);
    shuffle(&mut raw, p.disease_count.iter().map(|v| v.to_le_bytes()));
    shuffle(&mut raw, p.backwall_element_idx.iter().map(|v| v.to_le_bytes()));
    shuffle(&mut raw, p.backwall_mass.iter().map(|v| v.to_le_bytes()));
    shuffle(&mut raw, p.backwall_temperature.iter().map(|v| v.to_le_bytes()));
    shuffle(&mut raw, p.subworld_idx.iter().map(|v| v.to_le_bytes()));
    miniz_oxide::deflate::compress_to_vec(&raw, 6)
}

/// Inverse of [`encode_world`].
pub fn decode_world(blob: &[u8], width: usize, height: usize) -> Result<Planes, String> {
    let n = width * height;
    let raw = miniz_oxide::inflate::decompress_to_vec_with_limit(blob, n * 27).map_err(|e| format!("inflate: {e:?}"))?;
    if raw.len() != n * 27 { return Err("grid blob has the wrong size".into()); }
    let mut at = 0;
    let mut take = |w: usize| -> Vec<Vec<u8>> {
        let planes = (0..w).map(|b| raw[at + b * n..at + (b + 1) * n].to_vec()).collect();
        at += w * n;
        planes
    };
    fn join<const W: usize>(planes: Vec<Vec<u8>>, n: usize) -> impl Iterator<Item = [u8; W]> {
        (0..n).map(move |i| std::array::from_fn(|b| planes[b][i]))
    }
    Ok(Planes {
        width, height,
        element_idx: join::<2>(take(2), n).map(u16::from_le_bytes).collect(),
        mass: join::<4>(take(4), n).map(f32::from_le_bytes).collect(),
        temperature: join::<4>(take(4), n).map(f32::from_le_bytes).collect(),
        disease_idx: take(1).remove(0),
        disease_count: join::<4>(take(4), n).map(i32::from_le_bytes).collect(),
        backwall_element_idx: join::<2>(take(2), n).map(u16::from_le_bytes).collect(),
        backwall_mass: join::<4>(take(4), n).map(f32::from_le_bytes).collect(),
        backwall_temperature: join::<4>(take(4), n).map(f32::from_le_bytes).collect(),
        subworld_idx: join::<2>(take(2), n).map(u16::from_le_bytes).collect(),
    })
}

/// The compact container: `ONIC`, u32 format version 1, u32 JSON length, the
/// JSON metadata ([`compact_metadata`] plus `worlds[i].grid = {offset,
/// length}` into the blob area), then each world's [`encode_world`] blob.
/// `extra` fields (coordinateCheck, gameVersion, provenance) are merged into
/// the metadata.
pub fn encode(run: &TierRun, ds: &DataSource, extra: &[(&str, Value)]) -> Result<Vec<u8>, String> {
    let (elements, diseases, planes) = planes(run)?;
    let mut meta = compact_metadata(run, ds, &elements, &diseases);
    for (k, v) in extra { meta[*k] = v.clone(); }
    let blobs: Vec<Vec<u8>> = planes.iter().map(encode_world).collect();
    let mut offset = 0;
    for (w, b) in meta["worlds"].as_array_mut().expect("worlds").iter_mut().zip(&blobs) {
        w["grid"] = json!({"offset": offset, "length": b.len(), "planes": PLANES.iter().map(|p| p.0).collect::<Vec<_>>(), "byteShuffled": true, "compression": "deflate-raw"});
        offset += b.len();
    }
    let text = serde_json::to_vec(&meta).map_err(|e| e.to_string())?;
    let mut out = Vec::with_capacity(12 + text.len() + offset);
    out.extend_from_slice(b"ONIC");
    out.extend_from_slice(&1u32.to_le_bytes());
    out.extend_from_slice(&(text.len() as u32).to_le_bytes());
    out.extend_from_slice(&text);
    for b in &blobs { out.extend_from_slice(b); }
    Ok(out)
}

/// Split a container from [`encode`] into its metadata and world planes.
pub fn decode(bytes: &[u8]) -> Result<(Value, Vec<Planes>), String> {
    if bytes.len() < 12 || &bytes[..4] != b"ONIC" { return Err("not a compact container".into()); }
    if u32::from_le_bytes(bytes[4..8].try_into().unwrap()) != 1 { return Err("unsupported compact container version".into()); }
    let len = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
    let meta: Value = serde_json::from_slice(bytes.get(12..12 + len).ok_or("truncated metadata")?).map_err(|e| e.to_string())?;
    let base = 12 + len;
    let mut planes = Vec::new();
    for w in meta["worlds"].as_array().ok_or("missing worlds")? {
        let (offset, length) = (w["grid"]["offset"].as_u64().ok_or("missing grid")? as usize, w["grid"]["length"].as_u64().ok_or("missing grid")? as usize);
        let blob = bytes.get(base + offset..base + offset + length).ok_or("truncated grid")?;
        planes.push(decode_world(blob, w["width"].as_u64().unwrap_or(0) as usize, w["height"].as_u64().unwrap_or(0) as usize)?);
    }
    Ok((meta, planes))
}
