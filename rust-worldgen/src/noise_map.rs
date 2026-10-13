//! Rasterize per-subworld noise fields using the game's regional sample bounds.
use crate::{data_source::DataSource, generate::BiomeCell, geom::{Polygon,V2}, noise_tree};

#[derive(Debug,serde::Serialize)]
pub struct NoiseMaps {
    pub base_noise: Vec<f32>, pub override_noise: Vec<f32>,
    pub density_noise: Vec<f32>, pub backwall_noise: Vec<f32>,
}
/// Range of one noise tree's samples; `cells` is a bitset of the grid
/// cells to normalize.
struct Stats { channel:usize, min:f32, max:f32, cells:Vec<u64> }

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn evaluate_samples_bounds_or_world_coordinates() {
        let tree:noise_tree::TreeJson=crate::klei_yaml::from_str("settings: {zoom: 0.5, lowerBound: {X: 1, Y: 2}, upperBound: {X: 3, Y: 6}}
links:
- target: {type: Terminator, name: t}
  source0: {type: Primitive, name: p}
primitives:
  p: {primative: simplexperlin, seed: 3}
").unwrap();
        let noise=crate::noise::SimplexPerlin::new(10);
        let bounds=evaluate(&tree,2,2,7,Sampling::Bounds).unwrap();
        assert_eq!(bounds,[noise.get_value_3(1.5,0.0,3.0),noise.get_value_3(2.5,0.0,3.0),noise.get_value_3(1.5,0.0,5.0),noise.get_value_3(2.5,0.0,5.0)]);
        let world=evaluate(&tree,2,1,7,Sampling::World).unwrap();
        assert_eq!(world,[noise.get_value_3(0.0,0.0,0.0),noise.get_value_3(0.5,0.0,0.0)]);
        assert!(evaluate(&noise_tree::TreeJson::default(),2,2,0,Sampling::Bounds).is_err());
        assert_eq!(noise_tree::node_kinds().as_array().unwrap().len(),35);
    }
}

/// World-wide heat noise is generated before regional noise and normalized
/// over the complete grid, using the same world noise seed.
pub fn heat(ds:&DataSource,width:usize,height:usize,seed:i32)->Result<Vec<f32>,String> {
    if width==0 || height==0 { return Err("empty heat grid".into()); }
    let tree=ds.noise.get("noise/Heat").ok_or("missing heat tree")?;
    let source=noise_tree::build(tree,seed).ok_or("unsupported heat tree")?;
    let zoom=if tree.settings.zoom==0.0 {0.01} else {tree.settings.zoom};
    heat_plane(source.as_ref(),width,height,zoom,tree.settings.normalise)
}

fn heat_plane(source:&dyn crate::noise::Source3D,width:usize,height:usize,zoom:f32,normalise:bool)->Result<Vec<f32>,String> {
    let xs=axis(0,width,zoom);let ys=axis(0,height,zoom);
    let mut result=Vec::with_capacity(width.checked_mul(height).ok_or("heat grid size overflow")?);
    for y in ys { for &x in &xs { result.push(source.get_value_3(x,0.0,y)); } }
    if result.iter().any(|v|!v.is_finite()) {return Err("nonfinite heat noise".into());}
    if normalise {
        let low=result.iter().copied().fold(f32::MAX,f32::min);
        let high=result.iter().copied().fold(f32::MIN,f32::max);
        let span=high-low;
        if span==0.0 { return Err("constant normalized heat noise".into()); }
        for v in &mut result {*v=((*v as f64-low as f64)/span as f64) as f32;}
    }
    Ok(result)
}

/// How [`evaluate`] maps a `width` x `height` grid onto noise space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Sampling {
    /// Cell centres spread linearly over the tree's `settings.lowerBound` to
    /// `upperBound` rectangle (the game's defaults (2, 2)-(4, 4) when absent),
    /// ignoring `zoom`: the noise editor's view.
    Bounds,
    /// What worldgen samples for a world of that size at the origin: cell
    /// coordinates times `zoom` (0.01 when zero), before normalisation.
    World,
}

/// Evaluate `tree` with `seed` over a grid, row-major (`y * width + x`),
/// unnormalised. Fails for a tree without a terminator or with nodes this
/// port does not implement.
pub fn evaluate(tree:&noise_tree::TreeJson,width:usize,height:usize,seed:i32,sampling:Sampling)->Result<Vec<f32>,String> {
    let count=width.checked_mul(height).filter(|&n|n>0&&n<=1<<24).ok_or("noise grid must have 1 to 16777216 cells")?;
    if !tree.links.iter().any(|l|l.target.as_ref().is_some_and(|t|t.kind=="Terminator")) {return Err("noise tree has no Terminator link".into());}
    let source=noise_tree::build(tree,seed).ok_or("noise tree uses a node this engine does not implement, or a link is missing")?;
    let (xs,ys)=match sampling {
        Sampling::World => {
            let zoom=if tree.settings.zoom==0.0 {0.01} else {tree.settings.zoom};
            (axis(0,width,zoom),axis(0,height,zoom))
        }
        Sampling::Bounds => {
            let bound=|v:&Option<serde_json::Value>,default:f32|->(f32,f32) {
                let get=|k:&str|v.as_ref().and_then(|v|v.get(k).or_else(||v.get(k.to_lowercase()))).and_then(|x|x.as_f64()).map_or(default,|x|x as f32);
                (get("X"),get("Y"))
            };
            let (lower,upper)=(bound(&tree.settings.lower_bound,2.0),bound(&tree.settings.upper_bound,4.0));
            let spread=|n:usize,lo:f32,hi:f32|(0..n).map(|i|lo+(hi-lo)*((i as f32+0.5)/n as f32)).collect::<Vec<_>>();
            (spread(width,lower.0,upper.0),spread(height,lower.1,upper.1))
        }
    };
    let mut out=Vec::with_capacity(count);
    for &y in &ys { for &x in &xs { out.push(source.get_value_3(x,0.0,y)); } }
    Ok(out)
}

fn axis(offset: i32, size:usize, zoom:f32) -> Vec<f32> {
    let low = (offset as f64 * zoom as f64) as f32;
    let high = ((offset as f64 + size as f64) * zoom as f64) as f32;
    let step = (high-low)/size as f32;
    let mut value = low;
    (0..size).map(|_| {let v=value; value+=step; v}).collect()
}

/// [`heat`]'s checks that do not depend on noise values.
pub fn heat_check(ds:&DataSource,width:usize,height:usize,seed:i32)->Result<(),String> {
    if width==0 || height==0 { return Err("empty heat grid".into()); }
    let tree=ds.noise.get("noise/Heat").ok_or("missing heat tree")?;
    noise_tree::build(tree,seed).ok_or("unsupported heat tree")?;
    Ok(())
}

pub fn generate(ds:&DataSource, cells:&[BiomeCell], width:usize,height:usize,seed:i32) -> Result<NoiseMaps,String> {
    generate_partial(ds,cells,width,height,seed,Some(None))
}

/// [`generate`] restricted to some cells. `Some(None)` is the full map.
/// `None` evaluates nothing and only runs the checks that do not depend on
/// noise values, in the same order. `Some(Some(needed))`, with bit `1 << c`
/// of `needed[cell]` set where channel `c` must be exact, evaluates the
/// trees that can affect those values, each over all of its cells: the tree
/// writing channel `c` at the cell, and every tree of a containing overworld
/// cell whose normalization applies to channel `c` (a tree's statistics
/// normalize the channel of its first use, over all of its cells).
pub fn generate_partial(ds:&DataSource, cells:&[BiomeCell], width:usize,height:usize,seed:i32,select:Option<Option<&[u8]>>) -> Result<NoiseMaps,String> {
    let count = width.checked_mul(height).ok_or("noise map size overflow")?;
    let mut maps = [vec![0.0f32;count],vec![0.0;count],vec![0.0;count],vec![0.0;count]];
    let defaults = [Some("noise/Default"),Some("noise/DefaultCave"),Some("noise/DefaultDensity"),None];
    let mut stats:Vec<(String,Stats)> = Vec::new();
    // Trees depend only on their name and the seed; build each once.
    let mut sources:Vec<(&str,Box<dyn crate::noise::Source3D>)> = Vec::new();
    // Grid points inside the current cell, in the x-major scan order.
    let mut inside:Vec<(usize,usize,usize)> = Vec::new();
    let names=|cell:&BiomeCell|->Option<[Option<&str>;4]> {
        let sw=ds.resolve_subworld(cell.subworld.as_deref()?)?;
        Some(std::array::from_fn(|channel|sw.noise_refs[channel].as_deref().filter(|n|ds.noise.contains_key(*n)).or(defaults[channel])))
    };
    // Trees to evaluate when only some cells are needed.
    let mut wanted:Vec<&str>=Vec::new();
    if let Some(Some(needed))=select {
        let mut first_channel:Vec<(&str,usize)>=Vec::new();
        for cell in cells {
            for (channel,name) in names(cell).into_iter().flatten().enumerate() {
                if let Some(name)=name {if !first_channel.iter().any(|(n,_)|*n==name) {first_channel.push((name,channel));}}
            }
        }
        for cell in cells {
            let Some(trees)=names(cell) else {continue};
            let poly=Polygon::new(cell.polygon.iter().map(|&(x,y)|V2::new(x,y)).collect());
            let b=poly.bounds;
            let mut channels=0u8;
            for x in b.x.floor() as i32..=(b.x+b.width).ceil() as i32 {
                for y in b.y.floor() as i32..=(b.y+b.height).ceil() as i32 {
                    if x<0||y<0||x>=width as i32||y>=height as i32 {continue}
                    let bits=needed[x as usize+y as usize*width];
                    if bits&!channels!=0&&poly.contains(V2::new(x as f32,y as f32)) {channels|=bits;}
                }
            }
            for channel in (0..4).filter(|c|channels&(1<<c)!=0) {
                let affecting=trees.iter().enumerate().filter_map(|(c,name)|{
                    let name=(*name)?;
                    (c==channel||first_channel.iter().any(|&(n,f)|n==name&&f==channel)).then_some(name)
                });
                for name in affecting.collect::<Vec<_>>() {if !wanted.contains(&name) {wanted.push(name);}}
            }
        }
    }
    for cell in cells {
        let sw = ds.resolve_subworld(cell.subworld.as_deref().ok_or("unassigned subworld for noise")?).ok_or("missing noise subworld")?;
        let poly = Polygon::new(cell.polygon.iter().map(|&(x,y)|V2::new(x,y)).collect());
        let b=poly.bounds;
        let nw=(b.width+2.0).ceil() as usize;
        let nh=(b.height+2.0).ceil() as usize;
        let ox=(b.x-1.0).floor() as i32;
        let oy=(b.y-1.0).floor() as i32;
        inside.clear();
        if select.is_some() { for x in b.x.floor() as i32..=(b.x+b.width).ceil() as i32 {
            for y in b.y.floor() as i32..=(b.y+b.height).ceil() as i32 {
                if x<0||y<0||x>=width as i32||y>=height as i32||!poly.contains(V2::new(x as f32,y as f32)) {continue}
                inside.push(((x-ox) as usize,(y-oy) as usize,x as usize+y as usize*width));
            }
        }}
        for channel in 0..4 {
            let name = sw.noise_refs[channel].as_deref().filter(|n|ds.noise.contains_key(*n)).or(defaults[channel]);
            let Some(name)=name else {continue};
            let tree=ds.noise.get(name).ok_or_else(||format!("missing noise tree {name}"))?;
            let source_index=if let Some(i)=sources.iter().position(|(n,_)|*n==name) {i} else {
                sources.push((name,noise_tree::build(tree,seed).ok_or_else(||format!("unsupported or malformed noise tree {name}"))?));sources.len()-1
            };
            let source=&sources[source_index].1;
            let zoom=if tree.settings.zoom==0.0 {0.01} else {tree.settings.zoom};
            let xs=axis(ox,nw,zoom);let ys=axis(oy,nh,zoom);
            let stat_index=if let Some(i)=stats.iter().position(|(n,_)|n==name) {i} else {
                stats.push((name.to_string(),Stats{channel,min:f32::MAX,max:f32::MIN,cells:vec![0;count.div_ceil(64)]}));stats.len()-1
            };
            let stat=&mut stats[stat_index].1;
            if matches!(select,Some(Some(_)))&&!wanted.contains(&name) {continue;}
            for &(xi,yi,index) in &inside {
                let value=source.get_value_3(xs[xi],0.0,ys[yi]);
                if !value.is_finite() {return Err(format!("nonfinite noise {name} at {},{}",xi as i32+ox,yi as i32+oy));}
                maps[channel][index]=value;
                stat.min=stat.min.min(value);stat.max=stat.max.max(value);
                if tree.settings.normalise {stat.cells[index/64]|=1<<(index%64);}
            }
        }
    }
    for (_,stat) in stats {
        let span=stat.max-stat.min;
        for (word_index,&word) in stat.cells.iter().enumerate() {
            let mut bits=word;
            while bits!=0 {
                let cell=word_index*64+bits.trailing_zeros() as usize;
                bits&=bits-1;
                maps[stat.channel][cell]=((maps[stat.channel][cell] as f64-stat.min as f64)/span as f64) as f32;
            }
        }
    }
    let [base_noise,override_noise,density_noise,backwall_noise]=maps;
    Ok(NoiseMaps{base_noise,override_noise,density_noise,backwall_noise})
}
