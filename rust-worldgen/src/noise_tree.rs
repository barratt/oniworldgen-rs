//! Noise Tree loader + evaluator.
//!
//! Port of `ProcGen.Noise.Tree.BuildFinalModule` — parses a tree
//! dictionary (same shape as `settings_export_worldgen()["noise"][name]`)
//! and builds a `Box<dyn Source3D>` you can sample.
//!
//! Supports the implemented modules in `noise.rs`, including the game's
//! Displace/RotatePoint/TranslatePoint transformers. Unsupported nodes return
//! None instead of silently substituting a different noise function.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::noise::{
    Add, BevinsGradient, BevinsValue, Billow, Clamp, Constant, Curve, Cylinders, Displace, HybridMultiFractal,
    ImprovedPerlin, Invert, Max, Min, Multiply, NoiseQuality, Power, RidgedMultiFractal,
    RotatePoint, Scale2d, ScaleBias, ScalePoint, Select, SimplexPerlin, SinFractal, Source3D,
    SpectralWeights, Spheres, TranslatePoint, Turbulence, VoronoiFilter,
};

// ---- serde shapes for the game's exported noise JSON ----

#[derive(Debug, Clone, Deserialize, Default, Serialize)]
#[serde(default)]
pub struct TreeJson {
    pub settings: TreeSettings,
    pub links: Vec<LinkJson>,
    pub primitives: HashMap<String, PrimitiveJson>,
    pub filters: HashMap<String, FilterJson>,
    pub modifiers: HashMap<String, ModifierJson>,
    pub transformers: HashMap<String, TransformerJson>,
    pub selectors: HashMap<String, SelectorJson>,
    pub combiners: HashMap<String, CombinerJson>,
    pub floats: HashMap<String, FloatListJson>,
    pub controlpoints: HashMap<String, ControlPointListJson>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct TreeSettings {
    pub zoom: f32,
    pub normalise: bool,
    pub name: String,
    #[serde(rename = "lowerBound")]
    pub lower_bound: Option<serde_json::Value>,
    #[serde(rename = "upperBound")]
    pub upper_bound: Option<serde_json::Value>,
}
impl Default for TreeSettings {
    fn default()->Self {Self{zoom:0.1,normalise:false,name:String::new(),lower_bound:None,upper_bound:None}}
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LinkJson {
    pub target: Option<LinkRef>,
    pub source0: Option<LinkRef>,
    pub source1: Option<LinkRef>,
    pub source2: Option<LinkRef>,
    pub source3: Option<LinkRef>,
}
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LinkRef { #[serde(rename="type")] pub kind: String, pub name: String }

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct PrimitiveJson {
    pub primative: String,   // sic: game's spelling
    pub quality: String,
    pub seed: i32,
    pub offset: f32,
    pub frequency: Option<f32>,
    #[serde(rename = "constantValue")]
    pub constant_value: Option<f32>,
}
impl Default for PrimitiveJson {
    fn default()->Self {Self{primative:"ImprovedPerlin".into(),quality:"Best".into(),seed:0,offset:1.0,frequency:None,constant_value:None}}
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct FilterJson {
    pub filter: String,
    pub frequency: f32,
    pub lacunarity: f32,
    pub octaves: f32,
    pub offset: f32,
    pub gain: f32,
    pub exponent: f32,
    pub scale: f32,
    pub bias: f32,
    pub displacement: Option<f32>,
    pub distance: Option<bool>,
}
impl Default for FilterJson {
    fn default()->Self {Self{filter:"RidgedMultiFractal".into(),frequency:0.1,lacunarity:3.0,octaves:0.0,offset:1.0,gain:1.0,exponent:0.9,scale:1.0,bias:0.0,displacement:None,distance:None}}
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct ModifierJson {
    #[serde(rename = "modifyType")]
    pub modify_type: String,
    pub lower: f32,
    pub upper: f32,
    pub exponent: f32,
    pub scale: f32,
    pub bias: f32,
    pub scale2d: Option<Vector2Json>,
}
impl Default for ModifierJson {
    fn default()->Self {Self{modify_type:"Abs".into(),lower:-1.0,upper:1.0,exponent:0.02,scale:1.0,bias:0.0,scale2d:Some(Vector2Json{x:1.0,y:1.0})}}
}

#[derive(Debug, Clone, Deserialize, Default, Serialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct Vector2Json { pub x: f32, pub y: f32 }

#[derive(Debug, Clone, Deserialize, Default, Serialize)]
#[serde(default)]
pub struct TransformerJson {
    #[serde(rename = "transformType", alias = "transformerType")]
    pub transform_type: String,
    pub vector: Option<Vector2Json>,
    #[serde(default="one")]
    pub power: f32,
    #[serde(rename = "xAngle")]
    pub x_angle: f32,
    #[serde(rename = "yAngle")]
    pub y_angle: f32,
    #[serde(rename = "zAngle")]
    pub z_angle: f32,
    pub translate: Option<Vector3Json>,
    pub scale: Option<Vector3Json>,
}

#[derive(Debug, Clone, Deserialize, Default, Serialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct Vector3Json { pub x: f32, pub y: f32, pub z: f32 }
fn one()->f32 {1.0}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct SelectorJson {
    #[serde(rename = "selectType")]
    pub select_type: String,
    pub lower: f32,
    pub upper: f32,
    #[serde(rename = "edge", alias = "edgeFalloff")]
    pub edge_falloff: f32,
}
impl Default for SelectorJson {
    fn default()->Self {Self{select_type:"Blend".into(),lower:0.0,upper:1.0,edge_falloff:0.02}}
}

#[derive(Debug, Clone, Deserialize, Default, Serialize)]
#[serde(default)]
pub struct CombinerJson {
    #[serde(rename = "combineType")]
    pub combine_type: String,
}

#[derive(Debug, Clone, Deserialize, Default, Serialize)]
#[serde(default)]
pub struct FloatListJson { pub items: Vec<f32> }

#[derive(Debug, Clone, Deserialize, Default, Serialize)]
#[serde(default)]
pub struct ControlPointListJson {
    #[serde(rename = "controlPoints", default, alias = "items", alias = "points")]
    pub control_points: Vec<ControlPointJson>,
}

#[derive(Debug, Clone, Deserialize, Default, Serialize)]
#[serde(default)]
pub struct ControlPointJson { pub input: f32, pub output: f32 }

// ---- builder ----

fn quality_from(s: &str) -> NoiseQuality {
    match s {
        "Fast" => NoiseQuality::Fast,
        "Best" => NoiseQuality::Best,
        _ => NoiseQuality::Standard,
    }
}

fn build_primitive(p: &PrimitiveJson, global_seed: i32) -> Option<Box<dyn Source3D>> {
    let seed = global_seed.wrapping_add(p.seed);
    Some(match p.primative.as_str() {
        "ImprovedPerlin" => Box::new(ImprovedPerlin::new(seed, quality_from(&p.quality))),
        "SimplexPerlin"  => Box::new(SimplexPerlin::new(seed)),
        "BevinsValue"    => Box::new(BevinsValue::new(seed, quality_from(&p.quality))),
        "BevinsGradient" => Box::new(BevinsGradient {seed,quality:quality_from(&p.quality)}),
        "Constant"       => Box::new(Constant(p.constant_value.unwrap_or(p.offset))),
        "Cylinders"      => Box::new(Cylinders { frequency: p.frequency.unwrap_or(p.offset) }),
        "Spheres"        => Box::new(Spheres   { frequency: p.frequency.unwrap_or(p.offset) }),
        _ => return None,
    })
}

/// Every noise node type the game's `ProcGen.Noise` trees define, for editors:
/// `{category, kind, sources, params: [{name, type, default, enumValues?}],
/// evaluable}`. `category` names the tree map the node lives in (singular),
/// `kind` the discriminator value, `sources` the links it reads, `params` the
/// YAML fields with the game's constructor defaults. `evaluable` is false for
/// types [`build`] does not implement.
pub fn node_kinds()->serde_json::Value {
    use serde_json::json;
    let f=|name:&str,default:f64|json!({"name":name,"type":"f32","default":default});
    let primitive=vec![json!({"name":"quality","type":"enum","default":"Best","enumValues":["Fast","Standard","Best"]}),
        json!({"name":"seed","type":"i32","default":0}),f("offset",1.0)];
    let filter=vec![f("frequency",0.1),f("lacunarity",3.0),json!({"name":"octaves","type":"i32","default":0}),f("offset",1.0),f("gain",1.0),
        f("exponent",0.9),f("scale",1.0),f("bias",0.0)];
    let modifier=vec![f("lower",-1.0),f("upper",1.0),f("exponent",0.02),json!({"name":"invert","type":"bool","default":false}),f("scale",1.0),f("bias",0.0),
        json!({"name":"scale2d","type":"vec2","default":{"X":1,"Y":1}})];
    let transformer=vec![f("power",1.0),json!({"name":"vector","type":"vec2","default":{"X":0,"Y":0}})];
    let selector=vec![f("lower",0.0),f("upper",1.0),f("edge",0.02)];
    let mut out=Vec::new();
    let mut add=|category:&str,kinds:&[(&str,u32,bool)],params:&Vec<serde_json::Value>| for (kind,sources,evaluable) in kinds {
        out.push(json!({"category":category,"kind":kind,"sources":sources,"params":params,"evaluable":evaluable}));
    };
    add("primitive",&[("Constant",0,true),("Spheres",0,true),("Cylinders",0,true),("BevinsValue",0,true),("BevinsGradient",0,true),
        ("ImprovedPerlin",0,true),("SimplexPerlin",0,true)],&primitive);
    add("filter",&[("Pipe",1,true),("SumFractal",1,false),("SinFractal",1,true),("Billow",1,true),("MultiFractal",1,false),
        ("HeterogeneousMultiFractal",1,false),("HybridMultiFractal",1,true),("RidgedMultiFractal",1,true),("Voronoi",1,true)],&filter);
    // Curve reads its control points through source1.
    add("modifier",&[("Abs",1,false),("Clamp",1,true),("Exponent",1,false),("Invert",1,true),("ScaleBias",1,true),("Scale2d",1,true),
        ("Curve",2,true),("Terrace",2,false)],&modifier);
    // Displace and Turbulence read the X and Z displacement through source1 and source2.
    add("transformer",&[("Displace",3,true),("Turbulence",3,true),("RotatePoint",1,true),("TranslatePoint",1,true)],&transformer);
    // source0 and source1 are the two inputs, source2 the control.
    add("selector",&[("Blend",3,false),("Select",3,true)],&selector);
    add("combiner",&[("Add",2,true),("Max",2,true),("Min",2,true),("Multiply",2,true),("Power",2,true)],&Vec::new());
    serde_json::Value::Array(out)
}

/// Build a sampler by walking the tree links backward from Terminator.
/// Returns `None` if the tree is malformed (no terminator, missing refs).
pub fn build(tree: &TreeJson, global_seed: i32) -> Option<Box<dyn Source3D>> {
    // Find terminator link.
    let term = tree.links.iter().find(|l| {
        l.target.as_ref().map(|t| t.kind == "Terminator").unwrap_or(false)
    })?;
    let root_ref = term.source0.as_ref()?;
    build_node(tree, root_ref, global_seed)
}

fn build_node(tree: &TreeJson, r: &LinkRef, global_seed: i32) -> Option<Box<dyn Source3D>> {
    // Find the link whose TARGET is this ref, then recurse on its source0.
    let link = tree.links.iter().find(|l| {
        l.target.as_ref().map(|t| t.kind == r.kind && t.name == r.name).unwrap_or(false)
    });

    match r.kind.as_str() {
        "Primitive" => {
            let p = tree.primitives.get(&r.name)?;
            build_primitive(p, global_seed)
        }
        "Filter" => {
            let f = tree.filters.get(&r.name)?;
            let s = build_node(tree, link?.source0.as_ref()?, global_seed)?;
            Some(match f.filter.as_str() {
                "RidgedMultiFractal" => Box::new(RidgedMultiFractal {
                    frequency: f.frequency, lacunarity: f.lacunarity, octave_count: f.octaves.clamp(1.0, 30.0),
                    offset: f.offset, gain: f.gain,
                    spectral: SpectralWeights::new(f.lacunarity, f.exponent),
                    source: s,
                }),
                "Billow" => Box::new(Billow {
                    frequency: f.frequency, lacunarity: f.lacunarity, octave_count: f.octaves.clamp(1.0, 30.0),
                    scale: f.scale, bias: f.bias,
                    spectral: SpectralWeights::new(f.lacunarity, f.exponent),
                    source: s,
                }),
                "HybridMultiFractal" => Box::new(HybridMultiFractal {
                    frequency: f.frequency, lacunarity: f.lacunarity, octave_count: f.octaves.clamp(1.0, 30.0),
                    offset: f.offset, gain: f.gain,
                    spectral: SpectralWeights::new(f.lacunarity, f.exponent),
                    source: s,
                }),
                "SinFractal" => Box::new(SinFractal {
                    frequency: f.frequency, lacunarity: f.lacunarity, octave_count: f.octaves.clamp(1.0, 30.0),
                    spectral: SpectralWeights::new(f.lacunarity, f.exponent),
                    source: s,
                }),
                "Voronoi" => Box::new(VoronoiFilter {
                    frequency: f.frequency,
                    displacement: f.displacement.unwrap_or(1.0),
                    distance: f.distance.unwrap_or(false),
                    source: s,
                    lattice: Default::default(),
                }),
                "Pipe" => s,
                _ => return None,
            })
        }
        "Modifier" => {
            let m = tree.modifiers.get(&r.name)?;
            let s = build_node(tree, link?.source0.as_ref()?, global_seed)?;
            Some(match m.modify_type.as_str() {
                "ScaleBias" => Box::new(ScaleBias { scale: m.scale, bias: m.bias, source: s }) as Box<dyn Source3D>,
                "Clamp"     => Box::new(Clamp { lower: m.lower, upper: m.upper, source: s }),
                "Invert"    => Box::new(Invert { source: s }),
                "Scale2d"   => {
                    let v = m.scale2d.clone().unwrap_or_default();
                    Box::new(Scale2d { scale_x: v.x, scale_y: v.y, source: s })
                }
                "Curve"     => {
                    // control points arrive via source1 of this link
                    let mut cps = link
                        .and_then(|l| l.source1.as_ref())
                        .and_then(|r| tree.controlpoints.get(&r.name))
                        .map(|c| c.control_points.iter().map(|p| (p.input, p.output)).collect::<Vec<_>>())
                        .unwrap_or_default();
                    if cps.is_empty() { return None; }
                    cps.sort_by(|a,b|a.0.total_cmp(&b.0));
                    Box::new(Curve { control_points: cps, source: s })
                }
                _ => return None,
            })
        }
        "Combiner" => {
            let c = tree.combiners.get(&r.name)?;
            let left = build_node(tree, link?.source0.as_ref()?, global_seed)?;
            let right = build_node(tree, link?.source1.as_ref()?, global_seed)?;
            Some(match c.combine_type.as_str() {
                "Add"      => Box::new(Add { left, right }) as Box<dyn Source3D>,
                "Max"      => Box::new(Max { left, right }),
                "Min"      => Box::new(Min { left, right }),
                "Multiply" => Box::new(Multiply { left, right }),
                "Power"    => Box::new(Power { left, right }),
                _ => return None,
            })
        }
        "Transformer" => {
            let s = build_node(tree, link?.source0.as_ref()?, global_seed)?;
            let t = tree.transformers.get(&r.name)?;
            Some(match t.transform_type.as_str() {
                "RotatePoint" | "Rotate" => {
                    let (x,y,z)=t.vector.as_ref().map(|v|(v.x,v.y,0.0)).unwrap_or((t.x_angle,t.y_angle,t.z_angle));
                    Box::new(RotatePoint::from_angles(x,y,z,s)) as Box<dyn Source3D>
                }
                "TranslatePoint" | "Translate" => {
                    // CreateModule sets X/Z only; TranslatePoint's default Y is 1.
                    let v = t.vector.as_ref().map(|v|Vector3Json{x:v.x,y:1.0,z:v.y}).or_else(||t.translate.clone()).unwrap_or(Vector3Json{x:0.0,y:1.0,z:0.0});
                    Box::new(TranslatePoint { dx: v.x, dy: v.y, dz: v.z, source: s })
                }
                "ScalePoint" | "Scale3d" => {
                    let v = t.scale.clone().unwrap_or_default();
                    Box::new(ScalePoint { sx: v.x, sy: v.y, sz: v.z, source: s })
                }
                "Displace" | "Turbulence" => {
                    let ln=link?;
                    let x_displace=match ln.source1.as_ref() {Some(r)=>build_node(tree,r,global_seed)?,None=>Box::new(Constant(0.0))};
                    let z_displace=match ln.source2.as_ref() {Some(r)=>build_node(tree,r,global_seed)?,None=>Box::new(Constant(0.0))};
                    if t.transform_type=="Turbulence" {
                        Box::new(Turbulence{source:s,x_distort:x_displace,z_distort:z_displace,power:t.power})
                    } else { Box::new(Displace{source:s,x_displace,z_displace}) }
                }
                _ => return None,
            })
        }
        "Selector" => {
            let s = tree.selectors.get(&r.name)?;
            let ln = link?;
            let control = build_node(tree, ln.source2.as_ref()?, global_seed)?;
            let left = build_node(tree, ln.source1.as_ref()?, global_seed)?;
            let right = build_node(tree, ln.source0.as_ref()?, global_seed)?;
            Some(match s.select_type.as_str() {
                "Select" => Box::new(Select {
                    lower: s.lower, upper: s.upper, edge_falloff: s.edge_falloff.min((s.upper-s.lower)/2.0),
                    control, left, right,
                }),
                _ => return None,
            })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn yaml_curve_points_are_applied_including_empty_zero_point() {
        let tree: TreeJson = serde_yaml::from_str(r#"
links:
  - target: {type: Terminator, name: end}
    source0: {type: Modifier, name: curve}
  - target: {type: Modifier, name: curve}
    source0: {type: Primitive, name: value}
    source1: {type: ControlPoints, name: points}
primitives:
  value: {primative: Constant, offset: 0.01}
modifiers:
  curve: {modifyType: Curve}
controlpoints:
  points:
    points: [{input: 1, output: 1}, {}, {input: 0.5, output: 0.75}]
"#).unwrap();
        // Moo-core's live sample at the origin exercises this transfer curve.
        let actual = build(&tree, 49).unwrap().get_value_3(0.0, 0.0, 0.0);
        let curve = Curve { control_points: vec![(0.0,0.0),(0.5,0.75),(1.0,1.0)], source: Box::new(Constant(0.01)) };
        assert_eq!(actual.to_bits(), curve.get_value_3(0.0,0.0,0.0).to_bits());
        assert!((actual - 0.014902).abs() < 1e-8);
    }

    #[test]
    fn yaml_translate_preserves_game_y_default_and_zero_octaves_clamp_to_one() {
        let tree: TreeJson = serde_yaml::from_str(r#"
links:
  - target: {type: Terminator, name: end}
    source0: {type: Transformer, name: translate}
  - target: {type: Transformer, name: translate}
    source0: {type: Primitive, name: value}
primitives:
  value: {primative: SimplexPerlin, seed: 0}
transformers:
  translate: {transformerType: TranslatePoint, vector: {X: -2}}
"#).unwrap();
        let actual = build(&tree,44).unwrap().get_value_3(3.12,0.0,6.11);
        assert_eq!(actual, SimplexPerlin::new(44).get_value_3(3.12-2.0,1.0,6.11));
        let tree: TreeJson = serde_yaml::from_str(r#"
links:
  - target: {type: Terminator, name: end}
    source0: {type: Filter, name: sin}
  - target: {type: Filter, name: sin}
    source0: {type: Primitive, name: value}
primitives:
  value: {primative: Constant, offset: 0.2}
filters:
  sin: {filter: SinFractal}
"#).unwrap();
        assert_eq!(build(&tree,44).unwrap().get_value_3(0.0,0.0,0.0), (0.2_f32 as f64).sin() as f32);
    }

    #[test]
    fn selector_uses_game_source_order_and_yaml_edge() {
        let mut tree:TreeJson=serde_yaml::from_str(r#"
links:
  - target: {type: Terminator, name: end}
    source0: {type: Selector, name: select}
  - target: {type: Selector, name: select}
    source0: {type: Primitive, name: right}
    source1: {type: Primitive, name: left}
    source2: {type: Primitive, name: control}
primitives:
  right: {primative: Constant, offset: 8}
  left: {primative: Constant, offset: 2}
  control: {primative: Constant, offset: 0.5}
selectors:
  select: {selectType: Select, lower: 0, upper: 1, edge: 0.125}
"#).unwrap();
        assert_eq!(tree.settings.zoom,0.1);
        assert_eq!(tree.selectors["select"].edge_falloff,0.125);
        assert_eq!(build(&tree,42).unwrap().get_value_3(0.0,0.0,0.0),8.0);
        tree.primitives.get_mut("right").unwrap().primative="unsupported".into();
        assert!(build(&tree,42).is_none());
    }
}

