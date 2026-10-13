//! Build the simulation's element table (the payload of the elements
//! message) from the game's element YAML, following the managed
//! ElementLoader / Sim.Element rules. Render-only fields (colours,
//! gradients, material flags) come from Unity assets the YAML lacks; the
//! simulation never reads them for cell state, so they are written as zero.
use crate::tables::{element_hash, ELEMENT_RECORD};
use serde_yaml::Value;

#[derive(Clone, Debug, Default)]
pub struct Entry {
    pub id: String,
    pub hash: u32,
    pub state: u8,
    pub unstable: bool,
    pub f: std::collections::BTreeMap<&'static str, f32>,
    pub low_target: u32,
    pub high_target: u32,
    pub low_ore: u32,
    pub high_ore: u32,
    pub sublimate: u32,
    pub convert: u32,
    pub sublimate_fx: u32,
    /// Ore tags as the managed loader builds them: YAML tags, the phase tag
    /// and the material category (defaulting to the phase tag).
    pub tags: Vec<String>,
}

const FLOATS: &[&str] = &[
    "specificHeatCapacity", "thermalConductivity", "molarMass", "solidSurfaceAreaMultiplier", "liquidSurfaceAreaMultiplier",
    "gasSurfaceAreaMultiplier", "flow", "speed", "minHorizontalFlow", "minVerticalFlow", "maxMass", "lowTemp", "highTemp",
    "strength", "lowTempTransitionOreMassConversion", "highTempTransitionOreMassConversion", "sublimateRate",
    "sublimateEfficiency", "sublimateProbability", "offGasPercentage", "lightAbsorptionFactor", "radiationAbsorptionFactor",
    "radiationPer1000Mass", "defaultTemperature", "defaultMass", "defaultPressure",
];

/// Hash.SDBMLower, with null/empty strings hashing to 0.
fn hash_opt(v: Option<&Value>) -> u32 {
    match v.and_then(Value::as_str) {
        Some(s) if !s.is_empty() => element_hash(s),
        _ => 0,
    }
}

fn number(v: &Value) -> Option<f32> {
    match v {
        Value::Number(n) => {
            // Parse the decimal text straight to f32, as the game's YAML reader does.
            n.to_string().parse::<f32>().ok()
        }
        Value::String(s) => s.parse::<f32>().ok(),
        _ => None,
    }
}

/// Collect entries from YAML documents in file order, keeping the first
/// entry per id and only those whose dlcId is active ("" is the base game).
pub fn collect(files: &[&str], active_dlcs: &[&str]) -> Result<Vec<Entry>, String> {
    let mut out: Vec<Entry> = Vec::new();
    for text in files {
        let doc: Value = serde_yaml::from_str(text).map_err(|e| e.to_string())?;
        let list = doc.get("elements").and_then(Value::as_sequence).ok_or("element YAML lacks elements")?;
        for item in list {
            let id = item.get("elementId").and_then(Value::as_str).ok_or("element without elementId")?.to_string();
            let hash = element_hash(&id);
            let dlc = item.get("dlcId").and_then(Value::as_str).unwrap_or("").trim();
            if out.iter().any(|e| e.hash == hash) || !active_dlcs.contains(&dlc) {
                continue;
            }
            let state = match item.get("state").and_then(Value::as_str).unwrap_or("Vacuum") {
                "Vacuum" => 0,
                "Gas" => 1,
                "Liquid" => 2,
                "Solid" => 3,
                s => return Err(format!("{id}: unknown state {s}")),
            };
            let mut e = Entry { id: id.clone(), hash, state, ..Default::default() };
            for k in FLOATS {
                let v = match item.get(*k) {
                    Some(v) if !v.is_null() => number(v).ok_or_else(|| format!("{id}: invalid {k}"))?,
                    _ => match *k {
                        "highTemp" => 10000.0,
                        _ => 0.0,
                    },
                };
                e.f.insert(k, v);
            }
            let phase_tag = ["Vacuum", "Gas", "Liquid", "Solid"][state as usize].to_string();
            let mut tags: Vec<String> = item
                .get("tags")
                .and_then(Value::as_sequence)
                .map(|t| t.iter().filter_map(|x| x.as_str()).filter(|x| !x.is_empty()).map(String::from).collect())
                .unwrap_or_default();
            tags.push(phase_tag.clone());
            let category = item.get("materialCategory").and_then(Value::as_str).filter(|c| !c.is_empty()).map(String::from).unwrap_or(phase_tag);
            if !tags.contains(&category) {
                tags.push(category);
            }
            e.tags = tags;
            e.unstable = item.get("tags").and_then(Value::as_sequence).is_some_and(|t| t.iter().any(|x| x.as_str() == Some("Unstable")));
            e.low_target = hash_opt(item.get("lowTempTransitionTarget"));
            e.high_target = hash_opt(item.get("highTempTransitionTarget"));
            e.low_ore = hash_opt(item.get("lowTempTransitionOreId"));
            e.high_ore = hash_opt(item.get("highTempTransitionOreId"));
            e.sublimate = hash_opt(item.get("sublimateId"));
            e.convert = hash_opt(item.get("convertId"));
            e.sublimate_fx = hash_opt(item.get("sublimateFx"));
            if state == 1 {
                // CopyEntryToElement: gases use default mass 1 and max mass 1.8.
                e.f.insert("defaultMass", 1.0);
                e.f.insert("maxMass", 1.8);
            }
            out.push(e);
        }
    }
    // FinaliseElementsTable: order by solid-bits descending, then id (signed).
    out.sort_by(|a, b| (b.state & 3).cmp(&(a.state & 3)).then((a.hash as i32).cmp(&(b.hash as i32))));
    Ok(out)
}

/// The elements message payload (count, 164-byte records, then names).
pub fn payload(entries: &[Entry]) -> Vec<u8> {
    let index = |h: u32| -> u16 { entries.iter().position(|e| e.hash == h).map(|i| i as u16).unwrap_or(0xffff) };
    let mut out = Vec::with_capacity(4 + entries.len() * (ELEMENT_RECORD + 8));
    out.extend((entries.len() as u32).to_le_bytes());
    for (i, e) in entries.iter().enumerate() {
        let f = |k: &str| e.f[k];
        let mut state = e.state;
        if f("thermalConductivity") == 0.0 {
            state |= 16;
        }
        if f("strength") == 0.0 {
            state |= 4;
        }
        if e.unstable {
            state |= 8;
        }
        let mut r = Vec::with_capacity(ELEMENT_RECORD);
        r.extend(e.hash.to_le_bytes());
        r.extend(index(e.low_target).to_le_bytes());
        r.extend(index(e.high_target).to_le_bytes());
        r.extend((i as u16).to_le_bytes());
        r.push(state);
        r.push(0); // numberOfGradientColors (render)
        for k in ["specificHeatCapacity", "thermalConductivity", "molarMass", "solidSurfaceAreaMultiplier", "liquidSurfaceAreaMultiplier",
            "gasSurfaceAreaMultiplier", "flow", "speed", "minHorizontalFlow", "minVerticalFlow", "maxMass", "lowTemp", "highTemp", "strength"] {
            r.extend(f(k).to_le_bytes());
        }
        r.extend(e.low_ore.to_le_bytes());
        r.extend(f("lowTempTransitionOreMassConversion").to_le_bytes());
        r.extend(e.high_ore.to_le_bytes());
        r.extend(f("highTempTransitionOreMassConversion").to_le_bytes());
        r.extend(index(e.sublimate).to_le_bytes());
        r.extend(index(e.convert).to_le_bytes());
        r.extend([0u8; 4 + 4 + 24]); // materialProperties, colour, gradient (render)
        r.extend(e.sublimate_fx.to_le_bytes());
        for k in ["sublimateRate", "sublimateEfficiency", "sublimateProbability", "offGasPercentage", "lightAbsorptionFactor",
            "radiationAbsorptionFactor", "radiationPer1000Mass", "defaultTemperature", "defaultMass", "defaultPressure"] {
            r.extend(f(k).to_le_bytes());
        }
        debug_assert_eq!(r.len(), ELEMENT_RECORD);
        out.extend(r);
    }
    for e in entries {
        // Names are display strings the simulation never reads; the id stands in.
        out.extend((e.id.len() as i32).to_le_bytes());
        out.extend(e.id.as_bytes());
    }
    out
}

/// Byte ranges of render-only fields inside a 164-byte record.
pub const RENDER_ONLY: &[(usize, usize)] = &[(0x0b, 0x0c), (0x58, 0x78)];
