//! Runtime loader for the game's worldgen YAML tree.
//!
//! The ONI data layout under `OxygenNotIncluded_Data/StreamingAssets/`
//! is a set of YAML files under three root directories:
//!
//!   worldgen/                      — base game
//!   dlc/expansion1/worldgen/       — Spaced Out
//!   dlc/dlc2/worldgen/             — Frosty Planet
//!   dlc/dlc3/worldgen/             — Bionic Booster (mixing.yaml only)
//!   dlc/dlc4/worldgen/             — Prehistoric Planet
//!   dlc/dlc5/worldgen/             — Aquatic Planet
//!
//! Each root has `clusters/`, `worlds/`, `subworlds/`, `features/`,
//! `traits/`, `biomes/`, plus top level `*.yaml` files. References
//! between files use `dlc_id::relative/path`; a bare path means
//! the same DLC tree the referrer lives in. This module resolves
//! those references eagerly on `load` and caches everything by
//! fully qualified name.
//!
//! The game data is not shipped in this crate (not ours to
//! redistribute). Callers supply the path to their local copy.

use std::collections::BTreeMap;
use crate::lazy::LazyMap;
use std::path::{Path, PathBuf};

/// Fully qualified name, e.g. `expansion1::worlds/VanillaSandstoneDefault`
/// or `worlds/SandstoneDefault` (base game).
pub type QName = String;

/// Parsed cluster YAML (subset we need).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ClusterYaml {
    /// `ClusterLayout.filePath` (`Cluster.Id`), e.g.
    /// `expansion1::clusters/MiniClusterRadioactiveOceanStart`.
    pub id: QName,
    pub coordinate_prefix: String,
    pub start_world_index: i32,
    /// `ClusterLayout.numRings`; the constructor default 12 when absent.
    pub num_rings: i32,
    pub world_placements: Vec<WorldPlacementYaml>,
    pub required_dlc_ids: Vec<String>,
    pub forbidden_dlc_ids: Vec<String>,
    pub cluster_tags: Vec<String>,
    /// `clusterLayout.disableStoryTraits`: the Cluster constructor then
    /// ignores every chosen story.
    pub disable_story_traits: bool,
    /// `ClusterLayout.fixedCoordinate`; -1 when absent. The destination
    /// screen forces the seed to this value.
    pub fixed_coordinate: i32,
    /// `ClusterLayout.skip` is `Always` or `EditorOnly`: outside the Unity
    /// editor `ClusterLayouts.UpdateClusterCache` does not load the cluster.
    pub skip: bool,
    /// `poiPlacements`; `None` when the YAML has no list.
    pub poi_placements: Option<Vec<PoiPlacementYaml>>,
}

/// `ProcGen.SpaceMapPOIPlacement`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PoiPlacementYaml {
    pub pois: Vec<String>,
    pub num_to_spawn: i32,
    /// Defaults to (0, 9999).
    pub allowed_rings: (i32, i32),
    pub avoid_clumping: bool,
    pub can_spawn_duplicates: bool,
    pub guarantee: bool,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WorldPlacementYaml {
    pub world_ref: QName,
    pub buffer: i32,
    pub location_type: String,
    pub allowed_rings: Option<(i32, i32)>,
    pub world_mixing: PlacementMixing,
}

/// `WorldPlacement.WorldMixing`. The additions only apply when world mixing
/// actually substituted this placement (`mixingWasApplied`).
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct PlacementMixing {
    pub required_tags: Vec<String>,
    pub forbidden_tags: Vec<String>,
    pub additional_world_template_rules: Vec<serde_yaml::Value>,
    pub additional_unknown_cell_filters: Vec<serde_yaml::Value>,
    pub additional_subworld_files: Vec<SubworldFileRef>,
    pub additional_seasons: Vec<String>,
}

/// `worldMixing/*.yaml` (`ProcGen.WorldMixingSettings`).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WorldMixingYaml {
    pub forbidden_cluster_tags: Vec<String>,
    pub world: QName,
}

/// `subworldMixing/*.yaml` (`ProcGen.SubworldMixingSettings`).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SubworldMixingYaml {
    pub forbidden_cluster_tags: Vec<String>,
    pub subworld: SubworldFileRef,
    pub mixing_tags: Vec<String>,
    pub additional_world_template_rules: Vec<serde_yaml::Value>,
}

/// Parsed world YAML (subset we need).
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct WorldYaml {
    pub starting_world_elements: Option<serde_yaml::Value>,
    pub modify_layout_tags: Vec<serde_yaml::Value>,
    pub data_overrides: serde_yaml::Value,
    pub qname: QName,
    pub size: (i32, i32),
    pub layout_method: String,
    pub starting_base_template: Option<String>,
    pub subworld_files: Vec<SubworldFileRef>,
    pub world_template_rules: Vec<serde_yaml::Value>,
    pub unknown_cells_allowed_subworlds: Vec<serde_yaml::Value>,
    /// Distilled `defaultsOverrides.data` settings that drive the
    /// overworld Voronoi sampler. Missing keys fall back to the
    /// game's `defaults.yaml` values (600 / 700 / 72 / UniformHex).
    pub overworld_density_min: f32,
    pub overworld_density_max: f32,
    pub overworld_avoid_radius: f32,
    pub overworld_sample_behaviour: String,
    pub disable_world_traits: bool,
    pub world_trait_rules: Vec<WorldTraitRule>,
    /// Scales `globalFeatureMods` from each trait. Small worlds use
    /// values like 0.2; full-size worlds use 1.0. A trait with
    /// `floor(scale * sum_of_mods) == 0` is filtered out.
    pub world_trait_scale: f32,
    pub subworld_mixing_rules: Vec<SubworldMixingRule>,
    pub world_tags: Vec<String>,
    pub seasons: Vec<String>,
    /// `startingBasePositionHorizontal` (fraction of map width) range.
    pub starting_base_position_horizontal: (f32, f32),
    /// `startingBasePositionVertical` (fraction of map height) range.
    pub starting_base_position_vertical: (f32, f32),
    pub start_subworld_name: Option<String>,
    /// YAML/dictionary insertion order controls global-feature assignment.
    pub global_features: Vec<(String, i32)>,
    pub default_move_tags: Vec<String>,
    pub overworld_add_tags: Vec<String>,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct WorldTraitRule {
    pub min: i32,
    pub max: i32,
    pub required_tags: Vec<String>,
    pub forbidden_tags: Vec<String>,
    pub forbidden_traits: Vec<String>,
    pub specific_traits: Vec<String>,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct SubworldMixingRule {
    pub name: String,         // e.g. "(Mixing1)"
    pub min_count: i32,
    pub max_count: i32,
    pub required_tags: Vec<String>,
    pub forbidden_tags: Vec<String>,
}

/// A world trait — adds extra rules / subworlds / cell filters when
/// selected via `worldTraitRules`.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct TraitYaml {
    pub element_band_modifiers: Vec<crate::terrain::BandModifier>,
    /// `startingBasePositionHorizontalMod` / `VerticalMod` (min, max), added to
    /// the world's start-position ranges by `ProcGen.World.ModStartLocation`.
    pub starting_base_position_mod: ((f32, f32), (f32, f32)),
    pub qname: QName,
    pub trait_tags: Vec<String>,
    pub exclusive_with: Vec<String>,
    pub exclusive_with_tags: Vec<String>,
    pub forbidden_dlc_ids: Vec<String>,
    pub additional_world_template_rules: Vec<serde_yaml::Value>,
    pub additional_subworld_files: Vec<SubworldFileRef>,
    pub additional_unknown_cell_filters: Vec<serde_yaml::Value>,
    /// `ruleId`s to drop from the world's existing rules before
    /// merging in `additional_world_template_rules`. Used by GeoDormant
    /// to swap out the default GenericGeysers rule for a smaller one.
    pub remove_world_template_rules_by_id: Vec<String>,
    /// Per-feature counts that scale with `worldTraitScale`. Used by
    /// `is_valid_for_world` to rule out a trait whose scaled count
    /// rounds to 0 on a given world.
    pub global_feature_mods: Vec<(String, f32)>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SubworldFileRef {
    pub name: QName,
    pub weight: f32,
    pub min_count: i32,
    pub max_count: i32,
    pub priority: i32,
    pub override_power: f32,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct DataSource {
    pub template_entities:LazyMap<QName,Result<crate::template_entities::Template,String>>,
    pub template_cells:LazyMap<QName,Result<Vec<crate::template_cells::Cell>,String>>,
    pub template_geometry:BTreeMap<QName,crate::template_placement::Geometry>,
    pub borders: BTreeMap<String,Vec<crate::foreground::Choice>>,
    pub rooms: BTreeMap<String,crate::action_cells::ActionSample>,
    pub mobs: BTreeMap<String,crate::action_cells::ActionSample>,
    pub elements: BTreeMap<String,crate::terrain::ElementDefinition>,
    pub temperatures: BTreeMap<String,(f32,f32)>,
    pub biomes: BTreeMap<QName, Vec<crate::terrain::ElementBand>>,
    pub defaults: serde_yaml::Value,
    pub clusters: LazyMap<String, ClusterYaml>, // keyed by coordinatePrefix
    pub worlds: LazyMap<QName, WorldYaml>,
    /// Template path (relative to `templates/`, e.g.
    /// `poi/geysers/geyser_steam`) → geyser entries extracted from
    /// that template's `otherEntities`.
    pub templates: BTreeMap<String, TemplateInfo>,
    /// Qname → subworld YAML. Qname uses the same
    /// `dlc::subworlds/...` convention as worlds.
    pub subworlds: LazyMap<QName, SubWorldYaml>,
    pub features: BTreeMap<QName, FeatureMetadata>,
    pub noise: BTreeMap<QName, crate::noise_tree::TreeJson>,
    /// Traits in load order. The game's `GetRandomTraits` consumes
    /// the pool in order, so insertion order matters.
    pub traits: Vec<(QName, TraitYaml)>,
    /// Keyed by scoped path, e.g. `dlc2::worldMixing/CeresMixingSettings`.
    pub world_mixing: BTreeMap<QName, WorldMixingYaml>,
    pub subworld_mixing: BTreeMap<QName, SubworldMixingYaml>,
    /// `SettingsCache.storyTraits`, keyed by scoped file path such as
    /// `storytraits/CritterManipulator`, in load order.
    pub story_traits: Vec<(QName, TraitYaml)>,
    /// `DlcMixingSettings.spacePois` from each DLC tree's `mixing.yaml`,
    /// keyed by DLC id (`GetDlcIdFromContentDirectory`).
    pub dlc_mixing_space_pois: BTreeMap<String, Vec<PoiPlacementYaml>>,
    /// `DlcMixingSettings.spaceDesinations` (sic) from the same files:
    /// (type, minTier, maxTier), `SpaceDestinationMix` defaulting to 0..=99.
    pub dlc_mixing_space_destinations: BTreeMap<String, Vec<(String, i32, i32)>>,
    /// Destination-screen metadata of each cluster, keyed by coordinatePrefix.
    pub cluster_menu: BTreeMap<String, ClusterMenu>,
    /// SHA-256 (hex) over the path and text of every file the loader read,
    /// overlay included; set by the `*_with` loaders. Equal file sets give
    /// equal checksums however the overlay was built.
    pub checksum: Option<String>,
}

/// Cluster fields only the game's destination screen uses.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct ClusterMenu {
    /// String keys such as `STRINGS.CLUSTER_NAMES.SANDSTONE_START_CLUSTER.NAME`;
    /// the text lives in the game assembly, not StreamingAssets.
    pub name: String,
    pub description: String,
    /// `ClusterLayout.ClusterCategory` by name; some files give the number.
    pub category: String,
    pub menu_order: i32,
    /// `skip`, e.g. `EditorOnly` for clusters the game hides outside debug mode.
    pub skip: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SubWorldYaml {
    pub border_override: Option<String>,
    pub border_override_priority: i32,
    pub border_size_override: crate::subworld::Density,
    pub qname: QName,
    pub zone_type: String,
    pub temperature_range: String,
    pub pd_weight: f32,
    /// Subworld tags such as `NoGlobalFeatureSpawning`,
    /// `IgnoreCaveOverride`. Tag-driven `allowedCellsFilter` clauses
    /// (e.g. `NotAtTag: NoGlobalFeatureSpawning`) consult these.
    pub tags: Vec<String>,
    /// Per-subworld template rules; the game appends these to the
    /// world's own `worldTemplateRules` before placement.
    pub subworld_template_rules: Vec<serde_yaml::Value>,
    pub children: crate::subworld::ChildConfig,
    pub noise_refs: [Option<String>;4],
}

/// Metadata needed for child classification. Unused legacy feature files can
/// contain duplicate terrain-choice keys; deserialize only the fields used here.
#[derive(Debug, Clone, Default, serde::Deserialize, serde::Serialize)]
#[serde(default, rename_all="camelCase")]
pub struct FeatureMetadata {
    pub internal_mobs:Vec<AmbientMobReference>,
    pub tags: Vec<String>,
    pub force_biome: Option<String>,
    pub biome_tags: Option<Vec<String>>,
    pub shape: Option<String>,
    pub blob_size: Option<crate::subworld::Density>,
    pub borders: Vec<i32>,
    #[serde(rename="ElementChoiceGroups")]
    pub element_choice_groups: BTreeMap<String,crate::foreground::ChoiceGroup>,
}

#[derive(Debug,Clone,serde::Deserialize,serde::Serialize)]
pub struct AmbientMobReference {
    #[serde(rename="type")] pub kind:String,
    pub count:crate::subworld::Density,
}

/// Minimal template parse — we only care about the geyser entries
/// inside a template's `otherEntities`. Other prefabs (buildings,
/// pickupables) are ignored for now; they don't emit geysers.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct TemplateInfo {
    pub path: String,
    pub geysers: Vec<TemplateGeyser>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TemplateGeyser {
    /// `GeyserGeneric_steam` → `steam`, `OilWell` as-is.
    pub type_id: String,
    pub location_x: i32,
    pub location_y: i32,
}

impl DataSource {
    pub fn world_setting(&self,world:&str,key:&str)->Result<&serde_yaml::Value,String> {
        let world=self.worlds.get(world).ok_or_else(||format!("missing world {world}"))?;
        world.data_overrides.get(key).or_else(||self.defaults.get("data").and_then(|v|v.get(key)))
            .ok_or_else(||format!("missing world setting {key}"))
    }
    /// Load from the bundle written by `tools/yaml_to_json`: a JSON object
    /// whose `files` map StreamingAssets-relative paths to file text. Used by
    /// the WASM build, which has no filesystem.
    pub fn from_bundle(bundle_json: &str) -> Result<Self, String> {
        #[derive(serde::Deserialize)]
        struct Bundle { files: BTreeMap<String, String> }
        let bundle: Bundle = serde_json::from_str(bundle_json).map_err(|e| format!("bundle parse: {e}"))?;
        Self::from_files(&bundle.files)
    }

    /// [`load`](Self::load) over StreamingAssets files keyed by relative path.
    pub fn from_files(files: &BTreeMap<String, String>) -> Result<Self, String> {
        Self::load_from(&Files::Bundle(files), Path::new(""), &Overlay::default(), false)
    }

    /// [`from_files`](Self::from_files) with `overlay` applied; also sets
    /// [`checksum`](Self::checksum).
    pub fn from_files_with(files: &BTreeMap<String, String>, overlay: &Overlay) -> Result<Self, String> {
        Self::load_from(&Files::Bundle(files), Path::new(""), overlay, true)
    }

    /// Load from `root` and return every file the loader read, keyed by path
    /// relative to `root`; [`from_files`](Self::from_files) on the result
    /// builds the same `DataSource`.
    pub fn bundle_files(root: impl AsRef<Path>) -> Result<BTreeMap<String, String>, String> {
        let files = Files::Record(root.as_ref().to_path_buf(), Default::default());
        Self::load_from(&files, root.as_ref(), &Overlay::default(), false)?;
        let Files::Record(_, read) = files else { unreachable!() };
        Ok(read.into_inner())
    }

    /// Load every cluster / world YAML under `root`. Root should
    /// point at the directory that contains `worldgen/` and
    /// optionally `dlc/expansion1/worldgen/` etc.
    pub fn load(root: impl AsRef<Path>) -> Result<Self, String> {
        Self::load_from(&Files::Disk, root.as_ref(), &Overlay::default(), false)
    }

    /// [`load`](Self::load) with `overlay` applied; also sets
    /// [`checksum`](Self::checksum).
    pub fn load_with(root: impl AsRef<Path>, overlay: &Overlay) -> Result<Self, String> {
        Self::load_from(&Files::Disk, root.as_ref(), overlay, true)
    }

    fn load_from(base: &Files, root: &Path, overlay: &Overlay, checksum: bool) -> Result<Self, String> {
        let fs = &View { base, root, overlay, reads: checksum.then(Default::default) };
        let mut ds = DataSource::default();
        // ElementLoader.CollectElementsFromYAML: every copy, in GetFiles order;
        // the first definition of an element id wins.
        #[derive(serde::Deserialize)]
        struct Elements {elements:Vec<crate::terrain::ElementDefinition>}
        for (path,src) in fs.sources(&root.join("elements")) {
            if path.file_name().and_then(|n|n.to_str()).is_some_and(|n|n.starts_with('.')) {continue;}
            let text=fs.read_from(&path,src).map_err(|e|e.to_string())?;
            let entries:Elements=crate::klei_yaml::from_str(&text).map_err(|e|format!("{}: {e}",path.display()))?;
            for e in entries.elements {ds.elements.entry(e.element_id.clone()).or_insert(e);}
        }
        let defaults_path = root.join("worldgen/defaults.yaml");
        if fs.exists(&defaults_path) {
            ds.defaults = crate::klei_yaml::parse(&fs.read(&defaults_path).map_err(|e|e.to_string())?)?;
        }

        // The base tree plus one per installed DLC; all optional. A DLC tree in the game data is
        // treated as subscribed content (LoadTrait's forbiddenDLCIds check);
        // edits and mods cannot add one.
        // `DlcManager.RELEASED_VERSIONS` order: base, Spaced Out, then the
        // content packs in `DLC_PACKS` order.
        let content: Vec<(&str, &str)> = std::iter::once(("EXPANSION1_ID", "expansion1")).chain(crate::dlc::CONTENT_DLCS).collect();
        let subscribed: Vec<&str> = content.iter()
            .filter(|(_, dir)| fs.installed(&root.join(format!("dlc/{dir}/worldgen")))).map(|(id, _)| *id).collect();
        let trees: Vec<(&str, PathBuf)> = std::iter::once(("", root.join("worldgen")))
            .chain(content.iter().map(|(_, dir)| (*dir, root.join(format!("dlc/{dir}/worldgen"))))).collect();

        for (dlc_id, tree_root) in &trees {
            if !fs.installed(tree_root) {
                continue;
            }
            // SettingsCache.MergeLoad tables; see `Overlay`.
            for (filename, table, target) in [("rooms.yaml",None,&mut ds.rooms),("mobs.yaml",Some("MobLookupTable"),&mut ds.mobs)] {
                let path=tree_root.join(filename);
                let Some(entries)=fs.merge_load(&path,table)? else {continue};
                merge_into(&path,target,entries,|name,value|serde_yaml::from_value(value).map_err(|e|format!("{} / {name}: {e}",path.display())))?;
            }
            let path=tree_root.join("temperatures.yaml");
            if let Some(entries)=fs.merge_load(&path,None)? {
                merge_into(&path,&mut ds.temperatures,entries,|_,value|Ok((value["min"].as_f64().ok_or("missing temperature min")? as f32,
                    value["max"].as_f64().ok_or("missing temperature max")? as f32)))?;
            }
            ds.load_clusters(fs, dlc_id, tree_root)?;
            ds.load_mixing(fs, dlc_id, tree_root)?;
            let path=tree_root.join("borders.yaml");
            if let Some(entries)=fs.merge_load(&path,None)? {
                merge_into(&path,&mut ds.borders,entries,|_,value|serde_yaml::from_value(value).map_err(|e|format!("border {}: {e}",path.display())))?;
            }
            ds.load_worlds(fs, dlc_id, tree_root)?;
            ds.load_subworlds(fs, dlc_id, tree_root)?;
            ds.load_features(fs, dlc_id, tree_root)?;
            ds.load_noise(fs, dlc_id, tree_root)?;
            let mut biome_result=Ok(());
            walk_yamls(fs, &tree_root.join("biomes"), &mut |path| {
                if biome_result.is_err() {return;}
                biome_result=(|| {
                    let entries=fs.merge_load(path,Some("TerrainBiomeLookupTable"))?.unwrap_or_default();
                    let relative=path.strip_prefix(tree_root).unwrap().with_extension("").to_string_lossy().replace('\\',"/");
                    ds.absorb_biome(dlc_id,&relative,entries)
                })();
            });
            biome_result?;
            ds.load_traits(fs, dlc_id, tree_root);
            ds.load_story_traits(fs, dlc_id, tree_root, &subscribed);
            // SettingsCache.LoadDlcMixingSettings: DLC trees only, and only when
            // the game's files have it (File.Exists on its own path; a mod can
            // replace it but not add it).
            let mixing_path = tree_root.join("mixing.yaml");
            if !dlc_id.is_empty() && fs.base_exists(&mixing_path) {
                let text = fs.read(&mixing_path).map_err(|e| e.to_string())?;
                let y = crate::klei_yaml::parse(&text).map_err(|e| format!("{}: {e}", mixing_path.display()))?;
                let pois = y.get("spacePois").and_then(|v| v.as_sequence()).map(|s| s.iter().map(poi_placement).collect()).unwrap_or_default();
                ds.dlc_mixing_space_pois.insert(format!("{}_ID", dlc_id.to_uppercase()), pois);
                let destinations = y.get("spaceDesinations").and_then(|v| v.as_sequence()).map(|s| s.iter().map(|d| (
                    d.get("type").and_then(|v| v.as_str()).unwrap_or_default().to_string(),
                    d.get("minTier").and_then(|v| v.as_i64()).unwrap_or(0) as i32,
                    d.get("maxTier").and_then(|v| v.as_i64()).unwrap_or(99) as i32)).collect()).unwrap_or_default();
                ds.dlc_mixing_space_destinations.insert(format!("{}_ID", dlc_id.to_uppercase()), destinations);
            }
        }

        // One template tree per content root. World rules reference DLC-scoped
        // templates with `expansion1::` / `dlc2::` / ... prefixes.
        let tmpl_trees: Vec<(&str, PathBuf)> = std::iter::once(("", root.join("templates")))
            .chain(content.iter().map(|(_, dir)| (*dir, root.join(format!("dlc/{dir}/templates"))))).collect();
        for (dlc_id, tmpl_root) in &tmpl_trees {
            if fs.exists(tmpl_root) {
                ds.load_templates(fs, dlc_id, tmpl_root)?;
            }
        }

        ds.checksum = fs.checksum();
        Ok(ds)
    }

    /// One biome file's merged `TerrainBiomeLookupTable` (`LoadBiome`).
    fn absorb_biome(&mut self, dlc_id: &str, path: &str, entries: Vec<(String, serde_yaml::Value)>) -> Result<(),String> {
        let path=path.strip_prefix("biomes/").unwrap_or(path).trim_end_matches(".yaml");
        for (name,value) in entries {
            let bands=serde_yaml::from_value(value).map_err(|e|format!("biome {path}/{name}: {e}"))?;
            self.biomes.insert(resolve_qname(dlc_id,&format!("biomes/{path}/{name}")),bands);
        }
        Ok(())
    }

    pub fn biome_bands(&self, name: &str, traits: &[String]) -> Result<Option<Vec<crate::terrain::ElementBand>>,String> {
        let Some(bands)=self.biomes.get(name) else {return Ok(None)};
        let mut modifiers=Vec::new();
        for name in traits {
            let (_,t)=self.traits.iter().find(|(id,_)|id==name).ok_or_else(||format!("missing trait {name}"))?;
            modifiers.extend(t.element_band_modifiers.iter().cloned());
        }
        Ok(Some(crate::terrain::resolve_bands(bands,&modifiers)))
    }

    fn load_noise(&mut self, fs: &View, dlc_id: &str, tree_root: &Path) -> Result<(), String> {
        let dir = tree_root.join("noise");
        let mut result = Ok(());
        walk_yamls(fs, &dir, &mut |path| {
            if result.is_err() { return; }
            result = (|| {
                let text = fs.read(path).map_err(|e|format!("{}: {e}",path.display()))?;
                let tree = crate::klei_yaml::from_str(&text).map_err(|e|format!("{}: {e}",path.display()))?;
                let relative = path.strip_prefix(&dir).unwrap().with_extension("").to_string_lossy().replace('\\',"/");
                self.noise.insert(resolve_qname(dlc_id,&format!("noise/{relative}")),tree);
                Ok(())
            })();
        });
        result
    }

    fn load_features(&mut self, fs: &View, dlc_id: &str, tree_root: &Path) -> Result<(), String> {
        let dir = tree_root.join("features");
        let mut result = Ok(());
        walk_yamls(fs, &dir, &mut |path| {
            if result.is_err() { return; }
            result = (|| {
                let text = fs.read(path).map_err(|e|format!("{}: {e}",path.display()))?;
                let y = crate::klei_yaml::from_str(&text).map_err(|e|format!("{}: {e}",path.display()))?;
                let relative = path.strip_prefix(&dir).unwrap().with_extension("").to_string_lossy().replace('\\',"/");
                self.features.insert(resolve_qname(dlc_id,&format!("features/{relative}")),y);
                Ok(())
            })();
        });
        result
    }

    /// Port of `SettingsCache.LoadWorldTraits`: `traits/*.yaml` sorted by full
    /// path ignoring case (Geodes precedes GeoDormant: 'd-e' < 'd-o' at
    /// offset 4). The game's pool keeps this load order.
    fn load_traits(&mut self, fs: &View, dlc_id: &str, tree_root: &Path) {
        for (path, src) in sorted_sources(fs, &tree_root.join("traits")) {
            let Ok(text) = fs.read_from(&path, src) else { continue };
            let Ok(y) = crate::klei_yaml::parse(&text) else { continue };
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else { continue };
            self.absorb_trait(dlc_id, &format!("traits/{}", stem), &y);
        }
    }

    /// Port of `SettingsCache.LoadStoryTraits`: `storytraits/*.yaml` of every
    /// loaded tree, sorted by full path ignoring case, skipping traits whose
    /// `forbiddenDLCIds` names subscribed content.
    fn load_story_traits(&mut self, fs: &View, dlc_id: &str, tree_root: &Path, subscribed: &[&str]) {
        for (path, src) in sorted_sources(fs, &tree_root.join("storytraits")) {
            let Ok(text) = fs.read_from(&path, src) else { continue };
            let Ok(y) = crate::klei_yaml::parse(&text) else { continue };
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else { continue };
            self.absorb_story_trait(dlc_id, stem, &y, subscribed);
        }
    }

    fn absorb_story_trait(&mut self, dlc_id: &str, stem: &str, y: &serde_yaml::Value, subscribed: &[&str]) {
        let t = parse_trait(resolve_qname(dlc_id, &format!("storytraits/{stem}")), y);
        if t.forbidden_dlc_ids.iter().any(|id| subscribed.contains(&id.as_str())) { return; }
        // LoadTrait overwrites an existing key (with a dev assert).
        if let Some(existing) = self.story_traits.iter_mut().find(|(k, _)| *k == t.qname) { existing.1 = t; }
        else { self.story_traits.push((t.qname.clone(), t)); }
    }

    pub fn story_trait(&self, qname: &str) -> Option<&TraitYaml> {
        self.story_traits.iter().find(|(k, _)| k == qname).map(|(_, t)| t)
    }

    fn absorb_trait(&mut self, dlc_id: &str, rel: &str, y: &serde_yaml::Value) {
        // rel may be "traits/Foo" (filesystem) or "traits/Foo" (bundle).
        let path = rel.strip_prefix("traits/").unwrap_or(rel);
        let info = parse_trait(resolve_qname(dlc_id, &format!("traits/{}", path)), y);
        // `traitsDict[key] = trait`: a later copy (an overlay's) keeps the slot.
        if let Some(existing) = self.traits.iter_mut().find(|(k, _)| *k == info.qname) { existing.1 = info; }
        else { self.traits.push((info.qname.clone(), info)); }
    }
}

/// [`View::sources`] in `List.Sort` order by full path ignoring case. Mono's
/// sort is unstable, which decides between copies of the same file.
fn sorted_sources(fs: &View, dir: &Path) -> Vec<(PathBuf, Src)> {
    let mut list = fs.sources(dir);
    crate::template_placement::mono_sort(&mut list, &|a: &(PathBuf, Src), b: &(PathBuf, Src)| cmp_ignore_case(&fs.rel(&a.0), &fs.rel(&b.0)).is_lt());
    list
}

fn parse_trait(qname: QName, y: &serde_yaml::Value) -> TraitYaml {
    let str_seq = |k: &str| -> Vec<String> {
        y.get(k)
            .and_then(|v| v.as_sequence())
            .map(|s| s.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect())
            .unwrap_or_default()
    };
    let trait_tags = str_seq("traitTags");
    let exclusive_with = str_seq("exclusiveWith");
    let exclusive_with_tags = str_seq("exclusiveWithTags");
    let forbidden_dlc_ids = str_seq("forbiddenDLCIds");
    let additional_world_template_rules: Vec<serde_yaml::Value> = y
        .get("additionalWorldTemplateRules")
        .and_then(|v| v.as_sequence())
        .cloned()
        .unwrap_or_default();
    let additional_unknown_cell_filters: Vec<serde_yaml::Value> = y
        .get("additionalUnknownCellFilters")
        .and_then(|v| v.as_sequence())
        .cloned()
        .unwrap_or_default();
    let mut additional_subworld_files = Vec::new();
    if let Some(list) = y
        .get("additionalSubworldFiles")
        .and_then(|v| v.as_sequence())
    {
        for s in list {
            let Some(name) = s.get("name").and_then(|v| v.as_str()) else { continue };
            let weight = s.get("weight").and_then(|v| v.as_f64()).unwrap_or(1.0) as f32;
            let min_count = s.get("minCount").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
            let max_count = s.get("maxCount").and_then(|v| v.as_i64()).unwrap_or(2147483647) as i32;
            let priority = s.get("priority").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
            let override_power = s.get("overridePower").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
            // References stay as-written: an unprefixed
            // `subworlds/x` is a base reference, not the
            // current-DLC's copy.
            additional_subworld_files.push(SubworldFileRef {
                name: name.to_string(),
                weight,
                min_count,
                max_count,
                priority,
                override_power,
            });
        }
    }
    let remove_world_template_rules_by_id = str_seq("removeWorldTemplateRulesById");
    // MinMax fields default to 0 when absent.
    let min_max = |k: &str| y.get(k).map_or((0.0, 0.0), |v| (
        v.get("min").and_then(|x| x.as_f64()).unwrap_or(0.0) as f32,
        v.get("max").and_then(|x| x.as_f64()).unwrap_or(0.0) as f32));
    let starting_base_position_mod = (min_max("startingBasePositionHorizontalMod"), min_max("startingBasePositionVerticalMod"));
    let mut global_feature_mods = Vec::new();
    if let Some(map) = y.get("globalFeatureMods").and_then(|v| v.as_mapping()) {
        for (k, v) in map {
            if let (Some(k), Some(n)) = (k.as_str(), v.as_f64()) {
                global_feature_mods.push((k.to_string(), n as f32));
            }
        }
    }
    TraitYaml {
        element_band_modifiers: y.get("elementBandModifiers").and_then(|v|serde_yaml::from_value(v.clone()).ok()).unwrap_or_default(),
        qname,
        trait_tags,
        exclusive_with,
        exclusive_with_tags,
        forbidden_dlc_ids,
        additional_world_template_rules,
        additional_subworld_files,
        additional_unknown_cell_filters,
        remove_world_template_rules_by_id,
        global_feature_mods,
        starting_base_position_mod,
    }
}

impl DataSource {

    fn load_subworlds(&mut self, fs: &View, dlc_id: &str, tree_root: &Path) -> Result<(), String> {
        let dir = tree_root.join("subworlds");
        if !fs.exists(&dir) {
            return Ok(());
        }
        let mut result = Ok(());
        walk_yamls(fs, &dir, &mut |path| {
            if result.is_err() { return; }
            let Ok(text) = fs.read(path) else { return };
            let Ok(y) = crate::klei_yaml::parse(&text) else { return };
            let rel_str = match path.strip_prefix(&dir) {
                Ok(p) => p
                    .with_extension("")
                    .to_string_lossy()
                    .replace('\\', "/")
                    .to_string(),
                Err(_) => return,
            };
            result = self.absorb_subworld(dlc_id, &rel_str, &y);
        });
        result
    }

    fn absorb_subworld(&mut self, dlc_id: &str, rel_str: &str, y: &serde_yaml::Value) -> Result<(), String> {
        // rel_str may already include "subworlds/" prefix when called
        // from from_bundle; strip a leading "subworlds/" to keep the
        // qname canonical.
        let path = rel_str.strip_prefix("subworlds/").unwrap_or(rel_str);
        let qname = resolve_qname(dlc_id, &format!("subworlds/{}", path));
        let children = serde_yaml::from_value(y.clone()).map_err(|e| format!("subworld {qname}: {e}"))?;
        let zone_type = y
            .get("zoneType")
            .and_then(|v| v.as_str())
            .unwrap_or("Default")
            .to_string();
        let temperature_range = y
            .get("temperatureRange")
            .and_then(|v| v.as_str())
            .unwrap_or("Temperate")
            .to_string();
        let pd_weight = y
            .get("pdWeight")
            .and_then(|v| v.as_f64())
            .unwrap_or(1.0) as f32;
        let tags: Vec<String> = y
            .get("tags")
            .and_then(|v| v.as_sequence())
            .map(|s| s.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect())
            .unwrap_or_default();
        let subworld_template_rules: Vec<serde_yaml::Value> = y
            .get("subworldTemplateRules")
            .and_then(|v| v.as_sequence())
            .cloned()
            .unwrap_or_default();
        self.subworlds.insert(
            qname.clone(),
            SubWorldYaml {
                border_override_priority: y.get("borderOverridePriority").and_then(|v|v.as_i64()).unwrap_or(0) as i32,
                border_size_override: y.get("borderSizeOverride").map(|v|serde_yaml::from_value(v.clone())).transpose().map_err(|e|format!("border size {qname}: {e}"))?.unwrap_or(crate::subworld::Density {min:1.0,max:2.5}),
                border_override: y.get("borderOverride").and_then(|v|v.as_str()).map(str::to_string),
                qname,
                zone_type,
                temperature_range,
                pd_weight,
                tags,
                subworld_template_rules,
                children,
                noise_refs: ["biomeNoise","overrideNoise","densityNoise","backwallNoise"].map(|k|y.get(k).and_then(|v|v.as_str()).map(str::to_string)),
            },
        );
        Ok(())
    }

    fn load_templates(&mut self, fs: &View, dlc_id: &str, root: &Path) -> Result<(), String> {
        walk_yamls(fs, root, &mut |path| {
            let Ok(text) = fs.read(path) else { return };
            let Ok(y) = crate::klei_yaml::parse(&text) else { return };
            let rel = path
                .strip_prefix(root)
                .ok()
                .and_then(|p| p.with_extension("").to_str().map(|s| s.replace('\\', "/").to_string()));
            let Some(rel) = rel else { return };
            self.absorb_template(dlc_id, &rel, &y);
        });
        Ok(())
    }

    fn absorb_template(&mut self, dlc_id: &str, rel: &str, y: &serde_yaml::Value) {
        let qname = if dlc_id.is_empty() {
            rel.to_string()
        } else {
            format!("{}::{}", dlc_id, rel)
        };
        let mut info = TemplateInfo {
            path: qname.clone(),
            geysers: Vec::new(),
        };
        self.template_geometry.insert(qname.clone(),crate::template_placement::Geometry::from_yaml(y));
        let cells=match y.get("cells") {
            None|Some(serde_yaml::Value::Null)=>Ok(Vec::new()),
            Some(cells)=>serde_yaml::from_value(cells.clone()).map_err(|e|e.to_string()),
        };
        self.template_cells.insert(qname.clone(),cells);
        self.template_entities.insert(qname.clone(),crate::template_entities::Template::from_yaml(y));
        if let Some(others) = y.get("otherEntities").and_then(|v| v.as_sequence()) {
            for e in others {
                let Some(id) = e.get("id").and_then(|v| v.as_str()) else { continue };
                let x = e.get("location_x").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
                let yy = e.get("location_y").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
                if let Some(ty) = id.strip_prefix("GeyserGeneric_") {
                    info.geysers.push(TemplateGeyser {
                        type_id: ty.to_string(),
                        location_x: x,
                        location_y: yy,
                    });
                } else if id == "GeyserGeneric" {
                    info.geysers.push(TemplateGeyser {
                        type_id: "__GENERIC__".to_string(),
                        location_x: x,
                        location_y: yy,
                    });
                } else if id == "OilWell" {
                    info.geysers.push(TemplateGeyser {
                        type_id: "OilWell".to_string(),
                        location_x: x,
                        location_y: yy,
                    });
                }
            }
        }
        if let Some(bs) = y.get("buildings").and_then(|v| v.as_sequence()) {
            for b in bs {
                let Some(id) = b.get("id").and_then(|v| v.as_str()) else { continue };
                if id == "OilWell" {
                    let x = b.get("location_x").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
                    let yy = b.get("location_y").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
                    info.geysers.push(TemplateGeyser {
                        type_id: "OilWell".to_string(),
                        location_x: x,
                        location_y: yy,
                    });
                }
            }
        }
        if !info.geysers.is_empty() {
            self.templates.insert(qname, info);
        }
    }

    fn load_clusters(&mut self, fs: &View, dlc_id: &str, tree_root: &Path) -> Result<(), String> {
        let dir = tree_root.join("clusters");
        if !fs.exists(&dir) {
            return Ok(());
        }
        for path in
            fs.read_dir(&dir).map_err(|e| format!("read {}: {}", dir.display(), e))?
        {
            if path.extension().and_then(|s| s.to_str()) != Some("yaml") {
                continue;
            }
            let text = fs.read(&path)
                .map_err(|e| format!("read {}: {}", path.display(), e))?;
            let Ok(y) = crate::klei_yaml::parse(&text) else { continue };
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else { continue };
            self.absorb_cluster(dlc_id, stem, &y);
        }
        Ok(())
    }

    fn absorb_cluster(&mut self, dlc_id: &str, stem: &str, y: &serde_yaml::Value) {
        let Some(prefix) = y.get("coordinatePrefix").and_then(|v| v.as_str()) else {
            return;
        };
        let start_idx = y
            .get("startWorldIndex")
            .and_then(|v| v.as_i64())
            .unwrap_or(0) as i32;
        let num_rings = y.get("numRings").and_then(|v| v.as_i64()).unwrap_or(12) as i32;
        let mut placements = Vec::new();
        if let Some(list) = y.get("worldPlacements").and_then(|v| v.as_sequence()) {
            for p in list {
                let Some(world_ref) = p.get("world").and_then(|v| v.as_str()) else { continue };
                let buffer = p.get("buffer").and_then(|v| v.as_i64()).unwrap_or(2) as i32;
                let location_type = p
                    .get("locationType")
                    .and_then(|v| v.as_str())
                    .unwrap_or("Cluster")
                    .to_string();
                let allowed_rings = p.get("allowedRings").and_then(parse_minmax);
                let wm = p.get("worldMixing");
                let seq = |k: &str| wm.and_then(|m| m.get(k)).and_then(|v| v.as_sequence()).cloned().unwrap_or_default();
                let world_mixing = PlacementMixing {
                    required_tags: str_list(wm.and_then(|m| m.get("requiredTags"))),
                    forbidden_tags: str_list(wm.and_then(|m| m.get("forbiddenTags"))),
                    additional_world_template_rules: seq("additionalWorldTemplateRules"),
                    additional_unknown_cell_filters: seq("additionalUnknownCellFilters"),
                    additional_subworld_files: subworld_files(wm.and_then(|m| m.get("additionalSubworldFiles"))),
                    additional_seasons: str_list(wm.and_then(|m| m.get("additionalSeasons"))),
                };
                placements.push(WorldPlacementYaml {
                    world_ref: resolve_qname(dlc_id, world_ref),
                    buffer,
                    location_type,
                    allowed_rings,
                    world_mixing,
                });
            }
        }
        let str_seq = |k: &str| -> Vec<String> {
            y.get(k)
                .and_then(|v| v.as_sequence())
                .map(|s| s.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect())
                .unwrap_or_default()
        };
        let text = |k: &str| y.get(k).and_then(|v| v.as_str()).unwrap_or_default().to_string();
        let category = match y.get("clusterCategory") {
            Some(serde_yaml::Value::Number(n)) => n.as_u64().and_then(|n| ["Vanilla", "SpacedOutVanillaStyle", "SpacedOutStyle", "Special"].get(n as usize))
                .map_or_else(|| n.to_string(), |s| s.to_string()),
            _ => text("clusterCategory"),
        };
        self.cluster_menu.insert(prefix.to_string(), ClusterMenu { name: text("name"), description: text("description"), category,
            menu_order: y.get("menuOrder").and_then(|v| v.as_i64()).unwrap_or(0) as i32, skip: text("skip") });
        self.clusters.insert(
            prefix.to_string(),
            ClusterYaml {
                // ClusterLayout.GetName: Path.Combine(addPrefix + "clusters", stem).
                id: resolve_qname(dlc_id, &format!("clusters/{stem}")),
                coordinate_prefix: prefix.to_string(),
                start_world_index: start_idx,
                num_rings,
                world_placements: placements,
                required_dlc_ids: str_seq("requiredDlcIds"),
                forbidden_dlc_ids: str_seq("forbiddenDlcIds"),
                cluster_tags: str_seq("clusterTags"),
                disable_story_traits: y.get("disableStoryTraits").and_then(|v| v.as_bool()).unwrap_or(false),
                fixed_coordinate: y.get("fixedCoordinate").and_then(|v| v.as_i64()).map_or(-1, |v| v as i32),
                skip: y.get("skip").is_some_and(|v| v.as_str().map_or(v.as_i64().is_some_and(|n| n != 0), |s| s != "Never")),
                poi_placements: y.get("poiPlacements").and_then(|v| v.as_sequence()).map(|s| s.iter().map(poi_placement).collect()),
            },
        );
    }

    /// `LoadWorldMixingSettings` / `LoadSubworldMixingSettings`: every copy in
    /// GetFiles order, unsorted; a later copy of a key replaces the earlier.
    fn load_mixing(&mut self, fs: &View, dlc_id: &str, tree_root: &Path) -> Result<(), String> {
        for dir in ["worldMixing", "subworldMixing"] {
            for (path, src) in fs.sources(&tree_root.join(dir)) {
                let text = fs.read_from(&path, src).map_err(|e| format!("read {}: {e}", path.display()))?;
                let y = crate::klei_yaml::parse(&text).map_err(|e| format!("{}: {e}", path.display()))?;
                let stem = path.file_stem().and_then(|s| s.to_str()).ok_or("bad mixing filename")?;
                self.absorb_mixing(dlc_id, dir, stem, &y)?;
            }
        }
        Ok(())
    }

    /// Keys follow `SettingsCache.FileHandleToScopedPath`; the `world` and
    /// `subworld` references are used verbatim by the game.
    fn absorb_mixing(&mut self, dlc_id: &str, dir: &str, stem: &str, y: &serde_yaml::Value) -> Result<(), String> {
        let key = resolve_qname(dlc_id, &format!("{dir}/{stem}"));
        let forbidden_cluster_tags = str_list(y.get("forbiddenClusterTags"));
        if dir == "worldMixing" {
            let world = y.get("world").and_then(|v| v.as_str()).ok_or_else(|| format!("{key}: missing world"))?.to_string();
            self.world_mixing.insert(key, WorldMixingYaml { forbidden_cluster_tags, world });
        } else {
            let subworld = y.get("subworld").and_then(subworld_file).ok_or_else(|| format!("{key}: missing subworld"))?;
            self.subworld_mixing.insert(key, SubworldMixingYaml { forbidden_cluster_tags, subworld,
                mixing_tags: str_list(y.get("mixingTags")),
                additional_world_template_rules: y.get("additionalWorldTemplateRules").and_then(|v| v.as_sequence()).cloned().unwrap_or_default() });
        }
        Ok(())
    }

    fn load_worlds(&mut self, fs: &View, dlc_id: &str, tree_root: &Path) -> Result<(), String> {
        let dir = tree_root.join("worlds");
        if !fs.exists(&dir) {
            return Ok(());
        }
        for path in
            fs.read_dir(&dir).map_err(|e| format!("read {}: {}", dir.display(), e))?
        {
            if path.extension().and_then(|s| s.to_str()) != Some("yaml") {
                continue;
            }
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .ok_or_else(|| format!("bad filename {}", path.display()))?;
            let text = fs.read(&path)
                .map_err(|e| format!("read {}: {}", path.display(), e))?;
            let Ok(y) = crate::klei_yaml::parse(&text) else { continue };
            self.absorb_world(dlc_id, stem, &y);
        }
        Ok(())
    }

    fn absorb_world(&mut self, dlc_id: &str, stem: &str, y: &serde_yaml::Value) {
        let qname = resolve_qname(dlc_id, &format!("worlds/{}", stem));
        let size_x = y
                .get("worldsize")
                .and_then(|v| v.get("X"))
                .and_then(|v| v.as_i64())
                .unwrap_or(0) as i32;
            let size_y = y
                .get("worldsize")
                .and_then(|v| v.get("Y"))
                .and_then(|v| v.as_i64())
                .unwrap_or(0) as i32;
            let layout = y
                .get("layoutMethod")
                .and_then(|v| v.as_str())
                .unwrap_or("PowerTree")
                .to_string();
            let start_tmpl = y
                .get("startingBaseTemplate")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());

            let subs = subworld_files(y.get("subworldFiles"));
            let world_template_rules = y
                .get("worldTemplateRules")
                .and_then(|v| v.as_sequence())
                .cloned()
                .unwrap_or_default();
            let unknown_cells_allowed_subworlds = y
                .get("unknownCellsAllowedSubworlds")
                .and_then(|v| v.as_sequence())
                .cloned()
                .unwrap_or_default();
            let data_overrides = y
                .get("defaultsOverrides")
                .and_then(|v| v.get("data"));
            let get_override_f32 = |key: &str, default: f32| -> f32 {
                data_overrides
                    .and_then(|d| d.get(key))
                    .and_then(|v| v.as_f64())
                    .map(|v| v as f32)
                    .unwrap_or(default)
            };
            let get_override_str = |key: &str, default: &str| -> String {
                data_overrides
                    .and_then(|d| d.get(key))
                    .and_then(|v| v.as_str())
                    .unwrap_or(default)
                    .to_string()
            };
            let overworld_density_min = get_override_f32("OverworldDensityMin", 600.0);
            let overworld_density_max = get_override_f32("OverworldDensityMax", 700.0);
            let overworld_avoid_radius = get_override_f32("OverworldAvoidRadius", 72.0);
            let overworld_sample_behaviour =
                get_override_str("OverworldSampleBehaviour", "UniformHex");

            let disable_world_traits = y
                .get("disableWorldTraits")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let world_trait_scale = y
                .get("worldTraitScale")
                .and_then(|v| v.as_f64())
                .unwrap_or(1.0) as f32;
            let mm_pair = |k: &str, default: f32| -> (f32, f32) {
                y.get(k)
                    .map(|v| {
                        let lo = v.get("min").and_then(|x| x.as_f64()).unwrap_or(default as f64) as f32;
                        let hi = v.get("max").and_then(|x| x.as_f64()).unwrap_or(default as f64) as f32;
                        (lo, hi)
                    })
                    .unwrap_or((default, default))
            };
            let starting_base_position_horizontal = mm_pair("startingBasePositionHorizontal", 0.5);
            let starting_base_position_vertical = mm_pair("startingBasePositionVertical", 0.5);
            let start_subworld_name = y
                .get("startSubworldName")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let mut subworld_mixing_rules: Vec<SubworldMixingRule> = Vec::new();
            if let Some(list) = y.get("subworldMixingRules").and_then(|v| v.as_sequence()) {
                for r in list {
                    let mut rule = SubworldMixingRule::default();
                    rule.name = r
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    rule.min_count = r.get("minCount").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
                    rule.max_count = r.get("maxCount").and_then(|v| v.as_i64()).unwrap_or(i32::MAX as i64) as i32;
                    let str_seq = |k: &str| -> Vec<String> {
                        r.get(k)
                            .and_then(|v| v.as_sequence())
                            .map(|s| s.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect())
                            .unwrap_or_default()
                    };
                    rule.required_tags = str_seq("requiredTags");
                    rule.forbidden_tags = str_seq("forbiddenTags");
                    subworld_mixing_rules.push(rule);
                }
            }
            let mut world_trait_rules = Vec::new();
            if let Some(list) = y.get("worldTraitRules").and_then(|v| v.as_sequence()) {
                for r in list {
                    let mut rule = WorldTraitRule::default();
                    rule.min = r.get("min").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
                    rule.max = r.get("max").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
                    let str_seq = |k: &str| -> Vec<String> {
                        r.get(k)
                            .and_then(|v| v.as_sequence())
                            .map(|s| s.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect())
                            .unwrap_or_default()
                    };
                    rule.required_tags = str_seq("requiredTags");
                    rule.forbidden_tags = str_seq("forbiddenTags");
                    rule.forbidden_traits = str_seq("forbiddenTraits");
                    rule.specific_traits = str_seq("specificTraits");
                    world_trait_rules.push(rule);
                }
            }

        self.worlds.insert(
            qname.clone(),
            WorldYaml {
                starting_world_elements: y.get("defaultsOverrides").and_then(|v|v.get("startingWorldElements")).filter(|v|!v.is_null()).cloned(),
                modify_layout_tags:y.get("modifyLayoutTags").and_then(|v|v.as_sequence()).cloned().unwrap_or_default(),
                data_overrides: data_overrides.cloned().unwrap_or(serde_yaml::Value::Null),
                qname,
                size: (size_x, size_y),
                layout_method: layout,
                starting_base_template: start_tmpl,
                subworld_files: subs,
                world_template_rules,
                unknown_cells_allowed_subworlds,
                overworld_density_min,
                overworld_density_max,
                overworld_avoid_radius,
                overworld_sample_behaviour,
                disable_world_traits,
                world_trait_rules,
                world_trait_scale,
                subworld_mixing_rules,
                world_tags: str_list(y.get("worldTags")),
                seasons: str_list(y.get("seasons")),
                starting_base_position_horizontal,
                starting_base_position_vertical,
                start_subworld_name,
                default_move_tags: y.get("defaultsOverrides").and_then(|v|v.get("defaultMoveTags"))
                    .filter(|v|!v.is_null()).or_else(||self.defaults.get("defaultMoveTags"))
                    .and_then(|v|v.as_sequence()).map(|v|v.iter().filter_map(|v|v.as_str().map(str::to_string)).collect()).unwrap_or_default(),
                overworld_add_tags: y.get("defaultsOverrides").and_then(|v|v.get("overworldAddTags"))
                    .filter(|v|!v.is_null()).or_else(||self.defaults.get("overworldAddTags"))
                    .and_then(|v|v.as_sequence()).map(|v|v.iter().filter_map(|v|v.as_str().map(str::to_string)).collect()).unwrap_or_default(),
                global_features: y.get("globalFeatures").and_then(|v| v.as_mapping())
                    .map(|m| m.iter().filter_map(|(k,v)| Some((k.as_str()?.to_string(), v.as_i64()? as i32))).collect())
                    .unwrap_or_default(),
            },
        );
    }

    /// Find the cluster whose `coordinatePrefix` is the longest
    /// match for `coord` (V-SNDST-C must beat SNDST-C).
    pub fn cluster_for_coord(&self, coord: &str) -> Option<&ClusterYaml> {
        let mut best: Option<&ClusterYaml> = None;
        for (prefix, cluster) in &self.clusters {
            if coord.starts_with(&format!("{}-", prefix))
                && best.map_or(true, |b| prefix.len() > b.coordinate_prefix.len())
            {
                best = Some(cluster);
            }
        }
        best
    }

    pub fn world(&self, qname: &str) -> Option<&WorldYaml> {
        self.worlds.get(qname)
    }

    /// Whether the worldgen tree of a post-Spaced Out content DLC was loaded:
    /// every one of them ships a `mixing.yaml` (`LoadDlcMixingSettings`).
    pub fn dlc_installed(&self, dlc_id: &str) -> bool {
        self.dlc_mixing_space_pois.contains_key(dlc_id)
    }

    /// Pick traits for a single world given a seed. Mirrors the
    /// game's `SettingsCache.GetRandomTraits`: walks the world's
    /// `worldTraitRules`, builds a pool filtered by the rule's
    /// required/forbidden tags + forbidden traits, picks one trait
    /// per slot using a `KRandom(seed)` and skipping anything
    /// `exclusiveWith` an already-picked trait.
    ///
    /// `dlc_scope` is `(include_expansion1, include_dlc2)` — the
    /// game restricts the pool to traits available in the active DLC
    /// scope (`SettingsCache.LoadWorldTraits` / `LoadTrait`). Base game
    /// uses `(false, false)`; SpacedOut classic uses `(true, false)`;
    /// SpacedOut Frosty uses `(true, true)`. Only the first flag changes
    /// the pool for shipped data: DLC2 has no traits and none forbid it.
    pub fn select_traits(
        &self,
        seed: i32,
        world: &WorldYaml,
        dlc_scope: (bool, bool),
    ) -> Vec<&TraitYaml> {
        self.select_traits_logged(seed, world, dlc_scope, &mut Vec::new())
    }

    /// [`select_traits`](Self::select_traits), adding the warnings
    /// `GetRandomTraits` logs to `warnings`.
    pub fn select_traits_logged(
        &self,
        seed: i32,
        world: &WorldYaml,
        dlc_scope: (bool, bool),
        warnings: &mut Vec<String>,
    ) -> Vec<&TraitYaml> {
        use std::collections::BTreeSet;
        if world.disable_world_traits || world.world_trait_rules.is_empty() {
            return Vec::new();
        }
        let (include_e1, include_d2) = dlc_scope;
        let accept_dlc = |id: &str| -> bool {
            if id.starts_with("expansion1::") {
                include_e1
            } else if id.starts_with("dlc2::") {
                include_d2
            } else {
                true
            }
        };
        let forbidden_active = |t: &TraitYaml| -> bool {
            t.forbidden_dlc_ids.iter().any(|id| match id.as_str() {
                "EXPANSION1_ID" => include_e1,
                "DLC2_ID" => include_d2,
                _ => false,
            })
        };
        // The master pool — same shape as `traitList` in the JS port.
        // Filtered up-front by DLC scope; not sorted, preserves
        // insertion order (case-insensitive alphabetical from
        // `load_traits`).
        let mut pool: Vec<usize> = (0..self.traits.len())
            .filter(|&i| accept_dlc(&self.traits[i].0) && !forbidden_active(&self.traits[i].1))
            .collect();
        let mut selected: Vec<String> = Vec::new();
        let mut tagset: BTreeSet<String> = BTreeSet::new();
        let mut r = crate::rng::KRandom::new(seed);

        for rule in &world.world_trait_rules {
            // Specific traits are added unconditionally.
            // `worldTraits.TryGetValue`: only traits loaded for the active
            // DLC set (not forbidden, from a subscribed tree) can be forced.
            for sp in &rule.specific_traits {
                if !self.traits.iter().any(|(k, t)| k == sp && accept_dlc(k) && !forbidden_active(t)) {
                    warnings.push(format!("World traits {sp} doesn't exist, skipping."));
                } else {
                    selected.push(sp.clone());
                    if let Some(t) = self.trait_by_id(sp) {
                        for x in &t.exclusive_with_tags {
                            tagset.insert(x.clone());
                        }
                    }
                }
            }
            // `availableTraits = [...traitList]` filtered by tag /
            // forbiddenTrait predicate plus the trait's own
            // `isValid(world)` check.
            let req: BTreeSet<&str> = rule.required_tags.iter().map(|s| s.as_str()).collect();
            let forb: BTreeSet<&str> = rule.forbidden_tags.iter().map(|s| s.as_str()).collect();
            let forbt: BTreeSet<&str> = rule.forbidden_traits.iter().map(|s| s.as_str()).collect();
            let mut available: Vec<usize> = pool
                .iter()
                .copied()
                .filter(|&i| {
                    let (id, t) = &self.traits[i];
                    if forbt.contains(id.as_str()) {
                        return false;
                    }
                    let tags: BTreeSet<&str> = t.trait_tags.iter().map(|s| s.as_str()).collect();
                    if !req.is_empty() && !req.is_subset(&tags) {
                        return false;
                    }
                    if !forb.is_empty() && tags.intersection(&forb).next().is_some() {
                        return false;
                    }
                    if !is_trait_valid_for_world(t, world) {
                        return false;
                    }
                    true
                })
                .collect();
            let to_gen = r.next_range(rule.min, rule.min.max(rule.max + 1));
            let initial = selected.len();
            while (selected.len() as i32) < (initial as i32 + to_gen) && !available.is_empty() {
                let idx = r.next_max(available.len() as i32) as usize;
                let trait_idx = available[idx];
                let (id, t) = &self.traits[trait_idx];
                let mut is_exclusive = false;
                for ex in &t.exclusive_with {
                    if selected.iter().any(|s| s == ex) {
                        is_exclusive = true;
                        break;
                    }
                }
                if !is_exclusive {
                    for ex in &t.exclusive_with_tags {
                        if tagset.contains(ex.as_str()) {
                            is_exclusive = true;
                            break;
                        }
                    }
                }
                if !is_exclusive {
                    selected.push(id.clone());
                    pool.retain(|&i| i != trait_idx);
                    for x in &t.exclusive_with_tags {
                        tagset.insert(x.clone());
                    }
                }
                // GetRandomTraits calls `list3.RemoveAt(index)` whether or
                // not the candidate was exclusive, so a rejected trait is
                // never drawn again and later draws see the shorter list.
                // (The older JS port kept rejected traits available.)
                available.remove(idx);
            }
            // The game names the world by its `name` string key; this uses its path.
            let generated = selected.len() - initial;
            if generated as i32 != to_gen {
                warnings.push(format!("TraitRule on {} tried to generate {to_gen} but only generated {generated}", world.qname));
            }
        }
        selected
            .iter()
            .filter_map(|id| self.trait_by_id(id))
            .collect()
    }

    pub fn trait_by_id(&self, id: &str) -> Option<&TraitYaml> {
        self.traits
            .iter()
            .find_map(|(k, v)| if k == id { Some(v) } else { None })
    }

    /// Game-style qname resolution: references are tried in the
    /// referring world's DLC first, then fall back to base. Works
    /// for subworlds, templates and any other cross-DLC lookup.
    pub fn resolve_subworld<'a>(&'a self, name: &str) -> Option<&'a SubWorldYaml> {
        if let Some(sw) = self.subworlds.get(name) {
            return Some(sw);
        }
        // Strip a leading DLC prefix and retry against base.
        if let Some((_, rest)) = name.split_once("::") {
            return self.subworlds.get(rest);
        }
        None
    }

    /// Same for templates.
    pub fn resolve_template<'a>(&'a self, name: &str) -> Option<&'a TemplateInfo> {
        if let Some(t) = self.templates.get(name) {
            return Some(t);
        }
        if let Some((_, rest)) = name.split_once("::") {
            return self.templates.get(rest);
        }
        None
    }
}

/// Port of `WorldTrait.isValid(world)` from the reference JS impl.
/// A trait is invalid if its `globalFeatureMods` exists but the
/// scaled total (`floor(scale * each_mod)` summed) is zero — i.e.
/// the world is too small for the trait to add any features.
fn is_trait_valid_for_world(t: &TraitYaml, world: &WorldYaml) -> bool {
    if t.global_feature_mods.is_empty() {
        return true;
    }
    let mut scaled = 0i32;
    for &(_, v) in &t.global_feature_mods {
        scaled += (world.world_trait_scale * v).floor() as i32;
    }
    scaled != 0
}

/// Where [`DataSource::load`] reads game files: the StreamingAssets
/// directory, a bundle of the same files keyed by relative path (builds
/// without a filesystem), or the directory while keeping every file read so
/// a bundle holds exactly what the loader uses. Listings are sorted so every
/// source visits files in the same order.
enum Files<'a> {
    Disk,
    Bundle(&'a BTreeMap<String, String>),
    Record(PathBuf, std::cell::RefCell<BTreeMap<String, String>>),
}

impl Files<'_> {
    fn key(p: &Path) -> String { p.to_string_lossy().replace('\\', "/") }

    fn read(&self, p: &Path) -> std::io::Result<String> {
        match self {
            Files::Disk => std::fs::read_to_string(p),
            Files::Bundle(files) => files.get(&Self::key(p)).cloned().ok_or_else(|| std::io::ErrorKind::NotFound.into()),
            Files::Record(root, read) => {
                let text = std::fs::read_to_string(p)?;
                let rel = p.strip_prefix(root).map_err(|_| std::io::Error::other("outside the asset directory"))?;
                read.borrow_mut().insert(Self::key(rel), text.clone());
                Ok(text)
            }
        }
    }

    /// Bundle keys under `dir/`, which are contiguous in sorted order.
    fn under<'b>(files: &'b BTreeMap<String, String>, dir: &Path) -> impl Iterator<Item = &'b str> {
        let prefix = format!("{}/", Self::key(dir));
        let n = prefix.len();
        files.range(prefix.clone()..).map(|(k, _)| k.as_str()).take_while(move |k| k.starts_with(&prefix)).map(move |k| &k[n..])
    }

    fn is_dir(&self, p: &Path) -> bool {
        match self { Files::Bundle(files) => Self::under(files, p).next().is_some(), _ => p.is_dir() }
    }

    fn exists(&self, p: &Path) -> bool {
        match self { Files::Bundle(files) => files.contains_key(&Self::key(p)) || self.is_dir(p), _ => p.exists() }
    }

    fn read_dir(&self, p: &Path) -> std::io::Result<Vec<PathBuf>> {
        let mut entries: Vec<PathBuf> = match self {
            Files::Bundle(files) => {
                let mut names: Vec<&str> = Self::under(files, p).map(|rest| rest.split('/').next().unwrap_or(rest)).collect();
                if names.is_empty() { return Err(std::io::ErrorKind::NotFound.into()); }
                names.dedup();
                names.into_iter().map(|name| p.join(name)).collect()
            }
            _ => std::fs::read_dir(p)?.map(|e| e.map(|e| e.path())).collect::<Result<_, _>>()?,
        };
        entries.sort();
        Ok(entries)
    }
}

/// Worldgen files over the game's, as editors and mods supply them. Keys are
/// StreamingAssets-relative paths with `/` separators, e.g.
/// `dlc/expansion1/worldgen/traits/Volcanoes.yaml` ([`normalize_path`] also
/// takes the game's scoped form, `expansion1::worldgen/traits/Volcanoes.yaml`).
///
/// `edits` change the game's own files in place, as editing StreamingAssets
/// would: an edited file replaces the game's copy everywhere, a new one is
/// read as if shipped, and `removed` hides files.
///
/// `mods` are Workshop-style mods in load order, layered the way the game
/// layers mod content (`KMod.Mod.Load` inserts each mod's directory at the
/// front of `FileSystem.file_sources`, so a later mod wins):
/// - files loaded by path (worlds, subworlds, features, noise, templates,
///   `defaults.yaml`, a DLC `mixing.yaml` that the game has) come from the
///   last mod that has them;
/// - `FileSystem.GetFiles` listings (clusters, traits, story traits, world and
///   subworld mixing, elements) hold every source's copy, mods first. Traits
///   are then sorted by path with Mono's unstable sort and the copy loaded
///   last wins, so a mod's copy of a shipped trait may or may not replace it;
///   for mixing settings the game's own copy is loaded last and wins; for
///   elements the first definition of an id wins; clusters are read back by
///   path, so the mod's copy wins;
/// - `MergeLoad` tables (`rooms`, `mobs`, `borders`, `temperatures` and biome
///   files) merge every copy in load order, game first: each copy removes its
///   `remove` keys, then adds its `add` entries, and adding a key that is
///   already present fails in the game (`Dictionary.Add`).
///
/// A tree for a DLC that the game data lacks is not loaded, as the game only
/// loads subscribed content.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Overlay {
    pub edits: BTreeMap<String, String>,
    pub removed: std::collections::BTreeSet<String>,
    pub mods: Vec<Layer>,
}

impl Overlay {
    pub fn is_empty(&self) -> bool { self.edits.is_empty() && self.removed.is_empty() && self.mods.iter().all(|m| m.files.is_empty()) }
}

/// One mod: path -> file text.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Layer {
    pub name: String,
    pub files: BTreeMap<String, String>,
}

/// Normalize an overlay path to the StreamingAssets-relative form. Accepts
/// `\` separators and the game's scoped form (`dlc2::templates/x.yaml` is
/// `dlc/dlc2/templates/x.yaml`); refuses absolute paths and `..`.
pub fn normalize_path(path: &str) -> Result<String, String> {
    let path = path.trim().replace('\\', "/");
    let path = match path.split_once("::") {
        Some(("", rest)) => rest.to_string(),
        Some((dlc, rest)) => format!("dlc/{dlc}/{rest}"),
        None => path,
    };
    let path = path.trim_start_matches("./").to_string();
    if path.is_empty() || path.starts_with('/') || path.contains(':') || path.split('/').any(|c| c == ".." || c.is_empty()) {
        return Err(format!("invalid overlay path {path:?}: use a StreamingAssets-relative path such as worldgen/traits/Foo.yaml"));
    }
    Ok(path)
}

impl Layer {
    /// The worldgen files of a directory laid out like StreamingAssets: the
    /// YAML under `worldgen`, `templates`, `elements` and their `dlc/*`
    /// counterparts. A mod (a directory with `mod_info.yaml`) is read from
    /// the content root the game would pick ([`mod_content_root`]).
    pub fn from_dir(dir: &Path, installed_dlc: &[&str], game_build: u32) -> Result<Layer, String> {
        let root = mod_content_root(dir, installed_dlc, game_build)?;
        let mut files = BTreeMap::new();
        fn walk(dir: &Path, rel: &str, files: &mut BTreeMap<String, String>) -> Result<(), String> {
            let Ok(rd) = std::fs::read_dir(dir) else { return Ok(()) };
            for e in rd {
                let e = e.map_err(|e| e.to_string())?;
                let name = e.file_name().to_string_lossy().into_owned();
                let rel = format!("{rel}/{name}");
                if e.path().is_dir() { walk(&e.path(), &rel, files)?; }
                else if name.ends_with(".yaml") {
                    files.insert(rel, std::fs::read_to_string(e.path()).map_err(|err| format!("{}: {err}", e.path().display()))?);
                }
            }
            Ok(())
        }
        for content in ["worldgen", "templates", "elements"] {
            walk(&root.join(content), content, &mut files)?;
            for dlc in std::iter::once("expansion1").chain(crate::dlc::CONTENT_DLCS.iter().map(|(_, dir)| *dir)) {
                walk(&root.join("dlc").join(dlc).join(content), &format!("dlc/{dlc}/{content}"), &mut files)?;
            }
        }
        Ok(Layer { name: dir.display().to_string(), files })
    }
}

/// `KMod.Mod.GetMostSuitableArchive` for a directory with `mod_info.yaml`:
/// the root or the `archived_versions` entry with the highest
/// `minimumSupportedBuild` not above `game_build`, among those whose
/// `APIVersion` is 0 or 2 and whose `requiredDlcIds` / `forbiddenDlcIds`
/// `installed` satisfies. Any other directory is its own content root.
fn mod_content_root(dir: &Path, installed: &[&str], game_build: u32) -> Result<PathBuf, String> {
    #[derive(serde::Deserialize, Default)]
    #[serde(default)]
    struct ModInfo {
        #[serde(rename = "requiredDlcIds")] required: Option<Vec<String>>,
        #[serde(rename = "forbiddenDlcIds")] forbidden: Option<Vec<String>>,
        #[serde(rename = "minimumSupportedBuild")] minimum: i64,
        #[serde(rename = "APIVersion")] api: i64,
    }
    let info = |d: &Path| -> Result<Option<ModInfo>, String> {
        let Some(e) = std::fs::read_dir(d).ok().and_then(|rd| rd.flatten().find(|e| e.file_name().to_string_lossy().to_lowercase() == "mod_info.yaml")) else { return Ok(None) };
        let text = std::fs::read_to_string(e.path()).map_err(|err| format!("{}: {err}", e.path().display()))?;
        crate::klei_yaml::from_str::<ModInfo>(&text).map(Some).map_err(|err| format!("{}: {err}", e.path().display()))
    };
    let Some(root_info) = info(dir)? else { return Ok(dir.to_path_buf()) };
    let dlc_ok = |i: &ModInfo| i.required.iter().flatten().all(|id| installed.contains(&id.to_uppercase().as_str()))
        && !i.forbidden.iter().flatten().any(|id| installed.contains(&id.to_uppercase().as_str()));
    let mut versions = vec![(dir.to_path_buf(), root_info)];
    if let Ok(rd) = std::fs::read_dir(dir.join("archived_versions")) {
        let mut subdirs: Vec<_> = rd.flatten().filter(|e| e.path().is_dir()).map(|e| e.path()).collect();
        subdirs.sort();
        for d in subdirs { if let Some(i) = info(&d)? { versions.push((d, i)); } }
    }
    if versions.len() == 1 {
        return if dlc_ok(&versions[0].1) { Ok(dir.to_path_buf()) } else { Err(format!("{}: the mod's DLC requirements are not met", dir.display())) };
    }
    // `OrderByDescending` is stable: the first of equal builds wins.
    let mut best: Option<&(PathBuf, ModInfo)> = None;
    for v in versions.iter().filter(|(_, i)| dlc_ok(i) && (i.api == 2 || i.api == 0) && i.minimum <= game_build as i64) {
        if best.is_none_or(|b| v.1.minimum > b.1.minimum) { best = Some(v); }
    }
    best.map(|(d, _)| d.clone()).ok_or_else(|| format!("{}: no archived version supports this game build and DLC set", dir.display()))
}

/// `string.Compare(a, b, StringComparison.OrdinalIgnoreCase)`.
fn cmp_ignore_case(a: &str, b: &str) -> std::cmp::Ordering {
    a.chars().map(|c| c.to_ascii_uppercase()).cmp(b.chars().map(|c| c.to_ascii_uppercase()))
}

/// Which copy of a listed file: the game's (edits applied), or a mod's.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Src { Base, Mod(usize) }

/// The game files seen through an [`Overlay`]. Paths are `root`-joined, as the
/// loader builds them.
struct View<'a> {
    base: &'a Files<'a>,
    root: &'a Path,
    overlay: &'a Overlay,
    /// (path, SHA-256 of the text) of every file read, when checksumming.
    reads: Option<std::cell::RefCell<Vec<(String, [u8; 32])>>>,
}

impl View<'_> {
    fn rel(&self, p: &Path) -> String {
        Files::key(p.strip_prefix(self.root).unwrap_or(p)).trim_start_matches('/').to_string()
    }

    fn record(&self, p: &Path, text: &str) {
        if let Some(reads) = &self.reads { reads.borrow_mut().push((self.rel(p), crate::sha256::digest(text.as_bytes()))); }
    }

    /// Keys of `map` under the directory `rel` (no trailing slash), relative to it.
    fn under<'m>(map: &'m BTreeMap<String, String>, rel: &str) -> Vec<&'m str> {
        let prefix = format!("{rel}/");
        map.range(prefix.clone()..).map(|(k, _)| k.as_str()).take_while(|k| k.starts_with(&prefix)).map(|k| &k[prefix.len()..]).collect()
    }

    /// The game's copy of `p`, edits applied.
    fn base_read(&self, p: &Path, rel: &str) -> std::io::Result<String> {
        if self.overlay.removed.contains(rel) { return Err(std::io::ErrorKind::NotFound.into()); }
        match self.overlay.edits.get(rel) { Some(t) => Ok(t.clone()), None => self.base.read(p) }
    }

    fn base_file(&self, p: &Path, rel: &str) -> bool {
        !self.overlay.removed.contains(rel) && (self.overlay.edits.contains_key(rel) || (self.base.exists(p) && !self.base.is_dir(p)))
    }

    /// Mods holding `rel`, latest first.
    fn mod_text(&self, rel: &str) -> Option<&str> {
        self.overlay.mods.iter().rev().find_map(|l| l.files.get(rel)).map(String::as_str)
    }

    fn read(&self, p: &Path) -> std::io::Result<String> {
        let rel = self.rel(p);
        let text = match self.mod_text(&rel) { Some(t) => t.to_string(), None => self.base_read(p, &rel)? };
        self.record(p, &text);
        Ok(text)
    }

    fn read_from(&self, p: &Path, src: Src) -> std::io::Result<String> {
        let rel = self.rel(p);
        let text = match src {
            Src::Base => self.base_read(p, &rel)?,
            Src::Mod(i) => self.overlay.mods[i].files.get(&rel).cloned().ok_or(std::io::ErrorKind::NotFound)?,
        };
        self.record(p, &text);
        Ok(text)
    }

    /// Whether the game data itself (not edits or mods) has `p`, for what the
    /// game decides from its installation: subscribed DLC trees.
    fn installed(&self, p: &Path) -> bool { self.base.exists(p) }

    /// Whether the game's files (edits applied) have the file `p`.
    fn base_exists(&self, p: &Path) -> bool { self.base_file(p, &self.rel(p)) }

    fn is_dir(&self, p: &Path) -> bool {
        let rel = self.rel(p);
        self.base.is_dir(p) || !Self::under(&self.overlay.edits, &rel).is_empty() || self.overlay.mods.iter().any(|m| !Self::under(&m.files, &rel).is_empty())
    }

    fn exists(&self, p: &Path) -> bool {
        let rel = self.rel(p);
        self.base_file(p, &rel) || self.mod_text(&rel).is_some() || self.is_dir(p)
    }

    /// Directory entries from every source, deduplicated and sorted.
    fn read_dir(&self, p: &Path) -> std::io::Result<Vec<PathBuf>> {
        if self.overlay.is_empty() { return self.base.read_dir(p); }
        let rel = self.rel(p);
        let base = self.base.read_dir(p);
        let added: Vec<&str> = Self::under(&self.overlay.edits, &rel).into_iter()
            .chain(self.overlay.mods.iter().flat_map(|m| Self::under(&m.files, &rel))).collect();
        if base.is_err() && added.is_empty() { return base; }
        let mut entries: Vec<PathBuf> = base.unwrap_or_default().into_iter().filter(|e| !self.overlay.removed.contains(&self.rel(e))).collect();
        for rest in added {
            let entry = p.join(rest.split('/').next().unwrap_or(rest));
            if !entries.contains(&entry) { entries.push(entry); }
        }
        entries.sort();
        Ok(entries)
    }

    /// `FileSystem.GetFiles(dir, "*.yaml")`: the YAML files directly in `p`
    /// from each source, mods first (latest first), then the game's. Within a
    /// source, names are in case-insensitive order, as NTFS lists them.
    fn sources(&self, p: &Path) -> Vec<(PathBuf, Src)> {
        let rel = self.rel(p);
        let listed = |name: &str| name.ends_with(".yaml") && !name.starts_with("._") && !name.contains('/');
        let mut out = Vec::new();
        for i in (0..self.overlay.mods.len()).rev() {
            let mut names: Vec<&str> = Self::under(&self.overlay.mods[i].files, &rel).into_iter().filter(|n| listed(n)).collect();
            names.sort_by(|a, b| cmp_ignore_case(a, b));
            out.extend(names.into_iter().map(|n| (p.join(n), Src::Mod(i))));
        }
        let mut base: Vec<PathBuf> = self.base.read_dir(p).unwrap_or_default().into_iter()
            .filter(|e| e.file_name().and_then(|n| n.to_str()).is_some_and(listed) && !self.base.is_dir(e)).collect();
        for n in Self::under(&self.overlay.edits, &rel).into_iter().filter(|n| listed(n)) {
            if !base.contains(&p.join(n)) { base.push(p.join(n)); }
        }
        base.retain(|e| !self.overlay.removed.contains(&self.rel(e)));
        base.sort_by(|a, b| cmp_ignore_case(&a.to_string_lossy(), &b.to_string_lossy()));
        out.extend(base.into_iter().map(|e| (e, Src::Base)));
        out
    }

    /// Every copy of `p` in `MergeLoad` order: the game's, then each mod's.
    fn versions(&self, p: &Path) -> Result<Vec<String>, String> {
        let rel = self.rel(p);
        let mut out = Vec::new();
        if self.base_file(p, &rel) { out.push(self.read_from(p, Src::Base).map_err(|e| format!("{}: {e}", p.display()))?); }
        for (i, l) in self.overlay.mods.iter().enumerate() {
            if l.files.contains_key(&rel) { out.push(self.read_from(p, Src::Mod(i)).map_err(|e| e.to_string())?); }
        }
        Ok(out)
    }

    /// `MergeLoad` of a `ComposableDictionary` file: the merged `add` entries,
    /// in insertion order, of every copy of `p`. `table` is the key holding the
    /// dictionary (`MobLookupTable`) when it is not the document itself.
    /// `None` when no copy exists.
    fn merge_load(&self, p: &Path, table: Option<&str>) -> Result<Option<Vec<(String, serde_yaml::Value)>>, String> {
        let versions = self.versions(p)?;
        if versions.is_empty() { return Ok(None); }
        let mut merged: Vec<(String, serde_yaml::Value)> = Vec::new();
        for text in versions {
            let doc = crate::klei_yaml::parse(&text).map_err(|e| format!("{}: {e}", p.display()))?;
            // A copy without the table deserializes to an empty one.
            let doc = match table { Some(t) => doc.get(t).cloned().unwrap_or_default(), None => doc };
            if let Some(remove) = doc.get("remove").and_then(|v| v.as_sequence()) {
                for key in remove {
                    let key = key.as_str().ok_or_else(|| format!("{}: invalid removal name", p.display()))?;
                    merged.retain(|(k, _)| k != key);
                }
            }
            if let Some(add) = doc.get("add").and_then(|v| v.as_mapping()) {
                for (key, value) in add {
                    let key = key.as_str().ok_or_else(|| format!("{}: invalid entry name", p.display()))?;
                    if merged.iter().any(|(k, _)| k == key) { return Err(duplicate_entry(p, key)); }
                    merged.push((key.to_string(), value.clone()));
                }
            }
        }
        Ok(Some(merged))
    }

    /// SHA-256 over the sorted (path, SHA-256 of text) of every file read.
    fn checksum(&self) -> Option<String> {
        let reads = self.reads.as_ref()?.borrow();
        let mut entries: Vec<&(String, [u8; 32])> = reads.iter().collect();
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        entries.dedup();
        let mut text = String::new();
        for (path, digest) in entries {
            text.push_str(path);
            text.push('\0');
            text.push_str(&hex(digest));
            text.push('\n');
        }
        Some(hex(&crate::sha256::digest(text.as_bytes())))
    }
}

fn hex(bytes: &[u8]) -> String { bytes.iter().map(|b| format!("{b:02x}")).collect() }

/// The game throws from `ComposableDictionary.Merge` (`Dictionary.Add`).
fn duplicate_entry(p: &Path, key: &str) -> String {
    format!("{}: entry {key} is already defined; the game fails to load settings that add an existing entry (list it under remove in the same file first)", p.display())
}

/// Add `MergeLoad` results to a table loaded from an earlier tree
/// (`existing.Merge(val)`): an entry already present is an error in the game.
fn merge_into<T>(p: &Path, target: &mut BTreeMap<String, T>, entries: Vec<(String, serde_yaml::Value)>, convert: impl Fn(&str, serde_yaml::Value) -> Result<T, String>) -> Result<(), String> {
    for (key, value) in entries {
        if target.contains_key(&key) { return Err(duplicate_entry(p, &key)); }
        let value = convert(&key, value)?;
        target.insert(key, value);
    }
    Ok(())
}

fn walk_yamls(fs: &View, root: &Path, visit: &mut impl FnMut(&Path)) {
    let Ok(entries) = fs.read_dir(root) else { return };
    for p in entries {
        if fs.is_dir(&p) {
            walk_yamls(fs, &p, visit);
        } else if p.extension().and_then(|s| s.to_str()) == Some("yaml") {
            visit(&p);
        }
    }
}

/// `SpaceMapPOIPlacement` from YAML; omitted fields keep the constructor defaults.
fn poi_placement(p: &serde_yaml::Value) -> PoiPlacementYaml {
    let flag = |k: &str| p.get(k).and_then(|v| v.as_bool()).unwrap_or(false);
    PoiPlacementYaml {
        pois: str_list(p.get("pois")),
        num_to_spawn: p.get("numToSpawn").and_then(|v| v.as_i64()).unwrap_or(0) as i32,
        allowed_rings: p.get("allowedRings").and_then(parse_minmax).unwrap_or((0, 9999)),
        avoid_clumping: flag("avoidClumping"),
        can_spawn_duplicates: flag("canSpawnDuplicates"),
        guarantee: flag("guarantee"),
    }
}

fn str_list(v: Option<&serde_yaml::Value>) -> Vec<String> {
    v.and_then(|v| v.as_sequence()).map(|s| s.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default()
}

/// `WeightedSubworldName` defaults: weight 1, maxCount int.MaxValue.
fn subworld_file(s: &serde_yaml::Value) -> Option<SubworldFileRef> {
    Some(SubworldFileRef {
        name: s.get("name")?.as_str()?.to_string(),
        weight: s.get("weight").and_then(|v| v.as_f64()).unwrap_or(1.0) as f32,
        min_count: s.get("minCount").and_then(|v| v.as_i64()).unwrap_or(0) as i32,
        max_count: s.get("maxCount").and_then(|v| v.as_i64()).unwrap_or(2147483647) as i32,
        priority: s.get("priority").and_then(|v| v.as_i64()).unwrap_or(0) as i32,
        override_power: s.get("overridePower").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32,
    })
}

fn subworld_files(v: Option<&serde_yaml::Value>) -> Vec<SubworldFileRef> {
    v.and_then(|v| v.as_sequence()).map(|s| s.iter().filter_map(subworld_file).collect()).unwrap_or_default()
}

fn resolve_qname(default_dlc: &str, reference: &str) -> QName {
    if let Some((dlc, rest)) = reference.split_once("::") {
        format!("{}::{}", dlc, rest)
    } else if default_dlc.is_empty() {
        reference.to_string()
    } else {
        format!("{}::{}", default_dlc, reference)
    }
}

fn parse_minmax(v: &serde_yaml::Value) -> Option<(i32, i32)> {
    let min = v.get("min").and_then(|m| m.as_i64())?;
    let max = v.get("max").and_then(|m| m.as_i64())?;
    Some((min as i32, max as i32))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// GetRandomTraits removes every drawn candidate from the rule's list,
    /// including one rejected by `exclusiveWith`, so later draws use the
    /// shorter list. Compare against a direct transcription of that loop.
    #[test]
    fn rejected_exclusive_traits_leave_the_candidate_list() {
        let mut ds = DataSource::default();
        let traits = [
            ("GeoActive", "exclusiveWith: [traits/GeoDormant]"),
            ("GeoDormant", "exclusiveWith: [traits/GeoActive]"),
            ("MetalPoor", "exclusiveWith: [traits/MetalRich]"),
            ("MetalRich", "exclusiveWith: [traits/MetalPoor]"),
            ("SlimeSplats", "{}"),
        ];
        for (name, yaml) in traits {
            ds.absorb_trait("", &format!("traits/{name}"), &serde_yaml::from_str(yaml).unwrap());
        }
        ds.absorb_world("", "Test", &serde_yaml::from_str("worldTraitRules: [{min: 2, max: 3}]").unwrap());
        let world = ds.world("worlds/Test").unwrap().clone();
        let exclusive: Vec<(&str, &str)> = vec![
            ("traits/GeoActive", "traits/GeoDormant"), ("traits/GeoDormant", "traits/GeoActive"),
            ("traits/MetalPoor", "traits/MetalRich"), ("traits/MetalRich", "traits/MetalPoor"),
        ];
        let mut rejections = 0;
        for seed in 1..200 {
            let mut r = crate::rng::KRandom::new(seed);
            let mut candidates: Vec<String> = ds.traits.iter().map(|(k, _)| k.clone()).collect();
            let mut expected: Vec<String> = Vec::new();
            let count = r.next_range(2, 4) as usize;
            while expected.len() < count && !candidates.is_empty() {
                let index = r.next_max(candidates.len() as i32) as usize;
                let id = candidates[index].clone();
                if exclusive.iter().any(|(a, b)| *a == id && expected.iter().any(|s| s == b)) {
                    rejections += 1;
                } else {
                    expected.push(id);
                }
                candidates.remove(index);
            }
            let actual: Vec<String> = ds.select_traits(seed, &world, (false, false)).iter().map(|t| t.qname.to_string()).collect();
            assert_eq!(actual, expected, "seed {seed}");
        }
        assert!(rejections > 0);
    }
}

#[cfg(test)]
mod overlay_tests {
    use super::*;

    fn files(entries: &[(&str, &str)]) -> BTreeMap<String, String> {
        entries.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }
    fn base() -> BTreeMap<String, String> {
        files(&[
            ("worldgen/traits/Alpha.yaml", "traitTags: [Base]\n"),
            ("worldgen/traits/Beta.yaml", "traitTags: [Base]\n"),
            ("worldgen/rooms.yaml", "add:\n  r1: {}\n"),
        ])
    }
    fn tags(ds: &DataSource) -> Vec<(String, Vec<String>)> {
        ds.traits.iter().map(|(k, t)| (k.clone(), t.trait_tags.clone())).collect()
    }

    #[test]
    fn paths_normalize_to_streaming_assets_form() {
        assert_eq!(normalize_path("expansion1::worldgen/traits/X.yaml").unwrap(), "dlc/expansion1/worldgen/traits/X.yaml");
        assert_eq!(normalize_path(r".\worldgen\traits\X.yaml").unwrap(), "worldgen/traits/X.yaml");
        assert!(normalize_path("../worldgen/x.yaml").is_err() && normalize_path("C:/x.yaml").is_err());
    }

    #[test]
    fn edits_replace_add_and_remove_game_files() {
        let mut overlay = Overlay::default();
        overlay.edits.insert("worldgen/traits/Alpha.yaml".into(), "traitTags: [Edited]\n".into());
        overlay.edits.insert("worldgen/traits/Gamma.yaml".into(), "traitTags: [New]\n".into());
        overlay.removed.insert("worldgen/traits/Beta.yaml".into());
        let ds = DataSource::from_files_with(&base(), &overlay).unwrap();
        assert_eq!(tags(&ds), [("traits/Alpha".into(), vec!["Edited".into()]), ("traits/Gamma".into(), vec!["New".into()])]);
    }

    #[test]
    fn a_mod_copy_of_a_shipped_trait_loses_to_the_game_copy_in_small_directories() {
        // GetFiles lists the mod's copy first; List.Sort keeps equal paths in
        // order for up to 16 entries, so the game's copy is loaded last.
        let overlay = Overlay { mods: vec![Layer { name: "m".into(), files: files(&[
            ("worldgen/traits/Alpha.yaml", "traitTags: [Mod]\n"), ("worldgen/traits/Delta.yaml", "traitTags: [Mod]\n")]) }], ..Overlay::default() };
        let ds = DataSource::from_files_with(&base(), &overlay).unwrap();
        assert_eq!(tags(&ds), [("traits/Alpha".into(), vec!["Base".into()]), ("traits/Beta".into(), vec!["Base".into()]),
            ("traits/Delta".into(), vec!["Mod".into()])]);
    }

    #[test]
    fn merge_load_tables_follow_composable_dictionary() {
        let with_mod = |text: &str| DataSource::from_files_with(&base(), &Overlay { mods: vec![Layer { name: "m".into(),
            files: files(&[("worldgen/rooms.yaml", text)]) }], ..Overlay::default() });
        assert!(with_mod("add:\n  r1: {}\n").unwrap_err().contains("entry r1 is already defined"));
        let ds = with_mod("remove: [r1]\nadd:\n  r1: {}\n  r2: {}\n").unwrap();
        assert_eq!(ds.rooms.keys().collect::<Vec<_>>(), ["r1", "r2"]);
    }

    #[test]
    fn checksum_depends_on_content_not_on_how_the_overlay_was_built() {
        let plain = DataSource::from_files_with(&base(), &Overlay::default()).unwrap().checksum.unwrap();
        let mut same = Overlay::default();
        same.edits.insert("worldgen/traits/Alpha.yaml".into(), base()["worldgen/traits/Alpha.yaml"].clone());
        assert_eq!(DataSource::from_files_with(&base(), &same).unwrap().checksum.unwrap(), plain);
        same.edits.insert("worldgen/traits/Alpha.yaml".into(), "traitTags: [Other]\n".into());
        assert_ne!(DataSource::from_files_with(&base(), &same).unwrap().checksum.unwrap(), plain);
        assert!(DataSource::from_files(&base()).unwrap().checksum.is_none());
    }
}
