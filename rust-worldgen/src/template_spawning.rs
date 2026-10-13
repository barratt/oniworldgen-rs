//! Port scaffold for `ProcGenGame.TemplateSpawning` +
//! `GameSpawnData.AddTemplate`. These determine the `(type, x, y)`
//! of every geyser and POI the game spawns per world.
//!
//! Shape of the port:
//! 1. A `TemplateContainer` is a YAML file listing Prefabs (buildings,
//!    pickupables, elementalOres, otherEntities) each with a local
//!    `(location_x, location_y)`.
//! 2. `AddTemplate(template, position)` clones each Prefab with
//!    `location_x += position.x`, `location_y += position.y` into a
//!    per-world `GameSpawnData`.
//! 3. `WorldGenSpawner.PlaceTemplates` then adds each world's
//!    `worldOffset` on top.
//!
//! For bit-exact DB match we need all three layers.

use crate::geom::V2;

/// One entry in a template (building, ore, entity, pickupable).
#[derive(Debug, Clone)]
pub struct Prefab {
    pub id: String,
    pub kind: PrefabKind,
    pub location_x: i32,
    pub location_y: i32,
    pub element: Option<String>,
    pub temperature: Option<f32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrefabKind {
    Building,
    Pickupable,
    ElementalOre,
    OtherEntity,
}

/// Port of `TemplateContainer`. Holds the four prefab buckets plus
/// template cells (used by the start-base placer).
#[derive(Debug, Clone, Default)]
pub struct TemplateContainer {
    pub buildings: Vec<Prefab>,
    pub pickupables: Vec<Prefab>,
    pub elemental_ores: Vec<Prefab>,
    pub other_entities: Vec<Prefab>,
}

impl TemplateContainer {
    /// `AddTemplate` analogue. Returns a new `TemplateContainer`
    /// whose prefab locations have been offset by `position`.
    pub fn with_offset(&self, position: V2) -> TemplateContainer {
        let ox = position.x as i32;
        let oy = position.y as i32;
        let off = |p: &Prefab| Prefab {
            location_x: p.location_x + ox,
            location_y: p.location_y + oy,
            ..p.clone()
        };
        TemplateContainer {
            buildings: self.buildings.iter().map(off).collect(),
            pickupables: self.pickupables.iter().map(off).collect(),
            elemental_ores: self.elemental_ores.iter().map(off).collect(),
            other_entities: self.other_entities.iter().map(off).collect(),
        }
    }
}

/// Spawned geyser, ready for rate rolling. `world_local_x/y` are
/// the coordinates the game feeds into `GeyserConfigurator`.
#[derive(Debug, Clone)]
pub struct PlacedGeyser {
    pub type_id: String,
    pub world_local_x: i32,
    pub world_local_y: i32,
}

/// Extract geysers from a `TemplateContainer` after all offsets have
/// been applied. Geysers are recognized by prefab id prefix
/// (`GeyserGeneric_*`, `OilWell`) matching the DB's stored `type`.
pub fn extract_geysers(template: &TemplateContainer) -> Vec<PlacedGeyser> {
    let mut out = Vec::new();
    for p in template.other_entities.iter() {
        if let Some(ty) = geyser_type_id(&p.id) {
            out.push(PlacedGeyser {
                type_id: ty.to_string(),
                world_local_x: p.location_x,
                world_local_y: p.location_y,
            });
        }
    }
    for p in template.buildings.iter() {
        if p.id == "OilWell" {
            out.push(PlacedGeyser {
                type_id: "OilWell".into(),
                world_local_x: p.location_x,
                world_local_y: p.location_y,
            });
        }
    }
    out
}

fn geyser_type_id(prefab_id: &str) -> Option<&str> {
    // `GeyserGeneric_steam` -> "steam". OilWell handled separately.
    prefab_id.strip_prefix("GeyserGeneric_")
}

/// Skeleton for `DetermineTemplatesForWorld`. Given a seed and a set
/// of terrain cells (the Voronoi-classified cells from WorldLayout),
/// pick which templates to spawn. For now this is a TODO — it needs
/// the full TemplateSpawning + story-trait placement logic.
#[allow(clippy::too_many_arguments)]
pub fn determine_templates_for_world(
    _seed: i32,
    _map_width: i32,
    _map_height: i32,
    _terrain_cells: &[TerrainCell],
    _start_template: Option<&str>,
    _template_rules: &[TemplateSpawnRule],
) -> Vec<PlacedTemplate> {
    // TODO(Phase 3): full port of TemplateSpawning. Steps:
    //   1. SpawnStartingTemplate: place start template at start
    //      subworld's centroid.
    //   2. SpawnTemplatesFromTemplateRules: iterate rules, spawn
    //      templates on matching cells with seeded-random selection.
    //   3. SpawnStoryTraitTemplates: apply story-trait-driven extras.
    // Each step respects POI-overlap bounds stored in the world's
    // placedPOIBounds list.
    Vec::new()
}

#[derive(Debug, Clone)]
pub struct TerrainCell {
    pub cell_id: u32,
    pub centroid: V2,
    pub subworld: String,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct TemplateSpawnRule {
    pub template_names: Vec<String>,
    pub allowed_cell_tags: Vec<String>,
    pub min_count: i32,
    pub max_count: i32,
    pub priority: i32,
}

#[derive(Debug, Clone)]
pub struct PlacedTemplate {
    pub template_name: String,
    pub position: V2,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_geysers_from_entities() {
        let t = TemplateContainer {
            other_entities: vec![
                Prefab {
                    id: "GeyserGeneric_steam".into(),
                    kind: PrefabKind::OtherEntity,
                    location_x: 47,
                    location_y: 83,
                    element: None,
                    temperature: None,
                },
                Prefab {
                    id: "ShinyBauble".into(),
                    kind: PrefabKind::OtherEntity,
                    location_x: 10,
                    location_y: 10,
                    element: None,
                    temperature: None,
                },
            ],
            ..Default::default()
        };
        let g = extract_geysers(&t);
        assert_eq!(g.len(), 1);
        assert_eq!(g[0].type_id, "steam");
        assert_eq!(g[0].world_local_x, 47);
        assert_eq!(g[0].world_local_y, 83);
    }

    #[test]
    fn template_offset_stacks() {
        let mut t = TemplateContainer::default();
        t.buildings.push(Prefab {
            id: "OilWell".into(),
            kind: PrefabKind::Building,
            location_x: 5,
            location_y: 5,
            element: None,
            temperature: None,
        });
        let moved = t.with_offset(V2::new(100.0, 200.0));
        assert_eq!(moved.buildings[0].location_x, 105);
        assert_eq!(moved.buildings[0].location_y, 205);
    }
}
