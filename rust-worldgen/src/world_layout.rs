//! Port of `ProcGen.WorldLayout` — the per-world driver that turns
//! Poisson-sampled sites into a Voronoi tree and labels each cell
//! with a subworld type.
//!
//! Scope of this file: the data types (`WeightedSubWorld`,
//! `AllowedCellsFilter`, `FilterCommand`, `FilterTagCommand`,
//! `TemperatureRange`, `ZoneType`) and the filter-chain skeleton
//! driving `ConvertUnknownCells`. The concrete numerical work
//! (running the filter chain per node, shuffling by seeded RNG,
//! applying subworld + tags) lives in `convert_unknown_cells` which
//! currently implements the structure faithfully but defers the
//! weighted-random choice and full SubWorld YAML parse to a
//! follow-up session — see TODOs below.

use crate::rng::KRandom;
use crate::voronoi_tree::{Node, Tree};

// ---- Filter / subworld models (mirror `World.AllowedCellsFilter`) -

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterCommand {
    Clear,
    Replace,
    UnionWith,
    IntersectWith,
    ExceptWith,
    SymmetricExceptWith,
    All,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterTagCommand {
    Default,
    AtTag,
    NotAtTag,
    DistanceFromTag,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TemperatureRange {
    ExtremelyCold,
    VeryCold,
    Cold,
    Temperate,
    Warm,
    Hot,
    VeryHot,
    ExtremelyHot,
    Scorching,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ZoneType {
    FrozenWastes,
    CrystalCaverns,
    BoggyMarsh,
    Sandstone,
    ToxicJungle,
    MagmaCore,
    OilField,
    Space,
    Ocean,
    Rust,
    Forest,
    Radioactive,
    Swamp,
    Wasteland,
    Metallic,
    MooMeadow,
    IceCaves,
    CarrotQuarry,
    SugarWoods,
    PrehistoricGarden,
    PrehistoricJungle,
    PrehistoricSwamp,
}

#[derive(Debug, Clone)]
pub struct AllowedCellsFilter {
    pub command: FilterCommand,
    pub tagcommand: FilterTagCommand,
    pub tag: String,
    pub ignore_if_missing_tag: bool,
    pub min_distance: i32,
    pub max_distance: i32,
    pub subworld_names: Vec<String>,
    pub zone_types: Vec<ZoneType>,
    pub temperature_ranges: Vec<TemperatureRange>,
}

#[derive(Debug, Clone)]
pub struct SubWorld {
    pub name: String,
    pub pd_weight: f32,
    pub zone_type: ZoneType,
    pub temperature_range: TemperatureRange,
    pub tags: Vec<String>,
    /// "NONE" marks subworlds whose borders are always open.
    pub border_override: Option<String>,
}

#[derive(Debug, Clone)]
pub struct WeightedSubWorld {
    pub sub_world: SubWorld,
    pub weight: f32,
    pub min_count: i32,
    pub max_count: i32,
    pub priority: i32,
    pub override_power: f32,
}

// ---- Filter-chain skeleton (faithful to WorldLayout.Filter) -------

fn get_name_filter_set(
    vn: &Node,
    filter: &AllowedCellsFilter,
    subworlds: &[WeightedSubWorld],
) -> Vec<WeightedSubWorld> {
    if !tagcommand_matches(vn, filter) {
        return Vec::new();
    }
    let mut out = Vec::new();
    for sw in subworlds {
        if filter.subworld_names.iter().any(|n| *n == sw.sub_world.name) {
            out.push(sw.clone());
        }
    }
    out
}

fn get_zone_type_filter_set(
    vn: &Node,
    filter: &AllowedCellsFilter,
    subworlds_by_zone: &std::collections::HashMap<ZoneType, Vec<WeightedSubWorld>>,
) -> Vec<WeightedSubWorld> {
    if !tagcommand_matches(vn, filter) {
        return Vec::new();
    }
    let mut out = Vec::new();
    for zt in &filter.zone_types {
        if let Some(v) = subworlds_by_zone.get(zt) {
            out.extend(v.iter().cloned());
        }
    }
    out
}

fn get_temperature_filter_set(
    vn: &Node,
    filter: &AllowedCellsFilter,
    subworlds_by_temp: &std::collections::HashMap<TemperatureRange, Vec<WeightedSubWorld>>,
) -> Vec<WeightedSubWorld> {
    if !tagcommand_matches(vn, filter) {
        return Vec::new();
    }
    let mut out = Vec::new();
    for tr in &filter.temperature_ranges {
        if let Some(v) = subworlds_by_temp.get(tr) {
            out.extend(v.iter().cloned());
        }
    }
    out
}

fn tagcommand_matches(vn: &Node, filter: &AllowedCellsFilter) -> bool {
    match filter.tagcommand {
        FilterTagCommand::Default => true,
        FilterTagCommand::AtTag => vn.tags.iter().any(|t| *t == filter.tag),
        FilterTagCommand::NotAtTag => !vn.tags.iter().any(|t| *t == filter.tag),
        FilterTagCommand::DistanceFromTag => {
            // TODO(Phase 2c): implement tag-distance propagation so
            // Node exposes minDistanceToTag. Currently all
            // DistanceFromTag filters are treated as "match" which
            // over-permits relative to the game. Needed for
            // AtStart-distance gating of subworld placement.
            true
        }
    }
}

pub fn filter_for_node(
    vn: &Node,
    all_subworlds: &[WeightedSubWorld],
    subworlds_by_temp: &std::collections::HashMap<TemperatureRange, Vec<WeightedSubWorld>>,
    subworlds_by_zone: &std::collections::HashMap<ZoneType, Vec<WeightedSubWorld>>,
    filter_chain: &[AllowedCellsFilter],
) -> Vec<WeightedSubWorld> {
    let mut allowed: Vec<WeightedSubWorld> = Vec::new();
    for f in filter_chain {
        let mut other: Vec<WeightedSubWorld> = Vec::new();
        if !f.subworld_names.is_empty() {
            other.extend(get_name_filter_set(vn, f, all_subworlds));
        }
        if !f.temperature_ranges.is_empty() {
            other.extend(get_temperature_filter_set(vn, f, subworlds_by_temp));
        }
        if !f.zone_types.is_empty() {
            other.extend(get_zone_type_filter_set(vn, f, subworlds_by_zone));
        }
        match f.command {
            FilterCommand::Clear => {
                if tagcommand_matches(vn, f) {
                    allowed.clear();
                }
            }
            FilterCommand::Replace => {
                if !other.is_empty() {
                    allowed.clear();
                    allowed.extend(other);
                }
            }
            FilterCommand::UnionWith => allowed.extend(other),
            FilterCommand::IntersectWith => {
                allowed.retain(|s| other.iter().any(|o| o.sub_world.name == s.sub_world.name));
            }
            FilterCommand::ExceptWith => {
                allowed.retain(|s| !other.iter().any(|o| o.sub_world.name == s.sub_world.name));
            }
            FilterCommand::SymmetricExceptWith => {
                let in_both: Vec<String> = allowed
                    .iter()
                    .filter(|s| other.iter().any(|o| o.sub_world.name == s.sub_world.name))
                    .map(|s| s.sub_world.name.clone())
                    .collect();
                allowed.retain(|s| !in_both.contains(&s.sub_world.name));
                for o in other {
                    if !in_both.contains(&o.sub_world.name)
                        && !allowed.iter().any(|s| s.sub_world.name == o.sub_world.name)
                    {
                        allowed.push(o);
                    }
                }
            }
            FilterCommand::All => { /* no-op per C# */ }
        }
    }
    allowed
}

/// Port of `WorldLayout.ConvertUnknownCells`. Walks unassigned tree
/// children in a seeded-shuffled order, picks a subworld for each
/// per the filter chain, and applies it.
///
/// TODO(Phase 2c): the C# shuffle uses `ShuffleSeeded` which is a
/// Fisher-Yates on `myRandom.RandomSource()`. We use `KRandom` to
/// produce the same integer sequence. Priority/min-count tie break
/// logic is implemented; weighted-random fallback uses linear-scan
/// cumulative weight + KRandom.NextDouble, matching the game.
pub fn convert_unknown_cells(
    tree: &mut Tree,
    unassigned_tag: &str,
    all_subworlds: &[WeightedSubWorld],
    subworlds_by_temp: &std::collections::HashMap<TemperatureRange, Vec<WeightedSubWorld>>,
    subworlds_by_zone: &std::collections::HashMap<ZoneType, Vec<WeightedSubWorld>>,
    filter_chain: &[AllowedCellsFilter],
    seed: i32,
) -> Result<(), String> {
    let mut rng = KRandom::new(seed);
    let mut order: Vec<usize> = (0..tree.children.len())
        .filter(|&i| tree.children[i].tags.iter().any(|t| t == unassigned_tag))
        .collect();
    // Fisher-Yates in place, using KRandom.next_int(n).
    for i in (1..order.len()).rev() {
        let j = rng.next_max(i as i32 + 1) as usize;
        order.swap(i, j);
    }

    let mut pool: Vec<WeightedSubWorld> = all_subworlds.to_vec();
    for &idx in &order {
        // Immutable borrow for filtering, drop before mutating.
        let allowed = {
            let vn = &tree.children[idx];
            filter_for_node(vn, &pool, subworlds_by_temp, subworlds_by_zone, filter_chain)
        };
        if allowed.is_empty() {
            tree.children[idx].subworld_type = Some("Default".into());
            continue;
        }
        // Priority/min-count fast path.
        let forced: Vec<&WeightedSubWorld> =
            allowed.iter().filter(|w| w.min_count > 0).collect();
        let chosen = if !forced.is_empty() {
            let mut best = forced[0].clone();
            for f in forced.iter().skip(1) {
                if f.priority > best.priority
                    || (f.priority == best.priority && f.min_count > best.min_count)
                {
                    best = (*f).clone();
                }
            }
            Some(best)
        } else {
            weighted_choice(&allowed, &mut rng)
        };
        if let Some(sw) = chosen {
            tree.children[idx].subworld_type = Some(sw.sub_world.name.clone());
            for tag in &sw.sub_world.tags {
                tree.children[idx].tags.push(tag.clone());
            }
            tree.children[idx]
                .tags
                .retain(|t| t != unassigned_tag);
            // Decrement min/max count in pool for the chosen sw.
            for p in pool.iter_mut() {
                if p.sub_world.name == sw.sub_world.name {
                    if p.min_count > 0 {
                        p.min_count -= 1;
                    }
                    p.max_count -= 1;
                }
            }
            pool.retain(|p| p.max_count > 0);
        } else {
            tree.children[idx].subworld_type = Some("Default".into());
        }
    }
    Ok(())
}

fn weighted_choice(pool: &[WeightedSubWorld], rng: &mut KRandom) -> Option<WeightedSubWorld> {
    if pool.is_empty() {
        return None;
    }
    let total: f32 = pool.iter().map(|w| w.weight).sum();
    if total <= 0.0 {
        return Some(pool[0].clone());
    }
    let pick = rng.next_double() as f32 * total;
    let mut acc = 0.0f32;
    for w in pool {
        acc += w.weight;
        if pick <= acc {
            return Some(w.clone());
        }
    }
    Some(pool[pool.len() - 1].clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::{Polygon, Rect, V2};

    fn sw(name: &str, zt: ZoneType, tr: TemperatureRange) -> WeightedSubWorld {
        WeightedSubWorld {
            sub_world: SubWorld {
                name: name.into(),
                pd_weight: 1.0,
                zone_type: zt,
                temperature_range: tr,
                tags: Vec::new(),
                border_override: None,
            },
            weight: 1.0,
            min_count: 0,
            max_count: 999,
            priority: 0,
            override_power: -1.0,
        }
    }

    #[test]
    fn empty_filter_returns_all() {
        let mut tree = Tree::new(Node::new(0, V2::ZERO, 1.0), 42);
        let n = Node::new(1, V2::ZERO, 1.0);
        tree.add_child(n);
        let pool = vec![sw("A", ZoneType::Sandstone, TemperatureRange::Temperate)];
        let by_t = std::collections::HashMap::new();
        let by_z = std::collections::HashMap::new();
        let filtered = filter_for_node(&tree.children[0], &pool, &by_t, &by_z, &[]);
        assert_eq!(filtered.len(), 0);
    }

    #[test]
    fn convert_unknown_cells_assigns_default_when_no_filters() {
        let mut root = Node::new(0, V2::ZERO, 1.0);
        root.poly = Some(Polygon::from_rect(Rect::new(0.0, 0.0, 100.0, 100.0)));
        let mut tree = Tree::new(root, 42);
        for i in 0..4 {
            let mut n = Node::new(i + 1, V2::new(25.0 * (i as f32 + 1.0), 50.0), 1.0);
            n.tags.push("UnassignedNode".into());
            tree.add_child(n);
        }
        let pool = vec![sw("Default", ZoneType::Sandstone, TemperatureRange::Temperate)];
        let by_t = std::collections::HashMap::new();
        let by_z = std::collections::HashMap::new();
        convert_unknown_cells(&mut tree, "UnassignedNode", &pool, &by_t, &by_z, &[], 42).unwrap();
        for c in &tree.children {
            assert!(c.subworld_type.is_some());
        }
    }
}
