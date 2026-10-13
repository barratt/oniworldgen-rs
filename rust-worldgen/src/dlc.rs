//! Active DLC set (build 744825).
//!
//! The game decides what worldgen data exists from the DLC content that is
//! *subscribed* when it boots (`DlcManager.IsContentSubscribed`: owned,
//! installed and enabled in the content screen). Everything below is derived
//! from decompiled `DlcManager`, `ProcGen.SettingsCache`, `ProcGen.ClusterLayouts`,
//! `ProcGen.Worlds`, `GeyserGenericConfig`, `SaveLoader` and `GameSpawnData`:
//!
//! * `SettingsCache.LoadFiles` walks `DlcManager.RELEASED_VERSIONS` and only
//!   loads the `worldgen/` tree of subscribed content. A trait is skipped when
//!   any of its `forbiddenDLCIds` is subscribed (`LoadTrait`), so with
//!   Spaced Out active the base IrregularOil, SubsurfaceOcean and Volcanoes are
//!   replaced by the `expansion1::` traits. DLC2 ships no traits and no trait
//!   forbids DLC2, so DLC2 never changes a trait pool.
//! * `ClusterLayouts.UpdateClusterCache` gives a cluster without
//!   `requiredDlcIds` the implicit requirement `[EXPANSION1_ID]` when Spaced
//!   Out is active and `[""]` (vanilla) otherwise, then drops it unless
//!   `IsCorrectDlcSubscribed`. A coordinate whose cluster is not loaded is
//!   refused by the game; [`ActiveDlc::check_cluster`] does the same.
//! * `SaveLoader` stores the cluster's (resolved) `requiredDlcIds` plus the
//!   enabled DLC mixing ids as the save's DLC list. `GeyserGenericConfig` picks
//!   generic geysers from types valid for that list, so Spaced Out generic
//!   geysers (aluminum, cobalt, sulfur) depend on Spaced Out being active.
//! * `GameSpawnData.AddTemplate` only honours the Teleporters setting when
//!   `DlcManager.IsExpansion1Active()`.
//! * `DlcManager.IsPureVanilla()` (= Spaced Out inactive) swaps the Durability
//!   coordinate values; see `custom_settings`.

use crate::data_source::ClusterYaml;

pub const EXPANSION1_ID: &str = "EXPANSION1_ID";
pub const DLC2_ID: &str = "DLC2_ID";
pub const DLC3_ID: &str = "DLC3_ID";
pub const DLC4_ID: &str = "DLC4_ID";
pub const DLC5_ID: &str = "DLC5_ID";
/// Cosmetic content (`DlcInfo.isCosmetic`): no worldgen content, but
/// `GetActiveDLCIds` reports it when subscribed, so `--dlc` accepts and
/// ignores it.
pub const COSMETIC1_ID: &str = "COSMETIC1_ID";

/// Content DLCs after Spaced Out, in `DlcManager.DLC_PACKS` order, with the
/// StreamingAssets directory of each (`DlcInfo.directory`).
pub const CONTENT_DLCS: [(&str, &str); 4] = [(DLC2_ID, "dlc2"), (DLC3_ID, "dlc3"), (DLC4_ID, "dlc4"), (DLC5_ID, "dlc5")];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ActiveDlc {
    pub expansion1: bool,
    pub dlc2: bool,
    /// The Bionic Booster Pack: no worldgen content besides `mixing.yaml`
    /// (lore unlocks); it gates the BionicWattage setting and the DLC3 mixing toggle.
    pub dlc3: bool,
    /// The Prehistoric Planet Pack.
    pub dlc4: bool,
    /// The Aquatic Planet Pack.
    pub dlc5: bool,
}

impl ActiveDlc {
    pub const VANILLA: ActiveDlc = ActiveDlc { expansion1: false, dlc2: false, dlc3: false, dlc4: false, dlc5: false };

    /// Parse a comma-separated list of DLC ids, or `none` for the base game.
    pub fn parse(list: &str) -> Result<Self, String> {
        let list = list.trim();
        if list.eq_ignore_ascii_case("none") { return Ok(Self::VANILLA); }
        let mut out = Self::VANILLA;
        let mut cosmetic = false;
        for id in list.split(',').map(str::trim) {
            if id == COSMETIC1_ID {
                if cosmetic { return Err(format!("duplicate DLC id {id}")); }
                cosmetic = true;
                continue;
            }
            let flag = match out.flag_mut(id) {
                Some(flag) => flag,
                None if id.is_empty() => return Err(format!("empty DLC id in {list:?}")),
                None => return Err(format!("unknown DLC id {id:?}; use EXPANSION1_ID, DLC2_ID, DLC3_ID, DLC4_ID, DLC5_ID or none")),
            };
            if *flag { return Err(format!("duplicate DLC id {id}")); }
            *flag = true;
        }
        Ok(out)
    }

    fn flag_mut(&mut self, id: &str) -> Option<&mut bool> {
        Some(match id {
            EXPANSION1_ID => &mut self.expansion1,
            DLC2_ID => &mut self.dlc2,
            DLC3_ID => &mut self.dlc3,
            DLC4_ID => &mut self.dlc4,
            DLC5_ID => &mut self.dlc5,
            _ => return None,
        })
    }

    /// Add `id` to the set; false for ids that are not content DLCs.
    pub fn insert(&mut self, id: &str) -> bool {
        match self.flag_mut(id) { Some(flag) => { *flag = true; true } None => false }
    }

    /// `DlcManager.IsContentSubscribed`; vanilla content is always present.
    pub fn is_active(&self, id: &str) -> bool {
        match id {
            "" | "VANILLA_ID" => true,
            EXPANSION1_ID => self.expansion1,
            DLC2_ID => self.dlc2,
            DLC3_ID => self.dlc3,
            DLC4_ID => self.dlc4,
            DLC5_ID => self.dlc5,
            _ => false,
        }
    }

    pub fn is_pure_vanilla(&self) -> bool { !self.expansion1 }

    /// Ids in `DlcManager.GetActiveDLCIds` order (newest first), the order the
    /// static simulation profiles record.
    pub fn ids(&self) -> Vec<String> {
        let mut ids = Vec::new();
        for (id, _) in CONTENT_DLCS.iter().rev() { if self.is_active(id) { ids.push(id.to_string()); } }
        if self.expansion1 { ids.push(EXPANSION1_ID.to_string()); }
        ids
    }

    /// `requiredDlcIds` as resolved by `ClusterLayouts.UpdateClusterCache`.
    pub fn cluster_required_ids(&self, cluster: &ClusterYaml) -> Vec<String> {
        if !cluster.required_dlc_ids.is_empty() {
            return cluster.required_dlc_ids.iter().map(|id| if id == "VANILLA_ID" { String::new() } else { id.clone() }).collect();
        }
        vec![if self.expansion1 { EXPANSION1_ID.to_string() } else { String::new() }]
    }

    /// Refuse clusters the game would not load (`IsCorrectDlcSubscribed`).
    pub fn check_cluster(&self, cluster: &ClusterYaml) -> Result<(), String> {
        let required = self.cluster_required_ids(cluster);
        if let Some(missing) = required.iter().find(|id| !self.is_active(id)) {
            return Err(format!("cluster {} requires {} which is not in the active DLC set [{}]",
                cluster.coordinate_prefix, if missing.is_empty() { "the base game without Spaced Out" } else { missing }, self.ids().join(",")));
        }
        if let Some(forbidden) = cluster.forbidden_dlc_ids.iter().find(|id| self.is_active(id)) {
            return Err(format!("cluster {} is unavailable when {forbidden} is active", cluster.coordinate_prefix));
        }
        Ok(())
    }

    /// DLC ids stored in the save (`SaveLoader`): resolved cluster requirements
    /// plus enabled DLC mixing. Only the cluster part is modelled: the generic
    /// geyser pool is all this list decides here, and no DLC that can be mixed
    /// in adds a generic geyser (DLC5's murky brine geyser is not generic).
    pub fn save_dlc_ids(&self, cluster: &ClusterYaml) -> Vec<String> {
        self.cluster_required_ids(cluster).into_iter().filter(|id| !id.is_empty()).collect()
    }

    /// Whether Spaced Out generic geyser types are in the `GeyserGeneric` pool.
    pub fn expansion1_geysers(&self, cluster: &ClusterYaml) -> bool {
        self.save_dlc_ids(cluster).iter().any(|id| id == EXPANSION1_ID)
    }

    /// Scope inferred from the cluster definition alone. This is what the
    /// generator used before the active DLC set could be supplied, and it is
    /// still used when none is given.
    ///
    /// For every loadable cluster the inferred Spaced Out flag equals the flag
    /// of the natural game configuration (Spaced Out clusters infer it, base
    /// and base-game Ceres clusters do not). The inferred DLC2 flag is only set
    /// for clusters that reference `dlc2::` worlds, so e.g. SNDST-C infers
    /// `[EXPANSION1_ID]` although the game may run it with
    /// `EXPANSION1_ID,DLC2_ID`. That makes no difference to the output: DLC2
    /// has no traits, forbids nothing, adds no generic geyser and is not
    /// referenced by base or Spaced Out worldgen data. The one cluster whose
    /// inferred flag differs from a valid configuration is SNDST-A, which is
    /// also loadable under Spaced Out (no `forbiddenDlcIds`); pass the DLC set
    /// explicitly for that case. DLC3/4/5 follow the same rule (required or
    /// referenced by a world), so the Prehistoric and Aquatic clusters infer
    /// their pack; [`infer`](Self::infer) adds what the coordinate's settings need.
    pub fn infer_from_cluster(cluster: &ClusterYaml) -> Self {
        let has = |list: &[String], id: &str| list.iter().any(|s| s == id);
        let refs = |prefix: &str| cluster.world_placements.iter().any(|p| p.world_ref.starts_with(prefix));
        let flag = |id: &str, dir: &str| !has(&cluster.forbidden_dlc_ids, id) && (has(&cluster.required_dlc_ids, id) || refs(&format!("{dir}::")));
        ActiveDlc {
            expansion1: flag(EXPANSION1_ID, "expansion1"),
            dlc2: flag(DLC2_ID, "dlc2"),
            dlc3: flag(DLC3_ID, "dlc3"),
            dlc4: flag(DLC4_ID, "dlc4"),
            dlc5: flag(DLC5_ID, "dlc5"),
        }
    }

    /// [`infer_from_cluster`](Self::infer_from_cluster) plus the DLC3/4/5 packs
    /// that the coordinate's non-default quality and mixing levels need: the
    /// game only reads those levels when their content is subscribed
    /// (`GetCurrentQualitySetting` / `GetCurrentMixingSettingLevel`), so a
    /// coordinate that uses them implies the pack. Spaced Out and DLC2 keep
    /// the cluster rule, which leaves every coordinate without DLC3/4/5
    /// settings exactly as before.
    pub fn infer(cluster: &ClusterYaml, settings: &crate::custom_settings::CustomSettings) -> Self {
        let mut out = Self::infer_from_cluster(cluster);
        for id in settings.required_content() {
            if matches!(id, DLC3_ID | DLC4_ID | DLC5_ID) { out.insert(id); }
        }
        out
    }

    /// [`infer`](Self::infer) from a coordinate text; the cluster rule alone
    /// when its setting fields do not decode.
    pub fn infer_for_coordinate(cluster: &ClusterYaml, coordinate: &str) -> Self {
        let settings = crate::coord::Coord::parse(coordinate)
            .and_then(|c| crate::custom_settings::decode(&c.other, &c.story, &c.mixing).ok());
        match settings { Some(s) => Self::infer(cluster, &s), None => Self::infer_from_cluster(cluster) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data_source::WorldPlacementYaml;

    fn cluster(prefix: &str, required: &[&str], forbidden: &[&str], worlds: &[&str]) -> ClusterYaml {
        ClusterYaml {
            id: String::new(), poi_placements: None, coordinate_prefix: prefix.into(), start_world_index: 0, num_rings: 0,
            world_placements: worlds.iter().map(|w| WorldPlacementYaml {
                world_ref: (*w).into(), buffer: 0, location_type: String::new(), allowed_rings: None, world_mixing: Default::default(),
            }).collect(),
            required_dlc_ids: required.iter().map(|s| s.to_string()).collect(),
            forbidden_dlc_ids: forbidden.iter().map(|s| s.to_string()).collect(), cluster_tags: Vec::new(), disable_story_traits: false, fixed_coordinate: -1, skip: false,
        }
    }

    #[test]
    fn parses_cli_lists() {
        assert_eq!(ActiveDlc::parse("none").unwrap(), ActiveDlc::VANILLA);
        assert_eq!(ActiveDlc::parse("DLC2_ID").unwrap(), ActiveDlc { dlc2: true, ..ActiveDlc::VANILLA });
        let both = ActiveDlc::parse("EXPANSION1_ID, DLC2_ID").unwrap();
        assert_eq!(both, ActiveDlc { expansion1: true, dlc2: true, ..ActiveDlc::VANILLA });
        assert_eq!(both.ids(), vec!["DLC2_ID", "EXPANSION1_ID"]);
        for bad in ["", "EXPANSION1_ID,EXPANSION1_ID", "DLC6_ID", "expansion1", "EXPANSION1_ID,"] {
            assert!(ActiveDlc::parse(bad).is_err(), "{bad}");
        }
    }

    // Code-derived from ClusterLayouts.UpdateClusterCache + IsCorrectDlcSubscribed
    // and the shipped cluster YAML restrictions.
    #[test]
    fn cluster_availability_follows_the_game() {
        let base = ActiveDlc::VANILLA;
        let base_d2 = ActiveDlc { dlc2: true, ..ActiveDlc::VANILLA };
        let e1 = ActiveDlc { expansion1: true, ..ActiveDlc::VANILLA };
        let e1_d2 = ActiveDlc { expansion1: true, dlc2: true, ..ActiveDlc::VANILLA };
        let bad_a = cluster("BAD-A", &[], &["EXPANSION1_ID"], &["worlds/Badlands"]);
        let sndst_a = cluster("SNDST-A", &[], &[], &["worlds/SandstoneDefault"]);
        let sndst_c = cluster("SNDST-C", &["EXPANSION1_ID"], &[], &["expansion1::worlds/TerraMoonlet"]);
        let cer_a = cluster("CER-A", &["DLC2_ID"], &["EXPANSION1_ID"], &["dlc2::worlds/CeresBaseGameAsteroid"]);
        let cer_c = cluster("CER-C", &["EXPANSION1_ID", "DLC2_ID"], &[], &["dlc2::worlds/CeresSpacedOutAsteroid"]);
        let ok = |d: ActiveDlc, c: &ClusterYaml| d.check_cluster(c).is_ok();
        assert!(ok(base, &bad_a) && ok(base_d2, &bad_a) && !ok(e1, &bad_a) && !ok(e1_d2, &bad_a));
        assert!(ok(base, &sndst_a) && ok(base_d2, &sndst_a) && ok(e1, &sndst_a) && ok(e1_d2, &sndst_a));
        assert!(!ok(base, &sndst_c) && !ok(base_d2, &sndst_c) && ok(e1, &sndst_c) && ok(e1_d2, &sndst_c));
        assert!(!ok(base, &cer_a) && ok(base_d2, &cer_a) && !ok(e1, &cer_a) && !ok(e1_d2, &cer_a));
        assert!(!ok(base, &cer_c) && !ok(base_d2, &cer_c) && !ok(e1, &cer_c) && ok(e1_d2, &cer_c));
        // SNDST-A's implicit requirement follows the active set; it decides the
        // generic geyser pool.
        assert!(!base.expansion1_geysers(&sndst_a) && e1.expansion1_geysers(&sndst_a));
        assert!(e1_d2.expansion1_geysers(&cer_c) && !base_d2.expansion1_geysers(&cer_a));
    }

    #[test]
    fn inference_matches_the_previous_cluster_heuristic() {
        let sndst_c = cluster("SNDST-C", &["EXPANSION1_ID"], &[], &["expansion1::worlds/TerraMoonlet"]);
        assert_eq!(ActiveDlc::infer_from_cluster(&sndst_c), ActiveDlc { expansion1: true, ..ActiveDlc::VANILLA });
        let cer_a = cluster("CER-A", &["DLC2_ID"], &["EXPANSION1_ID"], &["dlc2::worlds/CeresBaseGameAsteroid"]);
        assert_eq!(ActiveDlc::infer_from_cluster(&cer_a), ActiveDlc { dlc2: true, ..ActiveDlc::VANILLA });
        let sndst_a = cluster("SNDST-A", &[], &[], &["worlds/SandstoneDefault"]);
        assert_eq!(ActiveDlc::infer_from_cluster(&sndst_a), ActiveDlc::VANILLA);
    }
}
