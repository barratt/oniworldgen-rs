//! Runtime geyser configuration from native template records and global offsets.
use crate::{generate::WorldOutput,template_entities::Output,rng::KRandom,geyser};

#[derive(Debug,serde::Serialize)]
pub struct Geyser {
    #[serde(rename="type")] pub type_id:String,
    pub position:(i32,i32),pub global_position:(i32,i32),pub rolls:[f32;5],pub scaled:[f32;5],
}
#[derive(Debug,serde::Serialize)]
pub struct Runtime {pub cluster_seed:i32,pub geysers:Vec<Geyser>}
impl Runtime {
    /// Preserve the existing seed-search record shape, sourcing positions and
    /// parameters from verified native template records rather than the old
    /// approximate parent-cell placement. Rates use the existing DB convention.
    pub fn summaries(&self)->Vec<crate::generate::GeyserOutput> {
        self.geysers.iter().map(|g|{
            let scaled=geyser::ScaledGeyser {scaled_rate:g.scaled[0],scaled_iter_len:g.scaled[1],scaled_iter_pct:g.scaled[2],scaled_year_len:g.scaled[3],scaled_year_pct:g.scaled[4]};
            crate::generate::GeyserOutput {type_id:g.type_id.clone(),x:g.position.0,y:g.position.1,scaled,
                emit_rate:geyser::db_emit_rate(scaled),avg_emit_rate:geyser::db_avg_emit_rate(scaled)}
        }).collect()
    }
}

pub fn generate(w:&WorldOutput,entities:&Output,cluster_seed:i32,expansion1:bool)->Result<Runtime,String> {
    let mut geysers=Vec::new();
    for p in entities.buildings.iter().chain(&entities.pickupables).chain(&entities.elemental_ores).chain(&entities.backwall_entities).chain(&entities.other_entities) {
        let Some(id)=p.id.as_deref() else{continue};
        let global=(p.location_x.wrapping_add(w.offset.0),p.location_y.wrapping_add(w.offset.1));
        let ty=if id=="GeyserGeneric" {geyser::pick_generic_type(cluster_seed,global.0,global.1,expansion1)}
            else if let Some(ty)=id.strip_prefix("GeyserGeneric_"){ty}else{continue};
        let params=geyser::params_for(ty).ok_or_else(||format!("unsupported runtime geyser {ty}"))?;
        let mut rng=KRandom::new(cluster_seed.wrapping_add(global.0).wrapping_add(global.1));
        let rolls=std::array::from_fn(|_|rng.next_double() as f32);
        let s=geyser::roll(cluster_seed,global.0,global.1,params);
        geysers.push(Geyser {type_id:ty.into(),position:(p.location_x,p.location_y),global_position:global,rolls,
            scaled:[s.scaled_rate,s.scaled_iter_len,s.scaled_iter_pct,s.scaled_year_len,s.scaled_year_pct]});
    }
    geysers.sort_by(|a,b|(a.position.1,a.position.0,&a.type_id).cmp(&(b.position.1,b.position.0,&b.type_id)));
    Ok(Runtime {cluster_seed,geysers})
}
