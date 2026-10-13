//! Outer world border: independent seed-zero random walk, drawn after biome borders.
use crate::{data_source::DataSource,generate::WorldOutput,geom::{Polygon,V2},sampler::SeededRandom,terrain_render::TerrainRender};

#[derive(Debug,serde::Serialize)]
pub struct WorldBorder {
    pub cells:Vec<usize>,
    /// RectInt x, y, width, height; used later to exclude template placements.
    pub poi_bounds:Vec<(i32,i32,i32,i32)>,
}
pub fn render(ds:&DataSource,w:&WorldOutput,terrain:&mut TerrainRender)->Result<WorldBorder,String> {
    let mut result=WorldBorder {cells:Vec::new(),poi_bounds:Vec::new()};
    let Some((force,thickness,range))=settings(ds,w)? else {return Ok(result)};
    let value=ds.elements.get("Unobtanium").ok_or("missing unobtanium element")?.default_cell();
    let mut seen=crate::fxhash::FxHashSet::default();
    result.poi_bounds=walk(w,thickness,range,|x,y,tag| {
        let index=(x+y*w.size_x) as usize;
        let cell=terrain.cells.get_mut(index).ok_or("world border outside grid")?;
        let old=cell.as_ref().ok_or("uninitialized world border cell")?;
        if force||!tag||!matches!(old.element.as_str(),"Vacuum"|"Void") {
            // DrawWorldBorder updates the physics cell without touching germs.
            let mut next=value.clone();next.disease=old.disease.clone();next.disease_count=old.disease_count;
            *cell=Some(next);if seen.insert(index){result.cells.push(index);}Ok(true)
        }else{Ok(false)}
    })?;
    Ok(result)
}

/// The cells whose current element [`render`] reads: border cells in rows
/// and columns that start inside a RemoveWorldBorderOverVacuum region, unless
/// the border is forced. `None` when the settings are unusable; [`render`]
/// then reports why.
pub fn band(ds:&DataSource,w:&WorldOutput)->Option<Vec<bool>> {
    let (force,thickness,range)=settings(ds,w).ok()??;
    let count=(w.size_x as usize).checked_mul(w.size_y as usize)?;
    let mut mask=vec![false;count];
    if force {return Some(mask);}
    walk(w,thickness,range,|x,y,tag| {
        if let Some(m)=mask.get_mut((x+y*w.size_x) as usize) {*m|=tag;}
        Ok(true)
    }).ok()?;
    Some(mask)
}

/// `Some((DrawWorldBorderForce, thickness, range))` when the world draws a
/// border, after checking the border dimensions.
fn settings(ds:&DataSource,w:&WorldOutput)->Result<Option<(bool,i32,i32)>,String> {
    if !ds.world_setting(&w.name,"DrawWorldBorder")?.as_bool().ok_or("invalid DrawWorldBorder")? {return Ok(None);}
    let force=ds.world_setting(&w.name,"DrawWorldBorderForce")?.as_bool().ok_or("invalid DrawWorldBorderForce")?;
    let thickness=ds.world_setting(&w.name,"WorldBorderThickness")?.as_i64().ok_or("invalid WorldBorderThickness")? as i32;
    let range=ds.world_setting(&w.name,"WorldBorderRange")?.as_i64().ok_or("invalid WorldBorderRange")? as i32;
    if range<0||thickness<0||thickness as i64+range as i64>w.size_x.min(w.size_y) as i64{return Err("unsupported world border dimensions".into());}
    Ok(Some((force,thickness,range)))
}

/// The border's seed-zero random walk. `set(x, y, tag)` draws one cell and
/// returns whether it was written; `tag` is whether the row or column starts
/// inside a RemoveWorldBorderOverVacuum region. Returns the POI bounds.
fn walk(w:&WorldOutput,thickness:i32,range:i32,mut set:impl FnMut(i32,i32,bool)->Result<bool,String>)->Result<Vec<(i32,i32,i32,i32)>,String> {
    let polygons:Vec<_>=w.terrain_regions.iter().filter(|r|r.leaf_tags.iter().any(|s|s=="RemoveWorldBorderOverVacuum"))
        .map(|r|Polygon::new(r.polygon.iter().map(|&(x,y)|V2::new(x,y)).collect())).collect();
    let tagged=|x:i32,y:i32|polygons.iter().any(|p|p.contains(V2::new(x as f32,y as f32)));
    let mut rng=SeededRandom::new(0);let mut left=0;let mut right=0;
    let mut left_extent=0;let mut right_extent=w.size_x-1;
    for y in (0..w.size_y).rev() {
        left=(left+rng.random_range_i(-2,2)).clamp(-range,range);
        let tag=tagged(0,y);
        for x in 0..thickness+left {if set(x,y,tag)?{left_extent=left_extent.max(x);}}
        right=(right+rng.random_range_i(-2,2)).clamp(-range,range);
        let tag=tagged(w.size_x-1,y);
        for offset in 0..thickness+right {let x=w.size_x-1-offset;if set(x,y,tag)?{right_extent=right_extent.min(x);}}
    }
    let mut bottom=0;let mut top=0;let mut bottom_extent=0;let mut top_extent=w.size_y-1;
    for x in 0..w.size_x {
        bottom=(bottom+rng.random_range_i(-2,2)).clamp(-range,range);
        let tag=tagged(x,0);
        for y in 0..thickness+bottom {if set(x,y,tag)?{bottom_extent=bottom_extent.max(y);}}
        top=(top+rng.random_range_i(-2,2)).clamp(-range,range);
        let tag=tagged(x,w.size_y-1);
        for offset in 0..thickness+top {let y=w.size_y-1-offset;if set(x,y,tag)?{top_extent=top_extent.min(y);}}
    }
    Ok(vec![(0,0,left_extent+1,w.size_y),(right_extent,0,w.size_x-right_extent,w.size_y),
        (0,0,w.size_x,bottom_extent+1),(0,top_extent,w.size_x,w.size_y-top_extent)])
}
