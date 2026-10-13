//! Count selection after ambient candidate shuffling (ONI 744825).
use crate::sampler::SeededRandom;
#[derive(Debug)]
pub struct Roll {pub sample:f32,pub count:i32}
pub fn roll(rng:&mut SeededRandom,min:f32,max:f32,candidates:usize,biome:bool,multiplier:f32)->Result<Option<Roll>,String>{
    if !min.is_finite()||!max.is_finite()||!multiplier.is_finite()||candidates>i32::MAX as usize{return Err("invalid ambient count inputs".into());}
    if candidates==0{return Ok(None);}
    let sample=rng.random_range_f(min,max);
    let value=if biome {let density=(sample*multiplier).min(1.0);candidates as f32*density}else{sample};
    let rounded=value.round_ties_even();
    if !sample.is_finite()||!value.is_finite()||f64::from(rounded)<i32::MIN as f64||f64::from(rounded)>i32::MAX as f64{return Err("ambient count outside integer range".into());}
    Ok(Some(Roll{sample,count:rounded as i32}))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ties_cap_and_empty_rng_follow_game(){
        let mut rng=SeededRandom::new(44);let before=rng.state();
        assert!(roll(&mut rng,0.0,1.0,0,true,1.0).unwrap().is_none());assert_eq!(rng.state(),before);
        assert_eq!(roll(&mut rng,0.5,0.5,5,true,1.0).unwrap().unwrap().count,2);
        assert_eq!(roll(&mut rng,0.5,0.5,7,true,1.0).unwrap().unwrap().count,4);
        assert_eq!(roll(&mut rng,2.0,2.0,7,true,1.0).unwrap().unwrap().count,7);
        assert_eq!(roll(&mut rng,2.5,2.5,1,false,1.0).unwrap().unwrap().count,2);
        assert_ne!(rng.state(),before,"Fixed ranges still consume RNG");
    }
}
