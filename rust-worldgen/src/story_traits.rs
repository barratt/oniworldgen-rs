//! Story traits (build 744825), ported from decompiled code and not yet
//! compared with the game's output.
//!
//! - `OfflineWorldGen.DoWorldGenInitialize` maps `CustomGameSettings.GetCurrentStories()`
//!   to `Story.worldgenStoryTraitKey`. That enumerates `currentStoryLevelsBySetting`,
//!   a Dictionary filled once in `GetStoriesSortedByCoordinateOrder` order and never
//!   removed from, so the chosen list is in coordinate order.
//! - The `Cluster` constructor resolves them through `SettingsCache.GetCachedStoryTrait`
//!   (asserting, so a missing trait throws) unless `clusterLayout.disableStoryTraits`.
//!   World constructors receive no story traits, so stories never apply
//!   subworlds, feature mods or element band changes; only their template rules run.
//! - `Cluster.BeginGeneration` sorts worlds by axial distance from the starting
//!   world's cluster location, but locations are only assigned afterwards by
//!   `AssignClusterLocations`, so every distance is 0 and the order is whatever
//!   Mono's unstable sort leaves (the original order for up to 16 worlds).
//! - Each world gets the still unplaced stories as candidates
//!   (`SetStoryTraitCandidates`); `TemplateSpawning.SpawnStoryTraitTemplates` runs
//!   after the template rules on the same `WorldGen.myRandom`. Placed stories
//!   are removed from the cluster list; leftovers after the last world fail
//!   generation ("Story trait failure, unable to place on any world").
use crate::{data_source::{ClusterYaml,DataSource},diagnostics::{Failure,RunError},generate::WorldOutput,template_placement::{self,Output}};

/// `Database.Stories`: (Story id, worldgenStoryTraitKey), in coordinate order.
pub const STORY_TRAIT_KEYS:[(&str,&str);6]=[
    ("MegaBrainTank","storytraits/MegaBrainTank"),("CreatureManipulator","storytraits/CritterManipulator"),
    ("LonelyMinion","storytraits/LonelyMinion"),("FossilHunt","storytraits/FossilHunt"),
    ("MorbRoverMaker","storytraits/MorbRoverMaker"),("HijackHeadquarters","storytraits/HijackHeadquarters"),
];

/// `Cluster.unplacedStoryTraits` at construction, as story trait file paths.
pub fn chosen(ds:&DataSource,cluster:&ClusterYaml,story_ids:&[String])->Result<Vec<String>,String> {
    if cluster.disable_story_traits {return Ok(Vec::new());}
    STORY_TRAIT_KEYS.iter().filter(|(id,_)|story_ids.iter().any(|s|s==id)).map(|(_,key)| {
        ds.story_trait(key).map(|_|key.to_string()).ok_or_else(||format!("Couldn't get story trait [{key}]"))
    }).collect()
}

/// World indices in `Cluster.BeginGeneration` processing order.
pub fn world_order(count:usize)->Vec<usize> {
    let distances=vec![0i32;count];
    let mut order:Vec<usize>=(0..count).collect();
    template_placement::mono_sort(&mut order,&|&a:&usize,&b:&usize|distances[a]<distances[b]);
    order
}

/// Run `place_world(index, candidates)` for each world in processing order,
/// carrying unplaced stories forward. Results are indexed by world.
pub fn place_in_order<T>(count:usize,chosen:&[String],mut place_world:impl FnMut(usize,&[String])->Result<(T,Vec<String>),RunError>)->Result<Vec<(T,Vec<String>)>,RunError> {
    let mut unplaced=chosen.to_vec();
    let mut results:Vec<Option<(T,Vec<String>)>>=(0..count).map(|_|None).collect();
    for index in world_order(count) {
        let (value,placed)=place_world(index,&unplaced)?;
        for story in &placed {if let Some(i)=unplaced.iter().position(|s|s==story){unplaced.remove(i);}}
        results[index]=Some((value,placed));
    }
    if !unplaced.is_empty() {
        // Reported through worlds[0]; the text has no `Worldgen failure: ` prefix.
        let message=format!("Story trait failure, unable to place on any world: {}",unplaced.join(", "));
        return Err(RunError::failure(message.clone(),Failure {stage:"StoryTraits",world_index:-1,reporting_world_index:Some(0),world:None,message,origin:None}));
    }
    Ok(results.into_iter().map(|r|r.expect("every world processed")).collect())
}

/// Template placement for every world of a generated cluster, including story
/// traits from the coordinate's STORY field. `initial_bounds[i]` are world i's
/// border POI bounds.
pub fn place_cluster_templates(ds:&DataSource,coordinate:&str,worlds:&[WorldOutput],initial_bounds:&[Vec<(i32,i32,i32,i32)>])->Result<Vec<(Output,Vec<String>)>,RunError> {
    let coord=crate::coord::Coord::parse(coordinate).ok_or("invalid coordinate")?;
    let settings=crate::custom_settings::decode(&coord.other,&coord.story,&coord.mixing)?;
    let cluster=ds.cluster_for_coord(coordinate).ok_or("missing cluster")?;
    let chosen=chosen(ds,cluster,&settings.stories)?;
    place_in_order(worlds.len(),&chosen,|i,candidates| {
        let rules=template_placement::input_rules(ds,&worlds[i],&[])?;
        template_placement::place_with_stories(ds,&worlds[i],rules,&initial_bounds[i],candidates)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::custom_settings::{decode,STORIES};
    fn cluster(disable:bool)->ClusterYaml {
        ClusterYaml {id:String::new(),poi_placements:None,coordinate_prefix:"T".into(),start_world_index:0,num_rings:0,world_placements:Vec::new(),
            required_dlc_ids:Vec::new(),forbidden_dlc_ids:Vec::new(),cluster_tags:Vec::new(),disable_story_traits:disable,fixed_coordinate:-1,skip:false}
    }
    fn data()->DataSource {
        let mut ds=DataSource::default();
        for (_,key) in STORY_TRAIT_KEYS {ds.story_traits.push((key.into(),Default::default()));}
        ds
    }
    #[test]
    fn story_keys_follow_database_stories_coordinate_order() {
        assert_eq!(STORY_TRAIT_KEYS.map(|(id,_)|id),STORIES);
    }
    #[test]
    fn chosen_story_traits_keep_coordinate_order_and_respect_disable_flag() {
        let ds=data();
        // "4A" = all six (coordinate-codes.js); the critter story's file is CritterManipulator.
        let all=chosen(&ds,&cluster(false),&decode("0","4A","0").unwrap().stories).unwrap();
        assert_eq!(all,STORY_TRAIT_KEYS.map(|(_,k)|k.to_string()));
        assert!(chosen(&ds,&cluster(true),&decode("0","4A","0").unwrap().stories).unwrap().is_empty());
        // Order is fixed by the table, not by the caller's list order.
        let two=chosen(&ds,&cluster(false),&["FossilHunt".into(),"MegaBrainTank".into()]).unwrap();
        assert_eq!(two,["storytraits/MegaBrainTank","storytraits/FossilHunt"]);
        // GetCachedStoryTrait(assertMissingTrait: true) throws for unknown traits.
        assert!(chosen(&DataSource::default(),&cluster(false),&["LonelyMinion".into()]).is_err());
    }
    #[test]
    fn worlds_are_processed_in_placement_order_because_cluster_locations_are_unset() {
        for n in [1,2,3,11,16] {assert_eq!(world_order(n),(0..n).collect::<Vec<_>>());}
        // Equal keys above 16 entries are permuted by Mono's introsort pivoting.
        assert_ne!(world_order(17),(0..17).collect::<Vec<_>>());
    }
    #[test]
    fn unplaced_stories_carry_over_and_leftovers_fail() {
        let chosen:Vec<String>=["a","b","c"].map(String::from).to_vec();
        let mut seen=Vec::new();
        // World 0 places b, world 1 nothing, world 2 places a then c.
        let placed=place_in_order(3,&chosen,|i,c| {
            seen.push((i,c.to_vec()));
            Ok(((),match i {0=>vec!["b".into()],2=>vec!["a".into(),"c".into()],_=>vec![]}))
        }).unwrap();
        assert_eq!(seen,[(0,chosen.clone()),(1,vec!["a".into(),"c".into()]),(2,vec!["a".into(),"c".into()])]);
        assert_eq!(placed.iter().map(|p|p.1.len()).collect::<Vec<_>>(),[1,0,2]);
        let err=place_in_order(2,&chosen,|_,_|Ok(((),vec![]))).unwrap_err();
        assert!(err.text.ends_with("a, b, c"),"{err}");
        assert_eq!(err.failure.unwrap().stage,"StoryTraits");
    }
}
