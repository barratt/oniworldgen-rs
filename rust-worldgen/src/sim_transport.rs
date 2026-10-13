//! Native serialization of the game's headless simulation protocol (744825).
//! Only static tables come from a game profile. Every per-world message is
//! built from Rust-generated materials, templates, coordinates and dimensions.
use std::collections::BTreeMap;
use serde::{Deserialize,Serialize};
use crate::{terrain::MaterialCell,template_cells::Write};

pub const INITIALIZE_CELLS:i32=2062421945;
pub const PREPARE:i32=1078620451;
pub const MODIFY_CELL:i32=-1252920804;
pub const BACKWALL:i32=-605421515;

#[derive(Deserialize)]
pub struct Profile {
    #[serde(rename="schemaVersion")] pub schema_version:u32,
    #[serde(rename="gameVersion")] pub game_version:String,
    #[serde(rename="activeDlcIds")] pub active_dlc_ids:Vec<String>,
    pub radiation:bool,pub mxcsr:u32,
    pub element_table:BTreeMap<String,String>,pub disease_table:BTreeMap<String,String>,
    pub elements_payload:String,pub diseases_payload:String,
}
#[derive(Deserialize)]
pub struct InputWorld {
    pub name:String,pub width:i32,pub height:i32,pub world_seed:i32,pub offset:(i32,i32),
    pub terrain_after_starting_adjustments:Vec<Option<MaterialCell>>,
    pub backwall_cells:Vec<MaterialCell>,pub template_cell_writes:Vec<Write>,
    /// The world border's cells: `RenderOffline` resets their managed backwall
    /// to Vacuum (mass 0, temperature 0) after the terrain is rendered, so the
    /// sim never sees a backwall under the border. Absent in older predictions.
    #[serde(default)] pub world_border:Option<BorderCells>,
}
#[derive(Deserialize,Default)]
pub struct BorderCells {pub cells:Vec<usize>}
#[derive(Deserialize)]
pub struct Prediction {
    pub source:String,pub coordinate:String,
    #[serde(rename="gameVersion")] pub game_version:String,
    /// Explicit active DLC set of the prediction (absent/null when it was inferred).
    #[serde(rename="activeDlcIds",default)] pub active_dlc_ids:Option<Vec<String>>,
    pub worlds:Vec<InputWorld>,
}
#[derive(Debug,Serialize)]
pub struct Message {pub frame:u16,pub id:i32,pub payload:String,pub mxcsr:u32}
#[derive(Serialize)]
pub struct World {
    pub name:String,pub width:i32,pub height:i32,pub world_seed:i32,pub sim_seed:u32,
    pub save_x:i32,pub save_y:i32,pub initialize_mxcsr:u32,pub frame_mxcsr:Vec<u32>,
    pub messages:Vec<Message>,pub frame_message:String,pub prepare_message:String,pub prepare_id:i32,
}
#[derive(Serialize)]
pub struct Transport {
    #[serde(rename="schemaVersion")] pub schema_version:u32,
    pub source:&'static str,pub phase:&'static str,pub coordinate:String,
    #[serde(rename="gameVersion")] pub game_version:String,
    #[serde(rename="activeDlcIds")] pub active_dlc_ids:Vec<String>,
    pub worlds:Vec<World>,
}
pub fn decode_hex(s:&str)->Result<Vec<u8>,String>{
    if s.len()%2!=0{return Err("invalid hex length".into());}
    s.as_bytes().chunks_exact(2).map(|p|{
        let digit=|c|match c {b'0'..=b'9'=>Some(c-b'0'),b'a'..=b'f'=>Some(c-b'a'+10),b'A'..=b'F'=>Some(c-b'A'+10),_=>None};
        Ok(digit(p[0]).ok_or("invalid hex digit")?*16+digit(p[1]).ok_or("invalid hex digit")?)
    }).collect()
}
pub fn encode_hex(bytes:&[u8])->String {
    const DIGITS:&[u8]=b"0123456789ABCDEF";
    let mut out=String::with_capacity(bytes.len()*2);
    for b in bytes {out.push(DIGITS[(b>>4) as usize] as char);out.push(DIGITS[(b&15) as usize] as char);}
    out
}
#[derive(Clone,Copy)]
struct Element {index:u16,max_mass:f32,default_temperature:f32}
struct Tables {elements:BTreeMap<String,Element>,diseases:BTreeMap<String,u8>}
fn tables(profile:&Profile)->Result<Tables,String>{
    if profile.schema_version!=1||profile.game_version!="744825" {return Err("unsupported simulation profile schema/build".into());}
    if profile.mxcsr!=0x1f80{return Err("unverified simulation floating-point control mode".into());}
    let bytes=decode_hex(&profile.elements_payload)?;
    let count=u32::from_le_bytes(bytes.get(..4).ok_or("truncated element table")?.try_into().unwrap()) as usize;
    // Sim.Element.Write uses 164 serialized bytes. The MemoryStream buffer can
    // also contain names and unused capacity after these records.
    if count==0||count>=65535||bytes.len()<4+count*164||profile.element_table.len()!=count{return Err("invalid static element table".into());}
    let mut elements=BTreeMap::new();
    for index in 0..count {
        let name=profile.element_table.get(&index.to_string()).ok_or("noncontiguous element indices")?;
        let record=&bytes[4+index*164..4+(index+1)*164];
        let hash=u32::from_le_bytes(record[..4].try_into().unwrap());
        let actual_index=u16::from_le_bytes(record[8..10].try_into().unwrap());
        if actual_index as usize!=index||hash!=crate::hash::sdbm_lower(name){return Err(format!("element profile identity differs: {name}"));}
        let max_mass=f32::from_le_bytes(record[52..56].try_into().unwrap());
        let default_temperature=f32::from_le_bytes(record[152..156].try_into().unwrap());
        if !max_mass.is_finite()||!default_temperature.is_finite(){return Err("nonfinite static element property".into());}
        if elements.insert(name.clone(),Element{index:actual_index,max_mass,default_temperature}).is_some(){return Err("duplicate element name".into());}
    }
    let disease_bytes=decode_hex(&profile.diseases_payload)?;
    if disease_bytes.len()<8{return Err("truncated disease table".into());}
    let diseases_count=u32::from_le_bytes(disease_bytes[..4].try_into().unwrap()) as usize;
    if diseases_count>=255||diseases_count!=profile.disease_table.len()||u32::from_le_bytes(disease_bytes[4..8].try_into().unwrap()) as usize!=count{return Err("disease table dimensions differ".into());}
    let mut diseases=BTreeMap::new();
    for index in 0..diseases_count {
        let name=profile.disease_table.get(&index.to_string()).ok_or("noncontiguous disease indices")?;
        if diseases.insert(name.clone(),index as u8).is_some(){return Err("duplicate disease name".into());}
    }
    Ok(Tables{elements,diseases})
}
impl Tables {
    fn element(&self,name:&str)->Result<Element,String>{self.elements.get(name).copied().ok_or_else(||format!("unknown simulation element {name}"))}
    fn disease(&self,name:Option<&str>)->Result<u8,String>{match name {None|Some("")=>Ok(255),Some(s)=>self.diseases.get(s).copied().ok_or_else(||format!("unknown simulation disease {s}"))}}
}
fn finite(v:f32)->Result<(),String>{if v.is_finite(){Ok(())}else{Err("nonfinite simulation input".into())}}
fn initial_cells(w:&InputWorld,t:&Tables,seed:u32,radiation:bool,count:usize)->Result<Vec<u8>,String>{
    if w.terrain_after_starting_adjustments.len()!=count||w.backwall_cells.len()!=count{return Err("incomplete simulation grid".into());}
    let mut bytes=Vec::with_capacity(20+count*40);
    bytes.extend(w.width.to_le_bytes());bytes.extend(w.height.to_le_bytes());bytes.extend(seed.to_le_bytes());bytes.extend([radiation as u8,1]);
    for c in &w.terrain_after_starting_adjustments {
        let c=c.as_ref().ok_or("missing foreground cell")?;finite(c.mass)?;finite(c.temperature)?;
        bytes.extend(t.element(&c.element)?.index.to_le_bytes());
        // Cell.Write clears properties/strength; SetValues sets insulation=255.
        bytes.extend([0,255,0,0,0,0]);bytes.extend(c.temperature.to_le_bytes());bytes.extend(c.mass.to_le_bytes());
    }
    for c in &w.terrain_after_starting_adjustments {
        let c=c.as_ref().unwrap();bytes.extend([t.disease(c.disease.as_deref())?,0,0,0]);bytes.extend(c.disease_count.to_le_bytes());bytes.extend([0;4]);
    }
    let mut border=vec![false;count];
    for &c in w.world_border.iter().flat_map(|b|&b.cells) {*border.get_mut(c).ok_or("world border cell outside the grid")?=true;}
    let vacuum=MaterialCell{element:"Vacuum".into(),..Default::default()};
    for (c,&on_border) in w.backwall_cells.iter().zip(&border) {
        let c=if on_border {&vacuum} else {c};
        finite(c.mass)?;finite(c.temperature)?;bytes.extend(t.element(&c.element)?.index.to_le_bytes());bytes.extend([0;2]);
        bytes.extend(c.mass.to_le_bytes());bytes.extend(c.temperature.to_le_bytes());
    }
    // Marshal.SizeOf(bool)=4 determines capacity, but BinaryWriter writes one
    // byte. SimDataInitializeFromCells sends GetBuffer(), including six zeros.
    bytes.extend([0;6]);Ok(bytes)
}
fn template_messages(c:&Write,t:&Tables)->Result<(Vec<u8>,Vec<u8>),String>{
    let element=t.element(&c.element)?;
    for v in [c.mass,c.temperature,c.backwall_mass,c.backwall_temperature]{finite(v)?;}
    let mass=if element.max_mass==0.0&&c.mass>0.0{0.0}else{c.mass};
    let temperature=if c.temperature<0.0||c.temperature>10000.0||(c.temperature==0.0&&mass>0.0){element.default_temperature}else{c.temperature};
    let index=i32::try_from(c.cell).map_err(|_|"template cell index overflow")?;
    let mut foreground=Vec::with_capacity(28);
    foreground.extend(index.to_le_bytes());foreground.extend((-1_i32).to_le_bytes());foreground.extend(temperature.to_le_bytes());foreground.extend(mass.to_le_bytes());
    foreground.extend(c.disease_count.to_le_bytes());foreground.extend(element.index.to_le_bytes());foreground.extend([1,t.disease(c.disease.as_deref())?,1,0,0,0]);
    let backwall_element=match c.backwall_element.as_deref(){Some(name)=>t.element(name)?.index,None=>65535};
    let mut backwall=Vec::with_capacity(16);backwall.extend(index.to_le_bytes());backwall.extend(backwall_element.to_le_bytes());backwall.extend([0;2]);
    backwall.extend(c.backwall_mass.to_le_bytes());backwall.extend(c.backwall_temperature.to_le_bytes());Ok((foreground,backwall))
}
pub fn build(profile:&Profile,prediction:&Prediction)->Result<Transport,String>{
    let t=tables(profile)?;
    if prediction.source!="rust-native"||prediction.game_version!=profile.game_version{return Err("prediction/profile provenance mismatch".into());}
    let coord=crate::coord::Coord::parse(&prediction.coordinate).ok_or("invalid coordinate")?;
    // Every per-world input is read from the prediction, which already holds
    // story templates, mixing-substituted worlds and subworlds. Quality settings
    // never change cells or settle inputs (Teleporters only removes entities).
    if coord.seed<0||coord.canonical()!=prediction.coordinate{return Err("unsupported simulation coordinate".into());}
    // The element table, its payload and the radiation flag depend on the
    // active DLC set (ElementLoader, FeatureRadiationEnabled).
    if let Some(ids)=&prediction.active_dlc_ids {
        let mut a=ids.clone();a.sort();let mut b=profile.active_dlc_ids.clone();b.sort();
        if a!=b{return Err(format!("prediction active DLC {ids:?} does not match profile {:?}",profile.active_dlc_ids));}
    }
    if prediction.worlds.is_empty(){return Err("empty prediction".into());}
    let mut worlds=Vec::new();let mut names=std::collections::HashSet::new();
    for w in &prediction.worlds {
        if !names.insert(&w.name){return Err("duplicate world".into());}
        let count=usize::try_from(w.width).ok().and_then(|x|usize::try_from(w.height).ok().and_then(|y|x.checked_mul(y))).ok_or("invalid grid dimensions")?;
        if count==0||count>10_000_000{return Err("unsupported grid size".into());}
        let msg=|frame,id,bytes:&[u8]|Message {frame,id,payload:encode_hex(bytes),mxcsr:profile.mxcsr};
        let mut messages=vec![msg(0,1108437482,&decode_hex(&profile.elements_payload)?),msg(0,825301935,&decode_hex(&profile.diseases_payload)?),
            msg(0,INITIALIZE_CELLS,&initial_cells(w,&t,coord.seed as u32,profile.radiation,count)?),msg(0,-931446686,&[])];
        let mut claimed=std::collections::HashSet::new();
        for c in &w.template_cell_writes {
            if c.cell>=count||!claimed.insert(c.cell){return Err("invalid or repeated template cell write".into());}
            let (foreground,backwall)=template_messages(c,&t)?;
            messages.push(msg(498,MODIFY_CELL,&foreground));messages.push(msg(498,BACKWALL,&backwall));
        }
        let mut frame=Vec::new();frame.extend(0.2_f32.to_le_bytes());
        for n in [0,0,w.width,w.height-1]{frame.extend(n.to_le_bytes());}
        // Game.SimActiveRegion's constructor uses FIXEDTRAITS defaults during
        // settling (not the world's eventual runtime sunlight/radiation traits).
        frame.extend(80000.0_f32.to_le_bytes());frame.extend(250.0_f32.to_le_bytes());
        worlds.push(World{name:w.name.clone(),width:w.width,height:w.height,world_seed:w.world_seed,sim_seed:coord.seed as u32,
            save_x:w.offset.0,save_y:w.offset.1,initialize_mxcsr:profile.mxcsr,frame_mxcsr:vec![profile.mxcsr;500],messages,
            frame_message:encode_hex(&frame),prepare_message:encode_hex(&vec![255;count]),prepare_id:PREPARE});
    }
    Ok(Transport{schema_version:1,source:"rust-native-inputs",phase:"rust-sim-transport",coordinate:prediction.coordinate.clone(),game_version:profile.game_version.clone(),
        active_dlc_ids:profile.active_dlc_ids.clone(),worlds})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn normalization_and_invalid_backwall_follow_managed_protocol(){
        let mut t=Tables{elements:BTreeMap::new(),diseases:BTreeMap::new()};
        t.elements.insert("Vacuum".into(),Element{index:211,max_mass:0.0,default_temperature:0.0});
        let c=Write{cell:19,element:"Vacuum".into(),mass:5.0,temperature:-1.0,disease:None,disease_count:0,backwall_element:None,backwall_mass:0.0,backwall_temperature:0.0};
        let (a,b)=template_messages(&c,&t).unwrap();
        assert_eq!(a,decode_hex("13000000FFFFFFFF000000000000000000000000D30001FF01000000").unwrap());
        assert_eq!(b,decode_hex("13000000FFFF00000000000000000000").unwrap());
    }
}
