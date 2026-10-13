//! GameSpawnData.AddTemplate records, before ambient mobs and instantiation.
use crate::{data_source::DataSource,generate::WorldOutput,template_placement::Placement};
use std::collections::BTreeMap;
use serde::{Deserialize,Serialize};

fn zero_enum()->serde_json::Value {serde_json::json!(0)}
fn neutral()->String {"Neutral".into()}
fn other()->String {"Other".into()}
fn fresh_rot()->Option<Rottable> {Some(Rottable::default())}

#[derive(Debug,Clone,Default,Deserialize,Serialize)]
#[serde(default,rename_all="camelCase")]
pub struct Rottable {pub rot_amount:f32}
#[derive(Debug,Clone,Default,Deserialize,Serialize)]
#[serde(default)]
pub struct Amount {pub id:Option<String>,pub value:f32}
#[derive(Debug,Clone,Default,Deserialize,Serialize)]
#[serde(default,rename_all="camelCase")]
pub struct Storage {
    pub id:Option<String>,#[serde(default="zero_enum")] pub element:serde_json::Value,
    pub units:f32,pub is_ore:bool,pub temperature:f32,pub disease_name:Option<String>,pub disease_count:i32,
    #[serde(default="fresh_rot")] pub rottable:Option<Rottable>,
}
#[derive(Debug,Clone,Default,Deserialize,Serialize)]
#[serde(default,rename_all="camelCase")]
pub struct Prefab {
    pub id:Option<String>,
    #[serde(rename="location_x")] pub location_x:i32,
    #[serde(rename="location_y")] pub location_y:i32,
    #[serde(default="zero_enum")] pub element:serde_json::Value,
    pub temperature:f32,pub units:f32,pub disease_name:Option<String>,pub disease_count:i32,
    #[serde(default="neutral")] pub rotation_orientation:String,
    pub storage:Option<Vec<Storage>>,
    #[serde(rename="type",default="other")] pub prefab_type:String,
    pub facade_id:Option<String>,pub connections:i32,pub lore_unlock_id:Option<String>,
    pub lore_display_text:Option<String>,pub lore_next_collection_id:Option<String>,
    pub rottable:Option<Rottable>,pub amounts:Option<Vec<Amount>>,
    #[serde(rename="other_values")] pub other_values:Option<Vec<Amount>>,
}
impl Prefab {
    fn at(&self,position:(i32,i32))->Result<Self,String> {
        let mut copy=self.clone();
        copy.location_x=copy.location_x.wrapping_add(position.0);
        copy.location_y=copy.location_y.wrapping_add(position.1);
        if copy.storage.as_ref().is_some_and(|v|v.is_empty()){copy.storage=None;}
        if copy.amounts.as_ref().is_some_and(|v|v.is_empty()){copy.amounts=None;}
        if copy.other_values.as_ref().is_some_and(|v|v.is_empty()){copy.other_values=None;}
        if copy.storage.as_ref().is_some_and(|v|v.iter().any(|s|s.rottable.is_none())){return Err("template storage has null rottable; game Clone would fail".into());}
        Ok(copy)
    }
}

const GROUPS:[&str;5]=["buildings","pickupables","elementalOres","backwallEntities","otherEntities"];
#[derive(Debug,Clone,Serialize,Deserialize)]
pub struct Template {pub groups:[Vec<Prefab>;5],pub discovered:Vec<String>}
impl Template {
    pub fn from_yaml(y:&serde_yaml::Value)->Result<Self,String> {
        let mut groups:[Vec<Prefab>;5]=Default::default();
        for (i,key) in GROUPS.iter().enumerate() {
            groups[i]=serde_yaml::from_value::<Option<Vec<Prefab>>>(y.get(*key).cloned().unwrap_or(serde_yaml::Value::Null)).map_err(|e|e.to_string())?.unwrap_or_default();
        }
        let discovered=serde_yaml::from_value::<Option<Vec<String>>>(y.get("info").and_then(|i|i.get("discover_tags")).cloned().unwrap_or(serde_yaml::Value::Null)).map_err(|e|e.to_string())?.unwrap_or_default();
        Ok(Self {groups,discovered})
    }
}
#[derive(Debug,Default,Serialize)]
#[serde(rename_all="camelCase")]
pub struct Output {
    pub buildings:Vec<Prefab>,pub pickupables:Vec<Prefab>,pub elemental_ores:Vec<Prefab>,pub backwall_entities:Vec<Prefab>,pub other_entities:Vec<Prefab>,
    pub discovered_resources:Vec<String>,
    #[serde(rename="preventFoWReveal")] pub prevent_fow_reveal:Vec<(i32,i32,bool)>,
    pub claimed_cells:Vec<(i32,i32)>,
}

/// `GameSpawnData.IsWarpTeleporter`.
pub fn is_warp_teleporter(id:Option<&str>)->bool {
    matches!(id,Some("WarpPortal"|"WarpReceiver"|"WarpConduitSender"|"WarpConduitReceiver"))
}

/// Records with teleporters enabled (default settings, or any game without Spaced Out).
pub fn generate(ds:&DataSource,w:&WorldOutput,placements:&[Placement],border_cells:&[usize])->Result<Output,String> {
    generate_with(ds,w,placements,border_cells,true)
}

/// `teleporters` is the `GameSpawnData.AddTemplate` flag: false only when
/// Spaced Out is active and the Teleporters setting is Disabled. It drops warp
/// entities from buildings, backwallEntities and otherEntities (never
/// pickupables or elementalOres); template cells and claims are unchanged.
pub fn generate_with(ds:&DataSource,w:&WorldOutput,placements:&[Placement],border_cells:&[usize],teleporters:bool)->Result<Output,String> {
    let mut groups:[Vec<Prefab>;5]=Default::default();let mut output=Output::default();
    let mut claimed:BTreeMap<i32,i32>=border_cells.iter().map(|&c|(c as i32,1)).collect();
    for p in placements {
        let template=ds.template_entities.get(&p.template).ok_or_else(||format!("missing template entities {}",p.template))?.as_ref().map_err(|e|format!("template {}: {e}",p.template))?;
        for (i,entities) in template.groups.iter().enumerate() {
            // GROUPS order: buildings, pickupables, elementalOres, backwallEntities, otherEntities.
            let filter_warps=!teleporters&&matches!(i,0|3|4);
            for entity in entities {
                let x=entity.location_x.wrapping_add(p.position.0);let y=entity.location_y.wrapping_add(p.position.1);
                let cell=y.wrapping_mul(w.size_x).wrapping_add(x);
                if !claimed.contains_key(&cell)&&!(filter_warps&&is_warp_teleporter(entity.id.as_deref())){groups[i].push(entity.at(p.position)?);}
            }
        }
        let cells=ds.template_cells.get(&p.template).ok_or("missing template cells")?.as_ref().map_err(Clone::clone)?;
        for c in cells {
            let x=p.position.0.wrapping_add(c.location_x);let y=p.position.1.wrapping_add(c.location_y);
            let cell=y.wrapping_mul(w.size_x).wrapping_add(x);
            let count=claimed.entry(cell).or_insert(0);
            if *count==0 {output.prevent_fow_reveal.push((x,y,c.prevent_fow_reveal));}
            *count+=1;
        }
        output.discovered_resources.extend(template.discovered.iter().cloned());
    }
    let [buildings,pickupables,elemental_ores,backwall_entities,other_entities]=groups;
    output.buildings=buildings;output.pickupables=pickupables;output.elemental_ores=elemental_ores;
    output.backwall_entities=backwall_entities;output.other_entities=other_entities;
    output.claimed_cells=claimed.into_iter().collect();
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn clone_preserves_game_defaults_and_normalizes_empty_optional_lists() {
        let p:Prefab=serde_yaml::from_str("id: Test\nlocation_x: 3\namounts: []\nother_values: []\nstorage: []").unwrap();
        let c=p.at((7,5)).unwrap();
        assert_eq!((c.location_x,c.location_y),(10,5));
        assert_eq!(c.element,serde_json::json!(0));assert_eq!(c.prefab_type,"Other");assert_eq!(c.rotation_orientation,"Neutral");
        assert!(c.amounts.is_none()&&c.other_values.is_none()&&c.storage.is_none());
        let p:Prefab=serde_yaml::from_str("id: Test\nstorage:\n- id: Food\n  units: 2.5").unwrap();
        assert_eq!(p.at((0,0)).unwrap().storage.unwrap()[0].rottable.as_ref().unwrap().rot_amount,0.0);
    }
    #[test]
    fn claims_block_later_templates_but_not_entities_in_the_same_template() {
        use crate::generate::{generate_with_placed_geysers,ClusterDescriptor,WorldDescriptor};
        let w=generate_with_placed_geysers("SNDST-C-1-0-0-0",&ClusterDescriptor {worlds:vec![WorldDescriptor {name:"test".into(),size_x:4,size_y:4,geysers:Vec::new()}]}).unwrap().worlds.remove(0);
        let mut ds=DataSource::default();
        for (name,source) in [
            ("first","otherEntities:\n- {id: A, location_x: 1, location_y: 1}\n- {id: B, location_x: 1, location_y: 1}\ninfo: {discover_tags: [Water]}"),
            ("second","otherEntities:\n- {id: C, location_x: 1, location_y: 1, storage: [{id: Food, rottable: null}]}\n- {id: D, location_x: 2, location_y: 1}\n- {id: E, location_x: 3, location_y: 1}\ninfo: {discover_tags: [Water]}")
        ] {
            ds.template_entities.insert(name.into(),Template::from_yaml(&serde_yaml::from_str(source).unwrap()));
            ds.template_cells.insert(name.into(),Ok(serde_yaml::from_str("- {element: SandStone, location_x: 1, location_y: 1, preventFoWReveal: true}").unwrap()));
        }
        let placements:Vec<_>=["first","second"].iter().map(|s|Placement {template:(*s).into(),region_id:0,position:(0,0),bounds:(0,0,1,1)}).collect();
        let out=generate(&ds,&w,&placements,&[6]).unwrap();
        assert_eq!(out.other_entities.iter().map(|p|p.id.as_deref().unwrap()).collect::<Vec<_>>(),vec!["A","B","E"]);
        assert_eq!(out.claimed_cells,vec![(5,2),(6,1)]);
        assert_eq!(out.prevent_fow_reveal,vec![(1,1,true)]);
        assert_eq!(out.discovered_resources,vec!["Water","Water"]);
    }
    // Code-derived from GameSpawnData.AddTemplate / IsWarpTeleporter.
    #[test]
    fn disabled_teleporters_drop_warp_entities_but_keep_cells_and_pickupables() {
        use crate::generate::{generate_with_placed_geysers,ClusterDescriptor,WorldDescriptor};
        let w=generate_with_placed_geysers("SNDST-C-1-0-0-0",&ClusterDescriptor {worlds:vec![WorldDescriptor {name:"test".into(),size_x:8,size_y:8,geysers:Vec::new()}]}).unwrap().worlds.remove(0);
        let mut ds=DataSource::default();
        let source="buildings:
- {id: WarpPortal, location_x: 1, location_y: 1}
- {id: Door, location_x: 2, location_y: 1}
pickupables:
- {id: WarpReceiver, location_x: 3, location_y: 1}
backwallEntities:
- {id: WarpConduitSender, location_x: 4, location_y: 1}
otherEntities:
- {id: WarpConduitReceiver, location_x: 5, location_y: 1}
- {id: WarpReceiver, location_x: 6, location_y: 1}
- {id: GeyserGeneric, location_x: 1, location_y: 2}";
        ds.template_entities.insert("warp".into(),Template::from_yaml(&serde_yaml::from_str(source).unwrap()));
        ds.template_cells.insert("warp".into(),Ok(serde_yaml::from_str("- {element: SandStone, location_x: 1, location_y: 1}").unwrap()));
        let placements=vec![Placement {template:"warp".into(),region_id:0,position:(0,0),bounds:(0,0,1,1)}];
        let ids=|v:&[Prefab]|v.iter().map(|p|p.id.clone().unwrap()).collect::<Vec<_>>();
        let on=generate_with(&ds,&w,&placements,&[],true).unwrap();
        assert_eq!(ids(&on.buildings),vec!["WarpPortal","Door"]);
        assert_eq!(ids(&on.other_entities),vec!["WarpConduitReceiver","WarpReceiver","GeyserGeneric"]);
        let off=generate_with(&ds,&w,&placements,&[],false).unwrap();
        assert_eq!(ids(&off.buildings),vec!["Door"]);
        assert_eq!(ids(&off.pickupables),vec!["WarpReceiver"]);
        assert!(off.backwall_entities.is_empty());
        assert_eq!(ids(&off.other_entities),vec!["GeyserGeneric"]);
        assert_eq!((off.claimed_cells,off.prevent_fow_reveal),(on.claimed_cells,on.prevent_fow_reveal));
    }
}
