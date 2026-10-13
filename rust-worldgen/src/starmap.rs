//! Cluster starmap placement (build 744825):
//! `ProcGenGame.Cluster.AssignClusterLocations`.
//!
//! `Cluster.BeginGeneration` calls it once every world has generated and
//! rendered and the unplaced-story-trait check has passed, so it can only
//! fail after every per-world failure. It draws from its own
//! `new SeededRandom(clusterSeed)` and touches no other RNG stream.
//!
//! Worlds are placed in cluster-YAML order. Each picks uniformly from the
//! hexes in `allowedRings` (capped at `numRings - 1`) that are not taken,
//! not inside an earlier world's `buffer` rings and not within this world's
//! `buffer` of a taken hex. If none remain, the buffers drop to a fixed 2
//! around taken hexes; failing that, worldgen fails. With Spaced Out's
//! cluster space feature, `poiPlacements` (plus the active DLC mixing
//! `spacePois`, appended in the `Cluster` constructor) are then placed
//! around them. `ClusterPOIManager.PopulatePOIsFromWorldGen` instantiates
//! each POI prefab at its hex.
//!
//! Spawning a POI starts `HarvestablePOIStates` / `ArtifactPOIStates`, whose
//! first `root.Enter` rolls the instance configuration from
//! `KRandom(globalWorldSeed + (int)x + (int)y)` at the hex's world position
//! ([`roll_pois`]). Without the cluster space feature `SpacecraftManager`
//! instead generates the base game rocket destinations from
//! `KRandom(globalWorldSeed)` ([`vanilla_destinations`]). `globalWorldSeed` is
//! `clusterDetailSave.globalWorldSeed`, the last world's seed.

use std::collections::HashSet;

use crate::data_source::{ClusterYaml, DataSource, PoiPlacementYaml};
use crate::mixing::{mono_sort, shuffle_seeded};
use crate::rng::KRandom;
use crate::sampler::SeededRandom;

/// `AxialI`: `r` then `q`, as `new AxialI(r, q)` takes them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize)]
pub struct Axial { pub r: i32, pub q: i32 }

impl Axial {
    pub const ZERO: Axial = Axial { r: 0, q: 0 };
    const fn new(r: i32, q: i32) -> Self { Axial { r, q } }
    fn add(self, o: Axial) -> Self { Axial::new(self.r + o.r, self.q + o.q) }
    fn mul(self, s: i32) -> Self { Axial::new(self.r * s, self.q * s) }
}

/// `AxialI.DIRECTIONS`: NW, NE, E, SE, SW, W.
const DIRECTIONS: [Axial; 6] = [Axial::new(0, -1), Axial::new(1, -1), Axial::new(1, 0), Axial::new(0, 1), Axial::new(-1, 1), Axial::new(-1, 0)];
/// `AxialI.CLOCKWISE`: E, SE, SW, W, NW, NE.
const CLOCKWISE: [Axial; 6] = [Axial::new(1, 0), Axial::new(0, 1), Axial::new(-1, 1), Axial::new(-1, 0), Axial::new(0, -1), Axial::new(1, -1)];

/// `AxialUtil.GetRing`: `6 * radius` hexes (one for radius 0), walking
/// clockwise from each direction's corner.
pub fn get_ring(center: Axial, radius: i32) -> Vec<Axial> {
    if radius == 0 { return vec![center]; }
    let mut out = Vec::with_capacity(6 * radius.max(0) as usize);
    for (dir, step) in DIRECTIONS.iter().zip(&CLOCKWISE) {
        let corner = center.add(dir.mul(radius));
        for j in 0..radius { out.push(corner.add(step.mul(j))); }
    }
    out
}

/// `AxialUtil.GetRings`: rings `min..=max` concatenated in order.
pub fn get_rings(center: Axial, min: i32, max: i32) -> Vec<Axial> {
    (min..=max).flat_map(|r| get_ring(center, r)).collect()
}

/// `SpaceMapPOIPlacement` defaults: `allowedRings` (0, 9999), flags false.
const DEFAULT_RINGS: (i32, i32) = (0, 9999);

#[derive(Debug, Clone, serde::Serialize)]
pub struct WorldLocation {
    pub name: String,
    pub location: Axial,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct PoiLocation {
    pub id: String,
    pub location: Axial,
    /// Harvestable POIs: `HarvestablePOIInstanceConfiguration.capacityRoll`
    /// and `rechargeRoll`, and the `GetMaxCapacity` / `GetRechargeTime` they
    /// map to.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capacity_roll: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recharge_roll: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_capacity: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recharge_time: Option<f32>,
    /// `ArtifactPOIInstanceConfiguration.rechargeRoll` and `GetRechargeTime`:
    /// artifact POIs, and harvestable POIs that `canProvideArtifacts`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact_recharge_roll: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact_recharge_time: Option<f32>,
}

impl PoiLocation {
    fn new(id: String, location: Axial) -> Self {
        PoiLocation { id, location, capacity_roll: None, recharge_roll: None, total_capacity: None, recharge_time: None,
            artifact_recharge_roll: None, artifact_recharge_time: None }
    }
}

/// `SpaceDestination` fields fixed at generation. Its constructor also rolls
/// `recoverableElements` and research opportunities, and
/// `RestoreDestinations` sets `startingOrbitPercentage`, all from the
/// unseeded `UnityEngine.Random`.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SpaceDestination {
    pub id: i32,
    #[serde(rename = "type")]
    pub type_id: String,
    pub distance: i32,
}

/// `Cluster.numRings` and the dictionaries `ClusterLayoutSave` stores.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Starmap {
    pub num_rings: i32,
    /// Cluster-YAML order, one per world.
    pub worlds: Vec<WorldLocation>,
    /// `poiPlacements` in insertion (= enumeration) order.
    pub pois: Vec<PoiLocation>,
    /// Placement indices whose spawn found no room (`Debug.LogWarning`, not
    /// a failure), once per missed spawn.
    pub unplaced_pois: Vec<usize>,
    /// `SpacecraftManager.destinations` after `RestoreDestinations`; empty
    /// with the cluster space feature.
    pub vanilla_destinations: Vec<SpaceDestination>,
}

/// Inputs gathered the way `Cluster` sees them.
pub struct Inputs<'a> {
    /// `Cluster.Id`, e.g. `expansion1::clusters/MiniClusterRadioactiveOceanStart`.
    pub cluster_id: &'a str,
    pub seed: i32,
    /// `ClusterLayout.numRings` (12 unless the YAML sets it).
    pub layout_num_rings: i32,
    /// Per world: (`Settings.world.filePath` after world mixing, allowedRings, buffer).
    pub worlds: Vec<(String, (i32, i32), i32)>,
    /// `clusterLayout.poiPlacements`; `None` when the YAML has none.
    pub poi_placements: Option<Vec<PoiPlacementYaml>>,
    /// `DlcManager.FeatureClusterSpaceEnabled()`.
    pub cluster_space: bool,
}

/// Collect [`Inputs`] for a generated cluster. `world_names` are the
/// generated worlds' file paths in cluster-YAML order and `dlc_mixing` is
/// `GetCurrentDlcMixingIds` ([`crate::mixing::Active::dlc`]).
pub fn inputs<'a>(ds: &DataSource, cluster: &'a ClusterYaml, seed: i32, world_names: &[String], dlc_mixing: &[String], cluster_space: bool) -> Result<Inputs<'a>, String> {
    if world_names.len() != cluster.world_placements.len() { return Err("world count differs from cluster worldPlacements".into()); }
    let worlds = world_names.iter().zip(&cluster.world_placements)
        .map(|(name, wp)| (name.clone(), wp.allowed_rings.unwrap_or(DEFAULT_RINGS), wp.buffer)).collect();
    // The Cluster constructor appends each active DLC mixing's spacePois,
    // but only to a layout that already has a poiPlacements list.
    let poi_placements = cluster.poi_placements.clone().map(|mut list| {
        for id in dlc_mixing {
            if let Some(extra) = ds.dlc_mixing_space_pois.get(id) { list.extend(extra.iter().cloned()); }
        }
        list
    });
    Ok(Inputs { cluster_id: &cluster.id, seed, layout_num_rings: cluster.num_rings, worlds, poi_placements, cluster_space })
}

/// `Cluster.AssignClusterLocations`. A world that fits nowhere returns the
/// game's `ReportWorldGenError` message.
pub fn assign(inputs: &Inputs) -> Result<Starmap, String> {
    // Cluster constructor: `if (clusterLayout.numRings > 0) numRings = clusterLayout.numRings;` (field default 5).
    let num_rings = if inputs.layout_num_rings > 0 { inputs.layout_num_rings } else { 5 };
    let mut rng = SeededRandom::new(inputs.seed);
    let mut assigned: HashSet<Axial> = HashSet::new();
    let mut world_forbidden: HashSet<Axial> = HashSet::new();
    let mut poi_world_avoidance: HashSet<Axial> = HashSet::new();
    let max_radius = 2;
    let mut worlds = Vec::with_capacity(inputs.worlds.len());
    for (name, (ring_min, ring_max), buffer) in &inputs.worlds {
        let rings = get_rings(Axial::ZERO, *ring_min, (*ring_max).min(num_rings - 1));
        let anti_buffer: HashSet<Axial> = assigned.iter().flat_map(|a| get_rings(*a, 1, *buffer)).collect();
        let mut candidates: Vec<Axial> = rings.iter().copied()
            .filter(|l| !assigned.contains(l) && !world_forbidden.contains(l) && !anti_buffer.contains(l)).collect();
        if candidates.is_empty() {
            // "Could not find a spot ..." (DevLogError), then retry with 2-ring buffers.
            let min_buffers: HashSet<Axial> = assigned.iter().flat_map(|a| get_rings(*a, 1, 2)).collect();
            candidates = rings.iter().copied().filter(|l| !assigned.contains(l) && !min_buffers.contains(l)).collect();
        }
        if candidates.is_empty() {
            return Err(format!("Worldgen failure: Could not find a spot in the cluster for {name} EVEN AFTER REDUCING BUFFERS. Check the placement settings in {}.yaml to ensure there are no conflicts.", inputs.cluster_id));
        }
        let location = candidates[rng.random_range_i(0, candidates.len() as i32) as usize];
        assigned.insert(location);
        world_forbidden.extend(get_rings(location, 1, *buffer));
        poi_world_avoidance.extend(get_rings(location, 1, max_radius));
        worlds.push(WorldLocation { name: name.clone(), location });
    }

    let mut pois = Vec::new();
    let mut unplaced_pois = Vec::new();
    if inputs.cluster_space {
        let mut clump: HashSet<Axial> = HashSet::new();
        let mut poi_forbidden: HashSet<Axial> = HashSet::new();
        let clump_chance = 0.5f32;
        let max_clump = 3;
        let mut clumped = 0;
        for (index, placement) in inputs.poi_placements.iter().flatten().enumerate() {
            let mut names = placement.pois.clone();
            let (ring_min, ring_max) = placement.allowed_rings;
            let rings = get_rings(Axial::ZERO, ring_min, ring_max.min(num_rings - 1));
            for _ in 0..placement.num_to_spawn {
                // RandomRange(0f, 1f) is drawn for every spawn, used or not.
                let try_clump = rng.random_range_f(0.0, 1.0) <= clump_chance;
                let mut candidates: Vec<Axial> = Vec::new();
                if try_clump && clumped < max_clump && !placement.avoid_clumping {
                    clumped += 1;
                    candidates = rings.iter().copied()
                        .filter(|l| !assigned.contains(l) && clump.contains(l) && !poi_world_avoidance.contains(l)).collect();
                }
                if candidates.is_empty() {
                    clumped = 0;
                    clump.clear();
                    candidates = rings.iter().copied()
                        .filter(|l| !assigned.contains(l) && !poi_world_avoidance.contains(l) && !poi_forbidden.contains(l)).collect();
                }
                if placement.guarantee && candidates.is_empty() {
                    clumped = 0;
                    clump.clear();
                    candidates = rings.iter().copied().filter(|l| !assigned.contains(l) && !poi_world_avoidance.contains(l)).collect();
                }
                if candidates.is_empty() {
                    unplaced_pois.push(index);
                    continue;
                }
                let location = candidates[rng.random_range_i(0, candidates.len() as i32) as usize];
                let id = names[rng.random_range_i(0, names.len() as i32) as usize].clone();
                if !placement.can_spawn_duplicates {
                    // List.Remove drops the first match.
                    if let Some(at) = names.iter().position(|n| *n == id) { names.remove(at); }
                }
                pois.push(PoiLocation::new(id, location));
                poi_forbidden.extend(get_rings(location, 1, 3));
                clump.extend(get_rings(location, 1, 1));
                assigned.insert(location);
            }
        }
    }
    Ok(Starmap { num_rings, worlds, pois, unplaced_pois, vanilla_destinations: Vec::new() })
}

/// `HarvestablePOIConfig.GenerateConfigs`: (type id, poiCapacityMin,
/// poiCapacityMax, poiRechargeMin, poiRechargeMax, canProvideArtifacts).
/// Prefab ids are `HarvestableSpacePOI_` + type id.
const HARVESTABLE_TYPES: &[(&str, f32, f32, f32, f32, bool)] = &[
    ("CarbonAsteroidField", 30000.0, 45000.0, 30000.0, 60000.0, true),
    ("MetallicAsteroidField", 54000.0, 81000.0, 30000.0, 60000.0, true),
    ("SatelliteField", 30000.0, 45000.0, 30000.0, 60000.0, true),
    ("RockyAsteroidField", 81000.0, 94000.0, 30000.0, 60000.0, true),
    ("InterstellarIceField", 54000.0, 81000.0, 30000.0, 60000.0, true),
    ("OrganicMassField", 54000.0, 81000.0, 30000.0, 60000.0, true),
    ("IceAsteroidField", 54000.0, 81000.0, 30000.0, 60000.0, true),
    ("GasGiantCloud", 15000.0, 20000.0, 30000.0, 60000.0, true),
    ("ChlorineCloud", 54000.0, 81000.0, 30000.0, 60000.0, true),
    ("GildedAsteroidField", 30000.0, 45000.0, 30000.0, 60000.0, true),
    ("GlimmeringAsteroidField", 30000.0, 45000.0, 30000.0, 60000.0, true),
    ("HeliumCloud", 30000.0, 45000.0, 30000.0, 60000.0, true),
    ("OilyAsteroidField", 15000.0, 25000.0, 30000.0, 60000.0, true),
    ("OxidizedAsteroidField", 54000.0, 81000.0, 30000.0, 60000.0, true),
    ("SaltyAsteroidField", 54000.0, 81000.0, 30000.0, 60000.0, true),
    ("FrozenOreField", 54000.0, 81000.0, 30000.0, 60000.0, true),
    ("ForestyOreField", 54000.0, 81000.0, 30000.0, 60000.0, true),
    ("SwampyOreField", 54000.0, 81000.0, 30000.0, 60000.0, true),
    ("SandyOreField", 54000.0, 81000.0, 30000.0, 60000.0, true),
    ("RadioactiveGasCloud", 5000.0, 10000.0, 30000.0, 60000.0, true),
    ("RadioactiveAsteroidField", 5000.0, 10000.0, 30000.0, 60000.0, true),
    ("OxygenRichAsteroidField", 15000.0, 25000.0, 30000.0, 60000.0, true),
    ("InterstellarOcean", 15000.0, 25000.0, 30000.0, 60000.0, true),
    ("DLC2CeresField", 15000.0, 25000.0, 30000.0, 60000.0, true),
    ("DLC2CeresOreField", 15000.0, 25000.0, 30000.0, 60000.0, true),
    ("DLC4PrehistoricMixingField", 15000.0, 25000.0, 30000.0, 60000.0, true),
    ("DLC4PrehistoricOreField", 15000.0, 25000.0, 30000.0, 60000.0, true),
    ("DLC4ImpactorDebrisField1", 35000.0, 45000.0, 30000.0, 30000.0, false),
    ("DLC4ImpactorDebrisField2", 33400.0, 66800.0, 30000.0, 30000.0, false),
    ("DLC4ImpactorDebrisField3", 110000.0, 137500.0, 30000.0, 30000.0, false),
    ("DLC5AquaticMixingField", 15000.0, 25000.0, 30000.0, 60000.0, true),
    ("DLC5AquaticOreField", 15000.0, 25000.0, 30000.0, 60000.0, true),
];

/// `ArtifactPOIConfig.GenerateConfigs` types (prefab `ArtifactSpacePOI_` +
/// id). They and `ArtifactPOIConfigurator.defaultArtifactPoiType`, which
/// harvestable POIs carry, all recharge in 30000..60000.
const ARTIFACT_TYPES: &[&str] = &["GravitasSpaceStation1", "GravitasSpaceStation2", "GravitasSpaceStation3", "GravitasSpaceStation4",
    "GravitasSpaceStation5", "GravitasSpaceStation6", "GravitasSpaceStation7", "GravitasSpaceStation8", "RussellsTeapot"];
const ARTIFACT_RECHARGE: (f32, f32) = (30000.0, 60000.0);

/// `(int)ClusterGrid.GetPosition(entity).x` and `.y`. With a single entity in
/// the hex (every POI at spawn: placements never share a hex) that is
/// `AxialUtil.AxialToWorld(r, q)` = (√3·r + √3/2·q, -1.5·q).
fn world_position(a: Axial) -> (i32, i32) {
    // Mathf.Sqrt(3f) is a float; Mono evaluates the rest in double and rounds
    // into the Vector3's float fields. (int) truncates toward zero.
    let sqrt3 = 3f32.sqrt() as f64;
    let x = (sqrt3 * a.r as f64 + sqrt3 / 2.0 * a.q as f64) as f32;
    let y = (-1.5 * a.q as f64) as f32;
    (x as i32, y as i32)
}

/// The `KRandom` each configurator's `CreateRandomInstance` builds.
fn poi_random(global_world_seed: i32, location: Axial) -> KRandom {
    let (x, y) = world_position(location);
    KRandom::new(global_world_seed.wrapping_add(x).wrapping_add(y))
}

/// `Roll(randomSource, presetMin, presetMax)`:
/// `(float)(NextDouble() * (max - min)) + min`, with the prefabs' presets 0
/// and 1 leaving the plain float.
fn roll(rng: &mut KRandom) -> f32 {
    rng.next_double() as f32
}

/// `MathUtil.ReRange(roll, 0, 1, min, max)`, in Mono's double evaluation.
fn re_range(roll: f32, min: f32, max: f32) -> f32 {
    (roll as f64 * (max as f64 - min as f64) + min as f64) as f32
}

/// Fill in each POI's configuration rolls. A harvestable POI that
/// `canProvideArtifacts` carries both configurators; each builds its own
/// `KRandom` on the same seed, so its artifact roll equals its capacity roll.
pub fn roll_pois(starmap: &mut Starmap, global_world_seed: i32) -> Result<(), String> {
    for poi in &mut starmap.pois {
        let artifacts = if let Some(ty) = poi.id.strip_prefix("HarvestableSpacePOI_") {
            let &(_, capacity_min, capacity_max, recharge_min, recharge_max, artifacts) = HARVESTABLE_TYPES.iter()
                .find(|t| t.0 == ty).ok_or_else(|| format!("unknown harvestable space POI {}", poi.id))?;
            let mut rng = poi_random(global_world_seed, poi.location);
            let capacity = roll(&mut rng);
            let recharge = roll(&mut rng);
            poi.capacity_roll = Some(capacity);
            poi.recharge_roll = Some(recharge);
            poi.total_capacity = Some(re_range(capacity, capacity_min, capacity_max));
            poi.recharge_time = Some(re_range(recharge, recharge_min, recharge_max));
            artifacts
        } else if let Some(ty) = poi.id.strip_prefix("ArtifactSpacePOI_") {
            if !ARTIFACT_TYPES.contains(&ty) { return Err(format!("unknown artifact space POI {}", poi.id)); }
            true
        } else {
            // TemporalTear has no configurator.
            false
        };
        if artifacts {
            let recharge = roll(&mut poi_random(global_world_seed, poi.location));
            poi.artifact_recharge_roll = Some(recharge);
            poi.artifact_recharge_time = Some(re_range(recharge, ARTIFACT_RECHARGE.0, ARTIFACT_RECHARGE.1));
        }
    }
    Ok(())
}

/// `SpacecraftManager.GenerateFixedDestinations`: (type, distance), ids 0..6.
const FIXED_DESTINATIONS: [(&str, i32); 6] = [("CarbonaceousAsteroid", 0), ("CarbonaceousAsteroid", 0), ("MetallicAsteroid", 1),
    ("RockyAsteroid", 2), ("IcyDwarf", 3), ("OrganicDwarf", 4)];

/// `GenerateRandomDestinations`' type lists, indexed by distance, as
/// `SpaceDestinationType` ids (`OilyAsteroid` is `OilyAsteriod` and
/// `HydrogenGiant` is `HeliumGiant`).
const DESTINATION_TIERS: [&[&str]; 17] = [
    &[],
    &["OilyAsteriod"],
    &["Satellite"],
    &["Satellite", "RockyAsteroid", "CarbonaceousAsteroid", "ForestPlanet"],
    &["MetallicAsteroid", "RockyAsteroid", "CarbonaceousAsteroid", "SaltDwarf"],
    &["MetallicAsteroid", "RockyAsteroid", "CarbonaceousAsteroid", "IcyDwarf", "OrganicDwarf"],
    &["IcyDwarf", "OrganicDwarf", "DustyMoon", "ChlorinePlanet", "RedDwarf"],
    &["DustyMoon", "TerraPlanet", "VolcanoPlanet"],
    &["TerraPlanet", "GasGiant", "IceGiant", "RustPlanet"],
    &["GasGiant", "IceGiant", "HeliumGiant"],
    &["RustPlanet", "VolcanoPlanet", "RockyAsteroid", "TerraPlanet", "MetallicAsteroid"],
    &["ShinyPlanet", "MetallicAsteroid", "RockyAsteroid"],
    &["GoldAsteroid", "OrganicDwarf", "ForestPlanet", "ChlorinePlanet"],
    &["IcyDwarf", "MetallicAsteroid", "DustyMoon", "VolcanoPlanet", "IceGiant"],
    &["ShinyPlanet", "RedDwarf", "RockyAsteroid", "GasGiant"],
    &["HeliumGiant", "ForestPlanet", "OilyAsteriod"],
    &["GoldAsteroid", "SaltDwarf", "TerraPlanet", "VolcanoPlanet"],
];

/// `SpacecraftManager.RestoreDestinations` on a new game: the fixed
/// destinations, then `GenerateRandomDestinations` and the sort by distance
/// (`List.Sort`, unstable). `dlc_mixing` is `GetCurrentDlcMixingIds`, whose
/// `spaceDesinations` either take an unused tier slot or replace the first
/// random destination in range.
pub fn vanilla_destinations(ds: &DataSource, global_world_seed: i32, dlc_mixing: &[String]) -> Vec<SpaceDestination> {
    let destination = |id: i32, ty: &str, distance: i32| SpaceDestination { id, type_id: ty.to_string(), distance };
    let mut destinations: Vec<SpaceDestination> = FIXED_DESTINATIONS.iter().enumerate()
        .map(|(i, &(ty, distance))| destination(i as i32, ty, distance)).collect();
    let mut rng = KRandom::new(global_world_seed);
    // Three slots per non-empty tier.
    let mut tiers: Vec<i32> = (0..DESTINATION_TIERS.len() as i32)
        .filter(|&i| !DESTINATION_TIERS[i as usize].is_empty()).flat_map(|i| [i; 3]).collect();
    let mut next_id = destinations.len() as i32;
    let count = rng.next_range(15, 25);
    let mut random = Vec::with_capacity(count as usize);
    for _ in 0..count {
        // Next(0, Count - 1) never picks the last slot.
        let tier = tiers.remove(rng.next_range(0, tiers.len() as i32 - 1) as usize);
        let types = DESTINATION_TIERS[tier as usize];
        random.push(destination(next_id, types[rng.next_range(0, types.len() as i32) as usize], tier));
        next_id += 1;
    }
    shuffle_seeded(&mut tiers, &mut rng);
    let mut mixed = Vec::new();
    for mixes in dlc_mixing.iter().filter_map(|id| ds.dlc_mixing_space_destinations.get(id)) {
        for (ty, min_tier, max_tier) in mixes {
            let in_range = |d: i32| d >= *min_tier && d <= *max_tier;
            if let Some(at) = tiers.iter().position(|&t| in_range(t)) {
                mixed.push(destination(next_id, ty, tiers.remove(at)));
                next_id += 1;
            } else if let Some(d) = random.iter_mut().find(|d| in_range(d.distance)) {
                d.type_id = ty.clone();
            }
            // Otherwise the game only logs "Mixing: Unable to place destination".
        }
    }
    destinations.extend(random);
    destinations.push(destination(next_id, "Earth", 4));
    destinations.push(destination(next_id + 1, "Wormhole", DESTINATION_TIERS.len() as i32));
    destinations.extend(mixed);
    mono_sort(&mut destinations, |a, b| a.distance.cmp(&b.distance));
    destinations
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_sizes_and_order() {
        assert_eq!(get_ring(Axial::ZERO, 0), vec![Axial::ZERO]);
        for r in 1..6 { assert_eq!(get_ring(Axial::ZERO, r).len(), 6 * r as usize); }
        // Starts at the NW corner and walks east.
        assert_eq!(&get_ring(Axial::ZERO, 2)[..3], &[Axial::new(0, -2), Axial::new(1, -2), Axial::new(2, -2)]);
        let all = get_rings(Axial::ZERO, 0, 4);
        assert_eq!(all.len(), 61);
        assert_eq!(all.iter().collect::<HashSet<_>>().len(), 61);
    }

    /// `expansion1::clusters/MiniClusterRadioactiveOceanStart` world placements.
    fn mini_radioactive(seed: i32) -> Inputs<'static> {
        let w = |n: &str, rings, buffer| (format!("expansion1::worlds/{n}"), rings, buffer);
        Inputs {
            cluster_id: "expansion1::clusters/MiniClusterRadioactiveOceanStart", seed, layout_num_rings: 14,
            worlds: vec![w("MiniBadlands", (0, 1), 2), w("MiniRadioactiveOceanStart", (2, 4), 2), w("MiniFlippedWarp", (2, 4), 2),
                w("MiniMetallicSwampy", (1, 3), 2), w("MiniForestFrozen", (1, 3), 2), w("TundraMoonlet", (7, 9), 4),
                w("MarshyMoonlet", (6, 8), 4), w("MooMoonlet", (7, 9), 3), w("WaterMoonlet", (7, 9), 4),
                w("NiobiumMoonlet", (8, 11), 3), w("RegolithMoonlet", (6, 11), 4)],
            poi_placements: None, cluster_space: true,
        }
    }

    /// The game reported this failure for M-RAD-C-5-0-0-0 (Spaced Out + DLC2).
    #[test]
    fn mini_radioactive_seed_5_fails_like_the_game() {
        assert_eq!(assign(&mini_radioactive(5)).unwrap_err(), "Worldgen failure: Could not find a spot in the cluster for expansion1::worlds/MiniForestFrozen EVEN AFTER REDUCING BUFFERS. Check the placement settings in expansion1::clusters/MiniClusterRadioactiveOceanStart.yaml to ensure there are no conflicts.");
        let ok = assign(&mini_radioactive(1)).unwrap();
        assert_eq!(ok.worlds.len(), 11);
        assert_eq!(ok.worlds.iter().map(|w| w.location).collect::<HashSet<_>>().len(), 11);
    }

    fn poi(id: &str, r: i32, q: i32) -> PoiLocation { PoiLocation::new(id.into(), Axial::new(r, q)) }

    /// SNDST-C-1-0-0-0 (nine worlds, so globalWorldSeed 9). The rolls agree
    /// with Onimaxxing; three POIs share one roll because (int) truncation and
    /// KRandom's Math.Abs map seeds 9 + 1 and 9 - 19 to the same stream.
    #[test]
    fn poi_rolls_follow_hex_world_positions() {
        let mut starmap = Starmap { num_rings: 12, worlds: Vec::new(), unplaced_pois: Vec::new(), vanilla_destinations: Vec::new(), pois: vec![
            poi("TemporalTear", 8, -5), poi("HarvestableSpacePOI_OrganicMassField", -2, -1), poi("HarvestableSpacePOI_FrozenOreField", 3, 6),
            poi("HarvestableSpacePOI_OilyAsteroidField", 3, 7), poi("HarvestableSpacePOI_RadioactiveGasCloud", -10, 4),
            poi("ArtifactSpacePOI_RussellsTeapot", 0, 11)] };
        roll_pois(&mut starmap, 9).unwrap();
        let p = &starmap.pois;
        assert!(p[0].capacity_roll.is_none() && p[0].artifact_recharge_roll.is_none());
        assert_eq!((p[1].capacity_roll, p[1].recharge_roll), (Some(0.8607951402664185), Some(0.5778370499610901)));
        // Mono rounds ReRange once; float32 per operator would give 47335.109375.
        assert_eq!((p[1].total_capacity, p[1].recharge_time), (Some(77241.46875), Some(47335.11328125)));
        assert_eq!(p[1].artifact_recharge_roll, p[1].capacity_roll);
        for q in &p[2..5] { assert_eq!(q.capacity_roll, Some(0.9504964351654053)); }
        assert_eq!(p[3].total_capacity, Some(24504.96484375));
        assert!(p[5].capacity_roll.is_none() && p[5].artifact_recharge_roll.is_some());
        let mut unknown = Starmap { pois: vec![poi("HarvestableSpacePOI_Nothing", 1, 1)], ..starmap };
        assert!(roll_pois(&mut unknown, 9).is_err());
    }

    /// SNDST-A-1-0-0-0. Onimaxxing lists the same destinations but keeps
    /// equal distances in id order; the game's List.Sort is an introsort.
    #[test]
    fn vanilla_destinations_seed_1() {
        let d = vanilla_destinations(&DataSource::default(), 1, &[]);
        assert_eq!(d.iter().map(|d| d.id).collect::<Vec<_>>(),
            [0, 1, 22, 2, 3, 6, 16, 4, 18, 23, 5, 11, 19, 17, 8, 14, 10, 13, 15, 21, 7, 20, 9, 12, 24]);
        assert_eq!(d.iter().map(|d| d.type_id.as_str()).collect::<Vec<_>>(), [
            "CarbonaceousAsteroid", "CarbonaceousAsteroid", "OilyAsteriod", "MetallicAsteroid", "RockyAsteroid", "Satellite", "Satellite",
            "IcyDwarf", "ForestPlanet", "Earth", "OrganicDwarf", "RockyAsteroid", "OrganicDwarf", "ChlorinePlanet", "TerraPlanet",
            "HeliumGiant", "RustPlanet", "ShinyPlanet", "RockyAsteroid", "ForestPlanet", "VolcanoPlanet", "RockyAsteroid", "HeliumGiant",
            "TerraPlanet", "Wormhole"]);
        assert_eq!(d.iter().map(|d| d.distance).collect::<Vec<_>>(), [0, 0, 1, 1, 2, 2, 2, 3, 3, 4, 4, 4, 5, 6, 7, 9, 10, 11, 11, 12, 13, 14, 15, 16, 17]);
    }

    /// SNDST-A-11-0-0-DX1WI3Y1 (DLC2 mixing): the Ceres destination takes a
    /// free tier slot and an id before Earth's.
    #[test]
    fn vanilla_destinations_take_dlc_mixing() {
        let mut ds = DataSource::default();
        ds.dlc_mixing_space_destinations.insert("DLC2_ID".into(), vec![("DLC2CeresSpaceDestination".into(), 3, 10)]);
        let d = vanilla_destinations(&ds, 11, &["DLC2_ID".into()]);
        let find = |ty: &str| d.iter().find(|d| d.type_id == ty).map(|d| (d.id, d.distance));
        assert_eq!((find("DLC2CeresSpaceDestination"), find("Earth"), find("Wormhole")), (Some((25, 5)), Some((26, 4)), Some((27, 17))));
        assert_eq!(vanilla_destinations(&ds, 11, &[]).len(), d.len() - 1);
    }
}
