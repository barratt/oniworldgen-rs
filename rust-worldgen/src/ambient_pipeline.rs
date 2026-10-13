//! Continuous ambient region execution, with source plans supplied by the caller.
use std::collections::HashSet;
use crate::{ambient_suitability::{Grid,Mob,Location},sampler::{SeededRandom,shuffle_seeded},ambient_count,ambient_placement};
#[derive(Clone,Debug,PartialEq,serde::Deserialize)]
pub struct Spawn {pub prefab:String,pub mob:Mob,pub min:f32,pub max:f32}
#[derive(Clone,serde::Deserialize)]
pub struct Region {pub biome:bool,pub multiplier:f32,pub candidates:Vec<i32>,pub forbidden:HashSet<i32>,pub mobs:Vec<Spawn>}
#[derive(serde::Serialize)]
pub struct Pass {pub region_id:usize,pub biome:bool,pub selected:Vec<(i32,String)>,pub rng_after:crate::rng::RandomState}
#[derive(serde::Serialize)]
pub struct Output {pub name:String,pub world_seed:i32,pub passes:Vec<Pass>,pub occupied:Vec<i32>}

/// Per-world terrain state an ambient pass needs, rendered before the
/// cluster's template placement is known (story traits span worlds).
pub struct Prepared {sources:Vec<crate::ambient_sources::Sources>,backwall_solid:Vec<bool>,pub border:crate::world_border::WorldBorder}
pub fn prepare(ds:&crate::data_source::DataSource,w:&crate::generate::WorldOutput)->Result<Prepared,String>{
    let mut terrain=crate::terrain_render::render(ds,w)?;
    let sources:Vec<_>=w.terrain_regions.iter().zip(&terrain.regions).map(|(r,t)|crate::ambient_sources::Sources::new(t,r.tags.iter().any(|t|t=="PreventAmbientMobsInFeature"))).collect();
    crate::biome_borders::render(ds,w,&mut terrain)?;
    let border=crate::world_border::render(ds,w,&mut terrain)?;
    // Managed backwalls retain their pre-settle state: DoSettleSim updates only
    // foreground/disease on substance notifications, not managed backwalls.
    let backwall_solid=terrain.backwall_cells.iter().map(|c|ds.elements.get(c.element.as_str()).map(|e|e.state=="Solid").ok_or("unknown managed backwall".to_string())).collect::<Result<Vec<_>,_>>()?;
    Ok(Prepared{sources,backwall_solid,border})
}
/// Template placement and entity records for every world, through the same
/// calls as the worldgen CLI: cluster-level story placement, mixing-aware
/// rules and the coordinate's Teleporters setting.
pub struct ClusterTemplates {pub placed:Vec<(crate::template_placement::Output,Vec<String>)>,pub entities:Vec<crate::template_entities::Output>}
pub fn cluster_templates(ds:&crate::data_source::DataSource,coordinate:&str,active_dlc:Option<crate::dlc::ActiveDlc>,
    worlds:&[crate::generate::WorldOutput],prepared:&[Prepared])->Result<ClusterTemplates,String>{
    let coord=crate::coord::Coord::parse(coordinate).ok_or("invalid coordinate")?;
    let cluster=ds.cluster_for_coord(coordinate).ok_or("missing cluster")?;
    let scope=active_dlc.unwrap_or_else(||crate::dlc::ActiveDlc::infer_for_coordinate(cluster,coordinate));
    let teleporters=crate::custom_settings::decode_for(&coord.other,&coord.story,&coord.mixing,&scope)?.teleporters_enabled(&scope);
    let poi_bounds:Vec<_>=prepared.iter().map(|p|p.border.poi_bounds.clone()).collect();
    let placed=crate::story_traits::place_cluster_templates(ds,coordinate,worlds,&poi_bounds)?;
    let entities=worlds.iter().zip(&placed).zip(prepared).map(|((w,(t,_)),p)|crate::template_entities::generate_with(ds,w,&t.placements,&p.border.cells,teleporters)).collect::<Result<_,_>>()?;
    Ok(ClusterTemplates{placed,entities})
}

/// Generate ambient output from native terrain/assets and a supplied managed
/// post-settle foreground grid. `claims` are the world's template-claimed
/// cells (`template_entities` claimed cells).
pub fn generate_world(ds:&crate::data_source::DataSource,w:&crate::generate::WorldOutput,prepared:Prepared,elements:&[String],claims:&HashSet<i32>)->Result<Output,String>{
    if elements.len()!=w.size_x as usize*w.size_y as usize{return Err("incomplete managed ambient grid".into());}
    let definitions=elements.iter().map(|name|ds.elements.get(name).ok_or_else(||format!("unknown ambient element {name}"))).collect::<Result<Vec<_>,_>>()?;
    let solid:Vec<_>=definitions.iter().map(|e|e.state=="Solid").collect();
    let liquid:Vec<_>=definitions.iter().map(|e|e.state=="Liquid").collect();
    let vacuum:Vec<_>=elements.iter().map(|e|e=="Vacuum").collect();
    let cavities=crate::ambient_cavities::detect(w.size_x as usize,w.size_y as usize,&solid)?.into_iter().map(|c|c as i32).collect();
    let Prepared{mut sources,backwall_solid,..}=prepared;
    if sources.len()!=w.terrain_regions.len()||backwall_solid.len()!=elements.len(){return Err("prepared ambient state belongs to another world".into());}
    let grid=Grid{width:w.size_x,height:w.size_y,solid:&solid,liquid:&liquid,vacuum:&vacuum,backwall_solid:&backwall_solid,cavities:&cavities};
    let mut rng=SeededRandom::new(w.world_seed);let mut occupied=HashSet::new();let mut passes=Vec::new();
    for (r,source) in w.terrain_regions.iter().zip(&mut sources){for biome in [false,true] {
        let mobs=crate::ambient_sources::mobs(ds,r,biome)?;
        let candidates=if mobs.is_empty(){Vec::new()}else{source.candidates(biome)};
        let mut forbidden=claims.clone();
        for &cell in &candidates {let element=elements.get(usize::try_from(cell).map_err(|_|"negative candidate")?).ok_or("candidate outside grid")?;
            if element=="Katairite"||element=="Unobtanium"{forbidden.insert(cell);}}
        let plan=Region{biome,multiplier:1.0,candidates,forbidden,mobs};
        let selected=region(&grid,&plan,&mut rng,&mut occupied)?;
        for (cell,_) in &selected {source.placed(*cell);}
        passes.push(Pass{region_id:r.id,biome,selected,rng_after:rng.state()});
    }}
    let mut occupied:Vec<_>=occupied.into_iter().collect();occupied.sort_unstable();
    Ok(Output{name:w.name.clone(),world_seed:w.world_seed,passes,occupied})
}
/// One settled world's managed foreground elements at the observation endpoint.
pub struct Observed {pub name:String,pub world_seed:i32,pub width:i32,pub height:i32,pub elements:Vec<String>}
/// Ambient output for a whole cluster, as `generate-ambient` writes it.
/// `prediction` is the worldgen CLI output: when given, its template records
/// must agree with the recomputation and its claimed cells are used.
pub fn generate_cluster(ds:&crate::data_source::DataSource,coordinate:&str,dlc:Option<crate::dlc::ActiveDlc>,
    prediction:Option<&serde_json::Value>,observed:&[Observed])->Result<Vec<Output>,String>{
    use serde_json::json;
    let json_err=|e:serde_json::Error|e.to_string();
    let generated=crate::generate::generate_for(coordinate,ds,dlc)?;
    if observed.len()!=generated.worlds.len(){return Err("simulated world count differs from the cluster".into());}
    let prepared=generated.worlds.iter().map(|w|prepare(ds,w)).collect::<Result<Vec<_>,_>>()?;
    let templates=cluster_templates(ds,coordinate,dlc,&generated.worlds,&prepared)?;
    let mut claims:Vec<HashSet<i32>>=templates.entities.iter().map(|e|e.claimed_cells.iter().map(|&(c,_)|c).collect()).collect();
    if let Some(p)=prediction {
        // Prefer the CLI's own records; they must agree with the recomputation.
        let predicted=p["worlds"].as_array().ok_or("missing prediction worlds")?;
        if predicted.len()!=generated.worlds.len(){return Err("prediction world count differs".into());}
        for (i,(w,r)) in generated.worlds.iter().zip(predicted).enumerate() {
            if (&r["name"],&r["world_seed"],&r["width"],&r["height"])!=(&json!(w.name),&json!(w.world_seed),&json!(w.size_x),&json!(w.size_y)){
                return Err(format!("{}: prediction world identity differs",w.name));
            }
            for (key,native) in [("mixing",serde_json::to_value(&w.mixing).map_err(json_err)?),("template_placement",serde_json::to_value(&templates.placed[i].0).map_err(json_err)?),
                ("story_traits",serde_json::to_value(&templates.placed[i].1).map_err(json_err)?),("template_entities",serde_json::to_value(&templates.entities[i]).map_err(json_err)?)] {
                if r[key]!=native{return Err(format!("{}: ambient {key} differs from the prediction",w.name));}
            }
            claims[i]=r["template_entities"]["claimedCells"].as_array().ok_or("missing predicted claimed cells")?.iter()
                .map(|c|c[0].as_i64().and_then(|c|i32::try_from(c).ok()).ok_or("invalid claimed cell")).collect::<Result<_,_>>()?;
        }
    }
    let mut worlds=Vec::new();
    for ((w,prepared),claims) in generated.worlds.iter().zip(prepared).zip(&claims) {
        let matches:Vec<_>=observed.iter().filter(|o|o.name==w.name).collect();
        if matches.len()!=1{return Err(format!("{}: expected one simulated world",w.name));}
        let o=matches[0];
        if (o.world_seed,o.width,o.height)!=(w.world_seed,w.size_x,w.size_y){return Err(format!("{}: simulated world identity differs",w.name));}
        worlds.push(generate_world(ds,w,prepared,&o.elements,claims)?);
    }
    Ok(worlds)
}
/// The ambient document around per-world outputs.
pub fn document(coordinate:&str,observation_frames:u32,worlds:&[Output])->serde_json::Value{
    serde_json::json!({"phase":"native-ambient-generated","source":"rust-native+oni-simdll-managed-state","gameVersion":"744825","coordinate":coordinate,
        "observation_frames":observation_frames,"scope":"Native ambient generation using explicit simulation observation endpoint; no game capture consumed; parity unproven","worlds":worlds})
}
pub fn region(grid:&Grid<'_>,plan:&Region,rng:&mut SeededRandom,occupied:&mut HashSet<i32>)->Result<Vec<(i32,String)>,String>{
    if plan.mobs.is_empty(){return Ok(Vec::new());}
    // The game's reverse removal loop stops at index 1, preserving index 0.
    let candidates:Vec<_>=plan.candidates.iter().enumerate().filter(|(i,c)|*i==0||!plan.forbidden.contains(c)).map(|(_,c)|*c).collect();
    if candidates.is_empty(){return Ok(Vec::new());}
    let mut mobs=plan.mobs.clone();if plan.biome {shuffle_seeded(&mut mobs,rng);}
    let mut output=Vec::new();
    for spawn in &mobs {
        let candidates=grid.candidates(&candidates,spawn.mob,occupied,rng)?;
        let Some(roll)=ambient_count::roll(rng,spawn.min,spawn.max,candidates.len(),plan.biome,plan.multiplier)? else {continue;};
        let selected=ambient_placement::spawn_count(grid.width,spawn.mob.width,spawn.mob.height,spawn.mob.padding,
            matches!(spawn.mob.location,Location::Ceiling),roll.count,&candidates,occupied)?;
        output.extend(selected.into_iter().map(|cell|(cell,spawn.prefab.clone())));
    }
    Ok(output)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn first_forbidden_entry_survives_and_occupancy_flows_between_regions(){
        let f=vec![false;49];let cavities=HashSet::new();
        let grid=Grid{width:7,height:7,solid:&f,liquid:&f,vacuum:&f,backwall_solid:&f,cavities:&cavities};
        let plan=Region{biome:true,multiplier:1.0,candidates:vec![24,25],forbidden:HashSet::from([24,25]),
            mobs:vec![Spawn{prefab:"Test".into(),mob:Mob{width:1,height:1,padding:0,location:Location::Air},min:1.0,max:1.0}]};
        let mut rng=SeededRandom::new(44);let start=rng.state();let mut occupied=HashSet::new();
        assert_eq!(region(&grid,&plan,&mut rng,&mut occupied).unwrap(),vec![(24,"Test".into())]);
        assert_eq!(occupied,HashSet::from([24]));assert_ne!(rng.state(),start);
        let after=rng.state();assert!(region(&grid,&plan,&mut rng,&mut occupied).unwrap().is_empty());
        assert_eq!(rng.state(),after,"An occupied singleton must not draw count RNG");
    }
}
