//! Full cluster pipeline: coordinate in, the whole cluster out. Shared by the
//! native CLI and the WASM build so both produce the same JSON.
//!
//! [`generate_cluster`] returns the CLI's output document without
//! `gameVersion` and `provenance`; those describe the caller (asset location,
//! declared build, timings) and are added by the caller through
//! [`ClusterRun::into_document`]. Callers validate the coordinate first
//! ([`validate`]) and add its `coordinateCheck`.
//!
//! Errors are [`RunError`]s: the CLI's text, plus the structured worldgen
//! failure and the warnings logged up to it. [`run_partial`] generates only
//! some of the worlds.
use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::coord::{validate_coordinate, Coord, CoordinateCheck};
use crate::canonical_json::Object;
use crate::custom_settings;
use crate::data_source::{ClusterYaml, DataSource};
use crate::diagnostics::{Event, Failure, RunError, PREFIX};
use crate::dlc::ActiveDlc;
use crate::generate::{generate_selected, ClusterOutput, Selection, WorldOutput, WorldSelector};
use crate::template_placement::Note;
use crate::terrain::MaterialCell;

/// Coordinate checks that need no game data, so callers can refuse a bad
/// coordinate before loading anything.
pub fn check_coordinate(coordinate: &str) -> Result<Coord, String> {
    let parsed = Coord::parse(coordinate).ok_or("invalid coordinate")?;
    if parsed.canonical() != coordinate || parsed.seed < 0 { return Err("use a canonical full coordinate with a nonnegative seed".into()); }
    if parsed.mixing != "0" && parsed.story != "0" {
        return Err("stories combined with mixing are not implemented; refusing to silently ignore coordinate fields".into());
    }
    custom_settings::decode(&parsed.other, &parsed.story, &parsed.mixing)?;
    Ok(parsed)
}

/// A requested coordinate and what the game would do with it
/// ([`validate_coordinate`]).
pub struct CheckedCoordinate {
    pub requested: String,
    pub check: CoordinateCheck,
}

/// Refuse coordinates the game would not generate as written
/// (`coord::validate_coordinate` mirrors CoordinateChanged and launch).
/// `explicit_dlc` is the active DLC set when given; otherwise the check infers
/// it from the cluster and assumes DLC2 is active when `dlc2_installed` and the
/// cluster allows it.
pub fn validate(requested: &str, ds: &DataSource, explicit_dlc: Option<ActiveDlc>, dlc2_installed: bool) -> Result<CheckedCoordinate, String> {
    let check_dlc = explicit_dlc.unwrap_or_else(|| {
        let cluster = ds.cluster_for_coord(requested);
        let mut dlc = cluster.map_or(ActiveDlc::VANILLA, |c| ActiveDlc::infer_for_coordinate(c, requested));
        dlc.dlc2 = dlc2_installed && cluster.is_some_and(|c| !c.forbidden_dlc_ids.iter().any(|id| id == "DLC2_ID"));
        dlc
    });
    let check = validate_coordinate(requested, ds, check_dlc).map_err(|e| format!("coordinate is not reproducible in-game: {e}"))?;
    Ok(CheckedCoordinate { requested: requested.to_string(), check })
}

impl CheckedCoordinate {
    /// The coordinate to generate. A rewrite is refused unless
    /// `accept_rewritten`; the message names `accept_option`, the caller's way
    /// to accept it.
    pub fn coordinate(&self, accept_rewritten: bool, accept_option: &str) -> Result<&str, String> {
        match &self.check {
            CoordinateCheck::Accepted(c) => Ok(c),
            CoordinateCheck::Rewritten { to, .. } if accept_rewritten => Ok(to),
            CoordinateCheck::Rewritten { to, reason } => Err(format!(
                "coordinate is not reproducible in-game; the game uses {to} ({reason}); pass {accept_option} to generate it")),
        }
    }

    /// The output document's `coordinateCheck` field.
    pub fn to_json(&self) -> Value {
        match &self.check {
            CoordinateCheck::Accepted(_) => json!({"status": "accepted"}),
            CoordinateCheck::Rewritten { reason, .. } => json!({"status": "rewritten", "requested": self.requested, "reason": reason}),
        }
    }
}

/// Every stage's result for one cluster, before it is turned into JSON.
pub struct ClusterRun {
    pub coordinate: String,
    pub explicit_dlc: Option<ActiveDlc>,
    pub scope: ActiveDlc,
    pub quality: BTreeMap<String, String>,
    pub teleporters: bool,
    pub cluster: ClusterOutput,
    pub worlds: Vec<WorldStages>,
    pub starmap: crate::starmap::Starmap,
    /// Warnings the game logs during this worldgen ([`Event`]).
    pub telemetry: Vec<Event>,
}

/// Per-world stages run after layout, in `ClusterOutput::worlds` order.
pub struct WorldStages {
    pub noise: crate::noise_map::NoiseMaps,
    pub heat: Vec<f32>,
    pub terrain_before_biome_borders: Vec<Option<MaterialCell>>,
    pub terrain_before_world_border: Vec<Option<MaterialCell>>,
    pub terrain_after_borders: Vec<Option<MaterialCell>>,
    pub terrain_after_starting_adjustments: Vec<Option<MaterialCell>>,
    pub starting_adjustments: Vec<crate::starting_elements::Adjustment>,
    pub world_border: crate::world_border::WorldBorder,
    pub backwall_cells: Vec<MaterialCell>,
    pub template_placement: crate::template_placement::Output,
    pub template_cell_writes: Vec<crate::template_cells::Write>,
    pub template_entities: crate::template_entities::Output,
    pub runtime_geysers: crate::runtime_geysers::Runtime,
    pub story_traits: Vec<String>,
}

/// [`validate`], [`run_cluster`] and [`ClusterRun::into_document`]: the cluster
/// as JSON text, without `gameVersion` and `provenance`.
pub fn generate_cluster(requested: &str, ds: &DataSource, explicit_dlc: Option<ActiveDlc>, dlc2_installed: bool, accept_rewritten: bool) -> Result<String, RunError> {
    let checked = validate(requested, ds, explicit_dlc, dlc2_installed)?;
    let mut doc = run_cluster(checked.coordinate(accept_rewritten, "accept_rewritten")?, ds, explicit_dlc)?.into_document("explicit")?;
    doc.set("coordinateCheck", &checked.to_json())?;
    Ok(String::from_utf8(doc.finish()).map_err(|e| e.to_string())?)
}

/// Run every worldgen stage for `coordinate`. `explicit_dlc` is the active DLC
/// set (`--dlc`); `None` infers it from the cluster definition and lets the
/// coordinate's mixing field enable DLC mixing, as `generate_for` does.
pub fn run_cluster(coordinate: &str, ds: &DataSource, explicit_dlc: Option<ActiveDlc>) -> Result<ClusterRun, RunError> {
    let prepared = prepare(coordinate, ds, explicit_dlc, Selection::All)?;
    let (worlds, telemetry) = prepared.render(ds, coordinate, Templates::WithStories)?;
    let starmap = prepared.starmap(ds, &telemetry)?;
    let Prepared {scope, quality, teleporters, generated, ..} = prepared;
    Ok(ClusterRun {coordinate:coordinate.to_string(),explicit_dlc,scope,quality,teleporters,cluster:generated,worlds,starmap,telemetry})
}

/// `AssignClusterLocations` reports through the starting world.
fn location_failure(text: String, cluster: &ClusterYaml) -> RunError {
    let Some(message) = text.strip_prefix(PREFIX).map(str::to_string) else { return text.into() };
    let world = message.strip_prefix("Could not find a spot in the cluster for ").and_then(|m| m.split(" EVEN AFTER").next()).map(str::to_string);
    RunError::failure(text, Failure {stage: "AssignClusterLocations", world_index: -1, reporting_world_index: Some(cluster.start_world_index),
        world, message, origin: None})
}

/// Which templates [`Prepared::render`] places.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Templates {
    /// Template rules, then the coordinate's story traits across the cluster.
    WithStories,
    /// Template rules only (partial runs).
    RulesOnly,
    /// None: the template stages see no placements.
    None,
}

/// The cluster-level state every world's stages use, and the layouts.
/// Shared with the output tiers of [`crate::compact`].
pub(crate) struct Prepared<'a> {
    pub(crate) cluster: &'a ClusterYaml,
    pub(crate) scope: ActiveDlc,
    pub(crate) quality: BTreeMap<String, String>,
    pub(crate) teleporters: bool,
    pub(crate) generated: ClusterOutput,
    pub(crate) runtime_seed: i32,
    pub(crate) expansion1: bool,
}

pub(crate) fn prepare<'a>(coordinate: &str, ds: &'a DataSource, explicit_dlc: Option<ActiveDlc>, selection: Selection) -> Result<Prepared<'a>, RunError> {
    let parsed = check_coordinate(coordinate)?;
    let cluster = ds.cluster_for_coord(coordinate).ok_or("missing cluster")?;
    usize::try_from(cluster.start_world_index).map_err(|_|"invalid starting world index")?;
    if let Some(dlc) = explicit_dlc { dlc.check_cluster(cluster)?; }
    let scope = explicit_dlc.unwrap_or_else(|| ActiveDlc::infer(cluster, &custom_settings::decode(&parsed.other, &parsed.story, &parsed.mixing).unwrap_or_default()));
    // Quality settings only change worldgen through Teleporters (see
    // custom_settings::CustomSettings::teleporters_enabled); every other
    // coordinated setting is runtime-only and is accepted and recorded.
    let settings = custom_settings::decode_for(&parsed.other, &parsed.story, &parsed.mixing, &scope)?;
    let quality = settings.effective_quality(&scope);
    let teleporters = settings.teleporters_enabled(&scope);
    let generated = generate_selected(coordinate, ds, explicit_dlc, selection)?;
    // SaveLoader copies each world's seed into clusterDetailSave in order;
    // the last world wins. Runtime geyser RNG uses that saved cluster seed.
    let runtime_seed=generated.base_seed.wrapping_add(generated.placements.len() as i32-1);
    // Without an explicit DLC set keep the original world-name heuristic; it
    // agrees with the save's DLC list for every cluster the game can load.
    let expansion1=match explicit_dlc {
        Some(dlc)=>dlc.expansion1_geysers(cluster),
        None=>generated.placements.iter().any(|w|w.starts_with("expansion1::")),
    };
    Ok(Prepared {cluster, scope, quality, teleporters, generated, runtime_seed, expansion1})
}

impl Prepared<'_> {
    /// The per-world stages after layout, for every generated world, and the
    /// run's warnings. A layout failure the generator deferred
    /// ([`ClusterOutput::pending`]) is returned once the worlds before it have
    /// placed their templates without failing, as the game would get there.
    fn render(&self, ds: &DataSource, coordinate: &str, templates: Templates) -> Result<(Vec<WorldStages>, Vec<Event>), RunError> {
        let generated = &self.generated;
        let telemetry_so_far = || generated.telemetry.clone();
        let starting_world = self.cluster.start_world_index as usize;
        let noise = generated.worlds.iter().map(|w| crate::noise_map::generate(
            ds, &w.cells, w.size_x as usize, w.size_y as usize, w.world_seed
        )).collect::<Result<Vec<_>,_>>().map_err(|e| RunError::from(e).after(telemetry_so_far()))?;
        let heat = generated.worlds.iter().map(|w| crate::noise_map::heat(
            ds,w.size_x as usize,w.size_y as usize,w.world_seed
        )).collect::<Result<Vec<_>,_>>().map_err(|e| RunError::from(e).after(telemetry_so_far()))?;
        let rendered = generated.worlds.iter().zip(noise.iter().zip(&heat)).map(|(w,(noise,heat))| -> Result<_,String> {
            let mut terrain=crate::terrain_render::render_with(ds,w,noise,heat)?;
            let before_biome=terrain.cells.clone();
            crate::biome_borders::render(ds,w,&mut terrain)?;
            let before_world=terrain.cells.clone();
            let world_border=crate::world_border::render(ds,w,&mut terrain)?;
            Ok((before_biome,before_world,terrain,world_border))
        }).collect::<Result<Vec<_>,_>>().map_err(|e| RunError::from(e).after(telemetry_so_far()))?;
        // Story traits make template placement depend on earlier worlds.
        let poi_bounds:Vec<_>=rendered.iter().map(|r|r.3.poi_bounds.clone()).collect();
        let placed=self.place_templates(ds, coordinate, templates, &poi_bounds)?;
        let telemetry = self.telemetry(&placed);
        let worlds = generated.worlds.iter().zip(rendered).zip(placed).zip(noise.into_iter().zip(heat))
            .map(|(((w,(before_biome,before_world,terrain,world_border)),(templates,story_traits)),(noise,heat))| -> Result<_,String> {
            let template_writes=crate::template_cells::writes(ds,w,&templates.placements)?;
            let template_entities=crate::template_entities::generate_with(ds,w,&templates.placements,&world_border.cells,self.teleporters)?;
            let runtime_geysers=crate::runtime_geysers::generate(w,&template_entities,self.runtime_seed,self.expansion1)?;
            let mut after_starting=terrain.cells.clone();
            let starting_adjustments=if w.index==starting_world {
                let indices=crate::starting_elements::region_cells(w)?;
                let settings=crate::starting_elements::settings(ds,w)?;
                crate::starting_elements::apply(&mut after_starting,&indices,&settings)?
            } else {Vec::new()};
            Ok(WorldStages {noise,heat,terrain_before_biome_borders:before_biome,terrain_before_world_border:before_world,
                terrain_after_borders:terrain.cells,terrain_after_starting_adjustments:after_starting,starting_adjustments,world_border,
                backwall_cells:terrain.backwall_cells,template_placement:templates,template_cell_writes:template_writes,template_entities,
                runtime_geysers,story_traits})
        })
            .collect::<Result<Vec<_>,_>>().map_err(|e| RunError::from(e).after(telemetry.clone()))?;
        Ok((worlds, telemetry))
    }

    /// Template placement for every generated world (`poi_bounds` from each
    /// world's border), then the deferred layout failure if there is one.
    pub(crate) fn place_templates(&self, ds: &DataSource, coordinate: &str, templates: Templates, poi_bounds: &[Vec<(i32,i32,i32,i32)>])
        -> Result<Vec<(crate::template_placement::Output, Vec<String>)>, RunError> {
        let generated = &self.generated;
        let telemetry_so_far = || generated.telemetry.clone();
        let placed=match templates {
            Templates::WithStories => match crate::story_traits::place_cluster_templates(ds,coordinate,&generated.worlds,poi_bounds) {
                // With a deferred layout failure the later worlds never ran, so
                // the leftover story check does not apply.
                Err(e) if generated.pending.is_some() && e.failure.as_ref().is_some_and(|f| f.stage == "StoryTraits") => Vec::new(),
                other => other.map_err(|e| e.after(telemetry_so_far()))?,
            },
            Templates::RulesOnly => generated.worlds.iter().zip(poi_bounds).map(|(w,bounds)| {
                let rules=crate::template_placement::input_rules(ds,w,&[])?;
                crate::template_placement::place_with_stories(ds,w,rules,bounds,&[])
            }).collect::<Result<Vec<_>,RunError>>().map_err(|e| e.after(telemetry_so_far()))?,
            Templates::None => generated.worlds.iter().zip(poi_bounds).map(|(w,bounds)| (crate::template_placement::Output {
                placements:Vec::new(),final_bounds:bounds.clone(),after_rng:crate::template_placement::entry_rng(w).state(),notes:Vec::new()},Vec::new())).collect(),
        };
        if let Some(pending) = &generated.pending { return Err(pending.clone().after(telemetry_so_far())); }
        Ok(placed)
    }

    /// The starmap of a full run, after every world's stages; `telemetry` is
    /// the run's warnings ([`Self::telemetry`]).
    pub(crate) fn starmap(&self, ds: &DataSource, telemetry: &[Event]) -> Result<crate::starmap::Starmap, RunError> {
        let Prepared {cluster, scope, generated, runtime_seed, ..} = self;
        let runtime_seed = *runtime_seed;
        // Cluster.AssignClusterLocations runs after every world and the story
        // trait check, on its own SeededRandom(clusterSeed).
        let world_names:Vec<String>=generated.worlds.iter().map(|w|w.name.clone()).collect();
        let starmap_inputs=crate::starmap::inputs(ds,cluster,generated.base_seed,&world_names,&generated.mixing.dlc,scope.expansion1)?;
        let mut starmap=crate::starmap::assign(&starmap_inputs).map_err(|e| location_failure(e, cluster).after(telemetry.to_vec()))?;
        // POI configurations and base game destinations roll from the saved
        // cluster seed, like runtime geysers.
        crate::starmap::roll_pois(&mut starmap,runtime_seed)?;
        if !scope.expansion1 {starmap.vanilla_destinations=crate::starmap::vanilla_destinations(ds,runtime_seed,&generated.mixing.dlc);}
        Ok(starmap)
    }

    /// Warnings in the order the game logs them: trait selection for every
    /// world (the `Cluster` constructor), then per world its layout and
    /// template warnings.
    pub(crate) fn telemetry(&self, placed: &[(crate::template_placement::Output, Vec<String>)]) -> Vec<Event> {
        let events = &self.generated.telemetry;
        let mut out: Vec<Event> = events.iter().filter(|e| e.category == "traits").cloned().collect();
        for (w, (templates, _)) in self.generated.worlds.iter().zip(placed) {
            let world_index = w.index as i32;
            out.extend(events.iter().filter(|e| e.category != "traits" && e.world_index == world_index).cloned());
            for note in &templates.notes {
                let (category, message) = match note {
                    Note::Relaxed {template, near_start} => ("template_rules",
                        format!("Could not place {template} using normal rules, trying relaxed{}", if *near_start {" near start"} else {""})),
                    Note::Unplaced {rule, list_rule, templates} => ("template_rules", format!("Could not place templates: {} (listRule={list_rule}{})",
                        templates.join(", "), rule.as_ref().map_or(String::new(), |o| format!(", {} {}[{}]", o.kind, o.file, o.index)))),
                    // DlcManager.FeatureClusterSpaceEnabled picks the wording.
                    Note::StoryNotPlaced {story, error} => ("story_traits", if self.scope.expansion1 {
                        format!("Cannot place story trait on '{story}' and will try another world. error='{error}'.")
                    } else { format!("Cannot place story trait '{story}' error='{error}'") }),
                };
                out.push(Event {category, world_index, message});
            }
        }
        out
    }
}

/// Partial generation: only the worlds `selectors` pick
/// ([`WorldSelector`]), with template rules (`templates`) or no templates.
/// Story traits and the starmap are cluster-level and are skipped, so a
/// selected world matches the full cluster's except for story templates
/// placed on it (and their cells and entities); geysers, traits, layout and
/// terrain are unaffected.
pub struct PartialRun {
    pub coordinate: String,
    pub explicit_dlc: Option<ActiveDlc>,
    pub scope: ActiveDlc,
    pub quality: BTreeMap<String, String>,
    pub teleporters: bool,
    pub templates: bool,
    pub cluster: ClusterOutput,
    pub worlds: Vec<WorldStages>,
    pub telemetry: Vec<Event>,
}

pub fn run_partial(coordinate: &str, ds: &DataSource, explicit_dlc: Option<ActiveDlc>, selectors: &[WorldSelector], templates: bool) -> Result<PartialRun, RunError> {
    let prepared = prepare(coordinate, ds, explicit_dlc, Selection::Worlds(selectors))?;
    let (worlds, telemetry) = prepared.render(ds, coordinate, if templates {Templates::RulesOnly} else {Templates::None})?;
    let Prepared {scope, quality, teleporters, generated, ..} = prepared;
    Ok(PartialRun {coordinate: coordinate.to_string(), explicit_dlc, scope, quality, teleporters, templates, cluster: generated, worlds, telemetry})
}

impl PartialRun {
    /// The partial document: the full document's cluster fields without
    /// `starmap` and `coverage`, plus `partial`, `templates`, every
    /// placement's world after mixing (`placements`), the world-mixing
    /// substitutions (`mixingApplied`), the selectors that matched nothing
    /// (`absent`) and `telemetry`; each world object is the full one with its
    /// placement `index` first.
    pub fn into_document(self, dlc_source: &str) -> Result<Object, String> {
        let PartialRun {coordinate,explicit_dlc,scope,quality,teleporters,templates,cluster,worlds,telemetry} = self;
        let mut out_worlds = b"[".to_vec();
        for (i, (w, s)) in cluster.worlds.iter().zip(worlds).enumerate() {
            let mut world = Object::default();
            world.set("index", &w.index)?;
            world_fields(&mut world, w, s)?;
            if i > 0 { out_worlds.push(b','); }
            out_worlds.extend(world.finish());
        }
        out_worlds.push(b']');
        let mut doc = Object::default();
        doc.set("schemaVersion", &1)?; doc.set("source", "rust-native")?; doc.set("coordinate", &coordinate)?;
        doc.set("partial", &true)?; doc.set("templates", if templates {"templates"} else {"notemplates"})?;
        doc.set("storyTraitsSkipped", &true)?; doc.set("starmapSkipped", &true)?;
        doc.set("activeDlcIds", &explicit_dlc.map(|d| d.ids()))?;
        doc.set("dlcScope", &dlc_scope_json(explicit_dlc, scope, dlc_source))?;
        doc.set("settings", &json!({"quality": quality, "teleportersEnabled": teleporters}))?;
        doc.set("mixing", &cluster.mixing)?;
        doc.set("placements", &cluster.placements)?;
        doc.set("mixingApplied", &cluster.substitutions.iter().map(|(slot, from, to)| json!({"slot": slot, "from": from, "to": to})).collect::<Vec<_>>())?;
        doc.set("absent", &cluster.absent)?;
        doc.set_raw("worlds", out_worlds);
        doc.set("telemetry", &telemetry)?;
        Ok(doc)
    }
}

fn dlc_scope_json(explicit_dlc: Option<ActiveDlc>, scope: ActiveDlc, dlc_source: &str) -> Value {
    json!({"source": if explicit_dlc.is_some() {dlc_source} else {"inferred-from-cluster"},
        "ids": scope.ids(), "expansion1": scope.expansion1, "dlc2": scope.dlc2, "pureVanilla": scope.is_pure_vanilla()})
}

/// One world's fields of the output document, in document order.
fn world_fields(world: &mut Object, w: &WorldOutput, s: WorldStages) -> Result<(), String> {
    world.set("name", &w.name)?; world.set("width", &w.size_x)?; world.set("height", &w.size_y)?;
    world.set("offset", &w.offset)?; world.set("world_seed", &w.world_seed)?; world.set("world_traits", &w.world_traits)?;
    world.set("story_traits", &s.story_traits)?;
    world.set("geysers", &s.runtime_geysers.summaries())?; world.set("biome_cells", &w.cells)?; world.set("child_regions", &w.child_regions)?;
    world.set("noise_seed", &w.world_seed)?; world.set("mixing", &w.mixing)?;
    world.set("heat", &s.heat)?;
    world.set("parent_metadata", &w.parent_metadata)?;
    world.set("terrain_regions", &w.terrain_regions)?;
    world.set("terrain_before_biome_borders", &s.terrain_before_biome_borders)?;
    world.set("terrain_before_world_border", &s.terrain_before_world_border)?;
    world.set("terrain_after_borders", &s.terrain_after_borders)?;
    world.set("terrain_after_starting_adjustments", &s.terrain_after_starting_adjustments)?;
    world.set("starting_adjustments", &s.starting_adjustments)?;
    world.set("world_border", &s.world_border)?;
    world.set("backwall_cells", &s.backwall_cells)?;
    world.set("template_placement", &s.template_placement)?;
    world.set("template_cell_writes", &s.template_cell_writes)?;
    world.set("template_entities", &s.template_entities)?;
    world.set("runtime_geysers", &s.runtime_geysers)?;
    world.set("biome_edges", &w.biome_edges)?;
    world.set("base_noise", &s.noise.base_noise)?; world.set("override_noise", &s.noise.override_noise)?;
    world.set("density_noise", &s.noise.density_noise)?; world.set("backwall_noise", &s.noise.backwall_noise)?;
    world.set("initial_sites", &w.initial_sites)?; world.set("before_subworld_assignment", &w.before_subworld_assignment)?;
    world.set("after_initial_voronoi", &w.after_initial_voronoi)?; world.set("assignment_trace", &w.assignment_trace)?;
    Ok(())
}

impl ClusterRun {
    /// The CLI output document, without `gameVersion` and `provenance`.
    /// `dlc_source` is recorded as `dlcScope.source` when an explicit DLC set
    /// was given. Fields are serialized straight to canonical JSON text, which
    /// is what `serde_json` would print for the same `Value`.
    pub fn into_document(self, dlc_source: &str) -> Result<Object, String> {
        let ClusterRun {coordinate,explicit_dlc,scope,quality,teleporters,cluster,worlds,starmap,..} = self;
        let mut out_worlds = b"[".to_vec();
        for (i, (w, s)) in cluster.worlds.iter().zip(worlds).enumerate() {
            let mut world = Object::default();
            world_fields(&mut world, w, s)?;
            if i > 0 { out_worlds.push(b','); }
            out_worlds.extend(world.finish());
        }
        out_worlds.push(b']');
        let mut doc = Object::default();
        doc.set("schemaVersion", &1)?; doc.set("source", "rust-native")?; doc.set("coordinate", &coordinate)?;
        doc.set("activeDlcIds", &explicit_dlc.map(|d| d.ids()))?;
        doc.set("dlcScope", &dlc_scope_json(explicit_dlc, scope, dlc_source))?;
        doc.set("settings", &json!({"quality": quality, "teleportersEnabled": teleporters}))?;
        doc.set("phase", "rust-layout-approximate")?; doc.set("mixing", &cluster.mixing)?; doc.set_raw("worlds", out_worlds);
        doc.set("starmap", &starmap)?;
        doc.set("coverage", &json!({"terrainCells": false, "settledPhysics": false, "geyserPlacement": "native",
            "runtimeGeyserConfiguration": true,
            "preBiomeBorderMaterials": true,
            "postBiomeBorderMaterials": true, "postWorldBorderMaterials": true,
            "terrainDisease": true, "backwallMaterials": true,
            "ordinaryTemplatePlacement": true, "templateContents": false, "storyTemplates": "unverified",
            "templateCellMessageInputs": true,
            "templateEntityRecords": true, "ambientMobs": false,
            "worldMixing": true, "subworldMixing": true, "dlcMixingStarmap": false,
            "childInitialGeometry": true, "childLabels": true, "childRelaxation": true, "noiseMaps": true, "heatField": true}))?;
        Ok(doc)
    }
}


/// Every loaded cluster, for building coordinates: prefix, name string key,
/// category, DLC restrictions, the DLC set used when none is given and the
/// seed the game forces (`fixedCoordinate`, -1 for none). Clusters
/// whose YAML the loader rejects (four editor-only Spaced Out test clusters in
/// build 744825) are absent, as they are for the CLI.
pub fn cluster_catalog(ds: &DataSource) -> Value {
    Value::Array(ds.clusters.values().map(|c| {
        let menu = ds.cluster_menu.get(&c.coordinate_prefix).cloned().unwrap_or_default();
        json!({"prefix": c.coordinate_prefix, "id": c.id, "name": menu.name, "description": menu.description,
            "category": menu.category, "menuOrder": menu.menu_order, "skip": menu.skip,
            "requiredDlcIds": c.required_dlc_ids, "forbiddenDlcIds": c.forbidden_dlc_ids, "clusterTags": c.cluster_tags,
            "inferredDlcIds": ActiveDlc::infer_from_cluster(c).ids(), "startWorldIndex": c.start_world_index,
            "worlds": c.world_placements.iter().map(|w| &w.world_ref).collect::<Vec<_>>(), "disableStoryTraits": c.disable_story_traits,
            "fixedCoordinate": c.fixed_coordinate})
    }).collect())
}
