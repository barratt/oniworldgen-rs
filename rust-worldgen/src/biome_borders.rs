//! WorldGen.ProcessByTerrainCell's border pass, after all region rendering.
use crate::{data_source::DataSource,generate::WorldOutput,geom::{V2,Polygon},
    sampler::SeededRandom,foreground::ChoiceGroup,terrain_render::TerrainRender};

fn magnitude(v:V2)->f32 {((v.x as f64*v.x as f64+v.y as f64*v.y as f64).sqrt()) as f32}
fn normal(v:V2)->V2 {let n=magnitude(v);if n>0.00001 {V2::new(-v.y/n,v.x/n)}else{V2::ZERO}}

pub fn stagger(a:V2,b:V2,max_distance:f32,range:f32,rng:&mut SeededRandom)->Vec<(V2,V2)> {
    let delta=b.sub(a);let n=magnitude(delta)/max_distance;let perpendicular=normal(delta);
    let mut previous=a;let mut next=b;let mut result=Vec::new();let mut i=0;
    while (i as f32)<n {
        next=a.add(delta.mul(1.0/n).mul(i as f32)).add(perpendicular.mul(rng.random_range_f(-range,range)));
        result.push((previous,next));previous=next;i+=1;
    }
    result.push((next,b));result
}

pub fn render(ds:&DataSource,w:&WorldOutput,terrain:&mut TerrainRender)->Result<usize,String> {
    let state=&terrain.regions.last().ok_or("missing final region RNG")?.after_background_rng;
    let mut rng=SeededRandom::from_state(state)?;
    let mut borders=Vec::new();
    for e in w.biome_edges.iter().filter(|e|e.closed) {
        let sub=|i:usize|w.cells[i].subworld.as_ref().and_then(|s|ds.subworlds.get(s)).ok_or("missing border subworld");
        let a=sub(e.parents.0)?;let b=sub(e.parents.1)?;
        let x=a.border_override.as_deref().filter(|s|!s.is_empty());let y=b.border_override.as_deref().filter(|s|!s.is_empty());
        let kind=match (x,y) {
            (Some(x),Some(y))=>if a.border_override_priority==b.border_override_priority {if rng.random_value()>0.5{y}else{x}}
                else if a.border_override_priority>b.border_override_priority{x}else{y},
            (Some(x),None)=>x,(None,Some(y))=>y,(None,None)=>"hardToDig",
        };
        if kind=="NONE" {continue;}
        let x=&a.border_size_override;let y=&b.border_size_override;
        let valid=|v:&crate::subworld::Density|v.min!=0.0||v.max!=0.0;
        let (low,high)=match (valid(x),valid(y)) {
            (true,true)=>if x.max>y.max{(x.min,x.max)}else{(y.min,y.max)},
            (true,false)=>(x.min,x.max),(false,true)=>(y.min,y.max),(false,false)=>(1.5,2.0),
        };
        let width=rng.random_range_f(low,high);
        let start=V2::new(e.start.0,e.start.1);let end=V2::new(e.end.0,e.end.1);let delta=end.sub(start);
        let probe=start.add(V2::new(delta.x/2.0,delta.y/2.0)).add(normal(delta));
        let poly=Polygon::new(w.cells[e.parents.0].polygon.iter().map(|&(x,y)|V2::new(x,y)).collect());
        let (start,end)=if poly.contains(probe){(start,end)}else{(end,start)};
        let t0=*ds.temperatures.get(&a.temperature_range).ok_or("missing border temperature")?;
        let t1=*ds.temperatures.get(&b.temperature_range).ok_or("missing border temperature")?;
        borders.push((kind.to_string(),width,start,end,t0,t1));
    }
    let priority:crate::fxhash::FxHashSet<_>=terrain.priority_cells.iter().copied().collect();
    let mut written=0;
    for (kind,mut width,start,end,a,b) in borders {
        let t0=((a.0 as f64+a.1 as f64)/2.0) as f32;
        let t1=((b.0 as f64+b.1 as f64)/2.0) as f32;
        let range=a.1.max(b.1)-a.0.min(b.0);
        let mut mid=((t0 as f64+t1 as f64)/2.0) as f32;
        let (low,high,snap)=if range>=150.0 {
            let high=width*0.2;width=width.max(2.0);
            mid=if (t0-273.15).abs()<(t1-273.15).abs(){t0}else{t1};(0.0,high,2)
        }else{(2.0,5.0,1)};
        let max_distance=rng.random_range_i(8,13) as f32;let range=rng.random_range_f(low,high);
        let before_stagger=rng.state();
        let segments=stagger(start,end,max_distance,range,&mut rng);
        let mut trace=serde_json::json!({"width":width,"kind":kind,"max_distance":max_distance,"stagger_range":range,
            "initial_segments":[[[start.x,start.y],[end.x,end.y]]],"before_stagger_rng":before_stagger,
            "segments":segments.iter().map(|(a,b)|[[a.x,a.y],[b.x,b.y]]).collect::<Vec<_>>(),"after_stagger_rng":rng.state(),
            "temperatures":[t0,t1,mid],"snap":snap});
        let group=ChoiceGroup {selection_method:String::new(),choices:ds.borders.get(&kind).ok_or_else(||format!("missing border {kind}"))?.clone()};
        for (start,end) in segments {
            let perpendicular=normal(end.sub(start));
            for (x,y) in crate::feature_shapes::line(start,end) {
                let mut set=|x:i32,y:i32,temp:f32|->Result<(),String> {
                    let index=x as i64+y as i64*w.size_x as i64;
                    if index<0||index>=terrain.cells.len() as i64{return Ok(());}
                    // SetCell chooses before the setter checks priority claims.
                    let choice=group.choose(&mut rng).ok_or("empty border choices")?;
                    let def=crate::terrain::find_element(&ds.elements,&choice.element).ok_or("missing border element")?;
                    let mut cell=def.default_cell();let mut override_temp=false;
                    cell.apply_disease(choice.overrides.as_ref());
                    if let Some(o)=&choice.overrides {
                        if let Some(v)=o.mass_override{cell.mass=v;}if let Some(v)=o.mass_multiplier{cell.mass*=v;}
                        if let Some(v)=o.temperature_override{cell.temperature=v;override_temp=true;}
                        if let Some(v)=o.temperature_multiplier{cell.temperature*=v;override_temp=true;}
                    }
                    if !override_temp{cell.temperature=temp;}
                    if def.material_category=="Special"||def.tags.iter().any(|t|t=="Special"){
                        let disease=(cell.disease,cell.disease_count);cell=def.default_cell();cell.disease=disease.0;cell.disease_count=disease.1;
                    }
                    if !priority.contains(&(index as usize)){terrain.cells[index as usize]=Some(cell);written+=1;}
                    Ok(())
                };
                set(x,y,mid)?;
                let mut distance=0.5_f32;
                while distance<=width {
                    let mut mix=((distance-0.5)/(width-0.5)).clamp(0.0,1.0);
                    if distance+snap as f32>width{mix=1.0;}
                    let offset=perpendicular.mul(distance);
                    let temp0=(mid as f64+(t0 as f64-mid as f64)*mix as f64) as f32;
                    let temp1=(mid as f64+(t1 as f64-mid as f64)*mix as f64) as f32;
                    set((x as f32+offset.x) as i32,(y as f32+offset.y) as i32,temp0)?;
                    set((x as f32-offset.x) as i32,(y as f32-offset.y) as i32,temp1)?;
                    distance+=1.0;
                }
            }
        }
        trace["after_render_rng"]=serde_json::to_value(rng.state()).map_err(|e|e.to_string())?;
        terrain.border_trace.push(trace);
    }
    Ok(written)
}
