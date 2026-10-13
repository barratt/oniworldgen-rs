//! Ordered candidate sources retained from native terrain rendering.
use std::collections::HashSet;
use crate::{ambient_pipeline::Spawn,ambient_suitability::Mob,data_source::DataSource,subworld::{TerrainRegion,Density}};

/// Resolve ordered mob definitions from native graph tags and installed assets.
pub fn mobs(ds:&DataSource,region:&TerrainRegion,biome:bool)->Result<Vec<Spawn>,String>{
    let spawn=|name:&str,range:&Density|->Result<Option<Spawn>,String>{
        let Some(m)=ds.mobs.get(name) else{return Ok(None)};
        let location=if m.location.is_empty(){"Floor"}else{&m.location};
        Ok(Some(Spawn{prefab:m.prefab_name.clone().unwrap_or_else(||name.into()),min:range.min,max:range.max,
            mob:Mob{width:m.width,height:m.height,padding:m.padding_x,
                location:serde_yaml::from_value(serde_yaml::Value::String(location.into())).map_err(|e|e.to_string())?}}))
    };
    let mut output=Vec::new();
    if biome {
        for name in &region.biome_tags {if let Some(m)=ds.mobs.get(name.as_str()){if let Some(s)=spawn(name,&m.density)?{output.push(s);}}}
    }else if let Some(feature)=region.feature_tags.iter().find_map(|name|ds.features.get(name.as_str())) {
        for entry in &feature.internal_mobs {if let Some(s)=spawn(&entry.kind,&entry.count)? {output.push(s);}}
    }
    Ok(output)
}
pub struct Sources {
    available:Vec<i32>,feature:HashSet<i32>,feature_for_biome:HashSet<i32>,pub prevent_in_feature:bool,
}
impl Sources {
    pub fn new(trace:&crate::terrain_render::RegionTrace,prevent_in_feature:bool)->Self{
        Self{available:trace.spawn_cells.clone(),feature:trace.feature_cells.iter().copied().collect(),
            feature_for_biome:trace.feature_cells_for_biome.iter().copied().collect(),prevent_in_feature}
    }
    pub fn candidates(&self,biome:bool)->Vec<i32>{
        self.available.iter().copied().filter(|c|if !biome {self.feature.contains(c)}else{!self.prevent_in_feature||!self.feature_for_biome.contains(c)}).collect()
    }
    pub fn placed(&mut self,cell:i32){self.available.retain(|&c|c!=cell);}
}
