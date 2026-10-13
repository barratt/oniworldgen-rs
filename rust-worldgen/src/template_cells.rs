//! Ordered template cell message inputs, injected before settle frame 498.
//! These are not the post-physics grid or SimMessages' normalized payloads.
use crate::{data_source::DataSource,generate::WorldOutput,template_placement::Placement};
use crate::fxhash::FxHashSet as HashSet;

#[derive(Debug,Clone,serde::Deserialize,serde::Serialize)]
#[serde(default)]
pub struct Cell {
    pub element:String,pub mass:f32,pub temperature:f32,
    #[serde(rename="diseaseName")] pub disease:Option<String>,
    #[serde(rename="diseaseCount")] pub disease_count:i32,
    pub location_x:i32,pub location_y:i32,
    #[serde(rename="preventFoWReveal")] pub prevent_fow_reveal:bool,
    #[serde(rename="backwallElement")] pub backwall_element:Option<String>,
    #[serde(rename="backwallMass")] pub backwall_mass:f32,
    #[serde(rename="backwallTemperature")] pub backwall_temperature:f32,
}
impl Default for Cell {
    fn default()->Self {Self {element:String::new(),mass:0.0,temperature:0.0,disease:None,disease_count:0,location_x:0,location_y:0,prevent_fow_reveal:false,
        backwall_element:None,backwall_mass:0.0,backwall_temperature:0.0}}
}

#[derive(Debug,serde::Serialize,serde::Deserialize)]
pub struct Write {
    pub cell:usize,pub element:String,pub mass:f32,pub temperature:f32,pub disease:Option<String>,pub disease_count:i32,
    pub backwall_element:Option<String>,pub backwall_mass:f32,pub backwall_temperature:f32,
}

pub fn writes(ds:&DataSource,w:&WorldOutput,placements:&[Placement])->Result<Vec<Write>,String> {
    let mut claimed=HashSet::default();let mut out=Vec::new();
    for p in placements {
        let cells=ds.template_cells.get(&p.template).ok_or_else(||format!("missing template cells {}",p.template))?.as_ref().map_err(|e|format!("template {}: {e}",p.template))?;
        for c in cells {
            // Grid.OffsetCell and Grid.IsValidCell validate the flat index, not
            // separate coordinates. Preserve its wrap behavior at horizontal edges.
            let index=(p.position.1 as i64+c.location_y as i64)*w.size_x as i64+p.position.0 as i64+c.location_x as i64;
            if index<0||index>=w.size_x as i64*w.size_y as i64||!claimed.insert(index){continue;}
            // ElementLoader.FindElementByName hashes the lowercased name, so template
            // element names are case-insensitive (e.g. "Sandstone" for SandStone).
            let canonical=|name:&str|ds.elements.get_key_value(name).map(|(k,_)|k.clone())
                .or_else(||ds.elements.keys().find(|k|k.eq_ignore_ascii_case(name)).cloned());
            let (Some(element),Ok(backwall_element))=(canonical(&c.element),c.backwall_element.as_deref().map(|e|canonical(e).ok_or(())).transpose()) else {
                return Err(format!("unknown template element in {}: {} / {:?}",p.template,c.element,c.backwall_element));
            };
            if [c.mass,c.temperature,c.backwall_mass,c.backwall_temperature].iter().any(|v|!v.is_finite()){return Err(format!("nonfinite template material in {}",p.template));}
            out.push(Write {cell:index as usize,element,mass:c.mass,temperature:c.temperature,
                disease:c.disease.clone().filter(|s|!s.is_empty()),disease_count:c.disease_count,
                backwall_element,backwall_mass:c.backwall_mass,backwall_temperature:c.backwall_temperature});
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn template_writes_use_first_claim_and_flat_grid_validity() {
        use crate::generate::{generate_with_placed_geysers,ClusterDescriptor,WorldDescriptor};
        let world=generate_with_placed_geysers("SNDST-C-1-0-0-0",&ClusterDescriptor {worlds:vec![WorldDescriptor {
            name:"test".into(),size_x:4,size_y:4,geysers:Vec::new()}]}).unwrap().worlds.remove(0);
        let mut ds=DataSource::default();
        ds.elements.insert("SandStone".into(),crate::terrain::ElementDefinition::default());
        ds.template_cells.insert("first".into(),Ok(serde_yaml::from_str("- {element: SandStone, mass: 10, location_x: -1, location_y: 1}\n- {element: SandStone, mass: 99, location_x: -1}\n- {element: SandStone, mass: 99, location_y: 4}").unwrap()));
        ds.template_cells.insert("second".into(),Ok(serde_yaml::from_str("- {element: SandStone, mass: 20, location_x: 3}\n- {element: SandStone, mass: 30, location_x: 1}").unwrap()));
        let placements:Vec<_>=["first","second"].iter().map(|name|Placement {template:(*name).into(),region_id:0,position:(0,0),bounds:(0,0,1,1)}).collect();
        let output=writes(&ds,&world,&placements).unwrap();
        assert_eq!(output.iter().map(|w|(w.cell,w.mass)).collect::<Vec<_>>(),vec![(3,10.0),(1,30.0)]);
    }
    #[test]
    fn template_element_names_resolve_case_insensitively() {
        // expansion1::poi/wasteland/worm_a writes "Sandstone"; the game hashes lowercased names.
        use crate::generate::{generate_with_placed_geysers,ClusterDescriptor,WorldDescriptor};
        let world=generate_with_placed_geysers("SNDST-C-1-0-0-0",&ClusterDescriptor {worlds:vec![WorldDescriptor {
            name:"test".into(),size_x:4,size_y:4,geysers:Vec::new()}]}).unwrap().worlds.remove(0);
        let mut ds=DataSource::default();
        ds.elements.insert("SandStone".into(),crate::terrain::ElementDefinition::default());
        ds.elements.insert("Vacuum".into(),crate::terrain::ElementDefinition::default());
        ds.template_cells.insert("t".into(),Ok(serde_yaml::from_str("- {element: Sandstone, mass: 800, backwallElement: vacuum}").unwrap()));
        let output=writes(&ds,&world,&[Placement {template:"t".into(),region_id:0,position:(0,0),bounds:(0,0,1,1)}]).unwrap();
        assert_eq!((output[0].element.as_str(),output[0].backwall_element.as_deref()),("SandStone",Some("Vacuum")));
        ds.template_cells.insert("bad".into(),Ok(serde_yaml::from_str("- {element: NotAnElement}").unwrap()));
        assert!(writes(&ds,&world,&[Placement {template:"bad".into(),region_id:0,position:(0,0),bounds:(0,0,1,1)}]).is_err());
    }
    #[test]
    fn omitted_backwall_is_invalid_element_sentinel_not_vacuum() {
        let implicit:Cell=serde_yaml::from_str("element: SandStone\nmass: 700\ntemperature: 295.0506896972656").unwrap();
        let explicit:Cell=serde_yaml::from_str("element: SandStone\nbackwallElement: Vacuum").unwrap();
        assert_eq!(implicit.backwall_element,None);
        assert_eq!(explicit.backwall_element.as_deref(),Some("Vacuum"));
        assert_eq!(implicit.temperature.to_bits(),295.0507f32.to_bits());
        assert_eq!(implicit.disease,None);
        assert_eq!((implicit.location_x,implicit.location_y),(0,0));
        // Missing foreground data must fail element resolution, not silently
        // turn a malformed template into vacuum cells.
        assert!(serde_yaml::from_str::<Cell>("{}").unwrap().element.is_empty());
    }
}
