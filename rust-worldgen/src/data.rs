//! Serde models for the YAML files under the game's
//! `worldgen/<base | dlc/expansion1 | dlc/dlc2>/` and `templates/`
//! trees.
//!
//! The goal is to deserialize the same data the C# `SettingsCache`
//! builds in-game, then iterate on algorithm ports against those
//! models.

#![allow(non_snake_case, dead_code)]

use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct WorldSize {
    pub x: i32,
    pub y: i32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct World {
    pub name: Option<String>,
    pub description: Option<String>,
    #[serde(default)]
    pub nameTables: Vec<String>,
    #[serde(default)]
    pub forbiddenDLCIds: Vec<String>,
    pub asteroidIcon: Option<String>,
    pub disableWorldTraits: Option<bool>,
    pub worldsize: Option<WorldSize>,
    pub layoutMethod: Option<String>,
    pub seasons: Option<Vec<String>>,
    pub worldTraitScale: Option<f32>,
    #[serde(default)]
    pub worldTraitRules: Vec<TraitRule>,
    #[serde(default)]
    pub defaultsOverrides: serde_yaml::Value,
    #[serde(default)]
    pub globalFeatures: serde_yaml::Value,
    #[serde(default)]
    pub subworldFiles: Vec<SubworldFile>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_yaml::Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SubworldFile {
    pub name: Option<String>,
    pub weight: Option<f32>,
    pub minCount: Option<i32>,
    pub maxCount: Option<i32>,
    pub overrides: Option<serde_yaml::Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TraitRule {
    #[serde(default)]
    pub min: i32,
    #[serde(default)]
    pub max: i32,
    #[serde(default)]
    pub requiredTags: Vec<String>,
    #[serde(default)]
    pub forbiddenTags: Vec<String>,
    #[serde(default)]
    pub forbiddenTraits: Vec<String>,
    #[serde(default)]
    pub specificTraits: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WorldTrait {
    pub name: Option<String>,
    pub description: Option<String>,
    pub colorHex: Option<String>,
    pub icon: Option<String>,
    #[serde(default)]
    pub forbiddenDLCIds: Vec<String>,
    #[serde(default)]
    pub exclusiveWith: Vec<String>,
    #[serde(default)]
    pub exclusiveWithTags: Vec<String>,
    #[serde(default)]
    pub traitTags: Vec<String>,
    #[serde(default)]
    pub globalFeatureMods: BTreeMap<String, f32>,
    #[serde(default)]
    pub removeWorldTemplateRulesById: Vec<String>,
    #[serde(default)]
    pub elementBandModifiers: Vec<serde_yaml::Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Cluster {
    pub name: Option<String>,
    pub description: Option<String>,
    pub menuOrder: Option<i32>,
    pub coordinatePrefix: Option<String>,
    #[serde(default)]
    pub forbiddenDLCIds: Vec<String>,
    pub startWorldIndex: Option<i32>,
    pub numRings: Option<i32>,
    pub difficulty: Option<String>,
    #[serde(default)]
    pub worldPlacements: Vec<WorldPlacement>,
    #[serde(default)]
    pub poiPlacements: Vec<serde_yaml::Value>,
    pub clusterCategory: Option<String>,
    pub clusterTags: Option<Vec<String>>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_yaml::Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WorldPlacement {
    pub world: String,
    pub locationType: Option<String>,
    pub allowedRings: Option<MinMax>,
    pub startWorldIndex: Option<i32>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_yaml::Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MinMax {
    #[serde(default)]
    pub min: i32,
    #[serde(default)]
    pub max: i32,
}

/// Loaded fully-resolved settings cache (worlds, traits, clusters).
/// Trait order is preserved as `(id, trait)` pairs — the game iterates
/// traits in load order in `SettingsCache.GetRandomTraits`, and that
/// order changes which traits RNG picks.
#[derive(Default, Debug)]
pub struct SettingsCache {
    pub worlds: BTreeMap<String, World>,
    pub traits: Vec<(String, WorldTrait)>,
    pub clusters: BTreeMap<String, Cluster>,
}

impl SettingsCache {
    pub fn trait_by_id(&self, id: &str) -> Option<&WorldTrait> {
        self.traits
            .iter()
            .find_map(|(k, v)| if k == id { Some(v) } else { None })
    }
    pub fn has_trait(&self, id: &str) -> bool {
        self.traits.iter().any(|(k, _)| k == id)
    }
}

impl SettingsCache {
    /// Load from a directory containing the game's worldgen YAML
    /// trees. Traits load in the game's order: base-game
    /// `worldgen/traits/*.yaml` (filename alphabetical), then
    /// `dlc/expansion1/worldgen/traits/*.yaml` as
    /// `expansion1::traits/<name>`, then `dlc/dlc2/...` as
    /// `dlc2::traits/<name>`. That insertion order is what drives
    /// `GetRandomTraits` RNG consumption.
    pub fn load_from_dir(root: &std::path::Path) -> Result<Self, String> {
        use std::fs;
        use std::path::PathBuf;

        let mut cache = SettingsCache::default();

        let trees = [
            ("", "worldgen"),
            ("expansion1::", "dlc/expansion1/worldgen"),
            ("dlc2::", "dlc/dlc2/worldgen"),
        ];

        for (id_prefix, rel) in trees.iter() {
            let base: PathBuf = root.join(rel);
            if !base.exists() {
                continue;
            }
            for sub in ["worlds", "clusters", "traits"] {
                let dir = base.join(sub);
                if !dir.exists() {
                    continue;
                }
                // Load in case-insensitive filename order. Matches
                // the game's trait pool ordering: the second RNG pick
                // resolves to Geodes only when Geodes precedes
                // GeoDormant, which is how a case-insensitive sort
                // orders "Geodes" (g-e-o-d-e) before "GeoDormant"
                // (g-e-o-d-o).
                let mut entries: Vec<PathBuf> = fs::read_dir(&dir)
                    .map_err(|e| e.to_string())?
                    .filter_map(|e| e.ok().map(|e| e.path()))
                    .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("yaml"))
                    .collect();
                entries.sort_by(|a, b| {
                    a.file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or("")
                        .to_lowercase()
                        .cmp(&b.file_name().and_then(|s| s.to_str()).unwrap_or("").to_lowercase())
                });
                for p in entries {
                    let text = fs::read_to_string(&p).map_err(|e| e.to_string())?;
                    let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("?").to_string();
                    let id = match sub {
                        "worlds" => format!("{}worlds/{}", id_prefix, stem),
                        "clusters" => format!("{}clusters/{}", id_prefix, stem),
                        "traits" => format!("{}traits/{}", id_prefix, stem),
                        _ => unreachable!(),
                    };
                    match sub {
                        "worlds" => match serde_yaml::from_str::<World>(&text) {
                            Ok(w) => { cache.worlds.insert(id, w); },
                            Err(e) => eprintln!("skip world {}: {}", p.display(), e),
                        },
                        "clusters" => match serde_yaml::from_str::<Cluster>(&text) {
                            Ok(c) => { cache.clusters.insert(id, c); },
                            Err(e) => eprintln!("skip cluster {}: {}", p.display(), e),
                        },
                        "traits" => match serde_yaml::from_str::<WorldTrait>(&text) {
                            Ok(t) => { cache.traits.push((id, t)); },
                            Err(e) => eprintln!("skip trait {}: {}", p.display(), e),
                        },
                        _ => {}
                    }
                }
            }
        }

        Ok(cache)
    }
}
