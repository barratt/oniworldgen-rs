//! YAML the way the game reads it. `Klei.YamlIO.Parse` replaces every tab
//! with four spaces and hands the text to YamlDotNet, which deserializes into
//! typed fields: a repeated mapping key overwrites the earlier one, a bool
//! field accepts YAML 1.1 words (`yes`, `off`), and an enum field matches its
//! names ignoring case (`Enum.Parse(..., ignoreCase: true)`), or its number.
//! serde_yaml alone rejects duplicate keys and keeps those scalars as
//! strings, so the loader parses through [`parse`] instead.
//!
//! The loader reads untyped values, so [`parse`] applies the typed
//! conversions by field name: YAML 1.1 bool words become bools wherever they
//! appear as values, and the enum fields in [`ENUMS`] get their canonical
//! names (`locationType` is left out: shipped clusters write `StartWorld`
//! for `Startworld` and the mixing code compares it ignoring case). Shipped
//! files are otherwise unaffected (no tabs, duplicates, bool words or
//! miscased enum values outside `listRule`, which the loader already folds).
//! Element ids (`SimHashes`) are also parsed ignoring case by the game but
//! are not folded here.

use serde::de::{Deserialize, Deserializer, EnumAccess, MapAccess, SeqAccess, VariantAccess, Visitor};
use serde_yaml::{value::{Tag, TaggedValue}, Mapping, Value};

/// Enum-typed fields of the worldgen schema, by YAML key, with their names
/// and values in declaration order. A key holding a list (`zoneTypes`) folds
/// each item.
pub const ENUMS: &[(&[&str], &[(&str, i64)])] = &[
    // SubWorld.ZoneType
    (&["zoneType", "zoneTypes"], &[("FrozenWastes", 0), ("CrystalCaverns", 1), ("BoggyMarsh", 2), ("Sandstone", 3), ("ToxicJungle", 4),
        ("MagmaCore", 5), ("OilField", 6), ("Space", 7), ("Ocean", 8), ("Rust", 9), ("Forest", 10), ("Radioactive", 11), ("Swamp", 12),
        ("Wasteland", 13), ("RocketInterior", 14), ("Metallic", 15), ("Barren", 16), ("Moo", 17), ("IceCaves", 18), ("CarrotQuarry", 19),
        ("SugarWoods", 20), ("PrehistoricGarden", 21), ("PrehistoricRaptor", 22), ("PrehistoricWetlands", 23), ("KelpForest", 24),
        ("Reef", 25), ("Abyss", 26), ("Beach", 27)]),
    // Temperature.Range (mods may add more through temperatures.yaml; those stay as written)
    (&["temperatureRange", "temperatureRanges"], &[("ExtremelyCold", 0), ("VeryVeryCold", 1), ("VeryCold", 2), ("Cold", 3), ("Chilly", 4),
        ("Cool", 5), ("Mild", 6), ("Room", 7), ("HumanWarm", 8), ("HumanHot", 9), ("SomewhatHot", 10), ("Hot", 11), ("VeryHot", 12),
        ("ExtremelyHot", 13)]),
    // World.LayoutMethod
    (&["layoutMethod"], &[("VoronoiTree", 0), ("PowerTree", 1), ("Default", 0)]),
    // World.TemplateSpawnRules.ListRule
    (&["listRule"], &[("GuaranteeOne", 0), ("GuaranteeSome", 1), ("GuaranteeSomeTryMore", 2), ("GuaranteeAll", 3), ("GuaranteeRange", 4),
        ("TryOne", 5), ("TrySome", 6), ("TryRange", 7), ("TryAll", 8)]),
    // World.AllowedCellsFilter.TagCommand / Command
    (&["tagcommand"], &[("Default", 0), ("AtTag", 1), ("NotAtTag", 2), ("DistanceFromTag", 3)]),
    (&["command"], &[("Clear", 0), ("Replace", 1), ("UnionWith", 2), ("IntersectWith", 3), ("ExceptWith", 4), ("SymmetricExceptWith", 5), ("All", 6)]),
    // PointGenerator.SampleBehaviour
    (&["sampleBehaviour"], &[("UniformSquare", 0), ("UniformHex", 1), ("UniformScaledHex", 2), ("UniformSpiral", 3), ("UniformCircle", 4),
        ("PoissonDisk", 5), ("StdRand", 6)]),
    // Room.Shape (feature shapes) and Room.Selection (element choice groups)
    (&["shape"], &[("Circle", 0), ("Oval", 1), ("Blob", 2), ("Line", 3), ("Square", 4), ("TallThin", 5), ("ShortWide", 6), ("Template", 7),
        ("PhysicalLayout", 8), ("Splat", 9)]),
    (&["selectionMethod"], &[("None", 0), ("OneOfEach", 1), ("NOfEach", 2), ("Weighted", 3), ("WeightedBucket", 4), ("WeightedResample", 5),
        ("PickOneWeighted", 6), ("HorizontalSlice", 7)]),
    // Noise tree node types
    (&["primative"], &[("Constant", 1), ("Spheres", 2), ("Cylinders", 3), ("BevinsValue", 4), ("BevinsGradient", 5), ("ImprovedPerlin", 7),
        ("SimplexPerlin", 8)]),
    (&["quality"], &[("Fast", 0), ("Standard", 1), ("Best", 2)]),
    (&["filter"], &[("_UNSET_", 0), ("Pipe", 1), ("SumFractal", 2), ("SinFractal", 3), ("Billow", 4), ("MultiFractal", 5),
        ("HeterogeneousMultiFractal", 6), ("HybridMultiFractal", 7), ("RidgedMultiFractal", 8), ("Voronoi", 9)]),
    (&["modifyType"], &[("_UNSET_", 0), ("Abs", 1), ("Clamp", 2), ("Exponent", 3), ("Invert", 4), ("ScaleBias", 5), ("Scale2d", 6),
        ("Curve", 7), ("Terrace", 8)]),
    (&["selectType"], &[("_UNSET_", 0), ("Blend", 1), ("Select", 2)]),
    (&["transformerType", "transformType"], &[("_UNSET_", 0), ("Displace", 1), ("Turbulence", 2), ("RotatePoint", 3), ("TranslatePoint", 4)]),
    (&["combineType"], &[("_UNSET_", 0), ("Add", 1), ("Max", 2), ("Min", 3), ("Multiply", 4), ("Power", 5)]),
];

/// Parse `text` as `YamlIO.Parse` would; see the module documentation.
pub fn parse(text: &str) -> Result<Value, String> {
    let expanded;
    let text = if text.contains('\t') { expanded = text.replace('\t', "    "); &expanded } else { text };
    let mut value = KleiValue::deserialize(serde_yaml::Deserializer::from_str(text)).map_err(|e| e.to_string())?.0;
    fold(&mut value, None);
    Ok(value)
}

/// [`parse`], then deserialize into `T`.
pub fn from_str<T: serde::de::DeserializeOwned>(text: &str) -> Result<T, String> {
    serde_yaml::from_value(parse(text)?).map_err(|e| e.to_string())
}

/// [`parse`] as JSON text, for editors. Mapping keys keep their order and
/// become strings; a tagged value is written as `{"tag": ..., "value": ...}`.
pub fn to_json(text: &str) -> Result<String, String> {
    serde_json::to_string(&Json(&parse(text)?)).map_err(|e| e.to_string())
}

struct Json<'a>(&'a Value);

impl serde::Serialize for Json<'_> {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::{SerializeMap, SerializeSeq};
        match self.0 {
            Value::Mapping(m) => {
                let mut map = s.serialize_map(Some(m.len()))?;
                for (k, v) in m { map.serialize_entry(&key_text(k), &Json(v))?; }
                map.end()
            }
            Value::Sequence(items) => {
                let mut seq = s.serialize_seq(Some(items.len()))?;
                for v in items { seq.serialize_element(&Json(v))?; }
                seq.end()
            }
            Value::Tagged(t) => {
                let mut map = s.serialize_map(Some(2))?;
                map.serialize_entry("tag", &t.tag.to_string())?;
                map.serialize_entry("value", &Json(&t.value))?;
                map.end()
            }
            other => other.serialize(s),
        }
    }
}

fn key_text(k: &Value) -> String {
    match k {
        Value::String(s) => s.clone(),
        other => serde_yaml::to_string(other).map(|s| s.trim_end().to_string()).unwrap_or_default(),
    }
}

/// YamlDotNet's YAML 1.1 bool words. `y`/`n` are left alone: they are
/// vector keys (`Y: 4`) far more often than flags.
fn bool_word(s: &str) -> Option<bool> {
    match s.to_ascii_lowercase().as_str() {
        "yes" | "on" | "true" => Some(true),
        "no" | "off" | "false" => Some(false),
        _ => None,
    }
}

fn enum_for(key: &str) -> Option<&'static [(&'static str, i64)]> {
    ENUMS.iter().find(|(keys, _)| keys.contains(&key)).map(|(_, names)| *names)
}

fn canonical(names: &[(&'static str, i64)], v: &Value) -> Option<&'static str> {
    match v {
        Value::String(s) => names.iter().find(|(n, _)| n.eq_ignore_ascii_case(s.trim()))
            .or_else(|| s.trim().parse::<i64>().ok().and_then(|i| names.iter().find(|(_, n)| *n == i))).map(|(n, _)| *n),
        Value::Number(n) => n.as_i64().and_then(|i| names.iter().find(|(_, v)| *v == i)).map(|(n, _)| *n),
        _ => None,
    }
}

/// Apply the typed conversions. `field` is the mapping key `v` sits under.
fn fold(v: &mut Value, field: Option<&str>) {
    if let Some(names) = field.and_then(enum_for) {
        if let Value::Sequence(items) = v {
            for item in items { if let Some(name) = canonical(names, item) { *item = Value::String(name.into()); } }
        } else if let Some(name) = canonical(names, v) { *v = Value::String(name.into()); return }
    }
    match v {
        Value::String(s) => if let Some(b) = bool_word(s) { *v = Value::Bool(b); },
        Value::Sequence(items) => for item in items { fold(item, None); },
        Value::Mapping(m) => for (k, item) in m.iter_mut() { fold(item, k.as_str()); },
        Value::Tagged(t) => fold(&mut t.value, field),
        _ => {}
    }
}

/// `serde_yaml::Value` built with last-wins duplicate keys (keeping the first
/// key's position, as `Dictionary[key] = value` does).
struct KleiValue(Value);

impl<'de> Deserialize<'de> for KleiValue {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = KleiValue;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result { f.write_str("any YAML value") }
            fn visit_bool<E>(self, b: bool) -> Result<KleiValue, E> { Ok(KleiValue(Value::Bool(b))) }
            fn visit_i64<E>(self, i: i64) -> Result<KleiValue, E> { Ok(KleiValue(Value::Number(i.into()))) }
            fn visit_u64<E>(self, u: u64) -> Result<KleiValue, E> { Ok(KleiValue(Value::Number(u.into()))) }
            fn visit_f64<E>(self, f: f64) -> Result<KleiValue, E> { Ok(KleiValue(Value::Number(f.into()))) }
            fn visit_str<E>(self, s: &str) -> Result<KleiValue, E> { Ok(KleiValue(Value::String(s.into()))) }
            fn visit_string<E>(self, s: String) -> Result<KleiValue, E> { Ok(KleiValue(Value::String(s))) }
            fn visit_unit<E>(self) -> Result<KleiValue, E> { Ok(KleiValue(Value::Null)) }
            fn visit_none<E>(self) -> Result<KleiValue, E> { Ok(KleiValue(Value::Null)) }
            fn visit_some<D: Deserializer<'de>>(self, d: D) -> Result<KleiValue, D::Error> { KleiValue::deserialize(d) }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<KleiValue, A::Error> {
                let mut items = Vec::new();
                while let Some(KleiValue(v)) = seq.next_element()? { items.push(v); }
                Ok(KleiValue(Value::Sequence(items)))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<KleiValue, A::Error> {
                let mut m = Mapping::new();
                while let Some((KleiValue(k), KleiValue(v))) = map.next_entry()? { m.insert(k, v); }
                Ok(KleiValue(Value::Mapping(m)))
            }
            fn visit_enum<A: EnumAccess<'de>>(self, data: A) -> Result<KleiValue, A::Error> {
                let (tag, contents) = data.variant::<String>()?;
                let KleiValue(value) = contents.newtype_variant()?;
                Ok(KleiValue(Value::Tagged(Box::new(TaggedValue { tag: Tag::new(tag), value }))))
            }
        }
        d.deserialize_any(V)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tabs_duplicates_bool_words_and_enum_case_follow_yamlio() {
        let v = parse("a:\n\tb: 1\n\tb: 2\nc: yes\nd: Off\nzoneType: frozenwastes\nzoneTypes: [sandstone, 12]\nlistRule: trysome\nY: 3\nn: n\n").unwrap();
        assert_eq!(v["a"]["b"], Value::Number(2.into()));
        assert_eq!(v["c"], Value::Bool(true));
        assert_eq!(v["d"], Value::Bool(false));
        assert_eq!(v["zoneType"], Value::String("FrozenWastes".into()));
        assert_eq!(v["zoneTypes"], serde_yaml::from_str::<Value>("[Sandstone, Swamp]").unwrap());
        assert_eq!(v["listRule"], Value::String("TrySome".into()));
        assert_eq!(v["Y"], Value::Number(3.into()));
        assert_eq!(v["n"], Value::String("n".into()));
        // Duplicate keys keep the first position.
        let keys: Vec<_> = parse("x: 1\ny: 2\nx: 3\n").unwrap().as_mapping().unwrap().keys().map(|k| k.as_str().unwrap().to_string()).collect();
        assert_eq!(keys, ["x", "y"]);
        // Unknown enum names stay as written; mods register custom temperatures.
        assert_eq!(parse("temperatureRange: HellHot").unwrap()["temperatureRange"], Value::String("HellHot".into()));
        assert_eq!(parse("layoutMethod: 1").unwrap()["layoutMethod"], Value::String("PowerTree".into()));
    }

    #[test]
    fn json_keeps_numbers_and_key_order() {
        assert_eq!(to_json("b: .5\na: [1, -2, on]\n").unwrap().to_string(), r#"{"b":0.5,"a":[1,-2,true]}"#);
    }
}
