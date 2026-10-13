//! Resource minimums applied to the starting world's StartWorld leaf regions,
//! after borders and before settling. Assertions in the game do not clamp mass.
use crate::{data_source::DataSource, generate::WorldOutput, geom::{Polygon,V2}, terrain::{claim_cells,MaterialCell}};

#[derive(Debug,Clone,serde::Deserialize,serde::Serialize)]
pub struct Setting { pub element:String, pub amount:f32 }

#[derive(Debug,serde::Serialize)]
pub struct Adjustment {
    pub element:String, pub amount:f32, pub count:usize,
    pub mass_before:f32, pub increment:f32,
}

pub fn settings(ds:&DataSource,w:&WorldOutput)->Result<Vec<Setting>,String> {
    let world=ds.world(&w.name).ok_or("starting resources: missing world")?;
    match world.starting_world_elements.as_ref().or_else(||ds.defaults.get("startingWorldElements")) {
        Some(v)=>serde_yaml::from_value(v.clone()).map_err(|e|format!("starting resources: {e}")),
        None=>Ok(Vec::new()),
    }
}

pub fn region_cells(w:&WorldOutput)->Result<Vec<usize>,String> {
    let polygons:Vec<_>=w.terrain_regions.iter().map(|r|Polygon::new(r.polygon.iter().map(|&(x,y)|V2::new(x,y)).collect())).collect();
    let width=usize::try_from(w.size_x).map_err(|_|"invalid starting world width")?;
    let height=usize::try_from(w.size_y).map_err(|_|"invalid starting world height")?;
    let claims=claim_cells(&polygons,width,height)?;
    Ok(w.terrain_regions.iter().zip(claims).filter(|(r,_)|r.leaf_tags.iter().any(|t|t=="StartWorld"))
        .flat_map(|(_,cells)|cells).collect())
}

/// Ordered accumulation rounds at each local float store; the increment's
/// subtraction/division stay extended until their shared float store in Mono.
pub fn apply(cells:&mut [Option<MaterialCell>],indices:&[usize],rules:&[Setting])->Result<Vec<Adjustment>,String> {
    for &i in indices {
        if cells.get(i).and_then(Option::as_ref).is_none() {return Err(format!("missing starting resource cell {i}"));}
    }
    let mut output=Vec::new();
    for rule in rules {
        let selected:Vec<_>=indices.iter().copied().filter(|&i|cells[i].as_ref().unwrap().element.eq_ignore_ascii_case(&rule.element)).collect();
        let mut total=0.0_f32;
        for &i in &selected {total+=cells[i].as_ref().unwrap().mass;}
        let increment=if total<rule.amount&&!selected.is_empty() {
            ((rule.amount as f64-total as f64)/(selected.len() as f32 as f64)) as f32
        } else {0.0};
        if total<rule.amount&&!selected.is_empty() {
            for &i in &selected {cells[i].as_mut().unwrap().mass+=increment;}
        }
        output.push(Adjustment {element:rule.element.clone(),amount:rule.amount,count:selected.len(),mass_before:total,increment});
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn cell(element:&str,mass:f32)->Option<MaterialCell> {Some(MaterialCell {element:element.into(),mass,..Default::default()})}
    #[test]
    fn minimum_only_affects_selected_matching_cells_and_preserves_mass_differences() {
        let mut cells=vec![cell("Algae",1.0),cell("Algae",3.0),cell("Algae",7.0),cell("Dirt",1.0)];
        let report=apply(&mut cells,&[0,1,3],&[Setting {element:"algae".into(),amount:10.0}]).unwrap();
        assert_eq!(report[0].count,2);
        assert_eq!(report[0].increment,3.0);
        assert_eq!(cells.iter().map(|c|c.as_ref().unwrap().mass).collect::<Vec<_>>(),vec![4.0,6.0,7.0,1.0]);
    }
    #[test]
    fn absent_resources_are_not_created_and_excess_is_not_removed() {
        let mut cells=vec![cell("Algae",20.0)];
        let report=apply(&mut cells,&[0],&[Setting{element:"Algae".into(),amount:10.0},Setting{element:"Dirt".into(),amount:10.0}]).unwrap();
        assert_eq!(report[0].increment,0.0);assert_eq!(report[1].count,0);
        assert_eq!(cells[0].as_ref().unwrap().mass,20.0);
    }
}
