//! End-to-end `generate(coord) -> ClusterOutput` driver.
//!
//! Wires together every previously-ported stage:
//!
//! 1. Parse coord (`crate::coord`).
//! 2. Look up cluster YAML by prefix → worldPlacements.
//! 3. `BestFit` (`crate::bestfit`) → per-world `(x, y)` offset.
//! 4. For each world in cluster-YAML order:
//!    a. compute per-world `globalWorldSeed` via
//!       [`crate::cluster::seed_for_world`]
//!    b. run Poisson-disk sampling (`crate::sampler`)
//!    c. run PowerDiagram (`crate::voronoi_tree`) →
//!       biome cell polygons
//!    d. run subworld filter chain (`crate::world_layout`) →
//!       per-cell subworld labels
//!    e. run template spawning (`crate::template_spawning`) →
//!       geyser `(type, local_x, local_y)` list
//!    f. roll each geyser's rate (`crate::geyser::roll`) →
//!       scaled values + `emitRate` / `avgEmitRate`
//! 5. Emit `ClusterOutput` (mirrors DB record shape).
//!
//! Status: steps 1–3 and 4a/4f are bit-exact in the crate's unit
//! tests and the harness. Steps 4b–4e have the scaffolding in place
//! (sampler, PowerDiagram, filter chain, template extractor) but the
//! end-to-end integration — specifically YAML subworld-file loading,
//! `DetermineTemplatesForWorld` rule evaluation, and start-template
//! placement — needs its remaining port work to produce a full DB
//! matching output. See TODOs inline.

use crate::bestfit::{best_fit, Placement};
use crate::diagnostics::{Event, Failure, RuleOrigin, RunError};
use crate::cluster::seed_for_world;
use crate::coord::Coord;
use crate::data_source::DataSource;
use crate::dlc::ActiveDlc;
use crate::geom::{Polygon, Rect, V2};
use crate::geyser::{db_avg_emit_rate, db_emit_rate, params_for, pick_generic_type, roll, ScaledGeyser};
use crate::sampler::{get_random_points, SampleBehaviour, SeededRandom};
use crate::template_spawning::PlacedGeyser;
use crate::voronoi_tree::{PowerDiagram, PowerDiagramSite};

/// Full DB-record-equivalent cluster output. Shape mirrors the
/// fields of a public seed database record.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ClusterOutput {
    pub coordinate: String,
    pub base_seed: i32,
    /// Mixing settings in effect after `RemoveInvalidMixingSettings`.
    pub mixing: crate::mixing::Active,
    /// The generated worlds: every placement, or the selected ones.
    pub worlds: Vec<WorldOutput>,
    /// World path of every placement after world mixing.
    #[serde(skip)]
    pub placements: Vec<String>,
    /// (placement, cluster YAML world, mixing world) for every placement
    /// world mixing substituted.
    #[serde(skip)]
    pub substitutions: Vec<(usize, String, String)>,
    /// Selectors that matched no placement ([`Selection::Worlds`]).
    #[serde(skip)]
    pub absent: Vec<String>,
    /// Warnings logged while generating the layout.
    #[serde(skip)]
    pub telemetry: Vec<Event>,
    /// A layout failure (`ConvertUnknownCells`) on the world after the last
    /// one in `worlds`. The game renders and places templates on each world
    /// before laying out the next, so the caller first runs those stages on
    /// `worlds`, whose own failures come first.
    #[serde(skip)]
    pub pending: Option<RunError>,
}

/// Which worlds to generate.
#[derive(Debug, Clone, Copy)]
pub enum Selection<'a> {
    All,
    Worlds(&'a [WorldSelector]),
}

/// A placement index, or a world path after world mixing, matched in full
/// (`expansion1::worlds/SandstoneDefault`) or by its last segment, ignoring case.
#[derive(Debug, Clone, PartialEq)]
pub enum WorldSelector {
    Index(usize),
    Path(String),
}

impl WorldSelector {
    /// `"2"` is an index; anything else a path.
    pub fn parse(s: &str) -> Self {
        s.trim().parse().map(WorldSelector::Index).unwrap_or_else(|_| WorldSelector::Path(s.trim().to_string()))
    }
    fn matches(&self, index: usize, path: &str) -> bool {
        match self {
            WorldSelector::Index(i) => *i == index,
            WorldSelector::Path(p) => p.eq_ignore_ascii_case(path) || path.rsplit('/').next().is_some_and(|last| last.eq_ignore_ascii_case(p)),
        }
    }
    fn label(&self) -> String {
        match self { WorldSelector::Index(i) => i.to_string(), WorldSelector::Path(p) => p.clone() }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct WorldOutput {
    /// Placement index in the cluster.
    #[serde(skip)]
    pub index: usize,
    pub name: String,
    pub size_x: i32,
    pub size_y: i32,
    pub offset: (i32, i32),
    pub world_seed: i32,
    /// None on the supplied-geyser validation path: it does not select traits.
    pub world_traits: Option<Vec<String>>,
    pub initial_sites: Vec<(f32, f32)>,
    pub after_initial_voronoi: Vec<BiomeCell>,
    pub assignment_trace: Vec<serde_json::Value>,
    pub before_subworld_assignment: Vec<BiomeCell>,
    pub biome_paths: String, // TODO(Phase 2c): emit compact biomePaths
    pub geysers: Vec<GeyserOutput>,
    /// Biome cells produced by the PowerDiagram pass. One per
    /// Voronoi site; subworld labelling (currently empty) is the
    /// next TODO.
    pub cells: Vec<BiomeCell>,
    pub child_regions: Vec<crate::subworld::ChildRegion>,
    pub parent_metadata: Vec<crate::subworld::ParentMetadata>,
    pub terrain_regions: Vec<crate::subworld::TerrainRegion>,
    pub biome_edges: Vec<crate::terrain::BiomeEdge>,
    pub mixing: AppliedMixing,
}

/// Additions mixing made to this world, in the order the game appends them
/// (world mixing in `Cluster.InitializeWorlds`, then subworld mixing).
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct AppliedMixing {
    /// Cluster YAML world replaced by world mixing.
    pub previous_world: Option<String>,
    pub additional_world_template_rules: Vec<serde_yaml::Value>,
    pub additional_subworld_files: Vec<crate::data_source::SubworldFileRef>,
    pub additional_seasons: Vec<String>,
    /// (proxy, subworld) pairs from subworld mixing.
    pub subworld_mixing: Vec<(String, String)>,
    /// Where each of `additional_world_template_rules` is authored.
    #[serde(skip)]
    pub rule_origins: Vec<RuleOrigin>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct BiomeCell {
    pub position: (f32, f32),
    pub subworld: Option<String>,
    pub polygon: Vec<(f32, f32)>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct GeyserOutput {
    pub type_id: String,
    pub x: i32,
    pub y: i32,
    pub scaled: ScaledGeyser,
    pub emit_rate: i64,
    pub avg_emit_rate: i64,
}

/// Single coord, single call. Loads the cluster/world YAML from
/// `data_source`, BestFits world offsets, derives each world's
/// `KRandom` seed and — once the inner TODOs land — runs Poisson /
/// PowerDiagram / subworld assignment / template spawning, then
/// rolls each geyser's rate.
///
/// For the parts of the pipeline that are bit-exact today (coord
/// parsing, cluster layout, per-world seed derivation, geyser rate
/// roll) this is the real endpoint — no caller supplied descriptor
/// required. Geyser positions are currently sourced via the
/// [`generate_with_placed_geysers`] escape hatch until template
/// placement completes; once that lands, `generate(coord, ds)` will
/// own the full pipeline.
pub fn generate(coord_str: &str, data_source: &DataSource) -> Result<ClusterOutput, String> {
    generate_for(coord_str, data_source, None)
}

/// [`generate`] for an explicit active DLC set. `None` keeps the scope
/// inferred from the cluster definition ([`ActiveDlc::infer_from_cluster`]).
/// An explicit set is checked against the cluster's DLC requirements the way
/// the game filters its cluster list.
pub fn generate_for(coord_str: &str, data_source: &DataSource, active_dlc: Option<ActiveDlc>) -> Result<ClusterOutput, String> {
    let out = generate_selected(coord_str, data_source, active_dlc, Selection::All)?;
    match out.pending { Some(e) => Err(e.text), None => Ok(out) }
}

/// [`generate_for`] for the `selection` worlds, with failures and warnings
/// as data. Every world's stages depend only on cluster-level state (mixing,
/// offsets, the coordinate seed plus the placement index), so a selected
/// world comes out as it does in the full cluster.
pub fn generate_selected(coord_str: &str, data_source: &DataSource, active_dlc: Option<ActiveDlc>, selection: Selection) -> Result<ClusterOutput, RunError> {
    let coord: Coord =
        Coord::parse(coord_str).ok_or_else(|| format!("bad coord {coord_str}"))?;
    let cluster = data_source
        .cluster_for_coord(coord_str)
        .ok_or_else(|| format!("no cluster YAML matches {coord_str}"))?;
    if let Some(dlc) = active_dlc { dlc.check_cluster(cluster)?; }

    let settings = crate::custom_settings::decode(&coord.other, &coord.story, &coord.mixing)?;
    let mixing = crate::mixing::active(&settings, cluster, data_source, active_dlc)?;
    // WorldgenMixing's guarantee checks; the CLI text stays as it was.
    let mixing_failure = |stage: &'static str, reporting: Option<i32>| move |e: String| if e.starts_with("WorldgenMixing:") {
        RunError::failure(e.clone(), Failure {stage, world_index: -1, reporting_world_index: reporting, world: None, message: e, origin: None})
    } else { e.into() };
    let start_world = cluster.start_world_index;
    let substituted = crate::mixing::world_mixing(data_source, cluster, coord.seed, &mixing.world).map_err(mixing_failure("WorldMixing", None))?;
    let world_refs: Vec<&str> = cluster.world_placements.iter().zip(&substituted)
        .map(|(wp, sub)| sub.as_deref().unwrap_or(&wp.world_ref)).collect();

    // Pull world sizes in the cluster YAML's worldPlacements order.
    let mut worlds_meta = Vec::with_capacity(cluster.world_placements.len());
    for name in &world_refs {
        let w = data_source.world(name).ok_or_else(|| {
            format!("world {} not found in DataSource", name)
        })?;
        worlds_meta.push(WorldDescriptor {
            name: name.to_string(),
            size_x: w.size.0,
            size_y: w.size.1,
            geysers: Vec::new(), // TODO: populated by template spawning
        });
    }

    // BestFit world offsets (bit exact today).
    let mut placements: Vec<Placement> = worlds_meta
        .iter()
        .map(|w| Placement {
            width: w.size_x,
            height: w.size_y,
            x: 0,
            y: 0,
        })
        .collect();
    let is_multi = worlds_meta.len() > 1;
    best_fit(&mut placements, is_multi);

    let mixed_worlds = world_refs.iter().zip(&cluster.world_placements)
        .map(|(name, wp)| Ok((data_source.world(name).ok_or("missing mixed world")?, wp.location_type.as_str())))
        .collect::<Result<Vec<_>, String>>()?;
    let subworld_mixing = crate::mixing::subworld_mixing(data_source, &mixed_worlds, coord.seed, &mixing.subworld)
        .map_err(mixing_failure("SubworldMixing", Some(start_world)))?;

    let mut selected = vec![matches!(selection, Selection::All); worlds_meta.len()];
    let mut absent = Vec::new();
    if let Selection::Worlds(selectors) = selection {
        for sel in selectors {
            let mut found = false;
            for (i, name) in world_refs.iter().enumerate() {
                if sel.matches(i, name) { selected[i] = true; found = true; }
            }
            if !found { absent.push(sel.label()); }
        }
    }

    let mut telemetry = Vec::new();
    let mut pending = None;
    let mut out_worlds = Vec::with_capacity(worlds_meta.len());
    for (i, desc) in worlds_meta.iter().enumerate() {
        if !selected[i] { continue; }
        let off = &placements[i];
        // `worldMixing.additional*` only apply to placements world mixing substituted.
        let placement_mixing = substituted[i].as_ref().map(|_| &cluster.world_placements[i].world_mixing);
        let mut applied = AppliedMixing {
            previous_world: substituted[i].as_ref().map(|_| cluster.world_placements[i].world_ref.clone()),
            ..AppliedMixing::default()
        };
        if let Some(m) = placement_mixing {
            applied.additional_world_template_rules.extend(m.additional_world_template_rules.iter().cloned());
            applied.rule_origins.extend(m.additional_world_template_rules.iter().enumerate().map(|(j, r)| RuleOrigin {
                placement: Some(i), ..RuleOrigin::new("clusterMixing", &cluster.id, j, r).in_world(&desc.name) }));
            applied.additional_subworld_files.extend(m.additional_subworld_files.iter().cloned());
            applied.additional_seasons.extend(m.additional_seasons.iter().cloned());
        }
        for m in &subworld_mixing[i] {
            applied.additional_world_template_rules.extend(m.additional_world_template_rules.iter().cloned());
            applied.rule_origins.extend(m.additional_world_template_rules.iter().enumerate()
                .map(|(j, r)| RuleOrigin::new("subworldMixing", &m.settings, j, r).in_world(&desc.name)));
            applied.additional_subworld_files.push(m.file.clone());
            applied.subworld_mixing.push((m.proxy.clone(), m.file.name.clone()));
        }
        // ONI's `Cluster.Generate` derives every per-world seed as
        // `worldSeed + i` where worldSeed is the cluster's seed. All
        // four seeds (worldSeed, layoutSeed, terrainSeed, noiseSeed)
        // get the same value. The previous `seed_for_world(...)`
        // formula was an empirical guess off by `(numWorlds - 1) +
        // worldOffsetX + worldOffsetY - i`; correct for single-world
        // clusters but wrong for multi-world.
        let _ = off;
        let _ = seed_for_world;
        let world_seed = coord.seed.wrapping_add(i as i32);
        let wyaml = data_source.world(&desc.name).unwrap();

        // Trait selection consumes its own KRandom(world_seed) — the
        // result feeds `additionalSubworldFiles` (extra biomes the
        // chain can land on), `additionalUnknownCellFilters` (extra
        // chain entries, merged in by sortOrder) and
        // `additionalWorldTemplateRules` (extra rules merged into
        // the world's rule list later in this loop).
        // The trait pool is whatever `SettingsCache` loaded for the
        // active DLC set (see `crate::dlc`). Without an explicit set the
        // scope is inferred from the cluster's `requiredDlcIds` /
        // `forbiddenDlcIds` and world references; Ceres base-game
        // clusters forbid EXPANSION1_ID, which keeps DLC1 traits out.
        let scope = active_dlc.unwrap_or_else(|| ActiveDlc::infer(cluster, &settings));
        let dlc_scope = (scope.expansion1, scope.dlc2);
        // Trait selection: KRandom seeded with `coord.seed + worldIdx`
        // (matches the reference `traitsForSeed.js` from the JS port —
        // see mapsnotincluded.org/oniworldgen-js).
        let trait_seed = coord.seed.wrapping_add(i as i32);
        let mut trait_warnings = Vec::new();
        let traits_for_world = data_source.select_traits_logged(trait_seed, wyaml, dlc_scope, &mut trait_warnings);
        telemetry.extend(trait_warnings.into_iter().map(|message| Event {category: "traits", world_index: i as i32, message}));
        // Merge subworld_files: trait files are appended.
        let mut merged_subs: Vec<crate::data_source::SubworldFileRef> =
            wyaml.subworld_files.clone();
        for t in &traits_for_world {
            for s in &t.additional_subworld_files {
                merged_subs.push(s.clone());
            }
        }
        merged_subs.extend(applied.additional_subworld_files.iter().cloned());
        // Merge unknownCellsAllowedSubworlds chain: trait filters are
        // appended, then the whole list is stable-sorted by sortOrder
        // ascending (default 0). The game evaluates filters in
        // sortOrder; trait filters with sortOrder 100 land at the
        // end so they don't accidentally Replace earlier work.
        let mut merged_chain: Vec<serde_yaml::Value> =
            wyaml.unknown_cells_allowed_subworlds.clone();
        for t in &traits_for_world {
            for f in &t.additional_unknown_cell_filters {
                merged_chain.push(f.clone());
            }
        }
        if let Some(m) = placement_mixing { merged_chain.extend(m.additional_unknown_cell_filters.iter().cloned()); }
        merged_chain.sort_by_key(|f| {
            f.get("sortOrder").and_then(|v| v.as_i64()).unwrap_or(0)
        });

        // Subworld mixing rewrites `(MixingN)` proxies in place; unresolved
        // proxies are removed (`CleanupUnusedMixing`).
        let mixing_resolutions: std::collections::HashMap<String, Vec<String>> =
            crate::mixing::proxy_names(&subworld_mixing[i]).into_iter().map(|(k, v)| (k, vec![v])).collect();

        // Sample sites seeded by the world seed, using the YAML's
        // `OverworldSampleBehaviour` + density range. The game's
        // `WorldLayout.GenerateOverworld` order is:
        //   1. density = random_range(min,max)
        //   2. if startSubworldName: roll start position via random_range
        //      and add it as the first site (becomes the start cell).
        //   3. GetRandomPoints(rng, ..., avoidPoints=[start_pos]) for the rest
        let mut rng = SeededRandom::new(world_seed);
        let density = rng.random_range_f(
            wyaml.overworld_density_min,
            wyaml.overworld_density_max.max(wyaml.overworld_density_min),
        );
        let behaviour = SampleBehaviour::from_str(&wyaml.overworld_sample_behaviour);
        let mut start_site: Option<[f32; 2]> = None;
        let mut avoid_for_start: Vec<[f32; 2]> = Vec::new();
        if wyaml.start_subworld_name.is_some() {
            // MutatedWorldData.ApplyTrait: each trait's MinMax.Mod adds to the
            // world's ranges (e.g. MisalignedStart), in float.
            let (mut h, mut v) = (wyaml.starting_base_position_horizontal, wyaml.starting_base_position_vertical);
            for t in &traits_for_world {
                let ((hmin, hmax), (vmin, vmax)) = t.starting_base_position_mod;
                h = (h.0 + hmin, h.1 + hmax);
                v = (v.0 + vmin, v.1 + vmax);
            }
            let sx = desc.size_x as f32 * rng.random_range_f(h.0, h.1);
            let sy = desc.size_y as f32 * rng.random_range_f(v.0, v.1);
            start_site = Some([sx, sy]);
            avoid_for_start.push([sx, sy]);
        }
        let mut sites: Vec<[f32; 2]> = get_random_points(
            &mut rng,
            [0.0, 0.0],
            [desc.size_x as f32, desc.size_y as f32],
            density.max(1.0),
            wyaml.overworld_avoid_radius,
            &avoid_for_start,
            behaviour,
            false,
            false,
        );
        // If we got a start site, prepend it so it becomes site 0.
        if let Some(s) = start_site {
            sites.insert(0, s);
        }

        // Build PowerDiagram over the sample points. All sites have
        // equal weight for the first iteration, matching ONI's
        // initial state before `ComputePowerDiagram`.
        let bounds = Polygon::from_rect(Rect::new(
            0.0,
            0.0,
            desc.size_x as f32,
            desc.size_y as f32,
        ));
        // Site 0 (the start cell) gets the start subworld's pdWeight
        // up front; the game's `WorldLayout.GenerateOverworld` calls
        // `ApplySubworldToNode` on the start site before the first
        // Voronoi compute, so the first power diagram already accounts
        // for the start subworld weight.
        let start_weight: f32 = if start_site.is_some() {
            wyaml
                .start_subworld_name
                .as_deref()
                .and_then(|n| {
                    let from_files = wyaml
                        .subworld_files
                        .iter()
                        .find(|s| {
                            &s.name == n
                                || s.name.ends_with(&format!("::{}", n))
                                || n.ends_with(&format!("::{}", s.name))
                        })
                        .map(|s| s.override_power);
                    let pd_weight = data_source
                        .resolve_subworld(n)
                        .map(|sw| sw.pd_weight);
                    let op = from_files.unwrap_or(0.0);
                    if op > 0.0 { Some(op) } else { pd_weight }
                })
                .unwrap_or(1.0)
        } else {
            1.0
        };
        let pd_sites: Vec<PowerDiagramSite> = sites
            .iter()
            .enumerate()
            .map(|(idx, p)| {
                let w = if idx == 0 && start_site.is_some() { start_weight } else { 1.0 };
                PowerDiagramSite::new(idx as i32, V2::new(p[0], p[1]), w)
            })
            .collect();
        let mut pd = PowerDiagram::new(bounds, pd_sites);
        let initial_sites = sites.iter().map(|p| (p[0], p[1])).collect();
        pd.compute_node_voronoi();
        let after_initial_voronoi = pd.sites.iter().map(|s| BiomeCell {
            position: (s.position.x, s.position.y), subworld: None,
            polygon: s.poly.as_ref().map(|p| p.vertices.iter().map(|v| (v.x, v.y)).collect()).unwrap_or_default(),
        }).collect();
        if wyaml.layout_method == "PowerTree" {
            pd = PowerDiagram::new(pd.bounds.clone(), pd.sites.iter().map(|s|
                PowerDiagramSite::new(s.id, s.position, s.weight)).collect());
            pd.compute_vd()?;
            pd.compute_power_diagram(500, 0.2)?;
            pd.recenter();
        }
        let before_subworld_assignment = pd.sites.iter().map(|s| BiomeCell {
            position: (s.position.x, s.position.y), subworld: None,
            polygon: s.poly.as_ref().map(|p| p.vertices.iter().map(|v| (v.x, v.y)).collect()).unwrap_or_default(),
        }).collect();

        pd.filter_neighbours();

        // The game inserts the start site as site 0 in the Voronoi
        // tree (see `GenerateOverworld` line 154-158 of WorldLayout.cs).
        // We do the same above, so site 0 is the start cell.
        let start_idx: Option<usize> = if start_site.is_some() { Some(0) } else { None };

        // Tag cells whose polygon area crosses the inset edge lines
        // (y = mapH-5, y = 5, x = 5, x = mapW-5). Matches
        // `WorldLayout.TagTopAndBottomSites` / `TagEdgeSites` which
        // call `voronoiTree.GetIntersectingLeafSites(topEdge, ...)`.
        // C# uses ClipSegment on the line; the equivalent check here
        // is whether the polygon's bounding box spans the line.
        let mut at_surface = vec![false; pd.sites.len()];
        let mut at_depths = vec![false; pd.sites.len()];
        let mut at_edge = vec![false; pd.sites.len()];
        let mut at_left = vec![false; pd.sites.len()];
        let mut at_right = vec![false; pd.sites.len()];
        let map_h = desc.size_y as f32;
        let map_w = desc.size_x as f32;
        let inset = 5.0;
        for (i, s) in pd.sites.iter().enumerate() {
            let Some(p) = s.poly.as_ref() else { continue };
            if p.vertices.len() < 2 {
                continue;
            }
            let mut y_min = f32::INFINITY;
            let mut y_max = f32::NEG_INFINITY;
            let mut x_min = f32::INFINITY;
            let mut x_max = f32::NEG_INFINITY;
            for v in &p.vertices {
                if v.y < y_min { y_min = v.y; }
                if v.y > y_max { y_max = v.y; }
                if v.x < x_min { x_min = v.x; }
                if v.x > x_max { x_max = v.x; }
            }
            // Top edge at y = map_h - 5
            if y_min <= map_h - inset && y_max >= map_h - inset {
                at_surface[i] = true;
            }
            // Bottom edge at y = 5
            if y_min <= inset && y_max >= inset {
                at_depths[i] = true;
            }
            // Left edge at x = 5
            if x_min <= inset && x_max >= inset {
                at_edge[i] = true;
                at_left[i] = true;
            }
            // Right edge at x = map_w - 5
            if x_min <= map_w - inset && x_max >= map_w - inset {
                at_edge[i] = true;
                at_right[i] = true;
            }
        }
        // BFS distance from the start cell across the cell neighbour
        // graph (derived from PowerDiagram site neighbours). Used by
        // the `DistanceFromTag: AtStart` filter.
        //
        // The game measures these on the overworld MapGraph, whose arcs are
        // undirected: ResetMapGraphFromVoronoiTree adds one wherever either
        // cell kept the other in FilterNeighbours. SharesEdge is not
        // symmetric on edges shorter than its 0.001 tolerance (a collinear
        // test against a neighbouring edge through the shared vertex can
        // pass in one direction only), so walk both directions.
        let mut graph: Vec<Vec<usize>> = vec![Vec::new(); pd.sites.len()];
        for (i, s) in pd.sites.iter().enumerate() {
            for &nid in &s.neighbours {
                let j = nid as usize;
                if j >= graph.len() { continue; }
                if !graph[i].contains(&j) { graph[i].push(j); }
                if !graph[j].contains(&i) { graph[j].push(i); }
            }
        }
        let bfs = |seeds: &[usize]| -> Vec<i32> {
            let mut d = vec![i32::MAX; pd.sites.len()];
            let mut q: std::collections::VecDeque<usize> =
                std::collections::VecDeque::new();
            for &s in seeds {
                d[s] = 0;
                q.push_back(s);
            }
            while let Some(cur) = q.pop_front() {
                let dc = d[cur];
                for &ni in &graph[cur] {
                    if d[ni] == i32::MAX {
                        d[ni] = dc + 1;
                        q.push_back(ni);
                    }
                }
            }
            d
        };
        let distance_from_start: Vec<i32> = match start_idx {
            Some(s) => bfs(&[s]),
            None => vec![i32::MAX; pd.sites.len()],
        };
        let surface_seeds: Vec<usize> = (0..pd.sites.len()).filter(|&i| at_surface[i]).collect();
        let depths_seeds: Vec<usize> = (0..pd.sites.len()).filter(|&i| at_depths[i]).collect();
        let edge_seeds: Vec<usize> = (0..pd.sites.len()).filter(|&i| at_edge[i]).collect();
        let distance_from_surface = if surface_seeds.is_empty() {
            vec![i32::MAX; pd.sites.len()]
        } else {
            bfs(&surface_seeds)
        };
        let distance_from_depths = if depths_seeds.is_empty() {
            vec![i32::MAX; pd.sites.len()]
        } else {
            bfs(&depths_seeds)
        };
        let distance_from_edge = if edge_seeds.is_empty() {
            vec![i32::MAX; pd.sites.len()]
        } else {
            bfs(&edge_seeds)
        };
        let distance_from_left = bfs(&(0..pd.sites.len()).filter(|&i|at_left[i]).collect::<Vec<_>>());
        let distance_from_right = bfs(&(0..pd.sites.len()).filter(|&i|at_right[i]).collect::<Vec<_>>());

        // Per-cell subworld classification. Direct port of
        // `WorldLayout.ConvertUnknownCells` (C# source).
        //
        //   1. Build a mutable WeightedSubWorld pool from the world's
        //      subworlds + trait additions. Each carries (name, weight,
        //      minCount, maxCount, priority, overridePower).
        //   2. Collect all unassigned cells (everything except the
        //      pre-tagged start cell), shuffle them with a Fisher-Yates
        //      using `myRandom`.
        //   3. For each cell in shuffle order:
        //      a. Run the chain (Filter) to produce a candidate set.
        //      b. If any candidates have minCount > 0, pick the one
        //         with highest (priority, minCount), then minCount--.
        //         Else WeightedRandom.Choose using `weight`.
        //      c. ApplySubworld: maxCount-- on the picked subworld.
        //         If maxCount <= 0, remove from pool.
        let cells_count = pd.sites.len();
        let mut cell_subworld: Vec<Option<String>> = vec![None; cells_count];
        // Tag the start cell with its declared startSubworldName.
        if let Some(si) = start_idx {
            if let Some(name) = wyaml.start_subworld_name.clone() {
                cell_subworld[si] = Some(name);
            } else if let Some(name) = merged_subs.first().map(|s| s.name.clone()) {
                cell_subworld[si] = Some(name);
            }
        }

        #[derive(Clone, Debug)]
        struct PoolEntry {
            name: String,
            weight: f32,
            min_count: i32,
            max_count: i32,
            priority: i32,
        }
        let mut pool: Vec<PoolEntry> = merged_subs
            .iter()
            .map(|s| PoolEntry {
                name: s.name.clone(),
                weight: s.weight,
                min_count: s.min_count,
                max_count: s.max_count,
                priority: s.priority,
            })
            .collect();
        // Build shuffled list of unassigned cells via Fisher-Yates
        // using the world RNG (matches `list.ShuffleSeeded`).
        let mut unassigned: Vec<usize> = (0..cells_count)
            .filter(|&i| Some(i) != start_idx)
            .collect();
        for i in (1..unassigned.len()).rev() {
            let j = rng.random_range_i(0, i as i32 + 1) as usize;
            unassigned.swap(i, j.min(i));
        }

        let mut assignment_trace = Vec::new();
        for &ci in &unassigned {
            // Chain → candidate subworld names.
            let mut allowed = crate::slot_set::SlotSet::default();
            for f in &merged_chain {
                let cmd = f.get("command").and_then(|v| v.as_str()).unwrap_or("Replace");
                let raw_names: Vec<String> = f
                    .get("subworldNames")
                    .and_then(|v| v.as_sequence())
                    .map(|s| s.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect())
                    .unwrap_or_default();
                // Expand (MixingN) placeholders. Names starting with `(`
                // that have no resolution are stripped (matches C#
                // CleanupUnusedMixing).
                let mut names: Vec<String> = Vec::with_capacity(raw_names.len());
                for n in raw_names {
                    if let Some(resolved) = mixing_resolutions.get(&n) {
                        names.extend(resolved.iter().cloned());
                    } else if n.starts_with('(') {
                        continue;
                    } else {
                        names.push(n);
                    }
                }
                let tagcmd = f.get("tagcommand").and_then(|v| v.as_str()).unwrap_or("Default");
                let tag = f.get("tag").and_then(|v| v.as_str()).unwrap_or("");
                let dist_for_tag = |t: &str| -> Option<i32> {
                    let d = match t {
                        "AtStart" => distance_from_start[ci],
                        "AtSurface" => distance_from_surface[ci],
                        "AtDepths" => distance_from_depths[ci],
                        "AtEdge" => distance_from_edge[ci],
                        "AtLeft" => distance_from_left[ci],
                        "AtRight" => distance_from_right[ci],
                        _ => return None,
                    };
                    if d == i32::MAX { None } else { Some(d) }
                };
                let cell_has_tag = |t: &str| -> bool {
                    match t {
                        "AtStart" => Some(ci) == start_idx,
                        "AtSurface" => at_surface[ci],
                        "AtDepths" => at_depths[ci],
                        "AtEdge" => at_edge[ci],
                        "AtLeft" => at_left[ci],
                        "AtRight" => at_right[ci],
                        _ => false,
                    }
                };
                let tag_passes = match tagcmd {
                    "Default" => true,
                    "AtTag" => cell_has_tag(tag),
                    "NotAtTag" => !cell_has_tag(tag),
                    "DistanceFromTag" => match dist_for_tag(tag) {
                        Some(d) => {
                            let min = f.get("minDistance").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
                            let max = f.get("maxDistance").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
                            d >= min && d <= max
                        }
                        None => false,
                    },
                    _ => false,
                };
                // Effective filter set: empty when tag check fails.
                // Critical: IntersectWith with an empty set clears
                // `allowed` (not a no-op). ExceptWith / Replace with
                // an empty set are no-ops.
                let mut effective = crate::slot_set::SlotSet::default();
                if tag_passes {
                    for name in &names {
                        for (index, entry) in pool.iter().enumerate() {
                            if name == &entry.name || entry.name.ends_with(&format!("::{}", name)) || name.ends_with(&format!("::{}", entry.name)) {
                                effective.insert(index);
                            }
                        }
                    }
                }
                match cmd {
                    "Replace" => if effective.iter().next().is_some() {
                        allowed.clear();
                        for index in effective.iter() { allowed.insert(index); }
                    },
                    "UnionWith" => for index in effective.iter() { allowed.insert(index); },
                    "IntersectWith" => allowed.retain(|index| effective.contains(index)),
                    "ExceptWith" => for index in effective.iter() { allowed.remove(index); },
                    "SymmetricExceptWith" => for index in effective.iter() {
                        if !allowed.remove(index) { allowed.insert(index); }
                    },
                    "Clear" => if tag_passes { allowed.clear(); },
                    _ => {}
                }
            }
            // Mono enumerates live HashSet slots. Filter insertion/removal order
            // therefore matters to WeightedRandom and min-count tie breaking.
            let candidates: Vec<usize> = allowed.iter().collect();
            let mut assignment = serde_json::json!({"id": ci + 1,
                "distances": {"AtStart": distance_from_start[ci], "AtSurface": distance_from_surface[ci],
                    "AtDepths": distance_from_depths[ci], "AtEdge": distance_from_edge[ci]},
                "candidates": candidates.iter().map(|&i| serde_json::json!({"name": pool[i].name,
                    "weight": pool[i].weight, "minCount": pool[i].min_count, "maxCount": pool[i].max_count,
                    "priority": pool[i].priority})).collect::<Vec<_>>() });
            if candidates.is_empty() {
                // ConvertUnknownCells then sets the node to "Default"; the
                // later stages here have no "Default" subworld and fail.
                telemetry.push(Event {category: "layout", world_index: i as i32,
                    message: format!("No allowed Subworld types. Using default. (cell {})", ci + 1)});
                assignment_trace.push(assignment);
                continue;
            }
            // Priority pick: candidates with minCount > 0.
            let mc_indices: Vec<usize> = candidates
                .iter()
                .copied()
                .filter(|&i| pool[i].min_count > 0)
                .collect();
            let picked_idx = if !mc_indices.is_empty() {
                let mut best = mc_indices[0];
                for &i in &mc_indices[1..] {
                    let bp = pool[best].priority;
                    let bmc = pool[best].min_count;
                    if pool[i].priority > bp
                        || (pool[i].priority == bp && pool[i].min_count > bmc)
                    {
                        best = i;
                    }
                }
                pool[best].min_count -= 1;
                best
            } else {
                // WeightedRandom.Choose by weight.
                let total: f32 = candidates.iter().map(|&i| pool[i].weight).sum();
                let random = rng.random_value();
                assignment["random"] = serde_json::json!(random);
                let t = random * total;
                let mut cumulative = 0.0f32;
                let mut chosen = candidates[candidates.len() - 1];
                for &i in &candidates {
                    cumulative += pool[i].weight;
                    if cumulative > t {
                        chosen = i;
                        break;
                    }
                }
                chosen
            };
            assignment["chosen"] = serde_json::json!(pool[picked_idx].name);
            assignment_trace.push(assignment);
            cell_subworld[ci] = Some(pool[picked_idx].name.clone());
            pool[picked_idx].max_count -= 1;
            if pool[picked_idx].max_count <= 0 {
                pool.remove(picked_idx);
            }
        }
        // ConvertUnknownCells throws for any subworld still owed cells (entries
        // removed at maxCount 0 are no longer checked); the game reports a
        // worldgen failure for the coordinate.
        if let Some(owed) = pool.iter().find(|p| p.min_count > 0) {
            let message = format!("Could not guarantee minCount of Subworld {}, {} remaining on world {}.", owed.name, owed.min_count, desc.name);
            pending = Some(RunError::failure(format!("{}{message}", crate::diagnostics::PREFIX), Failure {stage: "ConvertUnknownCells",
                world_index: i as i32, reporting_world_index: Some(i as i32), world: Some(desc.name.clone()), message, origin: None}));
            break;
        }

        let mut parent_metadata = Vec::with_capacity(cells_count);
        for i in 0..cells_count {
            let mut metadata=crate::subworld::ParentMetadata::default();
            let sw=data_source.resolve_subworld(cell_subworld[i].as_deref().ok_or("missing parent subworld")?).ok_or("missing parent subworld data")?;
            let add=crate::subworld::tag_add;
            if Some(i)==start_idx {
                add(&mut metadata.tags,"AtStart");
                for tag in &sw.tags {add(&mut metadata.tags,tag);}
                add(&mut metadata.tags,&sw.zone_type);
            }
            add(&mut metadata.tags,"Overworld");
            for (tag,present) in [("AtSurface",at_surface[i]),("AtDepths",at_depths[i]),("AtEdge",at_edge[i]),("AtLeft",at_left[i]),("AtRight",at_right[i])] {
                if present {add(&mut metadata.tags,tag);}
            }
            for (tag,d) in [("AtSurface",distance_from_surface[i]),("AtDepths",distance_from_depths[i]),("AtEdge",distance_from_edge[i]),("AtStart",distance_from_start[i]),("AtLeft",distance_from_left[i]),("AtRight",distance_from_right[i])] {
                if d!=i32::MAX {metadata.distances.insert(tag.into(),d);if d>0 {add(&mut metadata.tags,&format!("{tag}_Distance{d}"));}}
            }
            if Some(i)!=start_idx {for tag in &sw.tags {add(&mut metadata.tags,tag);}add(&mut metadata.tags,&sw.zone_type);}
            parent_metadata.push(metadata);
        }
        for tag in &wyaml.overworld_add_tags {
            let i=rng.random_range_i(0,cells_count as i32) as usize;
            crate::subworld::tag_add(&mut parent_metadata[i].tags,tag);
        }

        // Second power-diagram pass: now that each cell has a
        // subworld, update its weight to the subworld's `pdWeight`
        // (or `overridePower` if set on the subworld file) and
        // re-run the diagram. WorldLayout.GenerateOverworld does
        // this after ConvertUnknownCells; the cell polygons displayed
        // in the save match this second-pass result.
        for (i, sw) in cell_subworld.iter().enumerate() {
            let Some(name) = sw else { continue };
            // Find the SubworldFileRef in merged_subs (it carries
            // overridePower); fall back to subworld's pdWeight.
            let override_power = merged_subs
                .iter()
                .find(|s| {
                    &s.name == name
                        || s.name.ends_with(&format!("::{}", name))
                        || name.ends_with(&format!("::{}", s.name))
                })
                .map(|s| s.override_power)
                .unwrap_or(0.0);
            let pd_weight = data_source
                .resolve_subworld(name)
                .map(|sw| sw.pd_weight)
                .unwrap_or(1.0);
            let weight = if override_power > 0.0 { override_power } else { pd_weight };
            pd.sites[i].weight = weight;
            pd.sites[i].current_weight = weight;
        }
        // A fresh PowerDiagram resets adaptive weights and recomputes the
        // weight sum after assigning subworlds, just as Node.ComputeNodePD does.
        if wyaml.layout_method == "PowerTree" {
            pd = PowerDiagram::new(pd.bounds.clone(), pd.sites.iter().map(|s|
                PowerDiagramSite::new(s.id, s.position, s.weight)).collect());
            pd.compute_vd()?;
            pd.compute_power_diagram(500, 0.2)?;
            pd.recenter();
        }

        pd.filter_neighbours();
        let cells: Vec<BiomeCell> = pd
            .sites
            .iter()
            .enumerate()
            .map(|(i, s)| BiomeCell {
                position: (s.position.x, s.position.y),
                subworld: cell_subworld[i].clone(),
                polygon: s
                    .poly
                    .as_ref()
                    .map(|p| {
                        p.vertices
                            .iter()
                            .map(|v| (v.x, v.y))
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default(),
            })
            .collect();
        let edge_neighbors:Vec<Vec<usize>>=pd.sites.iter().map(|s|s.neighbours.iter().filter_map(|id|pd.sites.iter().position(|n|n.id==*id)).collect()).collect();
        let biome_edges=crate::terrain::biome_edges(&cells,&edge_neighbors,data_source)?;
        let child_regions = match crate::subworld::generate_regions(&cells, wyaml,
            &traits_for_world, data_source, world_seed, &mut rng, &parent_metadata) {
            Ok(regions) => regions,
            // ChildrenFromSubworld logs "Assert failed: Overworld node N of
            // subworld S generated P points of an expected minimum M" and
            // returns without children; GenerateLayout then throws a
            // NullReferenceException, which ReportWorldGenError reports with
            // the exception's message (game log, M-FRZ-C-1213617569-O2A5L62-0-0).
            Err(e) if e.contains(crate::subworld::SHORT_CHILDREN) => {
                let message = "Object reference not set to an instance of an object".to_string();
                let mut error = RunError::failure(format!("{}{message}", crate::diagnostics::PREFIX), Failure {stage: "GenerateLayout",
                    world_index: i as i32, reporting_world_index: Some(i as i32), world: Some(desc.name.clone()), message, origin: None});
                error.telemetry.push(Event {category: "layout", world_index: i as i32, message: e});
                pending = Some(error);
                break;
            }
            Err(e) => return Err(e.into()),
        };
        let terrain_regions=crate::subworld::finalize_regions(&child_regions,&parent_metadata,&cells,wyaml,world_seed,data_source)?;
        // Per-cell zoneType (for filtering worldTemplateRules).
        let cell_zone: Vec<Option<String>> = cells
            .iter()
            .map(|c| {
                c.subworld
                    .as_ref()
                    .and_then(|q| data_source.resolve_subworld(q))
                    .map(|sw| sw.zone_type.clone())
            })
            .collect();

        // Naive template-rule placement (will be refined as the
        // filter chain + zone classification lands):
        //
        //   for each worldTemplateRule, walk cells in order and
        //   assign one geyser-containing template per rule, picked
        //   from `names[]` via the world RNG. `times` bounds the
        //   count; we simply take min(times, cells.len()).
        //
        // This is not bit-exact with the game — no tag filters,
        // no zoneType matching, no overlap detection, no priority
        // ordering, no UnknownCell / starting-template paths — but
        // it populates `geysers` end-to-end so the pipeline shape
        // is real. Each successive commit tightens it toward
        // byte-exact.
        // Track which cells are already occupied so two rules don't
        // land their template on the same cell.
        let mut placed: Vec<PlacedGeyser> = Vec::new();
        let mut cell_used = vec![false; pd.sites.len()];
        let mut placed_names: std::collections::HashSet<String> =
            std::collections::HashSet::new();

        // Sort rules by priority desc (game default = 0). Higher
        // priority rules claim cells first so they don't lose to
        // generic spam. Trait `additionalWorldTemplateRules` (geysers,
        // geodes, volcanoes, etc.) and mixing rules are appended below.
        let mut rules_sorted: Vec<&serde_yaml::Value> =
            wyaml.world_template_rules.iter().collect();
        // Per-subworld template rules — `TemplateSpawning.SpawnTemplates`
        // collects these from every subworld file the world loads.
        for sf in &merged_subs {
            if let Some(sw) = data_source.resolve_subworld(&sf.name) {
                rules_sorted.extend(sw.subworld_template_rules.iter());
            }
        }
        // Apply each trait's removeWorldTemplateRulesById before its
        // additional rules — GeoDormant uses this to swap out the
        // default GenericGeysers rule for its own smaller variant.
        for t in &traits_for_world {
            if t.remove_world_template_rules_by_id.is_empty() {
                continue;
            }
            rules_sorted.retain(|r| {
                let id = r.get("ruleId").and_then(|v| v.as_str()).unwrap_or("");
                !t.remove_world_template_rules_by_id.iter().any(|x| x == id)
            });
        }
        for t in &traits_for_world {
            rules_sorted.extend(t.additional_world_template_rules.iter());
        }
        rules_sorted.extend(applied.additional_world_template_rules.iter());
        rules_sorted.sort_by_key(|r| {
            -(r.get("priority").and_then(|v| v.as_i64()).unwrap_or(0) as i32)
        });
        for rule in rules_sorted {
            let Some(names) = rule.get("names").and_then(|v| v.as_sequence()) else {
                continue;
            };
            // Resolve `listRule` + `times`/`someCount`/`moreCount`
            // into a concrete placement count for this rule.
            let list_rule = crate::template_placement::canonical_list_rule(rule
                .get("listRule")
                .and_then(|v| v.as_str())
                .unwrap_or("TryOne"));
            let times_field = rule.get("times").and_then(|v| v.as_u64()).unwrap_or(1) as usize;
            let some_count = rule.get("someCount").and_then(|v| v.as_u64()).unwrap_or(1) as usize;
            let more_count = rule.get("moreCount").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
            let times = match list_rule {
                "TryOne" | "GuaranteeOne" => times_field,
                "GuaranteeAll" => names.len(),
                "GuaranteeSome" | "TrySome" => some_count,
                "GuaranteeSomeTryMore" => some_count + more_count,
                _ => times_field,
            };
            if names.is_empty() || pd.sites.is_empty() {
                continue;
            }
            // Evaluate the allowedCellsFilter chain to a concrete
            // set of allowed cell indices. Tagcommand filters we
            // cannot evaluate (distance-based, requires tag
            // propagation we haven't ported) are treated as
            // match-all so zoneType/subworld ExceptWith clauses
            // after them still apply.
            let mut allowed: Option<std::collections::HashSet<usize>> = None;
            let start_all = || -> std::collections::HashSet<usize> {
                (0..pd.sites.len()).collect()
            };
            // Build per-cell subworld-tag lookup once. Used by
            // tag-driven `allowedCellsFilter` clauses below.
            let cell_subworld_tags: Vec<Vec<String>> = (0..pd.sites.len())
                .map(|ci| {
                    cell_subworld[ci]
                        .as_ref()
                        .and_then(|q| data_source.resolve_subworld(q))
                        .map(|sw| sw.tags.clone())
                        .unwrap_or_default()
                })
                .collect();
            let match_set = |f: &serde_yaml::Value| -> Option<std::collections::HashSet<usize>> {
                let zones: Vec<String> = f
                    .get("zoneTypes")
                    .and_then(|v| v.as_sequence())
                    .map(|s| s.iter().filter_map(|x| x.as_str().map(|x| x.to_string())).collect())
                    .unwrap_or_default();
                let raw_subworlds: Vec<String> = f
                    .get("subworldNames")
                    .and_then(|v| v.as_sequence())
                    .map(|s| s.iter().filter_map(|x| x.as_str().map(|x| x.to_string())).collect())
                    .unwrap_or_default();
                let mut subworlds: Vec<String> = Vec::with_capacity(raw_subworlds.len());
                for n in raw_subworlds {
                    if let Some(resolved) = mixing_resolutions.get(&n) {
                        subworlds.extend(resolved.iter().cloned());
                    } else if n.starts_with('(') {
                        continue;
                    } else {
                        subworlds.push(n);
                    }
                }
                let tagcmd = f.get("tagcommand").and_then(|v| v.as_str()).unwrap_or("Default");
                let tag = f.get("tag").and_then(|v| v.as_str()).unwrap_or("");
                let has_tag_predicate = matches!(tagcmd, "AtTag" | "NotAtTag" | "DistanceFromTag");
                if zones.is_empty() && subworlds.is_empty() && !has_tag_predicate {
                    return None;
                }
                let cell_passes_tag = |ci: usize| -> bool {
                    match tagcmd {
                        "Default" => true,
                        "AtTag" => match tag {
                            "AtStart" => Some(ci) == start_idx,
                            "AtSurface" => at_surface[ci],
                            "AtDepths" => at_depths[ci],
                            "AtEdge" => at_edge[ci],
                            other => cell_subworld_tags[ci].iter().any(|t| t == other),
                        },
                        "NotAtTag" => !match tag {
                            "AtStart" => Some(ci) == start_idx,
                            "AtSurface" => at_surface[ci],
                            "AtDepths" => at_depths[ci],
                            "AtEdge" => at_edge[ci],
                            other => cell_subworld_tags[ci].iter().any(|t| t == other),
                        },
                        "DistanceFromTag" => {
                            let min = f.get("minDistance").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
                            let max = f.get("maxDistance").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
                            let d = match tag {
                                "AtStart" => distance_from_start[ci],
                                "AtSurface" => distance_from_surface[ci],
                                "AtDepths" => distance_from_depths[ci],
                                "AtEdge" => distance_from_edge[ci],
                                _ => return false,
                            };
                            d != i32::MAX && d >= min && d <= max
                        }
                        _ => true,
                    }
                };
                let mut out = std::collections::HashSet::new();
                for ci in 0..pd.sites.len() {
                    let zone_ok = if zones.is_empty() {
                        true
                    } else {
                        cell_zone[ci]
                            .as_ref()
                            .map(|z| zones.iter().any(|a| a == z))
                            .unwrap_or(false)
                    };
                    let sw_ok = if subworlds.is_empty() {
                        true
                    } else {
                        cell_subworld[ci]
                            .as_ref()
                            .map(|s| {
                                subworlds.iter().any(|a| {
                                    s == a
                                        || s.ends_with(&format!("::{}", a))
                                        || a.ends_with(&format!("::{}", s))
                                })
                            })
                            .unwrap_or(false)
                    };
                    let tag_ok = cell_passes_tag(ci);
                    if zone_ok && sw_ok && tag_ok {
                        out.insert(ci);
                    }
                }
                Some(out)
            };
            if let Some(filters) = rule
                .get("allowedCellsFilter")
                .and_then(|v| v.as_sequence())
            {
                for f in filters {
                    let cmd = f.get("command").and_then(|v| v.as_str()).unwrap_or("Replace");
                    // The `All` / `Clear` commands ignore their match set.
                    if cmd == "All" {
                        allowed = Some(start_all());
                        continue;
                    }
                    if cmd == "Clear" {
                        allowed = Some(std::collections::HashSet::new());
                        continue;
                    }
                    let m_opt = match_set(f);
                    // m_opt = None only when neither zones, subworlds
                    // nor tag predicate are set on the filter — treat
                    // as no-op.
                    let m = match m_opt {
                        Some(m) => m,
                        None => continue,
                    };
                    allowed = Some(match (allowed.take(), cmd) {
                        (_, "Replace") => m,
                        (Some(mut cur), "UnionWith") => {
                            cur.extend(m);
                            cur
                        }
                        (Some(cur), "IntersectWith") => cur.intersection(&m).copied().collect(),
                        (Some(cur), "ExceptWith") => cur.difference(&m).copied().collect(),
                        (Some(cur), "SymmetricExceptWith") => {
                            cur.symmetric_difference(&m).copied().collect()
                        }
                        (Some(cur), "Clear") => {
                            let _ = cur;
                            std::collections::HashSet::new()
                        }
                        (Some(cur), "All") => {
                            let _ = cur;
                            start_all()
                        }
                        (None, "UnionWith") | (None, "IntersectWith") | (None, "ExceptWith") => m,
                        _ => m,
                    });
                }
            }
            let allowed = allowed.unwrap_or_else(start_all);
            let allow_duplicates = rule
                .get("allowDuplicates")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            // Map listRule → (num3 = guaranteed count, num4 = max
            // attempts) per outer-times iteration. This mirrors
            // `TemplateSpawning.SpawnTemplatesFromTemplateRules`
            // exactly so RNG consumption sequencing matches.
            let names_len = names.len();
            for _ in 0..times_field.max(1) {
                // Build pooled name list — exclude already-placed
                // names if !allowDuplicates (tracked via `placed_names`).
                let mut pooled: Vec<String> = if allow_duplicates {
                    names
                        .iter()
                        .filter_map(|n| n.as_str().map(|s| s.to_string()))
                        .collect()
                } else {
                    names
                        .iter()
                        .filter_map(|n| n.as_str().map(|s| s.to_string()))
                        .filter(|s| !placed_names.contains(s.as_str()))
                        .collect()
                };
                // Fisher-Yates shuffle (matches `pooledList.ShuffleSeeded`).
                for i in (1..pooled.len()).rev() {
                    let j = rng.random_range_i(0, i as i32 + 1) as usize;
                    pooled.swap(i, j.min(i));
                }
                if pooled.is_empty() { continue; }
                let (mut num3, mut num4) = match list_rule {
                    "GuaranteeAll" => (pooled.len() as i32, pooled.len() as i32),
                    "GuaranteeSome" => (some_count as i32, some_count as i32),
                    "GuaranteeSomeTryMore" => (
                        some_count as i32,
                        (some_count + more_count) as i32,
                    ),
                    "GuaranteeOne" => (1, 1),
                    "TryAll" => (0, pooled.len() as i32),
                    "TrySome" => (0, some_count as i32),
                    "TryOne" => (0, 1),
                    _ => (0, 1),
                };
                for name in pooled.iter() {
                    if num4 <= 0 { break; }
                    let Some(info) = data_source.resolve_template(name) else { continue };
                    // Build per-rule candidate list (allowed minus
                    // already-claimed cells), shuffle, pick last.
                    // HashSet iteration is randomized on every construction.
                    // Shuffle candidates in stable cell order so the RNG seed
                    // controls placement. Exact game ordering remains subject
                    // to the native layout checkpoint comparison.
                    let mut cell_candidates: Vec<usize> = (0..cell_used.len())
                        .filter(|ci| allowed.contains(ci) && !cell_used[*ci])
                        .collect();
                    if cell_candidates.is_empty() { continue; }
                    for i in (1..cell_candidates.len()).rev() {
                        let j = rng.random_range_i(0, i as i32 + 1) as usize;
                        cell_candidates.swap(i, j.min(i));
                    }
                    let Some(&ci) = cell_candidates.last() else { continue };
                    cell_used[ci] = true;
                    placed_names.insert(name.clone());
                    let site = &pd.sites[ci];
                    let cx = site.position.x as i32;
                    let cy = site.position.y as i32;
                    for g in &info.geysers {
                        let gx = cx + g.location_x;
                        let gy = cy + g.location_y;
                        let type_id = if g.type_id == "__GENERIC__" {
                            pick_generic_type(world_seed, gx, gy, active_dlc.map_or(is_multi, |d| d.expansion1_geysers(cluster))).to_string()
                        } else {
                            g.type_id.clone()
                        };
                        placed.push(PlacedGeyser {
                            type_id,
                            world_local_x: gx,
                            world_local_y: gy,
                        });
                    }
                    num3 -= 1;
                    num4 -= 1;
                }
                let _ = num3; // not enforcing guarantee throw — partial placement is fine
            }
            let _ = times;
            let _ = names_len;
        }

        // OilWells are placed (and consume cells) but they aren't
        // geysers in the game's prefab classification, so the WASM
        // dumps don't include them. Match that here.
        let geysers = placed
            .iter()
            .filter(|g| g.type_id != "OilWell")
            .filter_map(|g| roll_geyser(world_seed, g).ok())
            .collect();

        out_worlds.push(WorldOutput {
            index: i,
            name: desc.name.clone(),
            size_x: desc.size_x,
            size_y: desc.size_y,
            offset: (off.x, off.y),
            world_seed,
            world_traits: Some(traits_for_world.iter().map(|t| t.qname.clone()).collect()),
            initial_sites,
            after_initial_voronoi,
            assignment_trace,
            before_subworld_assignment,
            biome_paths: String::new(),
            geysers,
            cells,
            child_regions,
            parent_metadata,
            terrain_regions,
            biome_edges,
            mixing: applied,
        });
    }

    Ok(ClusterOutput {
        coordinate: coord_str.to_string(),
        base_seed: coord.seed,
        mixing,
        worlds: out_worlds,
        placements: world_refs.iter().map(|s| s.to_string()).collect(),
        substitutions: substituted.iter().enumerate().filter_map(|(i, sub)| Some((i, cluster.world_placements[i].world_ref.clone(), sub.clone()?))).collect(),
        absent,
        telemetry,
        pending,
    })
}

/// Escape hatch while template placement is being ported: callers
/// supply geyser `(type, x, y)` per world and this runs the remainder
/// of the pipeline (seed derivation + rate rolling).
pub fn generate_with_placed_geysers(
    coord_str: &str,
    descriptor: &ClusterDescriptor,
) -> Result<ClusterOutput, String> {
    let coord: Coord =
        Coord::parse(coord_str).ok_or_else(|| format!("bad coord {coord_str}"))?;

    // Step 3: BestFit the worlds.
    let mut placements: Vec<Placement> = descriptor
        .worlds
        .iter()
        .map(|w| Placement {
            width: w.size_x,
            height: w.size_y,
            x: 0,
            y: 0,
        })
        .collect();
    let is_multi = descriptor.worlds.len() > 1;
    best_fit(&mut placements, is_multi);

    let mut out_worlds = Vec::with_capacity(descriptor.worlds.len());
    for (i, desc) in descriptor.worlds.iter().enumerate() {
        let off = &placements[i];
        let seed = seed_for_world(
            coord.seed,
            descriptor.worlds.len(),
            off.x,
            off.y,
        );

        // Step 4e → f: given a geyser list (type + local xy), roll
        // each rate. For now we accept pre-placed geysers from the
        // descriptor; when Phase 3's TemplateSpawning is complete this
        // becomes the spawn driver's output.
        let geysers = desc
            .geysers
            .iter()
            .map(|g| roll_geyser(seed, g))
            .collect::<Result<Vec<_>, _>>()?;

        out_worlds.push(WorldOutput {
            index: i,
            name: desc.name.clone(),
            size_x: desc.size_x,
            size_y: desc.size_y,
            offset: (off.x, off.y),
            world_seed: seed,
            world_traits: None,
            initial_sites: Vec::new(),
            after_initial_voronoi: Vec::new(),
            assignment_trace: Vec::new(),
            before_subworld_assignment: Vec::new(),
            biome_paths: String::new(), // TODO(Phase 2c): biomePaths emit
            geysers,
            cells: Vec::new(),
            child_regions: Vec::new(),
            parent_metadata: Vec::new(),
            terrain_regions: Vec::new(),
            biome_edges: Vec::new(),
            mixing: AppliedMixing::default(),
        });
    }

    Ok(ClusterOutput {
        coordinate: coord_str.to_string(),
        base_seed: coord.seed,
        mixing: crate::mixing::Active::default(),
        placements: out_worlds.iter().map(|w| w.name.clone()).collect(),
        substitutions: Vec::new(),
        worlds: out_worlds,
        absent: Vec::new(),
        telemetry: Vec::new(),
        pending: None,
    })
}

/// The `startSubworldName` field lives at a variable nesting in the
/// world YAML; for now pull it out of the raw data_source blob.
/// Reserved for when the distance-tag-aware subworld classifier
/// lands.
#[allow(dead_code)]
fn find_start_subworld_name(w: &crate::data_source::WorldYaml) -> Option<String> {
    w.subworld_files.first().map(|s| s.name.clone())
}

fn roll_geyser(seed: i32, g: &PlacedGeyser) -> Result<GeyserOutput, String> {
    // OilWells always emit 3333, so short-circuit.
    if g.type_id == "OilWell" {
        let scaled = ScaledGeyser {
            scaled_rate: 0.0,
            scaled_iter_len: 0.0,
            scaled_iter_pct: 0.0,
            scaled_year_len: 0.0,
            scaled_year_pct: 0.0,
        };
        return Ok(GeyserOutput {
            type_id: g.type_id.clone(),
            x: g.world_local_x,
            y: g.world_local_y,
            scaled,
            emit_rate: 3333,
            avg_emit_rate: 3333,
        });
    }
    let params = params_for(&g.type_id)
        .ok_or_else(|| format!("unknown geyser type {}", g.type_id))?;
    let scaled = roll(seed, g.world_local_x, g.world_local_y, params);
    Ok(GeyserOutput {
        type_id: g.type_id.clone(),
        x: g.world_local_x,
        y: g.world_local_y,
        scaled,
        emit_rate: db_emit_rate(scaled),
        avg_emit_rate: db_avg_emit_rate(scaled),
    })
}

/// Caller-supplied descriptor of the cluster to generate, for
/// [`generate_with_placed_geysers`].
#[derive(Debug, Clone)]
pub struct ClusterDescriptor {
    pub worlds: Vec<WorldDescriptor>,
}

#[derive(Debug, Clone)]
pub struct WorldDescriptor {
    pub name: String,
    pub size_x: i32,
    pub size_y: i32,
    pub geysers: Vec<PlacedGeyser>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// End-to-end: for a coord with known geyser positions, produce
    /// the same avgEmitRate the DB stores. This reproduces the
    /// validation the harness does today, but through the public
    /// `generate` API.
    #[test]
    fn bad_a_single_world_matches_db_rates() {
        let desc = ClusterDescriptor {
            worlds: vec![WorldDescriptor {
                name: "Badlands".into(),
                size_x: 256,
                size_y: 384,
                geysers: vec![PlacedGeyser {
                    type_id: "steam".into(),
                    world_local_x: 131,
                    world_local_y: 219,
                }],
            }],
        };
        let out = generate_with_placed_geysers("BAD-A-1000172956-0-0-0", &desc).unwrap();
        assert_eq!(out.worlds.len(), 1);
        let g = &out.worlds[0].geysers[0];
        // Same constants the harness sees.
        assert_eq!(g.emit_rate, 7627);
        assert_eq!(g.avg_emit_rate, 1318);
    }
}
