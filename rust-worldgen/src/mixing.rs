//! Worldgen mixing (build 744825): `ProcGenGame.WorldgenMixing` plus the
//! setting filters it depends on (`CustomGameSettings.GetActive*MixingSettings`,
//! `GetCurrentDlcMixingIds` and `RemoveInvalidMixingSettings`, which
//! `ColonyDestinationSelectScreen.LaunchClicked` runs before worldgen).
//!
//! World mixing runs in the `Cluster` constructor (`InitializeWorlds`) and
//! substitutes world placements before BestFit and trait selection. Subworld
//! mixing runs in `Cluster.BeginGeneration`, after traits and world mixing
//! additions are merged, and swaps `(MixingN)` proxies in each world's
//! `unknownCellsAllowedSubworlds`. Each uses its own `KRandom(clusterSeed)`.
//! DLC mixing only adds starmap content (`DlcMixingSettings.spacePois` /
//! `spaceDesinations`) and makes DLC-gated mixing settings valid.

use std::cmp::Ordering;
use std::collections::BTreeMap;

use crate::custom_settings::CustomSettings;
use crate::data_source::{ClusterYaml, DataSource, SubworldFileRef, SubworldMixingYaml, WorldYaml};
use crate::rng::KRandom;

#[derive(Debug, Clone, Copy, PartialEq)]
enum Kind { Dlc, World, Subworld }

/// `CustomMixingSettingsConfigs` in `CustomGameSettings.OnPrefabInit`
/// registration order (= `MixingSettings` dictionary order):
/// (id, kind, worldgenPath, required content / DLC id).
const CONFIGS: &[(&str, Kind, &str, &str)] = &[
    ("DLC2_ID", Kind::Dlc, "", "DLC2_ID"),
    ("IceCavesMixing", Kind::Subworld, "dlc2::subworldMixing/IceCavesMixingSettings", "DLC2_ID"),
    ("CarrotQuarryMixing", Kind::Subworld, "dlc2::subworldMixing/CarrotQuarryMixingSettings", "DLC2_ID"),
    ("SugarWoodsMixing", Kind::Subworld, "dlc2::subworldMixing/SugarWoodsMixingSettings", "DLC2_ID"),
    ("CeresAsteroidMixing", Kind::World, "dlc2::worldMixing/CeresMixingSettings", "DLC2_ID"),
    ("DLC3_ID", Kind::Dlc, "", "DLC3_ID"),
    ("DLC4_ID", Kind::Dlc, "", "DLC4_ID"),
    ("GardenMixing", Kind::Subworld, "dlc4::subworldMixing/GardenMixingSettings", "DLC4_ID"),
    ("RaptorMixing", Kind::Subworld, "dlc4::subworldMixing/RaptorMixingSettings", "DLC4_ID"),
    ("WetlandsMixing", Kind::Subworld, "dlc4::subworldMixing/WetlandsMixingSettings", "DLC4_ID"),
    ("PrehistoricAsteroidMixing", Kind::World, "dlc4::worldMixing/PrehistoricMixingSettings", "DLC4_ID"),
    ("DLC5_ID", Kind::Dlc, "", "DLC5_ID"),
    ("BeachMixing", Kind::Subworld, "dlc5::subworldMixing/BeachMixingSettings", "DLC5_ID"),
    ("ReefMixing", Kind::Subworld, "dlc5::subworldMixing/ReefMixingSettings", "DLC5_ID"),
    ("KelpForestMixing", Kind::Subworld, "dlc5::subworldMixing/KelpForestMixingSettings", "DLC5_ID"),
    ("AbyssMixing", Kind::Subworld, "dlc5::subworldMixing/AbyssMixingSettings", "DLC5_ID"),
    ("AquaticAsteroidMixing", Kind::World, "dlc5::worldMixing/AquaticMixingSettings", "DLC5_ID"),
];

/// `required_content` of a mixing setting (each needs exactly one DLC).
pub fn required_content(id: &str) -> Option<&'static str> {
    CONFIGS.iter().find(|c| c.0 == id).map(|c| c.3)
}

/// Mixing settings that survive `RemoveInvalidMixingSettings` for a cluster.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct Active {
    /// `GetCurrentDlcMixingIds`.
    pub dlc: Vec<String>,
    /// (worldgenPath, GuranteeMixing) in registration order.
    pub world: Vec<(String, bool)>,
    pub subworld: Vec<(String, bool)>,
    /// Non-disabled coordinate levels dropped as invalid for this cluster.
    pub removed: Vec<String>,
}

///
/// With an explicit active DLC set, a setting whose DLC is inactive reads as
/// Disabled (`GetCurrentMixingSettingLevel`) and the cluster's requirements are
/// resolved for that set; without one the cluster's declared requirements are used.
pub fn active(settings: &CustomSettings, cluster: &ClusterYaml, ds: &DataSource, active_dlc: Option<crate::dlc::ActiveDlc>) -> Result<Active, String> {
    let dlc_of = |id: &str| CONFIGS.iter().find(|c| c.0 == id).map(|c| c.3).unwrap_or("");
    let level = |id: &str| {
        if active_dlc.is_some_and(|d| !d.is_active(dlc_of(id))) { return "Disabled"; }
        settings.mixing.get(id).map(String::as_str).unwrap_or("Disabled")
    };
    let required = active_dlc.map_or_else(|| cluster.required_dlc_ids.clone(), |d| d.cluster_required_ids(cluster));
    for (id, _, _, dlc) in CONFIGS {
        if level(id) != "Disabled" && !ds.dlc_installed(dlc) {
            return Err(format!("mixing setting {id}={} needs {dlc}, whose worldgen data is not installed", level(id)));
        }
    }
    let mut out = Active::default();
    // A DLC the cluster already requires cannot also be mixed in.
    for (id, kind, _, _) in CONFIGS {
        if *kind == Kind::Dlc && level(id) == "Enabled" {
            if required.iter().any(|d| d == id) { out.removed.push(id.to_string()); } else { out.dlc.push(id.to_string()); }
        }
    }
    let available = |dlc: &str| out.dlc.iter().chain(&required).any(|d| d == dlc);
    let (mut world, mut subworld, mut removed) = (Vec::new(), Vec::new(), Vec::new());
    for (id, kind, path, dlc) in CONFIGS {
        if *kind == Kind::Dlc || level(id) == "Disabled" { continue; }
        let forbidden = match kind {
            Kind::World => &ds.world_mixing.get(*path).ok_or_else(|| format!("missing world mixing settings {path}"))?.forbidden_cluster_tags,
            _ => &ds.subworld_mixing.get(*path).ok_or_else(|| format!("missing subworld mixing settings {path}"))?.forbidden_cluster_tags,
        };
        if !available(dlc) || forbidden.iter().any(|t| cluster.cluster_tags.contains(t)) { removed.push(id.to_string()); continue; }
        let entry = (path.to_string(), level(id) == "GuranteeMixing");
        if *kind == Kind::World { world.push(entry) } else { subworld.push(entry) }
    }
    out.world = world;
    out.subworld = subworld;
    out.removed.extend(removed);
    Ok(out)
}

/// `WorldGenUtil.ShuffleSeeded(IList<T>, KRandom)`.
pub fn shuffle_seeded<T>(v: &mut [T], rng: &mut KRandom) {
    let mut n = v.len();
    while n > 1 {
        n -= 1;
        let k = rng.next_max(n as i32 + 1) as usize;
        v.swap(k, n);
    }
}

/// Mono `List<T>.Sort(Comparison<T>)` (`ArraySortHelper.IntrospectiveSort`); unstable.
pub fn mono_sort<T: Clone>(a: &mut [T], cmp: impl Fn(&T, &T) -> Ordering) {
    fn swap_if<T>(a: &mut [T], cmp: &impl Fn(&T, &T) -> Ordering, i: usize, j: usize) {
        if i != j && cmp(&a[i], &a[j]) == Ordering::Greater { a.swap(i, j); }
    }
    fn down<T: Clone>(a: &mut [T], cmp: &impl Fn(&T, &T) -> Ordering, mut i: usize, n: usize, lo: usize) {
        let value = a[lo + i - 1].clone();
        while i <= n / 2 {
            let mut child = 2 * i;
            if child < n && cmp(&a[lo + child - 1], &a[lo + child]) == Ordering::Less { child += 1; }
            if cmp(&value, &a[lo + child - 1]) != Ordering::Less { break; }
            a[lo + i - 1] = a[lo + child - 1].clone();
            i = child;
        }
        a[lo + i - 1] = value;
    }
    fn intro<T: Clone>(a: &mut [T], cmp: &impl Fn(&T, &T) -> Ordering, lo: usize, mut hi: usize, mut depth: u32) {
        while hi > lo {
            let n = hi - lo + 1;
            if n <= 16 {
                match n {
                    2 => swap_if(a, cmp, lo, hi),
                    3 => { swap_if(a, cmp, lo, hi - 1); swap_if(a, cmp, lo, hi); swap_if(a, cmp, hi - 1, hi); }
                    _ => for i in lo..hi {
                        let value = a[i + 1].clone();
                        let mut j = i as isize;
                        while j >= lo as isize && cmp(&value, &a[j as usize]) == Ordering::Less { a[j as usize + 1] = a[j as usize].clone(); j -= 1; }
                        a[(j + 1) as usize] = value;
                    },
                }
                return;
            }
            if depth == 0 {
                for i in (1..=n / 2).rev() { down(a, cmp, i, n, lo); }
                for i in (2..=n).rev() { a.swap(lo, lo + i - 1); down(a, cmp, 1, i - 1, lo); }
                return;
            }
            depth -= 1;
            let mid = lo + (hi - lo) / 2;
            swap_if(a, cmp, lo, mid); swap_if(a, cmp, lo, hi); swap_if(a, cmp, mid, hi);
            let pivot = a[mid].clone();
            a.swap(mid, hi - 1);
            let (mut left, mut right) = (lo, hi - 1);
            while left < right {
                left += 1;
                while cmp(&a[left], &pivot) == Ordering::Less { left += 1; }
                right -= 1;
                while cmp(&pivot, &a[right]) == Ordering::Less { right -= 1; }
                if left >= right { break; }
                a.swap(left, right);
            }
            a.swap(left, hi - 1);
            intro(a, cmp, left + 1, hi, depth);
            if left == 0 { return; }
            hi = left - 1;
        }
    }
    if a.len() < 2 { return; }
    let depth = 2 * (usize::BITS - a.len().leading_zeros());
    intro(a, &cmp, 0, a.len() - 1, depth);
}

/// `MixingOption<T>`: GuranteeMixing sets minCount 1.
#[derive(Debug, Clone)]
struct Option_<T> { path: String, settings: T, min: i32, max: i32 }

/// `FindWorldMixingOption` / `FindSubworldMixing`: walk a LINQ `OrderBy`
/// (stable) of `MixingOption.CompareTo` (minCount desc, then maxCount desc),
/// skipping exhausted options.
fn find<T>(options: &[Option_<T>], ok: impl Fn(&Option_<T>) -> bool) -> Option<usize> {
    let mut order: Vec<usize> = (0..options.len()).collect();
    order.sort_by(|&a, &b| options[b].min.cmp(&options[a].min).then(options[b].max.cmp(&options[a].max)));
    order.into_iter().find(|&i| options[i].max > 0 && ok(&options[i]))
}

fn consume<T>(options: &mut Vec<Option_<T>>, i: usize) {
    options[i].min -= 1;
    options[i].max -= 1;
    if options[i].max <= 0 { options.remove(i); }
}

fn unsatisfied<T>(options: &[Option_<T>], what: &str) -> Result<(), String> {
    let missing: Vec<String> = options.iter().filter(|o| o.min > 0).map(|o| format!("{} ({})", o.path, o.min)).collect();
    if missing.is_empty() { Ok(()) } else { Err(format!("WorldgenMixing: Could not guarantee these {what} mixings: {}", missing.join("\n - "))) }
}

/// `WorldgenMixing.DoWorldMixingInternal`. Returns the substituted world per
/// placement (None where the cluster YAML world stays).
pub fn world_mixing(ds: &DataSource, cluster: &ClusterYaml, seed: i32, active: &[(String, bool)]) -> Result<Vec<Option<String>>, String> {
    let mut options = Vec::new();
    for (path, guarantee) in active {
        let settings = ds.world_mixing.get(path).ok_or_else(|| format!("missing world mixing settings {path}"))?;
        let world = ds.world(&settings.world).ok_or_else(|| format!("missing mixing world {}", settings.world))?;
        options.push(Option_ { path: settings.world.clone(), settings: world, min: *guarantee as i32, max: 1 });
    }
    let mut rng = KRandom::new(seed);
    let mut order: Vec<usize> = (0..cluster.world_placements.len()).collect();
    shuffle_seeded(&mut order, &mut rng);
    let mut out = vec![None; order.len()];
    for i in order {
        let m = &cluster.world_placements[i].world_mixing;
        if m.required_tags.is_empty() && m.forbidden_tags.is_empty() { continue; }
        shuffle_seeded(&mut options, &mut rng);
        let pick = find(&options, |o| m.required_tags.iter().all(|t| o.settings.world_tags.contains(t))
            && !m.forbidden_tags.iter().any(|t| o.settings.world_tags.contains(t)));
        if let Some(k) = pick {
            out[i] = Some(options[k].path.clone());
            consume(&mut options, k);
        }
    }
    unsatisfied(&options, "world")?;
    Ok(out)
}

/// One proxy replacement from `ApplySubworldMixingToWorld`.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SubworldMix {
    /// Scoped path of the subworld mixing settings.
    #[serde(skip)]
    pub settings: String,
    pub proxy: String,
    /// Copy of the mixing subworld with counts clamped to the rule.
    pub file: SubworldFileRef,
    pub additional_world_template_rules: Vec<serde_yaml::Value>,
}

/// `WorldPlacement.GetSortOrder`.
fn sort_order(location_type: &str) -> i32 {
    match location_type.to_ascii_lowercase().as_str() { "startworld" => 1, "innercluster" => 2, _ => 3 }
}

/// `WorldgenMixing.DoSubworldMixing`. `worlds` are the (world-mixed) worlds
/// in placement order with their placement `locationType`.
pub fn subworld_mixing(ds: &DataSource, worlds: &[(&WorldYaml, &str)], seed: i32, active: &[(String, bool)]) -> Result<Vec<Vec<SubworldMix>>, String> {
    let mut options: Vec<Option_<&SubworldMixingYaml>> = Vec::new();
    for (path, guarantee) in active {
        let settings = ds.subworld_mixing.get(path).ok_or_else(|| format!("missing subworld mixing settings {path}"))?;
        options.push(Option_ { path: path.clone(), settings, min: *guarantee as i32, max: 3 });
    }
    let mut rng = KRandom::new(seed);
    let mut order: Vec<usize> = (0..worlds.len()).collect();
    shuffle_seeded(&mut order, &mut rng);
    mono_sort(&mut order, |&a, &b| sort_order(worlds[a].1).cmp(&sort_order(worlds[b].1)));
    let mut out = vec![Vec::new(); worlds.len()];
    for i in order {
        shuffle_seeded(&mut options, &mut rng);
        out[i] = apply_to_world(worlds[i].0, &mut options);
    }
    unsatisfied(&options, "subworld")?;
    Ok(out)
}

/// `ApplySubworldMixingToWorld`. Leftover proxies are dropped later by the
/// caller (`CleanupUnusedMixing` / `IsMixingProxyName`: names starting with '(').
fn apply_to_world(world: &WorldYaml, options: &mut Vec<Option_<&SubworldMixingYaml>>) -> Vec<SubworldMix> {
    let mut applied = Vec::new();
    for rule in &world.subworld_mixing_rules {
        if options.is_empty() { break; }
        let pick = find(options, |o| {
            let tags = &o.settings.mixing_tags;
            !rule.forbidden_tags.iter().any(|t| tags.contains(t)) && rule.required_tags.iter().all(|t| tags.contains(t))
                && rule.min_count.max(o.settings.subworld.min_count) <= rule.max_count.min(o.settings.subworld.max_count)
        });
        let Some(k) = pick else { continue };
        let settings = options[k].settings;
        let mut file = settings.subworld.clone();
        file.min_count = rule.min_count.max(file.min_count);
        file.max_count = rule.max_count.min(file.max_count);
        applied.push(SubworldMix { settings: options[k].path.clone(), proxy: rule.name.clone(), file, additional_world_template_rules: settings.additional_world_template_rules.clone() });
        consume(options, k);
    }
    applied
}

/// Proxy name -> replacement subworld. The game rewrites every occurrence of a
/// rule's proxy when the rule is applied, so a later rule reusing the same
/// proxy name finds nothing left to rewrite.
pub fn proxy_names(applied: &[SubworldMix]) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for m in applied { map.entry(m.proxy.clone()).or_insert_with(|| m.file.name.clone()); }
    map
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data_source::{PlacementMixing, SubworldMixingRule, WorldMixingYaml, WorldPlacementYaml};

    fn file(name: &str, min: i32, max: i32) -> SubworldFileRef {
        SubworldFileRef { name: name.into(), weight: 1.0, min_count: min, max_count: max, priority: 0, override_power: 0.0 }
    }
    fn world(name: &str, tags: &[&str], rules: &[(&str, i32, i32, &[&str], &[&str])]) -> WorldYaml {
        let mut w = WorldYaml { qname: name.into(), size: (64, 64), ..Default::default() };
        w.world_tags = tags.iter().map(|s| s.to_string()).collect();
        w.subworld_mixing_rules = rules.iter().map(|(n, min, max, req, forb)| SubworldMixingRule {
            name: n.to_string(), min_count: *min, max_count: *max,
            required_tags: req.iter().map(|s| s.to_string()).collect(), forbidden_tags: forb.iter().map(|s| s.to_string()).collect() }).collect();
        w
    }
    fn cluster(placements: Vec<(&str, &str, &[&str], &[&str])>, tags: &[&str], required: &[&str]) -> ClusterYaml {
        ClusterYaml { id: String::new(), poi_placements: None, coordinate_prefix: "T".into(), start_world_index: 0, num_rings: 0, disable_story_traits: false, fixed_coordinate: -1, skip: false,
            world_placements: placements.into_iter().map(|(w, loc, req, forb)| WorldPlacementYaml { world_ref: w.into(), buffer: 2,
                location_type: loc.into(), allowed_rings: None, world_mixing: PlacementMixing {
                    required_tags: req.iter().map(|s| s.to_string()).collect(), forbidden_tags: forb.iter().map(|s| s.to_string()).collect(),
                    ..Default::default() } }).collect(),
            required_dlc_ids: required.iter().map(|s| s.to_string()).collect(), forbidden_dlc_ids: vec![],
            cluster_tags: tags.iter().map(|s| s.to_string()).collect() }
    }
    fn mixing_data() -> DataSource {
        let mut ds = DataSource::default();
        // Installed DLC trees are those whose mixing.yaml loaded (DataSource::dlc_installed): DLC2 only here.
        ds.dlc_mixing_space_pois.insert("DLC2_ID".into(), Vec::new());
        ds.worlds.insert("dlc2::worlds/MixingCeresAsteroid".into(), world("dlc2::worlds/MixingCeresAsteroid",
            &["Mixing", "CeresWorld", "Cold", "SmallWorld", "SubsurfaceSubworldReserved", "NoDamagingMeteorShowers"], &[]));
        ds.world_mixing.insert("dlc2::worldMixing/CeresMixingSettings".into(), WorldMixingYaml {
            forbidden_cluster_tags: vec!["CeresCluster".into()], world: "dlc2::worlds/MixingCeresAsteroid".into() });
        for (id, name, tags) in [("IceCaves", "icecaves/IceCavesMix", &["StartSubworld", "Cold", "Ceres"][..]),
            ("CarrotQuarry", "carrotquarry/CarrotQuarryMix", &["Cold", "Ceres"][..]), ("SugarWoods", "sugarwoods/SugarWoodsMix", &["Cold", "Subsurface", "Ceres"][..])] {
            ds.subworld_mixing.insert(format!("dlc2::subworldMixing/{id}MixingSettings"), SubworldMixingYaml {
                forbidden_cluster_tags: vec!["CeresCluster".into()], subworld: file(&format!("dlc2::subworlds/{name}"), 0, i32::MAX),
                mixing_tags: tags.iter().map(|s| s.to_string()).collect(), additional_world_template_rules: vec![] });
        }
        ds
    }
    fn settings(levels: &[(&str, &str)]) -> CustomSettings {
        let mut s = crate::custom_settings::decode("0", "0", "0").unwrap();
        for (k, v) in levels { s.mixing.insert(k.to_string(), v.to_string()); }
        s
    }

    #[test]
    fn shuffle_matches_worldgen_util_fisher_yates() {
        // Hand-run of ShuffleSeeded: k = Next(n + 1) for n = len-1 .. 1, swap(k, n).
        let mut reference = KRandom::new(7);
        let mut expected: Vec<i32> = (0..6).collect();
        for n in (1..6).rev() { let k = reference.next_max(n as i32 + 1) as usize; expected.swap(k, n); }
        let mut v: Vec<i32> = (0..6).collect();
        shuffle_seeded(&mut v, &mut KRandom::new(7));
        assert_eq!(v, expected);
        // Lists of length 0/1 consume nothing.
        let (mut a, mut b) = (KRandom::new(3), KRandom::new(3));
        shuffle_seeded(&mut [1], &mut a);
        assert_eq!(a.next_raw(), b.next_raw());
    }

    #[test]
    fn mono_sort_three_element_network_is_unstable() {
        // IntroSort's size-3 case: SwapIfGreater(0,1), (0,2), (1,2).
        let mut v = [(3, 'a'), (3, 'b'), (1, 'c')];
        mono_sort(&mut v, |x, y| x.0.cmp(&y.0));
        assert_eq!(v, [(1, 'c'), (3, 'b'), (3, 'a')]);
        // Insertion sort (4..=16) is stable.
        let mut v = [(3, 'a'), (3, 'b'), (1, 'c'), (2, 'd')];
        mono_sort(&mut v, |x, y| x.0.cmp(&y.0));
        assert_eq!(v, [(1, 'c'), (2, 'd'), (3, 'a'), (3, 'b')]);
        let mut v: Vec<i32> = (0..40).map(|i| (i * 7919) % 41).collect();
        mono_sort(&mut v, |x, y| x.cmp(y));
        assert!(v.windows(2).all(|w| w[0] <= w[1]));
    }

    #[test]
    fn decodes_full_dlc2_mixing_code() {
        // node coordinate-codes.js X 1 mix:DLC2_ID=Enabled mix:{CeresAsteroid,IceCaves,CarrotQuarry,SugarWoods}Mixing=Guarantee
        let s = crate::custom_settings::decode("0", "0", "1UON93X2").unwrap();
        assert_eq!(s.level("DLC2_ID"), Some("Enabled"));
        for id in ["CeresAsteroidMixing", "IceCavesMixing", "CarrotQuarryMixing", "SugarWoodsMixing"] { assert_eq!(s.level(id), Some("GuranteeMixing")); }
        assert_eq!(CONFIGS.iter().map(|c| c.0).collect::<Vec<_>>(), crate::custom_settings::MIXING.iter().map(|c| c.0).collect::<Vec<_>>());
    }

    #[test]
    fn remove_invalid_mixing_settings() {
        let ds = mixing_data();
        let so = cluster(vec![("w", "StartWorld", &[], &[])], &[], &["EXPANSION1_ID"]);
        // DLC2 content is only available through DLC2 mixing or the cluster's required DLCs.
        let a = active(&settings(&[("CeresAsteroidMixing", "GuranteeMixing"), ("IceCavesMixing", "TryMixing")]), &so, &ds, None).unwrap();
        assert!(a.world.is_empty() && a.subworld.is_empty());
        assert_eq!(a.removed, ["IceCavesMixing", "CeresAsteroidMixing"]);
        let a = active(&settings(&[("DLC2_ID", "Enabled"), ("CeresAsteroidMixing", "GuranteeMixing"), ("SugarWoodsMixing", "TryMixing"),
            ("IceCavesMixing", "GuranteeMixing")]), &so, &ds, None).unwrap();
        assert_eq!(a.dlc, ["DLC2_ID"]);
        assert_eq!(a.world, [("dlc2::worldMixing/CeresMixingSettings".to_string(), true)]);
        assert_eq!(a.subworld, [("dlc2::subworldMixing/IceCavesMixingSettings".to_string(), true),
            ("dlc2::subworldMixing/SugarWoodsMixingSettings".to_string(), false)]);
        // Frosty clusters: DLC2 mixing is dropped (already required) but DLC2 settings stay valid...
        let frosty = cluster(vec![("w", "StartWorld", &[], &[])], &[], &["DLC2_ID"]);
        let a = active(&settings(&[("DLC2_ID", "Enabled"), ("CarrotQuarryMixing", "TryMixing")]), &frosty, &ds, None).unwrap();
        assert!(a.dlc.is_empty() && a.subworld.len() == 1);
        // ...except in CeresCluster-tagged clusters, which every shipped DLC2 mixing forbids.
        let ceres = cluster(vec![("w", "StartWorld", &[], &[])], &["CeresCluster"], &["DLC2_ID"]);
        let a = active(&settings(&[("CarrotQuarryMixing", "TryMixing"), ("CeresAsteroidMixing", "TryMixing")]), &ceres, &ds, None).unwrap();
        assert!(a.world.is_empty() && a.subworld.is_empty());
        // DLC3/4/5 data is not installed.
        for id in ["DLC3_ID", "GardenMixing", "AquaticAsteroidMixing"] {
            let level = if id.starts_with("DLC") { "Enabled" } else { "TryMixing" };
            let err = active(&settings(&[(id, level)]), &so, &ds, None).unwrap_err();
            assert!(err.contains("not installed"), "{err}");
        }
        // An explicit DLC set without DLC2 reads every DLC2 setting as Disabled, as
        // GetCurrentMixingSettingLevel does, instead of refusing or mixing.
        let so_only = crate::dlc::ActiveDlc { expansion1: true, ..crate::dlc::ActiveDlc::VANILLA };
        let a = active(&settings(&[("DLC2_ID", "Enabled"), ("CeresAsteroidMixing", "GuranteeMixing"), ("GardenMixing", "TryMixing")]), &so, &ds, Some(so_only)).unwrap();
        assert!(a.dlc.is_empty() && a.world.is_empty() && a.subworld.is_empty() && a.removed.is_empty());
        let with_dlc2 = crate::dlc::ActiveDlc { expansion1: true, dlc2: true, ..crate::dlc::ActiveDlc::VANILLA };
        let a = active(&settings(&[("DLC2_ID", "Enabled"), ("CeresAsteroidMixing", "GuranteeMixing")]), &so, &ds, Some(with_dlc2)).unwrap();
        assert_eq!((a.dlc.len(), a.world.len()), (1, 1));
    }

    #[test]
    fn world_mixing_follows_shuffle_and_tag_rules() {
        let ds = mixing_data();
        let c = cluster(vec![("start", "StartWorld", &[], &[]), ("inner", "InnerCluster", &[], &[]),
            ("a", "Cluster", &["Mixing", "SmallWorld"], &[]), ("b", "Cluster", &["Mixing", "SmallWorld", "Challenge"], &[]),
            ("c", "Cluster", &["Mixing", "SmallWorld"], &["NoDamagingMeteorShowers"]), ("d", "Cluster", &["Mixing", "SmallWorld"], &[])], &[], &[]);
        let ceres = vec![("dlc2::worldMixing/CeresMixingSettings".to_string(), true)];
        for seed in 1..200 {
            let out = world_mixing(&ds, &c, seed, &ceres).unwrap();
            // Exactly one eligible placement (a or d) gets the single Ceres option.
            let hit: Vec<usize> = (0..6).filter(|&i| out[i].is_some()).collect();
            assert_eq!(hit.len(), 1);
            assert!(hit[0] == 2 || hit[0] == 5);
            // The first eligible placement in shuffled order wins: replay the shuffle.
            let mut rng = KRandom::new(seed);
            let mut order: Vec<usize> = (0..6).collect();
            shuffle_seeded(&mut order, &mut rng);
            assert_eq!(hit[0], *order.iter().find(|&&i| i == 2 || i == 5).unwrap());
        }
        // GuranteeMixing with no placement that accepts the world fails worldgen.
        let none = cluster(vec![("start", "StartWorld", &[], &[]), ("b", "Cluster", &["Mixing", "Challenge"], &[])], &[], &[]);
        assert!(world_mixing(&ds, &none, 5, &ceres).unwrap_err().contains("Could not guarantee"));
        assert_eq!(world_mixing(&ds, &none, 5, &[(ceres[0].0.clone(), false)]).unwrap(), [None, None]);
    }

    #[test]
    fn subworld_mixing_assignment_matches_hand_run() {
        let ds = mixing_data();
        let start = world("start", &[], &[("(Mixing1)", 2, 4, &[], &[]), ("(Mixing2)", 2, 4, &[], &[]), ("(Mixing3)", 2, 4, &["Subsurface"], &[])]);
        let moon = world("moon", &[], &[("(Mixing1)", 2, 3, &[], &["Ceres"])]);
        let inner = world("inner", &[], &[("(Mixing1)", 2, 3, &[], &[]), ("(Mixing2)", 5, 9, &[], &[])]);
        let worlds = [(&moon, "Cluster"), (&inner, "InnerCluster"), (&start, "StartWorld")];
        let active: Vec<(String, bool)> = ["IceCaves", "CarrotQuarry", "SugarWoods"].iter()
            .map(|id| (format!("dlc2::subworldMixing/{id}MixingSettings"), true)).collect();
        for seed in [1, 2, 3, 77, 1000] {
            let out = subworld_mixing(&ds, &worlds, seed, &active).unwrap();
            // Hand-run: shuffle world indices, stable sort by GetSortOrder (start 1, inner 2, cluster 3),
            // then per world shuffle the option list and give each rule the first option of the
            // (minCount desc, maxCount desc) ordering that passes its tag and count checks.
            let mut rng = KRandom::new(seed);
            let mut order = vec![0usize, 1, 2];
            shuffle_seeded(&mut order, &mut rng);
            order.sort_by_key(|&i| [3, 2, 1][i]);
            let mut opts: Vec<(&str, i32, i32, &[&str])> = vec![("IceCaves", 1, 3, &["StartSubworld", "Cold", "Ceres"]),
                ("CarrotQuarry", 1, 3, &["Cold", "Ceres"]), ("SugarWoods", 1, 3, &["Cold", "Subsurface", "Ceres"])];
            let mut expected = vec![Vec::new(); 3];
            for i in order {
                shuffle_seeded(&mut opts, &mut rng);
                for rule in &worlds[i].0.subworld_mixing_rules {
                    if opts.is_empty() { break; }
                    let mut sorted: Vec<usize> = (0..opts.len()).collect();
                    sorted.sort_by(|&a, &b| (opts[b].1, opts[b].2).cmp(&(opts[a].1, opts[a].2)));
                    let Some(k) = sorted.into_iter().find(|&k| rule.required_tags.iter().all(|t| opts[k].3.contains(&t.as_str()))
                        && !rule.forbidden_tags.iter().any(|t| opts[k].3.contains(&t.as_str()))) else { continue };
                    expected[i].push((rule.name.clone(), opts[k].0));
                    opts[k].1 -= 1; opts[k].2 -= 1;
                    if opts[k].2 <= 0 { opts.remove(k); }
                }
            }
            let got: Vec<Vec<(String, &str)>> = out.iter().map(|w| w.iter().map(|m| (m.proxy.clone(),
                ["IceCaves", "CarrotQuarry", "SugarWoods"].into_iter().find(|id| m.file.name.to_lowercase().contains(&id.to_lowercase())).unwrap())).collect()).collect();
            assert_eq!(got, expected, "seed {seed}");
            // Ceres-forbidding moonlet rule never matches (every DLC2 mixing carries the Ceres tag).
            assert!(out[0].is_empty());
            // (Mixing3) requires Subsurface: only SugarWoods qualifies.
            assert!(out[2].iter().filter(|m| m.proxy == "(Mixing3)").all(|m| m.file.name.contains("SugarWoods")));
            // Counts are clamped to the rule.
            assert!(out.iter().flatten().all(|m| m.file.min_count == 2 || m.file.min_count == 5));
            assert!(out[1].iter().all(|m| m.file.max_count == if m.proxy == "(Mixing1)" { 3 } else { 9 }));
        }
    }

    #[test]
    fn subworld_mixing_guarantee_failure_and_count_clamp() {
        let ds = mixing_data();
        let w = world("w", &[], &[("(Mixing1)", 2, 3, &[], &["Ceres"])]);
        let active = vec![("dlc2::subworldMixing/IceCavesMixingSettings".to_string(), true)];
        assert!(subworld_mixing(&ds, &[(&w, "StartWorld")], 1, &active).unwrap_err().contains("IceCavesMixingSettings (1)"));
        assert!(subworld_mixing(&ds, &[(&w, "StartWorld")], 1, &[(active[0].0.clone(), false)]).unwrap()[0].is_empty());
        // A rule whose count window cannot overlap the subworld's is skipped.
        let mut ds = ds;
        ds.subworld_mixing.get_mut(&active[0].0).unwrap().subworld.max_count = 1;
        let w = world("w", &[], &[("(Mixing1)", 2, 3, &[], &[])]);
        assert!(subworld_mixing(&ds, &[(&w, "StartWorld")], 1, &[(active[0].0.clone(), false)]).unwrap()[0].is_empty());
    }

    #[test]
    fn repeated_proxy_name_keeps_first_replacement() {
        let m = |p: &str, n: &str| SubworldMix { settings: String::new(), proxy: p.into(), file: file(n, 0, 1), additional_world_template_rules: vec![] };
        let map = proxy_names(&[m("(Mixing1)", "a"), m("(Mixing2)", "b"), m("(Mixing1)", "c")]);
        assert_eq!(map.get("(Mixing1)").map(String::as_str), Some("a"));
        assert_eq!(map.len(), 2);
    }
}
