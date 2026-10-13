//! World trait selection, port of `SettingsCache.GetRandomTraits`.

use std::collections::BTreeSet;

use crate::data::{SettingsCache, World};
use crate::rng::KRandom;

/// Which DLC buckets to include in the trait pool. For BaseGame-only
/// clusters the game uses pool size 18, exactly the 18 base-game
/// traits in `worldgen/traits/*.yaml`. Expansion1 and dlc2 traits are
/// skipped.
#[derive(Copy, Clone, Debug)]
pub struct PoolScope {
    pub include_expansion1: bool,
    pub include_dlc2: bool,
}

impl PoolScope {
    pub fn basegame() -> Self { Self { include_expansion1: false, include_dlc2: false } }
    pub fn spacedout() -> Self { Self { include_expansion1: true, include_dlc2: false } }
    pub fn spacedout_frosty() -> Self { Self { include_expansion1: true, include_dlc2: true } }
    pub fn accept(&self, id: &str) -> bool {
        if id.starts_with("expansion1::") {
            self.include_expansion1
        } else if id.starts_with("dlc2::") {
            self.include_dlc2
        } else {
            true
        }
    }
}

/// Compute the selected traits for (world, seed, scope). Iterates
/// traits in load order (preserved as `Vec`).
pub fn traits_for_seed_scoped(
    cache: &SettingsCache,
    world: &World,
    seed: i32,
    scope: PoolScope,
) -> Vec<String> {
    if world.disableWorldTraits.unwrap_or(false) {
        return vec![];
    }
    if world.worldTraitRules.is_empty() {
        return vec![];
    }
    if seed == 0 {
        return vec![];
    }

    let mut r = KRandom::new(seed);
    // Pool keeps insertion order but is filtered by scope first.
    let mut pool_indices: Vec<usize> = (0..cache.traits.len())
        .filter(|&i| scope.accept(&cache.traits[i].0))
        .collect();
    let mut selected_ids: Vec<String> = Vec::new();
    let mut tagset: BTreeSet<String> = BTreeSet::new();

    for rule in world.worldTraitRules.iter() {
        for sp in rule.specificTraits.iter() {
            if cache.has_trait(sp) {
                selected_ids.push(sp.clone());
                if let Some(t) = cache.trait_by_id(sp) {
                    for x in t.exclusiveWithTags.iter() {
                        tagset.insert(x.clone());
                    }
                }
            }
        }

        let req: BTreeSet<&str> = rule.requiredTags.iter().map(|s| s.as_str()).collect();
        let forb: BTreeSet<&str> = rule.forbiddenTags.iter().map(|s| s.as_str()).collect();
        let forbt: BTreeSet<&str> = rule.forbiddenTraits.iter().map(|s| s.as_str()).collect();

        // Preserve the pool's iteration order.
        let mut available: Vec<usize> = pool_indices
            .iter()
            .copied()
            .filter(|&i| {
                let (id, t) = &cache.traits[i];
                if forbt.contains(id.as_str()) {
                    return false;
                }
                let tags: BTreeSet<&str> = t.traitTags.iter().map(|s| s.as_str()).collect();
                if !req.is_empty() && !req.is_subset(&tags) {
                    return false;
                }
                if !forb.is_empty() && tags.intersection(&forb).next().is_some() {
                    return false;
                }
                true
            })
            .collect();

        let to_gen = r.next_range(rule.min, rule.min.max(rule.max + 1));
        let initial = selected_ids.len();

        while (selected_ids.len() as i32) < (initial as i32 + to_gen) && !available.is_empty() {
            let idx = r.next_max(available.len() as i32) as usize;
            let trait_idx = available[idx];
            let (id, t) = &cache.traits[trait_idx];
            let mut exclusive = false;
            for ex in t.exclusiveWith.iter() {
                if selected_ids.iter().any(|s| s == ex) {
                    exclusive = true;
                    break;
                }
            }
            if !exclusive {
                for ex in t.exclusiveWithTags.iter() {
                    if tagset.contains(ex.as_str()) {
                        exclusive = true;
                        break;
                    }
                }
            }
            if exclusive {
                available.remove(idx);
                continue;
            }
            selected_ids.push(id.clone());
            available.remove(idx);
            pool_indices.retain(|&i| i != trait_idx);
            for x in t.exclusiveWithTags.iter() {
                tagset.insert(x.clone());
            }
        }
    }

    selected_ids
}

/// Shim for call-sites that haven't been updated to pass a PoolScope.
/// Defaults to BaseGame scope.
pub fn traits_for_seed(cache: &SettingsCache, world: &World, seed: i32) -> Vec<String> {
    traits_for_seed_scoped(cache, world, seed, PoolScope::basegame())
}
