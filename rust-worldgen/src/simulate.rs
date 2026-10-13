//! Simulation inputs: the parts of a generated cluster the settle step
//! (the `settle` crate) consumes.
use serde_json::{Value,json};

/// The fields of the worldgen CLI document that the settle step reads, from the typed
/// stage results: the `Prediction` inputs plus the template records ambient
/// placement checks against its recomputation.
pub fn prediction(run:&crate::pipeline::ClusterRun,game_version:&str)->Value{
    let worlds:Vec<Value>=run.cluster.worlds.iter().zip(&run.worlds).map(|(w,s)|json!({
        "name":w.name,"width":w.size_x,"height":w.size_y,"world_seed":w.world_seed,"offset":w.offset,
        "terrain_after_starting_adjustments":s.terrain_after_starting_adjustments,"backwall_cells":s.backwall_cells,"world_border":s.world_border,
        "template_cell_writes":s.template_cell_writes,"mixing":w.mixing,"template_placement":s.template_placement,
        "story_traits":s.story_traits,"template_entities":s.template_entities})).collect();
    json!({"source":"rust-native","coordinate":run.coordinate,"gameVersion":game_version,"activeDlcIds":run.explicit_dlc.map(|d|d.ids()),"worlds":worlds})
}
