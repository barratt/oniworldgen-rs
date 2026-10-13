//! Cluster-level seed derivation for geyser rate rolls.
//!
//! # Seed formula (validated byte-exact against 2928/2938 = 99.7% of
//! a 100-coord random DB sample across 10 cluster prefixes)
//!
//! The game seeds a fresh `KRandom` for each geyser with
//!
//! ```text
//! seed = globalWorldSeed(world) + (int)transform.x + (int)transform.y
//! ```
//!
//! where `transform.x/y` is the geyser's Unity-space position
//! (= local cell + worldOffset). That unrolls to
//!
//! ```text
//! seed = globalWorldSeed(world) + local_x + local_y
//!        + worldOffset.x + worldOffset.y
//! ```
//!
//! The empirical relationship between `globalWorldSeed(world)` and
//! the coord's base seed is
//!
//! ```text
//! globalWorldSeed(world) = baseSeed + (num_worlds_in_cluster - 1)
//! ```
//!
//! for every world in the cluster (the "-1" constant is likely an
//! artefact of per-world seed increments during trait selection in
//! `Cluster.InitializeWorlds`, but the specific value is treated
//! empirically here).
//!
//! Composing:
//!
//! ```text
//! seed = baseSeed + (num_worlds - 1)
//!        + worldOffset.x + worldOffset.y
//!        + local_x + local_y
//! ```
//!
//! `seed_for_world` returns everything up to (but not including) the
//! `local_x + local_y` term, so callers hand it and local cell coords
//! to [`crate::geyser::roll`].
//!
//! # DB coordinates
//!
//! The public seed DB stores geyser positions in a Y-flipped asteroid-
//! local frame:
//!
//! ```text
//! local_x_game = db_x
//! local_y_game = world.sizeY - db_y
//! ```

/// Return the effective `KRandom` seed to use for a geyser at local
/// `(x, y)` on a given world in a cluster.
///
/// * `base_seed` — the integer seed from the coordinate string
///   (e.g. `1_000_172_956` for `BAD-A-1000172956-0-0-0`).
/// * `num_worlds` — total worlds in the cluster (length of the
///   cluster YAML's `worldPlacements` list).
/// * `world_offset_x`, `world_offset_y` — the `(x, y)` placement
///   written by `BestFit.BestFitWorlds`. For single-world clusters
///   that's `(0, 0)`.
///
/// The caller combines the returned seed with `x + y` when calling
/// [`crate::geyser::roll`].
#[inline]
pub fn seed_for_world(
    base_seed: i32,
    num_worlds: usize,
    world_offset_x: i32,
    world_offset_y: i32,
) -> i32 {
    let c = (num_worlds as i32) - 1;
    base_seed
        .wrapping_add(c)
        .wrapping_add(world_offset_x)
        .wrapping_add(world_offset_y)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// BAD-A-1000172956 is single-world basegame: c = 0, offset = (0, 0).
    /// Must equal `baseSeed` exactly.
    #[test]
    fn single_world_basegame_is_identity() {
        let seed = seed_for_world(1_000_172_956, 1, 0, 0);
        assert_eq!(seed, 1_000_172_956);
    }

    /// V-SNDST-C (Classic Terra, 8 worlds). `VanillaSandstoneDefault`
    /// is the tallest and lands at offset (0, 0) after BestFit. The
    /// effective seed offset is `c = num_worlds - 1 = 7`.
    /// Matches the validated byte-exact K observed in the harness.
    #[test]
    fn v_sndst_c_vansd_seed_offset_is_seven() {
        let seed = seed_for_world(42, 8, 0, 0);
        assert_eq!(seed, 42 + 7);
    }

    /// V-SNDST-C `MediumRadioactiveVanillaWarpPlanet`: BestFit offset
    /// `(242, 0)`, so effective K = 242 + 7 = 249.
    #[test]
    fn v_sndst_c_medrad_seed_offset_is_249() {
        let seed = seed_for_world(42, 8, 242, 0);
        assert_eq!(seed, 42 + 249);
    }

    /// M-BAD-C (Mini Badlands Start, 11 worlds). Tundra's BestFit
    /// offset is `(732, 0)`, so K = 732 + 10 = 742.
    #[test]
    fn m_bad_c_tundra_seed_offset_is_742() {
        let seed = seed_for_world(1_000_031_275, 11, 732, 0);
        assert_eq!(seed, 1_000_031_275 + 742);
    }
}
