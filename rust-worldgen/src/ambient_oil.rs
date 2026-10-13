//! Estimated ambient `OilWell` placement on the unsettled grid, for the
//! summary tier ([`TierRun::estimate_ambient_oil`]).
//!
//! The game places ambient spawns (critters, plants, oil reservoirs) after
//! its settle simulation, on the settled grid. This runs the same ambient
//! passes (`ambient_*`) on the grid as world generation leaves it: terrain
//! after biome and world borders, with the template cells written over it.
//! Nothing is simulated.
//!
//! Every ambient `OilWell` comes from the `OilWell` feature, one per feature
//! region. The passes before it draw from the same per-world RNG, so the
//! settled grid's different candidate counts move the RNG on: the well lands
//! on a floor cell of the same oil pocket, the game's own cell only by
//! chance. Rarely an earlier spawn blocks a pocket in one grid and not the
//! other, and the count differs by one.
//!
//! Only worlds with a region that can spawn an `OilWell` are rendered, and
//! the passes stop after the last such region. The placement is the one the
//! `oniworldgen` package's `ambientOil` option appends.

use std::collections::HashSet;

use crate::ambient_pipeline::{self, Region, Spawn};
use crate::ambient_sources::{self, Sources};
use crate::ambient_suitability::Grid;
use crate::compact::TierRun;
use crate::data_source::DataSource;
use crate::generate::WorldOutput;
use crate::sampler::SeededRandom;
use crate::template_cells::Write;

pub const OIL_WELL: &str = "OilWell";

/// One ambient pass: a region's feature mobs (`biome == false`) or its biome
/// mobs, in the game's order (per region: feature, then biome).
pub struct Pass { pub region: usize, pub biome: bool, pub mobs: Vec<Spawn> }

/// The passes up to and including the last one that can place an
/// `OilWell`; empty when the world has none.
pub fn passes(ds: &DataSource, w: &WorldOutput) -> Result<Vec<Pass>, String> {
    let mut passes = Vec::new();
    let mut last = None;
    for (region, r) in w.terrain_regions.iter().enumerate() {
        for biome in [false, true] {
            let mobs = ambient_sources::mobs(ds, r, biome)?;
            if mobs.iter().any(|m| m.prefab == OIL_WELL) { last = Some(passes.len()); }
            passes.push(Pass { region, biome, mobs });
        }
    }
    match last {
        Some(i) => { passes.truncate(i + 1); Ok(passes) }
        None => Ok(Vec::new()),
    }
}

/// The unsettled grid the ambient passes read.
pub struct Terrain {
    /// Per terrain region, the ordered spawn cells and feature cells.
    pub sources: Vec<Sources>,
    /// Foreground element per cell.
    pub elements: Vec<&'static str>,
    /// Whether each cell's backwall is solid.
    pub backwall_solid: Vec<bool>,
}

/// Render `w`'s terrain as the full pipeline does (the heat field only sets
/// temperatures, so it is left at zero), apply biome and world borders, and
/// write the template cells over it.
pub fn terrain(ds: &DataSource, w: &WorldOutput, writes: &[Write]) -> Result<Terrain, String> {
    let (width, height) = (w.size_x as usize, w.size_y as usize);
    let noise = crate::noise_map::generate(ds, &w.cells, width, height, w.world_seed)?;
    let heat = vec![0.0; width * height];
    let mut render = crate::terrain_render::render_with(ds, w, &noise, &heat)?;
    drop(noise);
    let prevent = |r: &crate::subworld::TerrainRegion| r.tags.iter().any(|t| t.as_str() == "PreventAmbientMobsInFeature");
    let sources = w.terrain_regions.iter().zip(&render.regions).map(|(r, trace)| Sources::new(trace, prevent(r))).collect();
    crate::biome_borders::render(ds, w, &mut render)?;
    crate::world_border::render(ds, w, &mut render)?;
    let backwall_solid = render.backwall_cells.iter()
        .map(|c| ds.elements.get(c.element.as_str()).map(|e| e.state == "Solid").ok_or_else(|| format!("unknown backwall element {}", c.element.as_str())))
        .collect::<Result<Vec<_>, _>>()?;
    let mut elements = render.cells.iter().map(|c| c.as_ref().map(|c| c.element.as_str()).ok_or("missing terrain cell")).collect::<Result<Vec<_>, _>>()?;
    for write in writes {
        let cell = elements.get_mut(write.cell).ok_or("template cell outside the world")?;
        *cell = crate::name::Name::new(&write.element).as_str();
    }
    Ok(Terrain { sources, elements, backwall_solid })
}

/// Run `passes` on `terrain` with the game's per-world RNG and occupancy and
/// return the `OilWell` anchor cells in spawn order. `claims` are the cells
/// template entities claim.
pub fn place(ds: &DataSource, w: &WorldOutput, passes: Vec<Pass>, terrain: Terrain, claims: &HashSet<i32>) -> Result<Vec<i32>, String> {
    let Terrain { mut sources, elements, backwall_solid } = terrain;
    let n = w.size_x as usize * w.size_y as usize;
    if elements.len() != n || backwall_solid.len() != n || sources.len() != w.terrain_regions.len() {
        return Err("ambient grid does not match the world".into());
    }
    let state = |s: &str| elements.iter().map(|e| ds.elements.get(*e).map(|d| d.state == s).ok_or_else(|| format!("unknown element {e}"))).collect::<Result<Vec<_>, String>>();
    let (solid, liquid) = (state("Solid")?, state("Liquid")?);
    let vacuum: Vec<bool> = elements.iter().map(|e| *e == "Vacuum").collect();
    let cavities: HashSet<i32> = crate::ambient_cavities::detect(w.size_x as usize, w.size_y as usize, &solid)?.into_iter().map(|c| c as i32).collect();
    let grid = Grid { width: w.size_x, height: w.size_y, solid: &solid, liquid: &liquid, vacuum: &vacuum, backwall_solid: &backwall_solid, cavities: &cavities };
    let mut rng = SeededRandom::new(w.world_seed);
    let mut occupied = HashSet::new();
    let mut wells = Vec::new();
    for Pass { region, biome, mobs } in passes {
        let source = &mut sources[region];
        let candidates = if mobs.is_empty() { Vec::new() } else { source.candidates(biome) };
        let mut forbidden = claims.clone();
        for &cell in &candidates {
            let e = *elements.get(usize::try_from(cell).map_err(|_| "negative candidate")?).ok_or("candidate outside the world")?;
            if e == "Katairite" || e == "Unobtanium" { forbidden.insert(cell); }
        }
        let plan = Region { biome, multiplier: 1.0, candidates, forbidden, mobs };
        let selected = ambient_pipeline::region(&grid, &plan, &mut rng, &mut occupied)?;
        for (cell, prefab) in &selected {
            source.placed(*cell);
            if prefab == OIL_WELL { wells.push(*cell); }
        }
    }
    Ok(wells)
}

/// The estimated ambient `OilWell` positions of one generated world, `(x,
/// y)` in spawn order; empty, without rendering, for a world that cannot
/// have one. `writes` are the world's template cell writes, `claimed` its
/// template entities' claimed cells.
pub fn world_wells(ds: &DataSource, w: &WorldOutput, writes: &[Write], claimed: &[(i32, i32)]) -> Result<Vec<(i32, i32)>, String> {
    let passes = passes(ds, w)?;
    if passes.is_empty() { return Ok(Vec::new()); }
    let terrain = terrain(ds, w, writes)?;
    let claims: HashSet<i32> = claimed.iter().map(|&(c, _)| c).collect();
    let cells = place(ds, w, passes, terrain, &claims)?;
    Ok(cells.into_iter().map(|c| (c % w.size_x, c / w.size_x)).collect())
}

impl TierRun {
    /// Estimate every world's ambient `OilWell`s ([`world_wells`]) into
    /// [`TierRun::ambient_oil`]; the summary then carries them per world as
    /// `ambient_oil_wells`. Opt-in: the tiers do not run it themselves. A
    /// world whose estimate fails gets its error instead, and the rest of the
    /// run is unaffected (the estimate renders the full noise field, which
    /// the summary tier otherwise skips).
    pub fn estimate_ambient_oil(&mut self, ds: &DataSource) {
        let inputs: Vec<_> = self.cluster.worlds.iter().zip(&self.worlds).collect();
        self.ambient_oil = Some(crate::compact::map_worlds(&inputs, |&(w, s)| {
            world_wells(ds, w, &s.template_cell_writes, &s.template_entities.claimed_cells)
        }));
    }
}
