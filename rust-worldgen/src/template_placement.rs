//! Template placement primitives, matching TemplateSpawning rather than the
//! earlier approximate parent-cell geyser placement scaffold.
use crate::{data_source::DataSource,diagnostics::{Failure,RuleOrigin,RunError},generate::WorldOutput,geom::{Polygon,V2},sampler::SeededRandom,subworld::TerrainRegion};
use std::collections::BTreeMap;

#[derive(Debug,Clone,PartialEq,serde::Serialize,serde::Deserialize)]
#[serde(default,rename_all="camelCase")]
pub struct Filter {
    pub command:String,pub tagcommand:String,pub tag:Option<String>,
    pub min_distance:i32,pub max_distance:i32,pub ignore_if_missing_tag:bool,
    pub subworld_names:Vec<String>,pub zone_types:Vec<String>,pub temperature_ranges:Vec<String>,
}
impl Default for Filter {fn default()->Self {Self {command:"Replace".into(),tagcommand:"Default".into(),tag:None,
    min_distance:0,max_distance:0,ignore_if_missing_tag:false,subworld_names:Vec::new(),zone_types:Vec::new(),temperature_ranges:Vec::new()}}}

pub fn matches<T:AsRef<str>>(tags:&[T],distances:&BTreeMap<String,i32>,filters:&[Filter])->Result<bool,String> {
    let mut result=false;
    for f in filters {
        let tagged=f.tagcommand!="Default";
        let categories=tagged as usize+(!f.subworld_names.is_empty()) as usize+(!f.zone_types.is_empty()) as usize+(!f.temperature_ranges.is_empty()) as usize;
        let has=|s:&str|tags.iter().any(|t|t.as_ref()==s);
        let matched=if f.command!="All"&&categories!=1 {false} else if tagged {
            let tag=f.tag.as_deref().unwrap_or("");
            match f.tagcommand.as_str() {
                "AtTag"=>has(tag),"NotAtTag"=>!has(tag),
                "DistanceFromTag"=>{let Some(&d)=distances.get(tag) else {continue};d>=f.min_distance&&d<=f.max_distance},
                other=>return Err(format!("unsupported template tag command {other}")),
            }
        }else if !f.subworld_names.is_empty(){f.subworld_names.iter().any(|s|has(s))}
        else if !f.zone_types.is_empty(){f.zone_types.iter().any(|s|has(s))}
        else if !f.temperature_ranges.is_empty(){f.temperature_ranges.iter().any(|s|has(s))}else{true};
        result=match f.command.as_str() {
            "All"=>true,"Clear"=>false,"Replace"=>matched,
            "ExceptWith"|"SymmetricExceptWith"=>result&&!matched,
            "UnionWith"=>result||matched,"IntersectWith"=>result&&matched,
            other=>return Err(format!("unsupported template filter command {other}")),
        };
    }
    Ok(result)
}

pub fn safe_to_spawn<T:AsRef<str>>(tags:&[T],near_start:bool,relaxed:bool)->bool {
    !tags.iter().map(AsRef::as_ref).any(|t|t=="POI"||(!relaxed&&t=="Feature")||(!near_start&&matches!(t,"StartLocation"|"AtStart"|"NearStartLocation")))
}

#[derive(Debug,Clone,Copy,Default,PartialEq,serde::Serialize,serde::Deserialize)]
pub struct IntVector {
    #[serde(default,alias="X")] pub x:i32,
    #[serde(default,alias="Y")] pub y:i32,
}
#[derive(Debug,Clone,PartialEq,serde::Serialize,serde::Deserialize)]
#[serde(default,rename_all="camelCase")]
pub struct Rule {
    pub rule_id:Option<String>, pub names:Vec<String>, pub list_rule:String,
    pub some_count:i32,pub more_count:i32,pub range:IntVector,pub times:i32,pub priority:f32,
    pub allow_duplicates:bool,pub allow_extreme_temperature_overlap:bool,
    pub allow_near_start:bool,pub use_relaxed_filtering:bool,
    pub override_offset:IntVector,pub override_placement:IntVector,pub allowed_cells_filter:Vec<Filter>,
    /// Where the rule is authored; set by [`input_rules`].
    #[serde(skip)] pub origin:Option<RuleOrigin>,
}
impl Default for Rule {
    fn default()->Self {Self {rule_id:None,names:Vec::new(),list_rule:"GuaranteeOne".into(),
        some_count:0,more_count:0,range:IntVector::default(),times:1,priority:0.0,
        allow_duplicates:false,allow_extreme_temperature_overlap:false,allow_near_start:false,use_relaxed_filtering:false,
        override_offset:IntVector::default(),override_placement:IntVector{x:-1,y:-1},allowed_cells_filter:Vec::new(),origin:None}}
}

/// The game's YAML loader parses enums case-insensitively; worlds such as
/// Volcanic spell `listRule: Trysome`. Unknown names are returned unchanged.
pub fn canonical_list_rule(name:&str)->&str {
    const RULES:[&str;9]=["GuaranteeAll","GuaranteeSome","GuaranteeSomeTryMore","GuaranteeOne","TryAll","TrySome","TryOne","GuaranteeRange","TryRange"];
    RULES.iter().copied().find(|r|r.eq_ignore_ascii_case(name)).unwrap_or(name)
}

/// Build the unsorted list consumed by DetermineTemplatesForWorld. Trait
/// mutations apply sequentially to world rules, before subworld rules append.
pub fn input_rules(ds:&DataSource,w:&WorldOutput,additional:&[serde_yaml::Value])->Result<Vec<Rule>,String> {
    let world=ds.worlds.get(&w.name).ok_or("missing template world")?;
    let parse=|v:&serde_yaml::Value,origin:Option<RuleOrigin>|serde_yaml::from_value::<Rule>(v.clone()).map(|r|Rule{origin,..r}).map_err(|e|e.to_string());
    let mut rules:Vec<Rule>=world.world_template_rules.iter().enumerate().map(|(i,v)|parse(v,Some(RuleOrigin::new("world",&w.name,i,v))))
        .chain(additional.iter().map(|v|parse(v,None))).collect::<Result<_,_>>()?;
    let mut subworlds:Vec<_>=world.subworld_files.iter().map(|s|s.name.as_str()).collect();
    for name in w.world_traits.as_ref().ok_or("template rules require selected traits")? {
        let t=&ds.traits.iter().find(|(n,_)|n==name).ok_or_else(||format!("missing trait {name}"))?.1;
        rules.retain(|r|!r.rule_id.as_ref().is_some_and(|id|t.remove_world_template_rules_by_id.contains(id)));
        rules.extend(t.additional_world_template_rules.iter().enumerate()
            .map(|(i,v)|parse(v,Some(RuleOrigin::new("worldTrait",name,i,v).in_world(&w.name)))).collect::<Result<Vec<_>,_>>()?);
        subworlds.extend(t.additional_subworld_files.iter().map(|s|s.name.as_str()));
    }
    // Mixing appends after traits: world mixing in Cluster.InitializeWorlds, subworld mixing in BeginGeneration.
    rules.extend(w.mixing.additional_world_template_rules.iter().enumerate()
        .map(|(i,v)|parse(v,w.mixing.rule_origins.get(i).cloned())).collect::<Result<Vec<_>,_>>()?);
    subworlds.extend(w.mixing.additional_subworld_files.iter().map(|s|s.name.as_str()));
    for name in subworlds {
        let sw=ds.subworlds.get(name).ok_or_else(||format!("missing template subworld {name}"))?;
        for (i,raw) in sw.subworld_template_rules.iter().enumerate() {
            let mut rule=parse(raw,Some(RuleOrigin::new("subworld",name,i,raw).in_world(&w.name)))?;
            rule.allowed_cells_filter.insert(0,Filter{subworld_names:vec![name.into()],..Filter::default()});
            rules.push(rule);
        }
    }
    Ok(rules)
}

/// Mono's List.Sort is unstable, including for equal-priority rules. Rust's
/// sorting implementations produce different tie orders above 16 entries.
pub fn sort_rules(rules:&mut [Rule]) {
    mono_sort(rules,&|a:&Rule,b:&Rule|a.priority>b.priority||(!a.priority.is_nan()&&b.priority.is_nan()));
}

/// Mono's ArraySortHelper.IntrospectiveSort; `less(a,b)` is `comparer(a,b)<0`.
pub fn mono_sort<T:Clone>(a:&mut [T],less:&impl Fn(&T,&T)->bool) {
    fn swap_if<T>(a:&mut [T],less:&impl Fn(&T,&T)->bool,i:usize,j:usize){if less(&a[j],&a[i]){a.swap(i,j);}}
    fn down<T:Clone>(a:&mut [T],less:&impl Fn(&T,&T)->bool,mut i:usize,n:usize,lo:usize) {
        let value=a[lo+i-1].clone();
        while i<=n/2 {
            let mut child=2*i;
            if child<n&&less(&a[lo+child-1],&a[lo+child]){child+=1;}
            if !less(&value,&a[lo+child-1]){break;}
            a[lo+i-1]=a[lo+child-1].clone();i=child;
        }
        a[lo+i-1]=value;
    }
    fn intro<T:Clone>(a:&mut [T],less:&impl Fn(&T,&T)->bool,lo:usize,mut hi:usize,mut depth:u32) {
        while hi>lo {
            let n=hi-lo+1;
            if n<=16 {
                if n==2 {swap_if(a,less,lo,hi);}
                else if n==3 {swap_if(a,less,lo,hi-1);swap_if(a,less,lo,hi);swap_if(a,less,hi-1,hi);}
                else {for i in lo+1..=hi {let value=a[i].clone();let mut j=i;while j>lo&&less(&value,&a[j-1]){a[j]=a[j-1].clone();j-=1;}a[j]=value;}}
                return;
            }
            if depth==0 {
                for i in (1..=n/2).rev(){down(a,less,i,n,lo);}
                for i in (2..=n).rev(){a.swap(lo,lo+i-1);down(a,less,1,i-1,lo);}
                return;
            }
            depth-=1;
            let mid=lo+(hi-lo)/2;
            swap_if(a,less,lo,mid);swap_if(a,less,lo,hi);swap_if(a,less,mid,hi);
            let pivot=a[mid].clone();a.swap(mid,hi-1);
            let (mut left,mut right)=(lo,hi-1);
            loop {
                left+=1;while less(&a[left],&pivot){left+=1;}
                right-=1;while less(&pivot,&a[right]){right-=1;}
                if left>=right{break;}a.swap(left,right);
            }
            a.swap(left,hi-1);
            intro(a,less,left+1,hi,depth);hi=left-1;
        }
    }
    if a.len()>1 {let n=a.len();intro(a,less,0,n-1,2*(usize::BITS-n.leading_zeros()));}
}

#[derive(Debug,Clone,serde::Serialize,serde::Deserialize)]
pub struct Geometry {pub min:(i32,i32),pub size:(i32,i32)}
impl Geometry {
    pub fn from_yaml(y:&serde_yaml::Value)->Self {
        let v=|field:&str,axis:&str|y.get("info").and_then(|v|v.get(field)).and_then(|v|v.get(axis)).and_then(|v|v.as_f64()).unwrap_or(0.0) as f32;
        let mx=v("min","X");let my=v("min","Y");
        let mut result=Self {min:(mx as i32,my as i32),size:(v("size","X") as i32,v("size","Y") as i32)};
        if mx*mx+my*my<=1.0e-6 {
            if let Some(cells)=y.get("cells").and_then(|v|v.as_sequence()) {
                let (mut left,mut right,mut bottom,mut top)=(1,-1,1,-1);
                for c in cells {
                    let x=c.get("location_x").and_then(|v|v.as_i64()).unwrap_or(0) as i32;
                    let y=c.get("location_y").and_then(|v|v.as_i64()).unwrap_or(0) as i32;
                    left=left.min(x);right=right.max(x);bottom=bottom.min(y);top=top.max(y);
                }
                result.min=(left,bottom);result.size=(1+right-left,1+top-bottom);
            }
        }
        result
    }
    pub fn bounds(&self,position:(i32,i32),padding:i32)->(i32,i32,i32,i32) {
        (position.0+self.min.0-padding,position.1+self.min.1-padding,self.size.0+2*padding,self.size.1+2*padding)
    }
}
pub fn overlaps(a:(i32,i32,i32,i32),b:(i32,i32,i32,i32))->bool {
    a.0+a.2>b.0&&a.0<b.0+b.2&&a.1+a.3>b.1&&a.1<b.1+b.3
}

pub fn entry_rng(w:&WorldOutput)->SeededRandom {
    let mut rng=SeededRandom::new(w.world_seed);
    // WorldGen.myRandom is independent of the terrain-rendering RNG. Its only
    // preceding draws are the ordered SwapLakesToBelow child shuffles.
    for (r,parent) in w.child_regions.iter().zip(&w.parent_metadata) {
        if parent.tags.iter().any(|t|t=="SwapLakesToBelow")&&!parent.tags.iter().any(|t|t=="CenteralFeature") {
            let mut nodes:Vec<_>=r.after_relax_tags.iter().enumerate().filter(|(_,t)|!t.iter().any(|t|t=="CenteralFeature")).map(|(i,_)|i).collect();
            crate::sampler::shuffle_seeded(&mut nodes,&mut rng);
        }
    }
    rng
}

/// The world's regions with the layout tag rules applied; only the regions
/// a rule changes are copied.
pub fn input_regions<'a>(ds:&DataSource,w:&'a WorldOutput)->Result<Vec<std::borrow::Cow<'a,TerrainRegion>>,String> {
    let mut regions:Vec<_>=w.terrain_regions.iter().map(std::borrow::Cow::Borrowed).collect();
    for rule in &ds.worlds[&w.name].modify_layout_tags {
        let filters:Vec<Filter>=serde_yaml::from_value(rule.get("allowedCellsFilter").cloned().unwrap_or(serde_yaml::Value::Sequence(Vec::new()))).map_err(|e|e.to_string())?;
        for r in &mut regions {
            if !matches(&r.tags,&r.distances_to_tags,&filters)?{continue;}
            if let Some(add)=rule.get("addTags").and_then(|v|v.as_sequence()) {for tag in add {let tag=tag.as_str().ok_or("invalid layout tag")?;if !r.tags.iter().any(|t|t==tag){r.to_mut().tags.push(tag.into());}}}
            if let Some(remove)=rule.get("removeTags").and_then(|v|v.as_sequence()) {for tag in remove {let tag=tag.as_str().ok_or("invalid layout tag")?;if r.tags.iter().any(|t|t==tag){r.to_mut().tags.retain(|t|t!=tag);}}}
        }
    }
    Ok(regions)
}

#[derive(Debug,serde::Serialize)]
pub struct Placement {pub template:String,pub region_id:usize,pub position:(i32,i32),pub bounds:(i32,i32,i32,i32)}
/// The game's message for a starting template overlapping the world border
/// rectangles, followed by the world file path and a closing quote.
pub const START_OVERLAP:&str="TemplateSpawning: Starting template overlaps world boundaries in world '";
pub fn starting<R:std::borrow::Borrow<TerrainRegion>>(ds:&DataSource,w:&WorldOutput,regions:&[R],placed_bounds:&[(i32,i32,i32,i32)])->Result<Option<Placement>,String> {
    let Some(name)=ds.worlds[&w.name].starting_base_template.as_deref().filter(|s|!s.trim().is_empty()) else {return Ok(None)};
    let region=regions.iter().map(|r|r.borrow()).find(|r|r.tags.iter().any(|t|t=="StartLocation")).ok_or("starting template has no StartLocation")?;
    let geometry=ds.template_geometry.get(name).ok_or_else(||format!("missing template geometry {name}"))?;
    let point=Polygon::new(region.polygon.iter().map(|&(x,y)|V2::new(x,y)).collect()).centroid();
    let position=(point.x as i32,point.y as i32);
    let padding=ds.world_setting(&w.name,"POIPadding")?.as_i64().ok_or("invalid POIPadding")? as i32;
    let bounds=geometry.bounds(position,padding);
    // SpawnStartingTemplate throws a plain Exception with this text; RenderOffline
    // reports it through ReportWorldGenError, so it is a worldgen failure.
    if placed_bounds.iter().any(|&b|overlaps(bounds,b)){return Err(format!("{START_OVERLAP}{}'",w.name));}
    Ok(Some(Placement {template:name.into(),region_id:region.id,position,bounds}))
}

#[derive(Debug,serde::Serialize)]
pub struct Output {
    pub placements:Vec<Placement>,pub final_bounds:Vec<(i32,i32,i32,i32)>,pub after_rng:crate::rng::RandomState,
    #[serde(skip)] pub notes:Vec<Note>,
}

/// Place ordinary templates for a world whose layout is already generated.
pub fn place(ds:&DataSource,w:&WorldOutput,rules:Vec<Rule>,initial_bounds:&[(i32,i32,i32,i32)])->Result<Output,String> {
    Ok(place_with_stories(ds,w,rules,initial_bounds,&[])?.0)
}

/// A non-fatal outcome the game logs while placing templates.
#[derive(Debug,Clone,PartialEq)]
pub enum Note {
    /// `SpawnTemplate` retrying a guaranteed template with relaxed filtering.
    Relaxed{template:String,near_start:bool},
    /// A rule's iteration placed none of the templates it tried, without
    /// failing (a Try rule); the game collects them as "Could not place
    /// templates" but only reports guaranteed rules.
    Unplaced{rule:Option<RuleOrigin>,list_rule:String,templates:Vec<String>},
    /// `SpawnStoryTraitTemplates` giving up on a story for this world.
    StoryNotPlaced{story:String,error:String},
}

/// DetermineTemplatesForWorld: starting template, template rules, then
/// SpawnStoryTraitTemplates for `story_candidates` (story trait file paths in
/// queue order). Returns the placed story traits in ApplyStoryTrait order.
pub fn place_with_stories(ds:&DataSource,w:&WorldOutput,mut rules:Vec<Rule>,initial_bounds:&[(i32,i32,i32,i32)],story_candidates:&[String])->Result<(Output,Vec<String>),RunError> {
    let mut p=match Placer::new(ds,w,initial_bounds) {
        Ok(p)=>p,
        Err(error) if error.starts_with(START_OVERLAP)=>return Err(RunError::failure(format!("{}{error}",crate::diagnostics::PREFIX),Failure {stage:"TemplateSpawning",
            world_index:w.index as i32,reporting_world_index:Some(w.index as i32),world:Some(w.name.clone()),message:error,origin:None})),
        Err(error)=>return Err(error.into()),
    };
    let mut used=std::collections::HashSet::new();
    sort_rules(&mut rules);
    for rule in &rules {
        // TemplateSpawning throws WorldgenException(errorMessage).
        if let Some(error)=p.apply_rule(rule,&mut used)? {
            return Err(RunError::failure(format!("{}{error}",crate::diagnostics::PREFIX),Failure {stage:"TemplateSpawning",world_index:w.index as i32,
                reporting_world_index:Some(w.index as i32),world:Some(w.name.clone()),message:error,origin:rule.origin.clone()}));
        }
    }
    let placed=p.spawn_story_traits(story_candidates)?;
    Ok((Output {placements:p.placements,final_bounds:p.bounds,after_rng:p.rng.state(),notes:p.notes},placed))
}

/// `RectInt.center == (Vector2)position`, exact for integer rectangles.
fn centered_at(b:(i32,i32,i32,i32),position:(i32,i32))->bool {2*b.0+b.2==2*position.0&&2*b.1+b.3==2*position.1}

struct Placer<'a> {
    ds:&'a DataSource,regions:Vec<std::borrow::Cow<'a,TerrainRegion>>,polygons:Vec<Polygon>,centers:Vec<V2>,temperatures:Vec<(String,(f32,f32))>,
    padding:i32,bounds:Vec<(i32,i32,i32,i32)>,placements:Vec<Placement>,placement_regions:Vec<usize>,
    rng:SeededRandom,template_tags:Vec<Option<String>>,
    /// World file path and `GetWorldTraitIDs()`, for the game's failure messages.
    world_path:String,world_traits:String,
    notes:Vec<Note>,
}
impl<'a> Placer<'a> {
    fn new(ds:&'a DataSource,w:&'a WorldOutput,initial_bounds:&[(i32,i32,i32,i32)])->Result<Self,String> {
        let regions=input_regions(ds,w)?;
        let mut polygons:Vec<_>=regions.iter().map(|r|Polygon::new(r.polygon.iter().map(|&(x,y)|V2::new(x,y)).collect())).collect();
        let centers:Vec<_>=polygons.iter_mut().map(Polygon::centroid).collect();
        let temperatures:Vec<_>=regions.iter().map(|r|{
            let name=&ds.subworlds.get(&r.subworld).ok_or_else(||format!("missing subworld {}",r.subworld))?.temperature_range;
            Ok((name.clone(),*ds.temperatures.get(name).ok_or_else(||format!("missing temperature {name}"))?))
        }).collect::<Result<_,String>>()?;
        let padding=ds.world_setting(&w.name,"POIPadding")?.as_i64().ok_or("invalid POIPadding")? as i32;
        let mut bounds=initial_bounds.to_vec();let mut placements=Vec::new();let mut placement_regions=Vec::new();
        if let Some(start)=starting(ds,w,&regions,&bounds)? {
            bounds.push(start.bounds);
            placement_regions.push(regions.iter().position(|r|r.id==start.region_id).ok_or("missing starting region")?);
            placements.push(start);
        }
        let template_tags=vec![None;regions.len()];
        let world_traits=w.world_traits.as_deref().unwrap_or(&[]).join(", ");
        Ok(Self {ds,regions,polygons,centers,temperatures,padding,bounds,placements,placement_regions,rng:entry_rng(w),template_tags,world_path:w.name.clone(),world_traits,notes:Vec::new()})
    }

    /// ApplyTemplateRule. `Ok(Some(message))` is the game's `false` result.
    fn apply_rule(&mut self,rule:&Rule,used:&mut std::collections::HashSet<String>)->Result<Option<String>,String> {
        let ds=self.ds;
        for _ in 0..rule.times {
            let mut candidates:Vec<_>=rule.names.iter().filter(|name|(rule.allow_duplicates||!used.contains(*name))&&ds.template_geometry.contains_key(*name)).cloned().collect();
            crate::sampler::shuffle_seeded(&mut candidates,&mut self.rng);
            if candidates.is_empty(){continue;}
            let list_rule=canonical_list_rule(&rule.list_rule);
            let range=if matches!(list_rule,"GuaranteeRange"|"TryRange"){self.rng.random_range_i(rule.range.x,rule.range.y)}else{0};
            let (mut guaranteed,mut attempts)=match list_rule {
                "GuaranteeAll"=>(candidates.len() as i32,candidates.len() as i32),
                "GuaranteeSome"=>(rule.some_count,rule.some_count),
                "GuaranteeSomeTryMore"=>(rule.some_count,rule.some_count+rule.more_count),
                "GuaranteeOne"=>(1,1),"GuaranteeRange"=>(range,range),
                "TryAll"=>(0,candidates.len() as i32),"TrySome"=>(0,rule.some_count),"TryOne"=>(0,1),"TryRange"=>(0,range),
                other=>return Err(format!("unsupported template list rule {other}")),
            };
            let count=candidates.len();
            let mut unplaced=String::new();
            let mut unplaced_names=Vec::new();
            let placed_before=self.placements.len();
            for name in candidates {
                if attempts<=0{break;}
                let geometry=&ds.template_geometry[&name];
                let explicit=rule.override_placement!=IntVector{x:-1,y:-1};
                let target=if explicit {
                    let pos=V2::new(rule.override_placement.x as f32,rule.override_placement.y as f32);
                    let index=self.polygons.iter().position(|p|p.contains(pos)).ok_or("override template position outside all regions")?;
                    if let (true,Some(tag))=(guaranteed>0,&self.template_tags[index]) {
                        return Ok(Some(format!("Tried to place '{name}' at ({},{}) using overridePlacement but '{tag}' is already there.",rule.override_placement.x,rule.override_placement.y)));
                    }
                    Some(index)
                }else {
                    let (regions,centers,bounds,temperatures,polygons,padding)=(&self.regions,&self.centers,&self.bounds,&self.temperatures,&self.polygons,self.padding);
                    let filter=|near,relaxed|->Result<Vec<usize>,String> {
                        let mut valid=Vec::new();
                        for (i,r) in regions.iter().enumerate() {
                            if !safe_to_spawn(&r.tags,near,relaxed)||!matches(&r.tags,&r.distances_to_tags,&rule.allowed_cells_filter)?{continue;}
                            let c=centers[i];
                            let b=geometry.bounds(((c.x+rule.override_offset.x as f32) as i32,(c.y+rule.override_offset.y as f32) as i32),padding);
                            if bounds.iter().any(|&other|overlaps(b,other)){continue;}
                            if !rule.allow_extreme_temperature_overlap {
                                let (low,high)=temperatures[i].1;
                                let extreme=temperatures.iter().enumerate().any(|(j,(key,(l,h)))| {
                                    if *key==temperatures[i].0||high.max(*h)-low.min(*l)<=100.0{return false;}
                                    let other=polygons[j].bounds;
                                    let (x,y)=(b.0 as f32,b.1 as f32);
                                    let (width,height)=((b.2-2*padding+6) as f32,(b.3-2*padding+6) as f32);
                                    x+width>other.x&&x<other.x+other.width&&y+height>other.y&&y<other.y+other.height
                                });
                                if extreme{continue;}
                            }
                            valid.push(i);
                        }
                        Ok(valid)
                    };
                    let mut valid=filter(rule.allow_near_start,!rule.allow_near_start&&rule.use_relaxed_filtering)?;
                    if valid.is_empty()&&guaranteed>0 {
                        if rule.allow_near_start&&rule.use_relaxed_filtering {
                            self.notes.push(Note::Relaxed{template:name.clone(),near_start:true});
                            valid=filter(true,true)?;
                        }
                        else if !rule.use_relaxed_filtering {
                            self.notes.push(Note::Relaxed{template:name.clone(),near_start:false});
                            valid=filter(false,true)?;
                        }
                    }
                    crate::sampler::shuffle_seeded(&mut valid,&mut self.rng);
                    valid.last().copied()
                };
                if let Some(index)=target {
                    let c=self.centers[index];
                    let pos=if explicit {(rule.override_placement.x,rule.override_placement.y)}else{(c.x as i32+rule.override_offset.x,c.y as i32+rule.override_offset.y)};
                    let b=geometry.bounds(pos,self.padding);self.bounds.push(b);
                    self.placements.push(Placement{template:name.clone(),region_id:self.regions[index].id,position:pos,bounds:b});
                    self.placement_regions.push(index);
                    self.template_tags[index]=Some(name.clone());
                    for tag in [&name,"POI"] {if !self.regions[index].tags.iter().any(|t|t==tag){self.regions[index].to_mut().tags.push(tag.into());}}
                    used.insert(name);attempts-=1;guaranteed-=1;
                }else {unplaced+=&format!("
    - {name}");unplaced_names.push(name.clone());}
            }
            // Story traits are applied after the ordinary rules, so a failing world rule sees none.
            if guaranteed>0 {return Ok(Some(format!("TemplateSpawning: Guaranteed placement failure on {}
    listRule={list_rule} someCount={} moreCount={} count={count}
    Could not place templates:{unplaced}
    world traits={}
    story traits=",
                self.world_path,rule.some_count,rule.more_count,self.world_traits)));}
            if self.placements.len()==placed_before&&!unplaced_names.is_empty() {
                self.notes.push(Note::Unplaced{rule:rule.origin.clone(),list_rule:list_rule.to_string(),templates:unplaced_names});
            }
        }
        Ok(None)
    }

    /// SpawnStoryTraitTemplates. Each candidate is dequeued once; its rules run
    /// in priority order on the shared myRandom with a per-world usedTemplates
    /// set. A failed story's new spawners are undone with RemoveTemplate.
    fn spawn_story_traits(&mut self,candidates:&[String])->Result<Vec<String>,String> {
        let mut placed=Vec::new();
        let mut used=std::collections::HashSet::new();
        for id in candidates {
            let story=self.ds.story_trait(id).ok_or_else(||format!("missing story trait {id}"))?;
            let mut rules:Vec<Rule>=story.additional_world_template_rules.iter().enumerate()
                .map(|(i,v)|serde_yaml::from_value(v.clone()).map(|r|Rule{origin:Some(RuleOrigin::new("storyTrait",id,i,v).in_world(&self.world_path)),..r})
                .map_err(|e|format!("story trait {id}: {e}"))).collect::<Result<_,_>>()?;
            sort_rules(&mut rules);
            let first_new=self.placements.len();
            // `flag` starts false, so a story without rules is never placed.
            let mut success=false;
            let mut error=String::new();
            for rule in &rules {
                if let Some(message)=self.apply_rule(rule,&mut used)? {error=message;success=false;break;}
                success=true;
            }
            if success {placed.push(id.clone());continue;}
            self.notes.push(Note::StoryNotPlaced{story:id.clone(),error});
            let removed=self.placements.split_off(first_new).into_iter().zip(self.placement_regions.split_off(first_new));
            for (spawner,index) in removed {
                // UpdateNodeTags(remove: true) clears the template tag and POI.
                self.template_tags[index]=None;
                if self.regions[index].tags.iter().any(|t|*t==spawner.template||t=="POI") {self.regions[index].to_mut().tags.retain(|t|*t!=spawner.template&&t!="POI");}
                // placedPOIBounds.RemoveAll(bound.center == position): RectInt.center
                // is (x+w/2, y+h/2), so off-centre templates leave their bounds
                // behind, and any other bounds sharing that centre are dropped.
                self.bounds.retain(|&b|!centered_at(b,spawner.position));
                used.remove(&spawner.template);
            }
        }
        Ok(placed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn list_rules_are_case_insensitive_like_the_game_yaml_loader() {
        assert_eq!(canonical_list_rule("Trysome"),"TrySome");
        assert_eq!(canonical_list_rule("guaranteeall"),"GuaranteeAll");
        assert_eq!(canonical_list_rule("Bogus"),"Bogus");
    }
    #[test]
    fn remove_template_only_drops_bounds_centred_on_the_spawn_position() {
        // creature_manipulator.yaml: size 16x11, min (-7,-5); centre is (x+1, y+0.5).
        let g=Geometry{min:(-7,-5),size:(16,11)};
        assert!(!centered_at(g.bounds((50,60),2),(50,60)));
        let g=Geometry{min:(-8,-5),size:(16,10)};
        assert!(centered_at(g.bounds((50,60),2),(50,60)));
    }
    #[test]
    fn story_rules_parse_including_a_filter_without_tag() {
        // FossilHunt's magma rule has `tagcommand: AtTag` with no tag; Tag(null)
        // hashes to 0 and matches no node, so the ExceptWith is a no-op.
        let rule:Rule=serde_yaml::from_str("names: [storytraits/fossil_hunt_rock]
listRule: TryOne
priority: 300
useRelaxedFiltering: true
allowedCellsFilter:
  - command: Replace
    zoneTypes: [MagmaCore]
  - command: ExceptWith
    tagcommand: AtTag").unwrap();
        assert_eq!((rule.times,rule.allow_near_start,rule.override_placement),(1,false,IntVector{x:-1,y:-1}));
        assert!(matches(&["MagmaCore"],&BTreeMap::new(),&rule.allowed_cells_filter).unwrap());
    }
    #[test]
    fn missing_distance_skips_intersection_and_symmetric_except_does_not_toggle() {
        let filters:Vec<Filter>=serde_yaml::from_str("- command: All\n- command: IntersectWith\n  tagcommand: DistanceFromTag\n  tag: AtStart\n  minDistance: 1\n  maxDistance: 2\n  ignoreIfMissingTag: true").unwrap();
        assert!(matches::<&str>(&[],&BTreeMap::new(),&filters).unwrap());
        let filters:Vec<Filter>=serde_yaml::from_str("- command: SymmetricExceptWith\n  tagcommand: AtTag\n  tag: POI").unwrap();
        assert!(!matches(&["POI"],&BTreeMap::new(),&filters).unwrap());
    }
}
