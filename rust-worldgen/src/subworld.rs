//! Child-site sampling, relaxation and biome/feature selection. Sampling uses
//! a per-parent RNG while classifications consume the world's continuing RNG.
use crate::geom::Polygon;
use crate::name::Name;
use crate::sampler::{get_random_points_in_polygon, SampleBehaviour, SeededRandom};
use crate::{data_source::{DataSource, WorldYaml, TraitYaml}, generate::BiomeCell,
    geom::V2, voronoi_tree::{PowerDiagram, PowerDiagramSite}};

#[derive(Debug, Clone, serde::Serialize)]
pub struct ChildRegion {
    pub parent_id: usize,
    pub seed: i32,
    pub subworld: String,
    pub initial_sites: Vec<[f32; 2]>,
    pub after_initial_voronoi: Vec<BiomeCell>,
    pub after_relax: Vec<BiomeCell>,
    pub labels: Vec<ChildLabel>,
    pub initial_tags: Vec<Vec<Name>>,
    pub after_relax_tags: Vec<Vec<Name>>,
    pub neighbours: Vec<Vec<usize>>,
}

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct ParentMetadata {
    pub tags: Vec<Name>,
    pub distances: std::collections::BTreeMap<String,i32>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ChildLabel {
    pub node_type: String,
    pub biome_tags: Vec<Name>,
    pub feature_tags: Vec<Name>,
}

pub(crate) fn tag_add(tags: &mut Vec<Name>, value: &str) {
    if !tags.iter().any(|s|*s==value) { tags.push(Name::new(value)); }
}
fn tag_union(tags: &mut Vec<Name>, yaml: Option<&serde_yaml::Value>) {
    if let Some(values) = yaml.and_then(|v|v.as_sequence()) {
        for tag in values.iter().filter_map(|v|v.as_str()) { tag_add(tags,tag); }
    }
}
fn choose_label(config: &ChildConfig, feature: Option<&serde_yaml::Value>, ds: &DataSource,
    rng: &mut SeededRandom) -> Result<ChildLabel,String> {
    let mut label = ChildLabel {node_type:String::new(),biome_tags:Vec::new(),feature_tags:Vec::new()};
    let mut assigned = false;
    if let Some(feature) = feature {
        let name = feature.get("type").and_then(|v|v.as_str()).ok_or("feature missing type")?;
        let settings = ds.features.get(name).ok_or_else(||format!("missing feature {name}"))?;
        label.node_type = name.to_string();
        for t in &settings.tags { tag_add(&mut label.feature_tags,t); }
        tag_union(&mut label.feature_tags,feature.get("tags"));
        if let Some(excludes) = feature.get("excludesTags").and_then(|v|v.as_sequence()) {
            label.feature_tags.retain(|t|!excludes.iter().any(|e|e.as_str()==Some(t)));
        }
        tag_add(&mut label.feature_tags,name);
        tag_add(&mut label.feature_tags,"Feature");
        if let Some(biome) = settings.force_biome.as_deref() {
            tag_add(&mut label.biome_tags,biome);
            assigned = true;
        }
        if let Some(tags) = settings.biome_tags.as_ref() {
            for t in tags { tag_add(&mut label.biome_tags,t); }
        }
    }
    if !assigned && !config.biomes.is_empty() {
        let weight = |v: &serde_yaml::Value|v.get("weight").and_then(|v|v.as_f64()).unwrap_or(0.0) as f32;
        let total = config.biomes.iter().fold(0.0f32,|s,b|s+weight(b));
        let target = rng.random_value()*total;
        let mut sum = 0.0f32;
        let mut biome = config.biomes.last().unwrap();
        for b in &config.biomes { sum += weight(b); if sum>target { biome=b; break; } }
        let name = biome.get("name").and_then(|v|v.as_str()).ok_or("biome missing name")?;
        if label.node_type.is_empty() { label.node_type=name.to_string(); }
        tag_add(&mut label.biome_tags,name);
        tag_union(&mut label.biome_tags,biome.get("tags"));
        assigned=true;
    }
    if !assigned { return Err("no biome for child".into()); }
    Ok(label)
}

fn geometry(diagram: &PowerDiagram) -> Result<Vec<BiomeCell>, String> {
    diagram.sites.iter().map(|s| {
        let poly = s.poly.as_ref().ok_or("child polygon missing")?;
        Ok(BiomeCell { position:(s.position.x,s.position.y), subworld:None,
            polygon:poly.vertices.iter().map(|v|(v.x,v.y)).collect() })
    }).collect()
}

/// GenerateChildren's initial Voronoi already puts every child at its centroid,
/// so RelaxRecursive's measured energy is zero on its first iteration. It still
/// computes that iteration before stopping. Convex parent regions keep all the
/// centroids inside; reject degenerate inputs requiring PlaceSites recovery.
pub fn relax_initial_children(diagram: PowerDiagram, power: bool) -> Result<PowerDiagram, String> {
    for (i,s) in diagram.sites.iter().enumerate() {
        if !diagram.bounds.contains(s.position) || diagram.sites[..i].iter().any(|p|p.position==s.position) {
            return Err("degenerate child sites require PlaceSites recovery".into());
        }
    }
    let mut relaxed = PowerDiagram::new(diagram.bounds, diagram.sites.iter().map(|s|
        PowerDiagramSite::new(s.id,s.position,s.weight)).collect());
    if power {
        relaxed.compute_vd()?;
        relaxed.compute_power_diagram(500,0.2)?;
        relaxed.recenter();
    } else { relaxed.compute_node_voronoi(); }
    Ok(relaxed)
}

/// Native seed-driven geometry and classifications; inherited/moved tags follow.
pub fn generate_regions(cells: &[BiomeCell], world: &WorldYaml, traits: &[&TraitYaml],
    ds: &DataSource, seed: i32, world_rng: &mut SeededRandom, parents:&[ParentMetadata]) -> Result<Vec<ChildRegion>, String> {
    let mut features = world.global_features.clone();
    for t in traits {
        for (name, amount) in &t.global_feature_mods {
            let count = (world.world_trait_scale * amount).floor() as i32;
            if let Some((_, existing)) = features.iter_mut().find(|(n, _)| n == name) { *existing += count; }
            else { features.push((name.clone(), count)); }
        }
    }
    let mut eligible: Vec<usize> = cells.iter().enumerate().filter_map(|(i,c)| {
        c.subworld.as_deref()?;
        (!parents[i].tags.iter().any(|t| t == "NoGlobalFeatureSpawning")).then_some(i)
    }).collect();
    crate::sampler::shuffle_seeded(&mut eligible, world_rng);
    let mut assigned: Vec<Vec<serde_yaml::Value>> = vec![Vec::new(); cells.len()];
    let mut next = eligible.into_iter();
    for (name, count) in features {
        for _ in 0..count.max(0) {
            let Some(i) = next.next() else { break; };
            assigned[i].push(serde_yaml::to_value(serde_json::json!({"type":name})).unwrap());
        }
    }
    let mut regions:Vec<ChildRegion>=cells.iter().enumerate().map(|(i, cell)| {
        let name = cell.subworld.as_deref().ok_or("unassigned subworld")?;
        let sw = ds.resolve_subworld(name).ok_or_else(|| format!("missing subworld {name}"))?;
        let mut config = sw.children.clone();
        config.features.extend(assigned[i].iter().cloned());
        let child_seed = seed.wrapping_add(i as i32);
        let mut rng = SeededRandom::new(child_seed);
        let mut bounds = Polygon::new(cell.polygon.iter().map(|&(x,y)| V2::new(x,y)).collect());
        let initial_sites = sample_children(&mut bounds, &config, &mut rng)
            .map_err(|e| format!("{name}, parent {}: {e}", i+1))?;
        let mut labels = Vec::with_capacity(initial_sites.len());
        if let Some(feature) = config.central_feature.as_ref() {
            labels.push(choose_label(&config,Some(feature),ds,world_rng)?);
        }
        for j in 0..initial_sites.len()-labels.len() {
            labels.push(choose_label(&config,config.features.get(j),ds,world_rng)?);
        }
        let sites = initial_sites.iter().enumerate().map(|(j,p)|
            PowerDiagramSite::new(j as i32, V2::new(p[0],p[1]),1.0)).collect();
        let mut diagram = PowerDiagram::new(bounds, sites);
        diagram.compute_node_voronoi();
        let after_initial_voronoi = geometry(&diagram)?;
        if !config.sampling.dont_relax_children {
            diagram = relax_initial_children(diagram,world.layout_method=="PowerTree")?;
        }
        let after_relax = geometry(&diagram)?;
        diagram.filter_neighbours();
        let neighbours=diagram.sites.iter().map(|s|s.neighbours.iter().map(|&n|n as usize).collect()).collect();
        let mut inherited=parents[i].tags.clone();
        tag_add(&mut inherited,name);tag_add(&mut inherited,&sw.temperature_range);tag_add(&mut inherited,&sw.zone_type);
        let mut moved=Vec::new();
        for tag in &world.default_move_tags {
            if let Some(index)=inherited.iter().position(|t|t==tag) {inherited.remove(index);tag_add(&mut moved,tag);}
        }
        inherited.retain(|t|t!="Overworld");
        for tag in &sw.tags {tag_add(&mut inherited,tag);}
        let mut initial_tags:Vec<Vec<Name>>=labels.iter().enumerate().map(|(j,label)| {
            let mut tags=inherited.clone();tag_add(&mut tags,&label.node_type);
            for tag in label.biome_tags.iter().chain(&label.feature_tags) {tag_add(&mut tags,tag);}
            if j==0 && config.central_feature.is_some() {tag_add(&mut tags,"CenteralFeature");}
            tags
        }).collect();
        for tag in moved {let child=rng.random_range_i(0,initial_tags.len() as i32) as usize;tag_add(&mut initial_tags[child],&tag);}
        let after_relax_tags=initial_tags.clone();
        Ok(ChildRegion {parent_id:i+1, seed:child_seed, subworld:name.to_string(), initial_sites, after_initial_voronoi, after_relax, labels, initial_tags, after_relax_tags, neighbours})
    }).collect::<Result<_,String>>()?;
    for region in &mut regions {
        let starts:Vec<_>=region.initial_tags.iter().enumerate().filter_map(|(i,t)|t.iter().any(|t|t=="StartLocation").then_some(i)).collect();
        for start in starts {
            for &n in &region.neighbours[start] {tag_add(&mut region.after_relax_tags[n],"NearStartLocation");}
            tag_add(&mut region.after_relax_tags[start],"IgnoreCaveOverride");
        }
    }
    Ok(regions)
}

#[derive(Debug,Clone,serde::Serialize)]
pub struct TerrainRegion {
    pub id:usize, pub parent_id:usize, pub position:(f32,f32), pub polygon:Vec<(f32,f32)>,
    pub node_type:String, pub tags:Vec<Name>, pub leaf_tags:Vec<Name>,
    pub biome_tags:Vec<Name>, pub feature_tags:Vec<Name>,
    /// The parent's distances, shared by its regions.
    pub distances_to_tags:std::sync::Arc<std::collections::BTreeMap<String,i32>>,
    pub subworld:String, pub biome:String, pub biome_index:usize, pub neighbours:Vec<usize>,
}

/// CompleteLayout moves sites between leaf nodes, while the map graph retains
/// biome/feature metadata by site ID. Preserve those two identities separately.
pub fn finalize_regions(regions:&[ChildRegion],parents:&[ParentMetadata],cells:&[BiomeCell],world:&WorldYaml,seed:i32,ds:&DataSource)->Result<Vec<TerrainRegion>,String> {
    let mut tags:Vec<Vec<Vec<Name>>>=regions.iter().map(|r|r.after_relax_tags.clone()).collect();
    let start=regions.iter().enumerate().find_map(|(i,r)|r.after_relax_tags.iter().position(|t|t.iter().any(|t|t=="StartLocation")).map(|j|(i,j)));
    if let Some((i,_))=start {for t in &mut tags[i] {tag_add(t,"IgnoreCaveOverride");}}
    let mut random=SeededRandom::new(seed);
    let mut output=Vec::new();let mut offset=0;
    let distances:Vec<_>=parents.iter().map(|p|std::sync::Arc::new(p.distances.clone())).collect();
    for (i,r) in regions.iter().enumerate() {
        let mut site_map:Vec<usize>=(0..r.labels.len()).collect();
        let mut types:Vec<_>=r.labels.iter().map(|l|l.node_type.clone()).collect();
        if parents[i].tags.iter().any(|t|t=="SwapLakesToBelow") && !parents[i].tags.iter().any(|t|t=="CenteralFeature") {
            let mut nodes:Vec<usize>=(0..r.labels.len()).filter(|&j|!tags[i][j].iter().any(|t|t=="CenteralFeature")).collect();
            crate::sampler::shuffle_seeded(&mut nodes,&mut random);
            let mut bounds=Polygon::new(cells[i].polygon.iter().map(|&(x,y)|V2::new(x,y)).collect());
            let pivot=bounds.centroid().y;
            let mut wet=Vec::new();let mut dry=Vec::new();
            let swap=|a:usize,b:usize,sites:&mut Vec<usize>,types:&mut Vec<String>| {sites.swap(a,b);types.swap(sites[a],sites[b]);};
            for node in nodes {
                let is_wet=tags[i][node].iter().any(|t|t=="Wet");
                let mut p=Polygon::new(r.after_relax[site_map[node]].polygon.iter().map(|&(x,y)|V2::new(x,y)).collect());
                let above=p.centroid().y>pivot;
                if is_wet && above {if dry.is_empty(){wet.push(node);}else{swap(node,dry.remove(0),&mut site_map,&mut types);}}
                else if !is_wet && !above {if wet.is_empty(){dry.push(node);}else{swap(node,wet.remove(0),&mut site_map,&mut types);}}
            }
            for (a,b) in wet.into_iter().zip(dry) {swap(a,b,&mut site_map,&mut types);}
        }
        for (j,&site) in site_map.iter().enumerate() {
            let geom=&r.after_relax[site];let label=&r.labels[site];let mut node_tags=Vec::new();
            if world.start_subworld_name.is_some() && start==Some((i,site)) {tag_add(&mut node_tags,"StartLocation");}
            for t in &tags[i][j] {tag_add(&mut node_tags,t);}
            let subworld=node_tags.iter().find(|t|t.contains("subworlds/")).cloned().unwrap_or_else(||"MISSING".into());
            let biome=node_tags.iter().find(|t|t.contains("biomes/")).cloned().unwrap_or_else(||"MISSING".into());
            output.push(TerrainRegion{id:offset+site+1,parent_id:r.parent_id,position:geom.position,polygon:geom.polygon.clone(),node_type:types[site].clone(),
                tags:node_tags,leaf_tags:tags[i][j].clone(),biome_tags:label.biome_tags.clone(),feature_tags:label.feature_tags.clone(),distances_to_tags:distances[i].clone(),
                subworld:subworld.to_string(),biome:biome.to_string(),biome_index:usize::MAX,neighbours:Vec::new()});
        }
        offset+=r.labels.len();
    }
    let polygons:Vec<_>=output.iter().map(|r|Polygon::new(r.polygon.iter().map(|&(x,y)|V2::new(x,y)).collect())).collect();
    for i in 0..output.len() {for j in i+1..output.len() {
        if polygons[j].may_share_edge(&polygons[i]) && polygons[j].shares_edge(&polygons[i]).0==crate::geom::Commonality::Edge {
            output[i].neighbours.push(j);output[j].neighbours.push(i);
        }
    }}
    let zones=output.iter().map(|r|ds.resolve_subworld(&r.subworld).map(|sw|sw.zone_type.as_str()).ok_or("terrain region missing subworld".to_string())).collect::<Result<Vec<_>,_>>()?;
    let mut biome_index=0;
    for i in 0..output.len() {
        if output[i].biome_index!=usize::MAX {continue;}
        let mut pending=vec![i];output[i].biome_index=biome_index;
        let mut next=0;
        while next<pending.len() {
            for n in output[pending[next]].neighbours.clone() {
                if output[n].biome_index==usize::MAX && zones[n]==zones[i] {output[n].biome_index=biome_index;pending.push(n);}
            }
            next+=1;
        }
        biome_index+=1;
    }
    Ok(output)
}

#[derive(Debug, Clone, Default, serde::Deserialize, serde::Serialize)]
pub struct Density { pub min: f32, pub max: f32 }

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Sampling {
    pub density: Density,
    pub avoid_radius: f32,
    pub sample_behaviour: String,
    pub do_avoid_points: bool,
    pub dont_relax_children: bool,
}
impl Default for Sampling {
    fn default() -> Self {
        Self { density: Density::default(), avoid_radius: 0.0,
            sample_behaviour: "UniformSquare".into(), do_avoid_points: true,
            dont_relax_children: false }
    }
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ChildConfig {
    #[serde(flatten)]
    pub sampling: Sampling,
    pub min_child_count: usize,
    pub extra_biome_children: usize,
    pub single_child_count: bool,
    pub central_feature: Option<serde_yaml::Value>,
    pub features: Vec<serde_yaml::Value>,
    pub biomes: Vec<serde_yaml::Value>,
    pub samplers: Vec<Sampling>,
}
impl Default for ChildConfig {
    fn default() -> Self {
        Self { sampling: Sampling::default(), min_child_count: 2,
            extra_biome_children: 0, single_child_count: false,
            central_feature: None, features: Vec::new(), biomes: Vec::new(), samplers: Vec::new() }
    }
}

fn sample(polygon: &mut Polygon, rng: &mut SeededRandom, config: &Sampling,
    density: f32, radius: f32, avoid: &[[f32; 2]]) -> Result<Vec<[f32; 2]>, String> {
    if !density.is_finite() || density <= 0.0 {
        return Err(format!("invalid child sampler density {density}"));
    }
    let behaviour = match config.sample_behaviour.as_str() {
        "PoissonDisk" => SampleBehaviour::PoissonDisk,
        "UniformHex" => SampleBehaviour::UniformHex,
        "UniformSquare" => SampleBehaviour::UniformSquare,
        other => return Err(format!("unsupported child sampler {other}")),
    };
    Ok(get_random_points_in_polygon(rng, polygon, density, radius, avoid, behaviour, true, true, config.do_avoid_points))
}

/// Returns the central feature first (if present), then shuffled sampler sites.
/// `features` must include any global features assigned to this region. The
/// caller retains `rng` for the game's subsequent moved-tag selection.
/// Marks a child sampler shortfall (`WorldLayout.ChildrenFromSubworld` with
/// fewer points than the minimum), which the game turns into a failure.
pub const SHORT_CHILDREN:&str="child sampler produced";
pub fn sample_children(polygon: &mut Polygon, config: &ChildConfig, rng: &mut SeededRandom)
    -> Result<Vec<[f32; 2]>, String> {
    let mut density = rng.random_range_f(config.sampling.density.min, config.sampling.density.max);
    let mut radius = config.sampling.avoid_radius;
    let mut avoid = Vec::new();
    let mut sites = Vec::new();
    if config.central_feature.is_some() {
        let c = polygon.centroid();
        avoid.push([c.x, c.y]);
        sites.push([c.x, c.y]);
    }
    let minimum = if config.single_child_count { 1 } else {
        (config.features.len() + config.extra_biome_children).max(config.min_child_count)
    };
    let maximum = if config.single_child_count { 1 } else { usize::MAX };
    let mut points = Vec::new();
    for _ in 0..10 {
        points = sample(polygon, rng, &config.sampling, density, radius, &avoid)?;
        if points.len() >= minimum || points.len() > maximum { break; }
        density *= 0.8;
        radius *= 0.8;
    }
    points.truncate(maximum);
    for extra in &config.samplers {
        avoid.extend_from_slice(&points);
        let extra_density = rng.random_range_f(extra.density.min, extra.density.max);
        points.extend(sample(polygon, rng, extra, extra_density, extra.avoid_radius, &avoid)?);
    }
    points.truncate(200);
    if points.len() < minimum {
        return Err(format!("{SHORT_CHILDREN} {} points; minimum {minimum}", points.len()));
    }
    sites.extend(points);
    Ok(sites)
}
