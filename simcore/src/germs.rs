//! Cell-level germ bookkeeping used when mass moves between cells
//! (Disease::AddDiseaseToCell / CalculateFinalDiseaseCount). The only
//! disease-table value it reads is each disease's `strength`, which comes
//! from the diseases message. Disease phases (diffusion, growth) live
//! behind the `disease` seam instead.
use crate::fp::cvttss2si;
use crate::state::Cells;
use crate::tables::Disease;

/// CellSOA::ClearDisease.
pub fn clear(c: &mut Cells, i: usize) {
    c.disease[i] = 0xff;
    c.disease_count[i] = 0;
    c.infestation_ticks[i] = 0;
    c.growth_error[i] = 0.0;
}

/// Disease::AddDiseaseToCell on grid `c`.
pub fn add(diseases: &[Disease], c: &mut Cells, i: usize, idx: u8, count: i32) {
    let cur = c.disease[i];
    let cur_count = c.disease_count[i];
    let (fi, fc) = if cur == idx {
        (cur, cur_count.wrapping_add(count))
    } else if cur == 0xff {
        (idx, count)
    } else if idx == 0xff {
        (cur, cur_count)
    } else {
        let cf = cur_count as f32;
        let nf = count as f32;
        let sa = cf * diseases[cur as usize].strength;
        let sb = nf * diseases[idx as usize].strength;
        if sa > sb {
            let r = cvttss2si(nf - (sa / sb) * cf);
            (if r < 0 { cur } else { idx }, r.wrapping_abs())
        } else if cur_count >= 0 {
            (cur, cur_count)
        } else {
            (idx, cur_count.wrapping_neg())
        }
    };
    c.disease[i] = fi;
    c.disease_count[i] = fc;
    if fc > 0 {
        if fi != cur {
            c.infestation_ticks[i] = 0;
        }
    } else {
        clear(c, i);
    }
}

/// CellSOA::ModifyDiseaseCount.
pub fn modify(c: &mut Cells, i: usize, delta: i32) {
    c.disease_count[i] = c.disease_count[i].wrapping_add(delta);
    if c.disease_count[i] <= 0 {
        clear(c, i);
    }
}
