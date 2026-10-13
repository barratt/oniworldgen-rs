//! Numerical operations used while rasterizing terrain. Region orchestration
//! and border passes live in terrain_render, biome_borders and world_border.

use crate::geom::{Polygon,V2};
use crate::name::Name;

#[derive(Debug,Clone,serde::Serialize)]
pub struct BiomeEdge {
    pub parents:(usize,usize), pub start:(f32,f32),pub end:(f32,f32),pub closed:bool,
}

/// FlattenOverworld: preserve neighbor traversal and first matching corner.
/// Graph corners within one unit in both axes reuse the first inserted point.
pub fn biome_edges(cells:&[crate::generate::BiomeCell],neighbours:&[Vec<usize>],
    ds:&crate::data_source::DataSource)->Result<Vec<BiomeEdge>,String> {
    let polygons:Vec<_>=cells.iter().map(|c|Polygon::new(c.polygon.iter().map(|&(x,y)|V2::new(x,y)).collect())).collect();
    let mut corners:Vec<V2>=Vec::new();let mut pairs=std::collections::BTreeSet::new();let mut edges=Vec::new();
    let mut corner=|p:V2| {
        if let Some(&v)=corners.iter().find(|v|(v.x-p.x).abs()<1.0&&(v.y-p.y).abs()<1.0) {v}
        else {corners.push(p);p}
    };
    for (i,neighbors) in neighbours.iter().enumerate() {
        for &j in neighbors {
            if pairs.contains(&(i.min(j),i.max(j))) {continue;}
            let (kind,_,segment)=polygons[i].shares_edge(&polygons[j]);
            if kind!=crate::geom::Commonality::Edge {continue;}
            let segment=segment.ok_or("missing shared segment")?;
            let a=corner(segment.p0.ok_or("missing segment start")?);
            let b=corner(segment.p1.ok_or("missing segment end")?);
            let sub=|k:usize|cells[k].subworld.as_ref().and_then(|name|ds.subworlds.get(name)).ok_or("missing edge subworld");
            let x=sub(i)?;let y=sub(j)?;
            let closed=x.qname!=y.qname&&x.zone_type!=y.zone_type&&x.border_override.as_deref()!=Some("NONE")&&y.border_override.as_deref()!=Some("NONE");
            edges.push(BiomeEdge {parents:(i,j),start:(a.x,a.y),end:(b.x,b.y),closed});
            pairs.insert((i.min(j),i.max(j)));
        }
    }
    Ok(edges)
}

fn unity_distance(a:V2,b:V2)->f32 {
    let x=a.x-b.x;let y=a.y-b.y;
    ((x as f64*x as f64+y as f64*y as f64).sqrt()) as f32
}

pub fn segment_distance(a:V2,b:V2,p:V2)->f32 {
    let dx=b.x as f64-a.x as f64;let dy=b.y as f64-a.y as f64;
    let length=(dx*dx+dy*dy) as f32;
    if length<=0.0 {return unity_distance(a,p);}
    let dot=((p.x as f64-a.x as f64)*dx+(p.y as f64-a.y as f64)*dy) as f32;
    let time=(dot/length).clamp(0.0,1.0);
    unity_distance(a.add(b.sub(a).mul(time)),p)
}

/// TerrainCell's sequential erosion rules. Closed graph edges must be supplied
/// when border erosion is requested; missing topology is never approximated.
pub fn erosion<T:AsRef<str>>(poly:&Polygon, point:V2, tags:&[T], width:f32,height:f32,
    closed_edges:Option<&[(V2,V2)]>)->Result<f32,String> {
    erosion_with(poly,poly.clone().centroid(),&ErosionTags::new(tags),point,width,height,closed_edges)
}

/// The erosion tags of a region, read once for all of its cells.
#[derive(Debug,Clone,Copy,Default)]
pub struct ErosionTags {
    to_centroid:bool, to_centroid_inv:bool, point_edge:bool, to_edge:bool, to_edge_inv:bool, point_centroid:bool,
    to_border:bool, to_border_inv:bool, border_weak:bool, world_top:bool, world_top_or_side:bool,
}

impl ErosionTags {
    pub fn new<T:AsRef<str>>(tags:&[T])->Self {
        let has=|s:&str|tags.iter().any(|t|t.as_ref()==s);
        Self {to_centroid:has("ErodePointToCentroid"),to_centroid_inv:has("ErodePointToCentroidInv"),point_edge:has("DistFunctionPointEdge"),
            to_edge:has("ErodePointToEdge"),to_edge_inv:has("ErodePointToEdgeInv"),point_centroid:has("DistFunctionPointCentroid"),
            to_border:has("ErodePointToBorder"),to_border_inv:has("ErodePointToBorderInv"),border_weak:has("ErodePointToBorderWeak"),
            world_top:has("ErodePointToWorldTop"),world_top_or_side:has("ErodePointToWorldTopOrSide")}
    }
}

/// [`erosion`] with the polygon's centroid and tags already resolved.
pub fn erosion_with(poly:&Polygon, center:V2, t:&ErosionTags, point:V2, width:f32,height:f32,
    closed_edges:Option<&[(V2,V2)]>)->Result<f32,String> {
    let centroid_distance=unity_distance(center,point);
    let edge_distance=|| {
        let n=poly.vertices.len();
        (0..n).map(|i|segment_distance(poly.vertices[(i+n-1)%n],poly.vertices[i],point)).fold(f32::MAX,f32::min)
    };
    let mut value=1.0;
    if t.to_centroid||t.to_centroid_inv {
        value=(centroid_distance/if t.point_edge {edge_distance()} else {15.0}).clamp(0.0,1.0);
        if t.to_centroid_inv {value=1.0-value;}
    }
    if t.to_edge||t.to_edge_inv {
        value=(edge_distance()/if t.point_centroid {centroid_distance} else {15.0}).clamp(0.0,1.0);
        if t.to_edge_inv {value=1.0-value;}
    }
    if t.to_border||t.to_border_inv {
        let edges=closed_edges.ok_or("border erosion requires closed overworld edges")?;
        let distance=edges.iter().map(|&(a,b)|segment_distance(a,b,point)).fold(f32::MAX,f32::min);
        value=(distance/if t.point_centroid {centroid_distance} else if t.border_weak {7.0} else {20.0}).clamp(0.0,1.0);
        if t.to_border_inv {value=1.0-value;}
    }
    if t.world_top {value=((height-point.y-38.0)/20.0).clamp(0.0,1.0);}
    if t.world_top_or_side {value=((height-point.y).min(point.x).min(width-point.x)-2.0)/8.0;value=value.clamp(0.0,1.0);}
    Ok(value)
}

#[derive(Debug, Clone, Default, serde::Deserialize, serde::Serialize)]
#[serde(default, rename_all="camelCase")]
pub struct ElementDefinition {
    pub element_id: String,
    pub default_mass: f32,
    pub default_temperature: f32,
    pub low_temp: Option<f32>,
    pub low_temp_transition_target: Option<String>,
    pub state: String,
    pub material_category: String,
    pub tags: Vec<String>,
    pub dlc_id: String,
    /// `element_id`, interned for cells.
    #[serde(skip)]
    pub name: std::sync::OnceLock<Name>,
}

#[derive(Debug, Clone, Copy, Default, serde::Serialize,serde::Deserialize)]
pub struct MaterialCell { pub element: Name, pub mass: f32, pub temperature: f32,
    pub disease: Option<Name>, pub disease_count:i32 }

impl MaterialCell {
    pub fn apply_disease(&mut self,overrides:Option<&ElementOverrides>) {
        if let Some(o)=overrides {
            self.disease=o.disease_override.as_deref().map(Name::new);
            self.disease_count=o.disease_amount_override.unwrap_or(0);
        }
    }
}

impl ElementDefinition {
    pub fn default_cell(&self) -> MaterialCell {
        MaterialCell {element:*self.name.get_or_init(||Name::new(&self.element_id)),mass:if self.state=="Gas" {1.0} else {self.default_mass},temperature:self.default_temperature,..Default::default()}
    }
}

/// `ElementLoader.FindElementByName`: element ids hash lowercased, so worldgen
/// YAML may name an element in any case (e.g. `dirt` in DLC4 features).
pub fn find_element<'a>(elements: &'a std::collections::BTreeMap<String,ElementDefinition>, name: &str) -> Option<&'a ElementDefinition> {
    elements.get(name).or_else(|| elements.values().find(|e| e.element_id.eq_ignore_ascii_case(name)))
}

/// Background material operation after erosion and biome selection. Foreground
/// features and world borders are separate stages and must not use this kernel.
pub fn background_cell(elements: &std::collections::BTreeMap<String,ElementDefinition>,
    band: Option<&ElementBand>, density: f32, heat: f32, temperature_min: f32,
    temperature_range: f32, cave_override: f32, ignore_cave: bool, cave_sliver: bool,
    cave_max: f32, sliver_max: f32) -> Result<MaterialCell,String> {
    let find=|name:&str|find_element(elements,name).ok_or_else(||format!("missing element {name}"));
    if !ignore_cave && cave_override>=100.0 {
        return Ok(find(if cave_override>=300.0 {"Void"} else if cave_override>=200.0 {"Unobtanium"} else {"Katairite"})?.default_cell());
    }
    let name=match band {Some(b)=>b.content.as_deref().ok_or("null material in selected biome band")?,None=>"Void"};
    let element=find(name)?;
    let mut cell=element.default_cell();
    cell.apply_disease(band.and_then(|b|b.overrides.as_ref()));
    if let Some(overrides)=band.and_then(|b|b.overrides.as_ref()) {
        if let Some(m)=overrides.mass_override {cell.mass=m;}
        if let Some(m)=overrides.mass_multiplier {cell.mass*=m;}
        if let Some(t)=overrides.temperature_override {cell.temperature=t;}
        if let Some(t)=overrides.temperature_multiplier {cell.temperature*=t;}
    }
    cell.mass=background_mass(cell.mass,density);
    if element.state!="Vacuum" && element.element_id!="Katairite" && element.element_id!="Unobtanium" {
        let mut min=temperature_min;
        if element.low_temp_transition_target.as_ref().is_some_and(|n|elements.contains_key(n)) {
            min=min.max(element.low_temp.unwrap_or(0.0));
        }
        cell.temperature=(min as f64+heat as f64*temperature_range as f64) as f32;
    }
    if element.state=="Solid" && !ignore_cave && cave_override>cave_max && cave_override<100.0 {
        let disease=(cell.disease,cell.disease_count);
        cell=find(if cave_sliver && cave_override>sliver_max {"Void"} else {"Vacuum"})?.default_cell();
        cell.disease=disease.0;cell.disease_count=disease.1;
    }
    Ok(cell)
}

#[derive(Debug, Clone, Default, serde::Deserialize, serde::Serialize)]
#[serde(default, rename_all="camelCase")]
pub struct ElementOverrides {
    pub mass_override: Option<f32>,
    pub mass_multiplier: Option<f32>,
    pub temperature_override: Option<f32>,
    pub temperature_multiplier: Option<f32>,
    pub disease_override: Option<String>,
    pub disease_amount_override: Option<i32>,
}

/// Backwalls only write solid materials. Their heat rule also applies to
/// abyssalite/unobtanium, unlike foreground/background terrain materials.
pub fn backwall_cell(elements:&std::collections::BTreeMap<String,ElementDefinition>,band:Option<&ElementBand>,
    density:f32,heat:f32,min:f32,range:f32)->Result<Option<MaterialCell>,String> {
    let name=match band {Some(b)=>b.content.as_deref().ok_or("null backwall material")?,None=>"Void"};
    let def=find_element(elements,name).ok_or_else(||format!("missing backwall element {name}"))?;
    if def.state!="Solid" {return Ok(None);}
    let mut cell=def.default_cell();
    if let Some(o)=band.and_then(|b|b.overrides.as_ref()) {
        if let Some(v)=o.mass_override{cell.mass=v;}if let Some(v)=o.mass_multiplier{cell.mass*=v;}
    }
    cell.mass=background_mass(cell.mass,density);
    let min=if def.low_temp_transition_target.as_ref().is_some_and(|t|elements.contains_key(t)) {min.max(def.low_temp.unwrap_or(0.0))}else{min};
    cell.temperature=(min as f64+heat as f64*range as f64) as f32;
    Ok(Some(cell))
}

#[derive(Debug, Clone, Default, serde::Deserialize, serde::Serialize)]
#[serde(default, rename_all="camelCase")]
pub struct ElementBand {
    pub content: Option<String>,
    pub band_size: f32,
    pub max_value: f32,
    pub overrides: Option<ElementOverrides>,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
#[serde(rename_all="camelCase")]
pub struct BandModifier {
    pub element: String,
    pub band_multiplier: f32,
    pub mass_multiplier: f32,
}

/// Apply traits before normalization, in the same order as MutatedWorldData.
pub fn resolve_bands(source: &[ElementBand], modifiers: &[BandModifier]) -> Vec<ElementBand> {
    let mut bands=source.to_vec();
    for band in &mut bands {
        for modifier in modifiers.iter().filter(|m|Some(m.element.as_str())==band.content.as_deref()) {
            band.band_size *= modifier.band_multiplier;
            let overrides=band.overrides.get_or_insert_with(Default::default);
            overrides.mass_multiplier=Some(overrides.mass_multiplier.unwrap_or(1.0)*modifier.mass_multiplier);
        }
    }
    let total=bands.iter().fold(0.0_f32,|sum,b|sum+b.band_size);
    let mut previous=0.0_f32;
    for band in &mut bands {
        // Mono keeps the division in extended precision until the property store.
        band.max_value=(previous as f64+band.band_size as f64/total as f64) as f32;
        previous=band.max_value;
    }
    bands
}

/// Equality advances to the next band; values beyond the last threshold use
/// the final band. An empty table is the game's Void fallback.
pub fn select_band(bands: &[ElementBand], value: f32) -> Option<&ElementBand> {
    bands.iter().find(|b|value<b.max_value).or_else(||bands.last())
}

/// TerrainCell.GetDensityMassForCell, used by foreground features. Mono rounds
/// the variation local before evaluating the final compound expression.
pub fn foreground_mass(mass: f32, density: f32) -> f32 {
    let variation = (0.2_f32 as f64 * (density as f64 - 0.5)) as f32;
    let result = (mass as f64 + mass as f64 * variation as f64) as f32;
    result.min(10_000.0)
}

/// Background terrain uses a single expression and does not apply the
/// foreground's 10,000 kg cap. Keep these operations separate.
pub fn background_mass(mass: f32, density: f32) -> f32 {
    (mass as f64 + mass as f64 * 0.2_f32 as f64 * (density as f64 - 0.5)) as f32
}

/// Assign integer grid locations in terrain-region order. Shared boundary
/// locations belong to the first region that claims them, as in InitializeCells.
pub fn claim_cells(polygons: &[crate::geom::Polygon], width: usize, height: usize) -> Result<Vec<Vec<usize>>,String> {
    let count = width.checked_mul(height).ok_or("terrain grid size overflow")?;
    let mut claimed = vec![false; count];
    let mut result=raster_points(polygons,width,height)?;
    for cells in &mut result {cells.retain(|&index| {let fresh=!claimed[index];claimed[index]=true;fresh});}
    Ok(result)
}

/// Available terrain points are not restricted to first-claim ownership.
/// Later regions can overwrite shared locations while drawing backgrounds.
pub fn raster_points(polygons: &[crate::geom::Polygon], width: usize, height: usize) -> Result<Vec<Vec<usize>>,String> {
    width.checked_mul(height).ok_or("terrain grid size overflow")?;
    let mut result = Vec::with_capacity(polygons.len());
    for poly in polygons {
        let b = poly.bounds;
        let mut cells = Vec::new();
        let mut y = b.y as i32;
        while (y as f64) < b.y as f64 + b.height as f64 {
            let mut x = b.x as i32;
            while (x as f64) < b.x as f64 + b.width as f64 {
                if poly.contains(crate::geom::V2::new(x as f32,y as f32)) {
                    if x < 0 || y < 0 || x as usize >= width || y as usize >= height { return Err("terrain point outside grid".into()); }
                    let index=x as usize + y as usize*width;
                    cells.push(index);
                }
                x+=1;
            }
            y+=1;
        }
        result.push(cells);
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn erosion_requires_border_topology_and_applies_world_edge_rule_last() {
        let polygon=Polygon::from_rect(crate::geom::Rect::new(0.0,0.0,20.0,20.0));
        let tags=vec!["ErodePointToBorder"];
        assert!(erosion(&polygon,V2::new(5.0,5.0),&tags,20.0,20.0,None).is_err());
        let tags=vec!["ErodePointToCentroid","ErodePointToWorldTopOrSide"];
        assert_eq!(erosion(&polygon,V2::new(6.0,6.0),&tags,20.0,20.0,None).unwrap(),0.5);
    }
    #[test]
    fn rendering_points_preserve_overlap_while_claims_have_one_owner() {
        use crate::geom::{Polygon,Rect};
        let p=Polygon::from_rect(Rect::new(0.0,0.0,2.0,2.0));
        let polygons=vec![p.clone(),p];
        let points=raster_points(&polygons,4,4).unwrap();
        let claims=claim_cells(&polygons,4,4).unwrap();
        assert!(!points[0].is_empty());assert_eq!(points[0],points[1]);
        assert_eq!(claims[0],points[0]);assert!(claims[1].is_empty());
    }
    #[test]
    fn background_overrides_temperature_limits_and_cave_precedence() {
        let mut elements=std::collections::BTreeMap::new();
        for (name,state,mass,temp) in [("Vacuum","Vacuum",0.0,0.0),("Void","Vacuum",0.0,0.0),
            ("Katairite","Solid",500.0,290.0),("Unobtanium","Solid",20000.0,1.0),
            ("Water","Liquid",1000.0,300.0),("Oxygen","Gas",99.0,310.0),("Rock","Solid",1000.0,290.0)] {
            elements.insert(name.into(),ElementDefinition {element_id:name.into(),state:state.into(),default_mass:mass,default_temperature:temp,..Default::default()});
        }
        elements.get_mut("Water").unwrap().low_temp=Some(273.15);
        elements.get_mut("Water").unwrap().low_temp_transition_target=Some("Rock".into());
        let band=|name:&str|ElementBand {content:Some(name.into()),..Default::default()};
        let cell=|b:&ElementBand,over,ignore,sliver|background_cell(&elements,Some(b),0.5,0.25,260.0,20.0,over,ignore,sliver,0.65,0.97).unwrap();
        assert_eq!(cell(&band("Oxygen"),0.0,false,false).mass,1.0);
        assert_eq!(cell(&band("Water"),0.0,false,false).temperature,278.15);
        assert_eq!(cell(&band("Rock"),0.8,false,false).element,"Vacuum");
        assert_eq!(cell(&band("Rock"),0.98,false,true).element,"Void");
        assert_eq!(cell(&band("Rock"),0.98,true,true).element,"Rock");
        for (v,name) in [(100.0,"Katairite"),(200.0,"Unobtanium"),(300.0,"Void")] {
            let c=cell(&band("Water"),v,false,false);assert_eq!(c.element,name);assert_eq!(c.mass,elements[name].default_mass);
        }
        let mut b=band("Rock");b.overrides=Some(ElementOverrides {mass_override:Some(350.0),mass_multiplier:Some(2.0),temperature_override:Some(999.0),..Default::default()});
        let c=cell(&b,0.0,false,false);assert_eq!(c.mass,700.0);assert_eq!(c.temperature,265.0);
    }

    #[test]
    fn cave_material_replacement_preserves_germs_but_hard_override_clears_them() {
        let elements=[("Rock","Solid"),("Vacuum","Vacuum"),("Katairite","Solid")].into_iter().map(|(name,state)|
            (name.into(),ElementDefinition {element_id:name.into(),state:state.into(),..Default::default()})).collect();
        let band=ElementBand {content:Some("Rock".into()),overrides:Some(ElementOverrides {
            disease_override:Some("SlimeLung".into()),disease_amount_override:Some(12000),..Default::default()}),..Default::default()};
        let cave=background_cell(&elements,Some(&band),0.5,0.5,280.0,20.0,0.8,false,false,0.65,0.97).unwrap();
        assert_eq!(cave.element,"Vacuum");assert_eq!(cave.disease.as_deref(),Some("SlimeLung"));assert_eq!(cave.disease_count,12000);
        let hard=background_cell(&elements,Some(&band),0.5,0.5,280.0,20.0,100.0,false,false,0.65,0.97).unwrap();
        assert_eq!(hard.element,"Katairite");assert_eq!(hard.disease,None);assert_eq!(hard.disease_count,0);
    }

    #[test]
    fn backwall_filters_nonsolids_and_heats_abyssalite() {
        let elements=[("Katairite","Solid",500.0),("Oxygen","Gas",1.0)].into_iter().map(|(name,state,mass)|
            (name.into(),ElementDefinition {element_id:name.into(),state:state.into(),default_mass:mass,default_temperature:300.0,..Default::default()})).collect();
        let mut band=ElementBand {content:Some("Katairite".into()),..Default::default()};
        let cell=backwall_cell(&elements,Some(&band),0.5,0.5,200.0,20.0).unwrap().unwrap();
        assert_eq!(cell.temperature,210.0);assert_eq!(cell.mass,500.0);
        band.content=Some("Oxygen".into());assert!(backwall_cell(&elements,Some(&band),0.5,0.5,200.0,20.0).unwrap().is_none());
    }

    #[test]
    fn trait_band_changes_compose_before_normalization() {
        let input:Vec<ElementBand>=serde_json::from_value(serde_json::json!([
            {"content":"Cuprite","bandSize":1,"overrides":{"massOverride":350,"massMultiplier":3}},
            {"content":"Dirt","bandSize":2}
        ])).unwrap();
        let modifiers:Vec<BandModifier>=serde_json::from_value(serde_json::json!([
            {"element":"Cuprite","bandMultiplier":2,"massMultiplier":2},
            {"element":"Cuprite","bandMultiplier":0.5,"massMultiplier":0.25}
        ])).unwrap();
        let bands=resolve_bands(&input,&modifiers);
        assert_eq!(bands[0].band_size,1.0);
        assert_eq!(bands[0].max_value,1.0_f32/3.0);
        let overrides=bands[0].overrides.as_ref().unwrap();
        assert_eq!(overrides.mass_multiplier,Some(1.5));
        assert_eq!(overrides.mass_override,Some(350.0));
        assert_eq!(input[0].overrides.as_ref().unwrap().mass_multiplier,Some(3.0));
    }
}
