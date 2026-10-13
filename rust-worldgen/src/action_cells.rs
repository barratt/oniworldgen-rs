//! Action-cell sampling between foreground and background rendering.
//! Even centroid-only mobs consume a density draw from the terrain RNG.
use crate::{geom::{Polygon, V2}, sampler::{SeededRandom, SampleBehaviour, get_random_points_in_polygon}};
use std::collections::HashSet;

#[derive(Debug, Clone, Default, serde::Deserialize, serde::Serialize)]
#[serde(default, rename_all="camelCase")]
pub struct ActionSample {
    pub width:i32,
    pub height:i32,
    pub padding_x:i32,
    pub location:String,
    pub prefab_name:Option<String>,
    pub select_method: String,
    pub density: crate::subworld::Density,
    pub sample_behaviour: String,
    pub blob_size: crate::subworld::Density,
    pub mobselection: String,
}

/// The cells action samples may land on.
pub trait PossiblePoints { fn has(&self, cell: i32) -> bool; }
impl PossiblePoints for HashSet<i32> { fn has(&self, cell: i32) -> bool { self.contains(&cell) } }
impl<F: Fn(i32) -> bool> PossiblePoints for F { fn has(&self, cell: i32) -> bool { self(cell) } }

/// Run the action sampling and background-biome selection in game order.
/// `possible_points` must already exclude the foreground feature footprint.
pub fn after_foreground(
    region: &crate::subworld::TerrainRegion, ds: &crate::data_source::DataSource,
    possible_points: &impl PossiblePoints, width: i32, height: i32, rng: &mut SeededRandom,
) -> Result<(String, Vec<(i32,String)>),String> {
    let mut polygon=Polygon::new(region.polygon.iter().map(|&(x,y)|V2::new(x,y)).collect());
    let mut positions=Vec::new();
    for tag in &region.tags {
        let room=ds.rooms.get(tag.as_str());
        if let Some(sample)=room.or_else(||ds.mobs.get(tag.as_str())) {
            for cell in generate(sample,room.is_some(),&mut polygon,V2::new(region.position.0,region.position.1),possible_points,width,height,rng)? {
                positions.push((cell,tag.to_string()));
            }
        }
    }
    let biome=if ds.biomes.contains_key(&region.node_type) {region.node_type.clone()}else{
        let candidates:Vec<_>=region.biome_tags.iter().filter(|t|ds.biomes.contains_key(t.as_str())).collect();
        if candidates.is_empty(){return Err(format!("no background biome for region {}",region.id));}
        candidates[rng.random_range_i(0,candidates.len() as i32) as usize].to_string()
    };
    // Sprinkles consume further RNG after the background writes. Do not
    // silently claim a complete inter-region stream until they are ported.
    if region.tags.iter().any(|t|t=="SprinkleOfOxyRock"||t=="SprinkleOfMetal") {
        return Err("sprinkle rendering is not implemented".into());
    }
    Ok((biome,positions))
}

/// Returns the ordered cells appended to terrainPositions. Mob samples and
/// rooms with a mob selection still advance RNG, but append no terrain cells.
#[allow(clippy::too_many_arguments)]
pub fn generate(
    sample: &ActionSample, is_room: bool, polygon: &mut Polygon,
    node_position: V2, possible_points: &impl PossiblePoints, width: i32,
    height: i32, rng: &mut SeededRandom,
) -> Result<Vec<i32>, String> {
    if width <= 0 || height <= 0 || width.checked_mul(height).is_none() {
        return Err("invalid action grid dimensions".into());
    }
    let density = rng.random_range_f(sample.density.min, sample.density.max);
    let points = match sample.select_method.as_str() {
        "" | "RandomPoints" => {
            // The enum's zero value is UniformSquare, unlike the default
            // used by the overworld sampler's configuration parser.
            let behaviour = match sample.sample_behaviour.as_str() {
                "" | "UniformSquare" => SampleBehaviour::UniformSquare,
                "UniformHex" => SampleBehaviour::UniformHex,
                "PoissonDisk" => SampleBehaviour::PoissonDisk,
                other => return Err(format!("unsupported action sample behaviour {other}")),
            };
            if !density.is_finite() || density <= 0.0 {
                return Err(format!("invalid action sampling density {density}"));
            }
            get_random_points_in_polygon(rng, polygon, density, 0.0, &[], behaviour, true, true, true)
        }
        "Centroid" => vec![[node_position.x, node_position.y]],
        other => return Err(format!("unsupported action selection method {other}")),
    };
    if !is_room || !matches!(sample.mobselection.as_str(), "" | "None") {
        return Ok(Vec::new());
    }
    let mut seen = HashSet::new();
    Ok(points.into_iter().filter_map(|p| {
        // Grid.XYToCell uses truncation and validates the flat index; do not
        // independently clamp the x/y coordinates.
        let cell = (p[1] as i32).wrapping_mul(width).wrapping_add(p[0] as i32);
        (possible_points.has(cell) && cell >= 0 && cell < width * height && seen.insert(cell)).then_some(cell)
    }).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn polygon() -> Polygon {
        Polygon::new(vec![V2::new(0.0,0.0), V2::new(10.0,0.0), V2::new(10.0,10.0), V2::new(0.0,10.0)])
    }
    #[test]
    fn centroid_uses_node_position_and_consumes_equal_range_draw() {
        let sample: ActionSample = serde_yaml::from_str("selectMethod: Centroid").unwrap();
        let mut rng = SeededRandom::new(43);
        let cells = generate(&sample, true, &mut polygon(), V2::new(2.9,3.9), &HashSet::from([32]), 10, 10, &mut rng).unwrap();
        assert_eq!(cells, vec![32]);
        let mut reference = SeededRandom::new(43);
        reference.next_double();
        assert_eq!(rng.state(), reference.state());
    }
    #[test]
    fn mobs_consume_same_sampling_stream_as_rooms_without_appending() {
        let sample: ActionSample = serde_yaml::from_str("selectMethod: RandomPoints\ndensity: {min: 1, max: 2}\nsampleBehaviour: PoissonDisk").unwrap();
        let possible: HashSet<i32> = (0..100).collect();
        let mut room_rng = SeededRandom::new(43);
        let mut mob_rng = SeededRandom::new(43);
        assert!(!generate(&sample, true, &mut polygon(), V2::new(5.0,5.0), &possible, 10, 10, &mut room_rng).unwrap().is_empty());
        assert!(generate(&sample, false, &mut polygon(), V2::new(5.0,5.0), &possible, 10, 10, &mut mob_rng).unwrap().is_empty());
        assert_eq!(room_rng.state(), mob_rng.state());
    }
}
