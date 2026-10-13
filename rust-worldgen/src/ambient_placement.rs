//! ONI 744825 SpawnCountMobs: ordered placement after candidate filtering/shuffle.
use std::collections::HashSet;

/// Preserve the game's alternating horizontal offsets: 0,+1,-1,+2,-2,...
pub fn width_offset(i:i32)->i32 {if i%2==0 {-(i/2)} else {i/2+i%2}}

/// Candidates must already satisfy ONI's suitability checks. Returns accepted
/// anchors in order and reserves padded footprints in the shared occupied set.
/// Only Ceiling grows downwards here (LiquidCeiling deliberately grows upwards,
/// as in SpawnCountMobs, despite its different suitability rule).
pub fn spawn_count(grid_width:i32,width:i32,height:i32,padding:i32,ceiling:bool,
    count:i32,candidates:&[i32],occupied:&mut HashSet<i32>)->Result<Vec<i32>,String>{
    if grid_width<=0||width<=0||height<=0||padding<0{return Err("invalid mob dimensions".into());}
    let columns=padding.checked_mul(2).and_then(|n|width.checked_add(n)).ok_or("mob width overflow")?;
    let direction=if ceiling {-1_i32}else{1};
    let mut selected=Vec::new();
    for &anchor in candidates {
        if selected.len() as i64>=i64::from(count){break;}
        let mut footprint=Vec::new();
        let mut blocked=false;
        for x in 0..columns {
            for y in 0..height {
                let cell=y.checked_mul(direction).and_then(|v|v.checked_mul(grid_width))
                    .and_then(|v|anchor.checked_add(v)).and_then(|v|v.checked_add(width_offset(x)))
                    .ok_or("mob footprint overflow")?;
                if occupied.contains(&cell){blocked=true;break;}
                footprint.push(cell);
            }
            if blocked{break;}
        }
        if blocked{continue;}
        occupied.extend(footprint);
        selected.push(anchor);
    }
    Ok(selected)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn blocked_candidates_do_not_consume_count_and_padding_reserves_space(){
        let mut occupied=HashSet::from([11]);
        let selected=spawn_count(10,1,1,1,false,2,&[10,15,16,25],&mut occupied).unwrap();
        assert_eq!(selected,[15,25]);
        assert_eq!(occupied,HashSet::from([11,14,15,16,24,25,26]));
    }
    #[test]
    fn ceiling_direction_and_alternating_width_are_preserved(){
        assert_eq!((0..5).map(width_offset).collect::<Vec<_>>(),[0,1,-1,2,-2]);
        let mut occupied=HashSet::new();
        assert_eq!(spawn_count(10,2,2,0,true,1,&[33],&mut occupied).unwrap(),[33]);
        assert_eq!(occupied,HashSet::from([33,34,23,24]));
        assert!(spawn_count(10,1,1,0,false,0,&[50],&mut occupied).unwrap().is_empty());
    }
}
