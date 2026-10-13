//! Foreground material choices and ordered writes. The caller supplies point
//! eligibility so invalid cells and priority claims do not consume RNG.
use crate::{sampler::SeededRandom,terrain::{ElementDefinition,ElementOverrides,MaterialCell,foreground_mass},feature_shapes::Point};
use std::collections::BTreeMap;

#[derive(Debug,Clone,serde::Serialize)]
pub struct FeaturePlan {pub feature:String,pub size:f32,pub shape:String,pub center:Vec<Point>,pub borders:Vec<Vec<Point>>,pub claimed:Vec<Point>}

pub fn plan(region:&crate::subworld::TerrainRegion,ds:&crate::data_source::DataSource,rng:&mut SeededRandom)->Result<Option<FeaturePlan>,String> {
    let name=if ds.features.contains_key(&region.node_type) {Some(region.node_type.as_str())}else{
        let names:Vec<_>=region.tags.iter().filter(|t|ds.features.contains_key(t.as_str())).collect();
        if names.is_empty(){None}else{Some(names[rng.random_range_i(0,names.len() as i32) as usize].as_str())}
    };
    let Some(name)=name else{return Ok(None)};let feature=&ds.features[name];
    let range=feature.blob_size.as_ref().ok_or_else(||format!("missing feature size {name}"))?;
    let mut size=rng.random_range_f(range.min,range.max);
    let mut poly=crate::geom::Polygon::new(region.polygon.iter().map(|&(x,y)|crate::geom::V2::new(x,y)).collect());
    let center=poly.centroid();let n=poly.vertices.len();
    let distance=(0..n).map(|i|crate::terrain::segment_distance(poly.vertices[(i+n-1)%n],poly.vertices[i],center)).fold(f32::MAX,f32::min);
    if !region.tags.iter().any(|t|t=="AllowExceedNodeBorders")&&distance<size {size=distance-6.0;}
    if size<=0.0{return Ok(None);}
    let shape=feature.shape.as_deref().unwrap_or("Circle");
    use crate::feature_shapes as shapes;
    let points=if size<1.0 {Vec::new()}else{match shape {
        "Circle"=>shapes::circle(center,size),"Blob"=>shapes::blob(center,size,rng),"Splat"=>shapes::splat(center,size,rng),
        "Square"=>shapes::rectangle(center,size,size,rng),"TallThin"=>shapes::rectangle(center,size/4.0,size,rng),
        "ShortWide"=>shapes::rectangle(center,size,size/4.0,rng),_=>return Err(format!("unimplemented feature shape {shape}"))
    }};
    let mut seen=crate::fxhash::FxHashSet::default();let mut claimed:Vec<_>=points.iter().copied().filter(|p|seen.insert(*p)).collect();let mut borders=Vec::new();
    if !points.is_empty() {for &width in &feature.borders {
        if width<=0 {break;}
        let border=shapes::border(&claimed,width);
        for &p in &border {if seen.insert(p){claimed.push(p);}}
        borders.push(border);
    }}
    Ok(Some(FeaturePlan {feature:name.into(),size,shape:shape.into(),center:points,borders,claimed}))
}

#[derive(Debug,Clone,Default,serde::Deserialize,serde::Serialize)]
#[serde(default,rename_all="camelCase")]
pub struct Choice {pub element:String,pub weight:f32,pub overrides:Option<ElementOverrides>}
#[derive(Debug,Clone,Default,serde::Deserialize,serde::Serialize)]
#[serde(default,rename_all="camelCase")]
pub struct ChoiceGroup {pub selection_method:String,pub choices:Vec<Choice>}

impl ChoiceGroup {
    pub fn at_percentage(&self,percentage:f32)->Option<&Choice> {
        let mut sum=0.0_f32;
        self.choices.iter().find(|c| {sum+=c.weight;sum>percentage}).or_else(||self.choices.last())
    }
    pub fn choose(&self,rng:&mut SeededRandom)->Option<&Choice> {
        if self.choices.is_empty(){return None;}
        let total=self.choices.iter().fold(0.0_f32,|sum,c|sum+c.weight);
        self.at_percentage(rng.random_value()*total)
    }
}

pub fn material(choice:&Choice,elements:&BTreeMap<String,ElementDefinition>,density:f32,heat:f32,min:f32,range:f32)->Result<MaterialCell,String> {
    let element=crate::terrain::find_element(elements,&choice.element).ok_or_else(||format!("missing feature element {}",choice.element))?;
    let mut cell=element.default_cell();let mut mass_override=false;let mut temperature_override=false;
    cell.apply_disease(choice.overrides.as_ref());
    if let Some(o)=&choice.overrides {
        if let Some(v)=o.mass_override {cell.mass=v;mass_override=true;}
        if let Some(v)=o.mass_multiplier {cell.mass*=v;mass_override=true;}
        if let Some(v)=o.temperature_override {cell.temperature=v;temperature_override=true;}
        if let Some(v)=o.temperature_multiplier {cell.temperature*=v;temperature_override=true;}
    }
    if !mass_override {cell.mass=foreground_mass(cell.mass,density);}
    if !temperature_override {cell.temperature=(min as f64+heat as f64*range as f64) as f32;}
    Ok(cell)
}

pub fn place_group(group:&ChoiceGroup,points:&[Point],rng:&mut SeededRandom,elements:&BTreeMap<String,ElementDefinition>,
    density:&[f32],heat:&[f32],min:f32,range:f32,eligible:impl Fn(Point)->Option<usize>)->Result<Vec<(usize,MaterialCell)>,String> {
    if points.is_empty(){return Ok(Vec::new());}
    let per_cell=group.selection_method=="Weighted"||group.selection_method=="WeightedResample";
    let horizontal=group.selection_method=="HorizontalSlice";
    let fixed=if per_cell||horizontal {None}else{Some(group.choose(rng).ok_or("empty material choices")?)};
    let ymin=points.iter().map(|p|p.1).min().unwrap();let ymax=points.iter().map(|p|p.1).max().unwrap();
    let mut out=Vec::new();
    for &point in points {
        let Some(index)=eligible(point) else {continue};
        let choice=if per_cell {group.choose(rng)}else if horizontal {
            group.at_percentage((1.0-(point.1-ymin) as f64/(ymax-ymin) as f64) as f32)
        }else{fixed}.ok_or("empty material choices")?;
        out.push((index,material(choice,elements,*density.get(index).ok_or("missing density")?,*heat.get(index).ok_or("missing heat")?,min,range)?));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn saved_rng_state_replays_without_advancing_the_original() {
        let mut rng=SeededRandom::new(437);
        for _ in 0..123 {rng.next_double();}
        let state=rng.state();let mut replay=SeededRandom::from_state(&state).unwrap();
        for _ in 0..100 {assert_eq!(rng.next_double().to_bits(),replay.next_double().to_bits());}
        let mut invalid=state.clone();invalid.inext=56;assert!(SeededRandom::from_state(&invalid).is_err());
        invalid=state;invalid.seed_array.pop();assert!(SeededRandom::from_state(&invalid).is_err());
    }
    fn elements()->BTreeMap<String,ElementDefinition> {
        [ElementDefinition {element_id:"Rock".into(),state:"Solid".into(),default_mass:20000.0,default_temperature:100.0,..Default::default()}].into_iter().map(|e|(e.element_id.clone(),e)).collect()
    }
    #[test]
    fn priority_skips_rng_and_duplicates_keep_per_cell_draws() {
        let group=ChoiceGroup {selection_method:"WeightedResample".into(),choices:vec![Choice {element:"Rock".into(),weight:1.0,..Default::default()}]};
        let mut rng=SeededRandom::new(42);let mut expected=SeededRandom::new(42);
        let out=place_group(&group,&[(0,0),(1,0),(0,0)],&mut rng,&elements(),&[0.5,0.5],&[0.5,0.5],200.0,20.0,|p|if p.0==0 {Some(0)}else{None}).unwrap();
        assert_eq!(out.len(),2);assert_eq!(out[0].1.mass,10000.0);assert_eq!(out[0].1.temperature,210.0);
        expected.random_value();expected.random_value();assert_eq!(rng.next_double(),expected.next_double());
    }
    #[test]
    fn explicit_overrides_bypass_density_cap_and_heat() {
        let c=Choice {element:"Rock".into(),overrides:Some(ElementOverrides {mass_multiplier:Some(2.0),temperature_override:Some(123.0),temperature_multiplier:Some(2.0),..Default::default()}),..Default::default()};
        let result=material(&c,&elements(),0.0,1.0,300.0,50.0).unwrap();
        assert_eq!(result.mass,40000.0);assert_eq!(result.temperature,246.0);
    }
    #[test]
    fn horizontal_choices_use_raw_weights_and_strict_thresholds() {
        let g=ChoiceGroup {selection_method:"HorizontalSlice".into(),choices:vec![Choice {element:"A".into(),weight:2.0,..Default::default()},Choice {element:"B".into(),weight:8.0,..Default::default()}]};
        assert_eq!(g.at_percentage(1.0).unwrap().element,"A");
        assert_eq!(g.at_percentage(2.0).unwrap().element,"B");
        assert_eq!(g.at_percentage(f32::NAN).unwrap().element,"B");
    }
}
