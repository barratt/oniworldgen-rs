//! ONI 744825 IsSuitableMobSpawnPoint. Uses managed spawning state.
use std::collections::HashSet;
use crate::ambient_placement::width_offset;

#[derive(Clone,Copy,Debug,PartialEq,serde::Deserialize)]
pub enum Location {Solid,Ceiling,Floor,EntombedFloorPeek,LiquidFloor,LiquidFloorCavityNoRequired,
    LiquidCeiling,AnyFloor,Air,Water,Liquid,Surface,AnchoredToBackWall,BackWall,NearWater,NearLiquid,ShallowLiquid}
#[derive(Clone,Copy,Debug,PartialEq,serde::Deserialize)]
pub struct Mob {pub width:i32,pub height:i32,pub padding:i32,pub location:Location}
pub struct Grid<'a> {pub width:i32,pub height:i32,pub solid:&'a [bool],pub liquid:&'a [bool],
    pub vacuum:&'a [bool],pub backwall_solid:&'a [bool],pub cavities:&'a HashSet<i32>}
impl Grid<'_> {
    /// GetMobPossibleSpawnPoints: filter in input order, then shuffle the full
    /// accepted list. Empty/singleton lists consume no random draws.
    pub fn candidates(&self,input:&[i32],mob:Mob,occupied:&HashSet<i32>,rng:&mut crate::sampler::SeededRandom)->Result<Vec<i32>,String>{
        let mut accepted=Vec::new();
        for &cell in input {if self.suitable(cell,mob,occupied)? {accepted.push(cell);}}
        crate::sampler::shuffle_seeded(&mut accepted,rng);
        Ok(accepted)
    }
    pub fn validate(&self)->Result<(),String>{
        let n=self.width.checked_mul(self.height).filter(|_|self.width>0&&self.height>0).ok_or("invalid grid dimensions")? as usize;
        if [self.solid.len(),self.liquid.len(),self.vacuum.len(),self.backwall_solid.len()].iter().any(|&v|v!=n){return Err("incomplete suitability grid".into());}
        Ok(())
    }
    pub fn suitable(&self,cell:i32,mob:Mob,occupied:&HashSet<i32>)->Result<bool,String>{
        use Location::*;
        self.validate()?;
        if mob.width<=0||mob.height<=0||mob.padding<0{return Err("invalid mob dimensions".into());}
        if cell<0||cell as usize>=self.solid.len(){return Ok(false);}
        // i64 avoids overflow for malformed configuration while preserving valid game arithmetic.
        let columns=i64::from(mob.width)+2*i64::from(mob.padding);
        if columns>i64::from(i32::MAX){return Err("mob width overflow".into());}
        let direction=if matches!(mob.location,Ceiling|LiquidCeiling){-1}else{1};
        let left=columns/2-i64::from(mob.width)-i64::from(mob.padding);
        let top=if direction<0 {1}else{i64::from(mob.height)};
        let valid_offset=|dx:i64,dy:i64| {
            let x=i64::from(cell%self.width)+dx;let y=i64::from(cell/self.width)+dy;
            x>=0&&x<i64::from(self.width)&&y>=0&&y<i64::from(self.height)
        };
        if !valid_offset(left,top)||!valid_offset(left+i64::from(mob.width)+1,top-i64::from(mob.height)-1){return Ok(false);}
        let at=|x:i32,y:i32|->Result<usize,String>{
            let i=i64::from(cell)+i64::from(y)*i64::from(self.width)+i64::from(width_offset(x));
            if i<0||i>=self.solid.len() as i64{return Err("mob footprint outside grid".into());}Ok(i as usize)
        };
        for x in 0..columns as i32 {for y in 0..mob.height {
            let i=at(x,y*direction)?;if occupied.contains(&(i as i32)){return Ok(false);}
        }}
        let anchor=cell as usize;let gw=self.width as usize;
        if matches!(mob.location,Air){return Ok(!self.solid[anchor]&&!self.solid[anchor+gw]&&!self.liquid[anchor]);}
        if matches!(mob.location,BackWall|NearWater|NearLiquid|ShallowLiquid){return Ok(self.cavities.contains(&cell)&&!self.solid[anchor]);}
        let check_width=if matches!(mob.location,Floor){columns as i32}else{mob.width};
        let check_height=if matches!(mob.location,Surface){1}else{mob.height};
        let mut peek=false;
        for y in 0..check_height {for x in 0..check_width {
            let i=at(x,y*direction)?;
            let cavity=self.cavities.contains(&(i as i32));
            let good=match mob.location {
                Solid=>!cavity&&self.solid[i],
                Ceiling=>cavity&&!self.solid[i]&&!self.liquid[i]&&(y!=0||self.solid[i+gw]),
                LiquidCeiling=>cavity&&!self.solid[i]&&self.liquid[i]&&(y!=0||self.solid[i+gw]),
                Floor=>cavity&&!self.solid[i]&&!self.liquid[i]&&(y!=0||x>=mob.width||self.solid[i-gw]),
                EntombedFloorPeek=>{peek|=!self.solid[i];y!=0||self.solid[i-gw]},
                LiquidFloor|LiquidFloorCavityNoRequired=>(matches!(mob.location,LiquidFloorCavityNoRequired)||self.cavities.contains(&cell))
                    &&!self.solid[i]&&(y!=0||(self.liquid[i]&&self.solid[i-gw])),
                AnyFloor=>self.cavities.contains(&cell)&&!self.solid[i]&&(y!=0||self.solid[i-gw]),
                Water|Liquid=>self.liquid[i]&&(y!=mob.height-1||self.liquid[i+gw]),
                Surface=>self.vacuum[i]&&self.solid[i-gw],
                AnchoredToBackWall=>self.backwall_solid[anchor]&&!self.solid[i],
                Air|BackWall|NearWater|NearLiquid|ShallowLiquid=>unreachable!(),
            };
            if !good{return Ok(false);}
        }}
        Ok(!matches!(mob.location,EntombedFloorPeek)||peek)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn floor_needs_cavity_and_support_air_does_not(){
        let mut solid=vec![false;49];solid[17]=true;
        let f=vec![false;49];let cavities=HashSet::from([24]);
        let grid=Grid{width:7,height:7,solid:&solid,liquid:&f,vacuum:&f,backwall_solid:&f,cavities:&cavities};
        let mob=Mob{width:1,height:1,padding:0,location:Location::Floor};
        assert!(grid.suitable(24,mob,&HashSet::new()).unwrap());
        assert!(!grid.suitable(25,mob,&HashSet::new()).unwrap());
        assert!(grid.suitable(25,Mob{location:Location::Air,..mob},&HashSet::new()).unwrap());
        assert!(!grid.suitable(24,mob,&HashSet::from([24])).unwrap());
        assert!(!grid.suitable(0,mob,&HashSet::new()).unwrap());
        let mut rng=crate::sampler::SeededRandom::new(44);let before=rng.state();
        assert!(grid.candidates(&[0,25],mob,&HashSet::new(),&mut rng).unwrap().is_empty());
        assert_eq!(grid.candidates(&[0,24,25],mob,&HashSet::new(),&mut rng).unwrap(),[24]);
        assert_eq!(rng.state(),before,"Empty and singleton candidates must not draw RNG");
        for location in [Location::BackWall,Location::NearWater,Location::NearLiquid,Location::ShallowLiquid] {
            assert!(grid.suitable(24,Mob{location,..mob},&HashSet::new()).unwrap());
            assert!(!grid.suitable(25,Mob{location,..mob},&HashSet::new()).unwrap());
        }
    }
    #[test]
    fn backwall_checks_anchor_and_liquid_requires_extra_top_cell(){
        let f=vec![false;49];let mut liquid=f.clone();liquid[24]=true;
        let mut back=f.clone();back[24]=true;let cavities=HashSet::new();
        let mut grid=Grid{width:7,height:7,solid:&f,liquid:&liquid,vacuum:&f,backwall_solid:&back,cavities:&cavities};
        let mob=Mob{width:2,height:1,padding:0,location:Location::AnchoredToBackWall};
        assert!(grid.suitable(24,mob,&HashSet::new()).unwrap());
        let water=Mob{width:1,location:Location::Liquid,..mob};
        assert!(!grid.suitable(24,water,&HashSet::new()).unwrap());
        let mut full=liquid.clone();full[31]=true;grid.liquid=&full;
        assert!(grid.suitable(24,water,&HashSet::new()).unwrap());
    }
}
