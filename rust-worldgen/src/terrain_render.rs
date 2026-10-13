//! Native terrain rendering through the end of region processing, before
//! biome borders, world borders, templates or settling physics.
use crate::{data_source::DataSource, generate::WorldOutput, geom::{Polygon,V2}, foreground,
    noise_map, sampler::SeededRandom, terrain::{self,MaterialCell}, action_cells};

#[derive(serde::Serialize)]
pub struct RegionTrace {
    pub spawn_cells: Vec<i32>,
    pub feature_cells: Vec<i32>,
    pub feature_cells_for_biome: Vec<i32>,
    pub spawn_backwall: bool,
    pub id: usize,
    pub before_rng: crate::rng::RandomState,
    pub after_foreground_rng: crate::rng::RandomState,
    pub after_background_rng: crate::rng::RandomState,
    pub background_biome: String,
    pub foreground_writes: Vec<(usize,MaterialCell)>,
    pub action_positions: Vec<(i32,String)>,
}
#[derive(serde::Serialize)]
pub struct TerrainRender {
    pub backwall_cells: Vec<MaterialCell>,
    pub border_trace: Vec<serde_json::Value>,
    pub cells: Vec<Option<MaterialCell>>,
    pub regions: Vec<RegionTrace>,
    pub priority_cells: Vec<usize>,
}

pub fn render(ds:&DataSource,w:&WorldOutput)->Result<TerrainRender,String> {
    let width=usize::try_from(w.size_x).map_err(|_|"negative world width")?;
    let height=usize::try_from(w.size_y).map_err(|_|"negative world height")?;
    let noise=noise_map::generate(ds,&w.cells,width,height,w.world_seed)?;
    let heat=noise_map::heat(ds,width,height,w.world_seed)?;
    render_with(ds,w,&noise,&heat)
}

/// [`render`] with the world's noise maps and heat field already generated
/// (`noise_map::generate` and `noise_map::heat` for the same world).
pub fn render_with(ds:&DataSource,w:&WorldOutput,noise:&noise_map::NoiseMaps,heat:&[f32])->Result<TerrainRender,String> {
    render_impl(ds,w,noise,heat,Background::All)
}

/// Which background cells [`render_impl`] computes. Foreground features,
/// RNG and region traces are always complete.
pub enum Background<'a> {
    All,
    /// Only the masked cells' background materials; backwalls are skipped.
    /// Mass, temperature and disease of background cells are not meaningful.
    /// Elements are exact where they do not depend on the base or cave
    /// override noise, and elsewhere where the noise maps hold the true
    /// values. The optional slice receives, for those noise-dependent masked
    /// cells, bit 0 when the base noise matters and bit 1 for the override.
    Masked(&'a [bool],Option<&'a mut [u8]>),
}

/// Masked and probing renders refuse worlds whose band tables could fail at a
/// cell outside the mask; callers then render the world in full.
pub const MASKED_FALLBACK:&str="masked render needs the full render";

pub fn render_impl(ds:&DataSource,w:&WorldOutput,noise:&noise_map::NoiseMaps,heat:&[f32],mut background:Background)->Result<TerrainRender,String> {
    let width=usize::try_from(w.size_x).map_err(|_|"negative world width")?;
    let height=usize::try_from(w.size_y).map_err(|_|"negative world height")?;
    if noise.base_noise.len()!=width*height||heat.len()!=width*height {return Err("noise maps do not match the world size".into());}
    let cave_max=ds.world_setting(&w.name,"CaveOverrideMaxValue")?.as_f64().ok_or("invalid cave max")? as f32;
    let cave_sliver=ds.world_setting(&w.name,"CaveOverrideSliverValue")?.as_f64().ok_or("invalid cave sliver")? as f32;
    let poly=|v:&Vec<(f32,f32)>|Polygon::new(v.iter().map(|&(x,y)|V2::new(x,y)).collect());
    let polygons:Vec<_>=w.terrain_regions.iter().map(|r|poly(&r.polygon)).collect();
    let parents:Vec<_>=w.cells.iter().map(|r|poly(&r.polygon)).collect();
    let parent_backwalls:Vec<_>=w.cells.iter().map(|r|r.subworld.as_ref().and_then(|s|ds.resolve_subworld(s))
        .and_then(|s|s.noise_refs[3].as_ref()).is_some_and(|name|ds.noise.contains_key(name))).collect();
    let raster=terrain::raster_points(&polygons,width,height)?;
    // The first region whose polygon holds each grid point. Rasters hold
    // exactly the in-grid integer points their polygon contains, and no
    // polygon contains a point outside the grid (raster_points refuses it).
    let mut first_region=vec![u32::MAX;width*height];
    for (i,cells) in raster.iter().enumerate().rev() {for &c in cells {first_region[c]=i as u32;}}
    let edges:Vec<_>=w.biome_edges.iter().filter(|e|e.closed).map(|e|(V2::new(e.start.0,e.start.1),V2::new(e.end.0,e.end.1))).collect();
    // RenderToMap initializes the whole grid to abyssalite before processing
    // regions. Rare polygon/feature gaps retain this value (seed 44 exercises it).
    let initial=ds.elements.get("Katairite").ok_or("missing initial abyssalite element")?.default_cell();
    let partial=!matches!(background,Background::All);
    if partial&&["Unobtanium","Void","Vacuum"].iter().any(|e|!ds.elements.contains_key(*e)) {return Err(MASKED_FALLBACK.into());}
    let mut cells=vec![Some(initial);width*height];
    // Masked renders leave backwalls out.
    let mut backwall_cells=if partial {Vec::new()} else {vec![MaterialCell {element:"Vacuum".into(),..Default::default()};width*height]};
    // HighPriorityFeature claims, as flags plus the claimed cells.
    let mut priority=vec![false;width*height];
    let mut priority_list=Vec::new();
    // The current region's feature cells, which its background skips.
    let mut feature_claimed=vec![false;width*height];
    // The current region's raster cells.
    let mut in_region=vec![false;width*height];
    let mut regions=Vec::new();
    let mut rng=SeededRandom::new(w.world_seed);
    for (i,r) in w.terrain_regions.iter().enumerate() {
        let before_rng=rng.state();
        let center=polygons[i].clone().centroid();
        let spawn_backwall=parents.iter().zip(&parent_backwalls).any(|(p,&spawn)|spawn&&p.point_in_polygon(center));
        let point=V2::new(center.x as i32 as f32,center.y as i32 as f32);
        let temperature=parents.iter().position(|p|p.contains(point)).and_then(|j|w.cells[j].subworld.as_ref())
            .and_then(|s|ds.subworlds.get(s)).map(|s|s.temperature_range.as_str()).unwrap_or("Mild");
        let (min,max)=*ds.temperatures.get(temperature).ok_or_else(||format!("missing temperature {temperature}"))?;
        for &c in &raster[i] {in_region[c]=true;}
        let mut writes=Vec::new();
        let mut feature_cells=Vec::new();let mut feature_cells_for_biome=Vec::new();
        if let Some(plan)=foreground::plan(r,ds,&mut rng)? {
            feature_cells=plan.claimed.iter().map(|&(x,y)|x+y*w.size_x).collect();
            feature_cells_for_biome=feature_cells.clone();
            for &(x,y) in &plan.claimed {
                let flat=x+y*w.size_x;
                if flat>=0&&(flat as usize)<width*height {feature_claimed[flat as usize]=true;}
            }
            let exceed=r.tags.iter().any(|t|t=="AllowExceedNodeBorders");
            // Feature points are integer grid positions.
            let first=|point:V2|(point.x>=0.0&&point.y>=0.0&&point.x<width as f32&&point.y<height as f32)
                .then(||first_region[point.x as usize+point.y as usize*width]).filter(|&j|j!=u32::MAX).map(|j|j as usize);
            let within=|point:V2| {
                if !exceed||polygons[i].contains(point){return true;}
                if let Some(j)=first(point) {
                    ds.subworlds[&r.subworld].zone_type==ds.subworlds[&w.terrain_regions[j].subworld].zone_type
                }else{true}
            };
            let valid=|(x,y):(i32,i32)| {let n=x as i64+y as i64*width as i64;
                if n<0||n>=(width*height) as i64 {None}else{Some(n as usize)}};
            let feature=&ds.features[&plan.feature];
            let groups=std::iter::once(("RoomCenterElements".to_string(),&plan.center))
                .chain(plan.borders.iter().enumerate().map(|(j,b)|(format!("RoomBorderChoices{j}"),b)));
            for (name,points) in groups {
                if let Some(group)=feature.element_choice_groups.get(&name) {
                    writes.extend(foreground::place_group(group,points,&mut rng,&ds.elements,&noise.density_noise,heat,min,max-min,|p|
                        valid(p).filter(|&index|!priority[index]&&within(V2::new(p.0 as f32,p.1 as f32))))?);
                }
            }
            if feature.tags.iter().any(|t|t=="HighPriorityFeature") {
                feature_cells_for_biome.retain(|&index|index>=0&&(index as usize)<width*height&&within(V2::new((index as usize%width) as f32,(index as usize/width) as f32)));
                for p in plan.claimed {if let Some(index)=valid(p) {
                    if within(V2::new((index%width) as f32,(index/width) as f32))&&!priority[index] {priority[index]=true;priority_list.push(index);}
                }}
            }
        }
        for (index,cell) in &writes {cells[*index]=Some(cell.clone());}
        let after_foreground_rng=rng.state();
        // The raster's cells without the feature's claims.
        let available=|cell:i32|cell>=0&&(cell as usize)<width*height&&in_region[cell as usize]&&!feature_claimed[cell as usize];
        let (biome,positions)=action_cells::after_foreground(r,ds,&available,w.size_x,w.size_y,&mut rng)?;
        let bands=ds.biome_bands(&biome,w.world_traits.as_deref().unwrap_or(&[]))?.ok_or("missing selected biome")?;
        let backwall_bands=if spawn_backwall {ds.biome_bands(&format!("{}_backwall",r.biome),w.world_traits.as_deref().unwrap_or(&[]))?}else{None};
        let has=|tag:&str|r.leaf_tags.iter().any(|t|t==tag);
        let (ignore_cave,cave_void_sliver)=(has("IgnoreCaveOverride"),has("CaveVoidSliver"));
        // A band without a known material can fail the full render at a cell
        // outside the mask.
        let unusable=|bands:&[terrain::ElementBand]|bands.iter().any(|b|b.content.as_ref().is_none_or(|c|!ds.elements.contains_key(c)));
        if partial&&(unusable(&bands)||(spawn_backwall&&unusable(backwall_bands.as_deref().unwrap_or(&bands)))) {return Err(MASKED_FALLBACK.into());}
        let erosion_tags=terrain::ErosionTags::new(&r.leaf_tags);
        if let Background::Masked(mask,Some(needs))=&mut background {
            // Uniform bands make the base noise irrelevant to the element, and
            // IgnoreCaveOverride the cave override noise.
            let uniform=bands.iter().all(|b|b.content==bands[0].content);
            let bits=if uniform {0} else {1}|if ignore_cave {0} else {2};
            if bits!=0 {
                for &index in &raster[i] {
                    if mask[index]&&!feature_claimed[index]&&!priority[index] {needs[index]|=bits;}
                }
            }
        }
        {
            let mask=match &background {Background::Masked(mask,_)=>Some(*mask),Background::All=>None};
            for &index in &raster[i] {
                if feature_claimed[index]||priority[index]||mask.is_some_and(|m|!m[index]) {continue;}
                let point=V2::new((index%width) as f32,(index/width) as f32);
                let erosion=terrain::erosion_with(&polygons[i],center,&erosion_tags,point,width as f32,height as f32,Some(&edges))?;
                cells[index]=Some(terrain::background_cell(&ds.elements,terrain::select_band(&bands,noise.base_noise[index]*erosion),
                    noise.density_noise[index],heat[index],min,max-min,noise.override_noise[index],ignore_cave,cave_void_sliver,cave_max,cave_sliver)?);
                // The terrain override branch continues before reaching backwalls.
                if mask.is_none()&&spawn_backwall&&(ignore_cave||noise.override_noise[index]<100.0) {
                    let bands=backwall_bands.as_deref().unwrap_or(&bands);
                    if let Some(cell)=terrain::backwall_cell(&ds.elements,terrain::select_band(bands,noise.backwall_noise[index]),noise.density_noise[index],heat[index],min,max-min)? {
                        backwall_cells[index]=cell;
                    }
                }
            }
        }
        for &flat in &feature_cells {if flat>=0&&(flat as usize)<width*height {feature_claimed[flat as usize]=false;}}
        for &c in &raster[i] {in_region[c]=false;}
        regions.push(RegionTrace {spawn_cells:raster[i].iter().map(|&c|c as i32).collect(),feature_cells,feature_cells_for_biome,
            id:r.id,spawn_backwall,before_rng,after_foreground_rng,after_background_rng:rng.state(),
            background_biome:biome,foreground_writes:writes,action_positions:positions});
    }
    priority_list.sort_unstable();
    Ok(TerrainRender {cells,backwall_cells,regions,priority_cells:priority_list,border_trace:Vec::new()})
}
