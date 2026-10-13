//! Decode the SETTINGS, STORY and MIXING fields of a coordinate (build 744825).
//!
//! `CustomGameSettings.ParseAndApply*Code` read a base36 BigInteger (least
//! significant digit first) and peel `value % range` off for each coordinated
//! setting in *reverse* registration order. Registration orders and level values
//! are from decompiled `CustomGameSettings`, `CustomGameSettingConfigs`,
//! `CustomMixingSettingsConfigs` and `Database.Stories`.

use crate::dlc::ActiveDlc;
use std::collections::BTreeMap;

/// (setting id, coordinate range, level ids indexed by coordinate value).
type Table = &'static [(&'static str, u32, &'static [&'static str])];

pub const QUALITY: Table = &[
    ("ImmuneSystem", 8, &["Default", "Strong", "Invincible", "Weak", "Compromised"]),
    ("CalorieBurn", 8, &["Default", "Easy", "Disabled", "Hard", "VeryHard"]),
    ("Morale", 8, &["Default", "Easy", "Disabled", "Hard", "VeryHard"]),
    // Pure vanilla swaps the values of Indestructible (0) and Default (2); see `decode_for`.
    ("Durability", 8, &["Default", "Reinforced", "Indestructible", "Flimsy", "Threadbare"]),
    ("MeteorShowers", 8, &["Default", "Infrequent", "ClearSkies", "Intense", "Doomed"]),
    ("Radiation", 8, &["Default", "Easier", "Easiest", "Harder", "Hardest"]),
    ("Stress", 8, &["Default", "Optimistic", "Indomitable", "Pessimistic", "Doomed"]),
    ("StressBreaks", 5, &["Default", "Disabled"]),
    ("CarePackages", 5, &["Enabled", "Disabled"]),
    ("Teleporters", 5, &["Enabled", "Disabled"]),
    ("BionicWattage", 8, &["Default", "Easy", "VeryEasy", "Hard", "VeryHard"]),
    // Off and Hard share value 3 in the game's table; decoding yields the first match.
    ("DemoliorDifficulty", 8, &["Default", "Easy", "VeryEasy", "Off", "VeryHard"]),
];

pub const STORIES: [&str; 6] = ["MegaBrainTank", "CreatureManipulator", "LonelyMinion", "FossilHunt", "MorbRoverMaker", "HijackHeadquarters"];

const MIX: &[&str] = &["Disabled", "TryMixing", "GuranteeMixing"];
const DLC: &[&str] = &["Disabled", "Enabled"];
pub const MIXING: Table = &[
    ("DLC2_ID", 5, DLC), ("IceCavesMixing", 5, MIX), ("CarrotQuarryMixing", 5, MIX), ("SugarWoodsMixing", 5, MIX),
    ("CeresAsteroidMixing", 5, MIX), ("DLC3_ID", 5, DLC), ("DLC4_ID", 5, DLC), ("GardenMixing", 5, MIX),
    ("RaptorMixing", 5, MIX), ("WetlandsMixing", 5, MIX), ("PrehistoricAsteroidMixing", 5, MIX), ("DLC5_ID", 5, DLC),
    ("BeachMixing", 5, MIX), ("ReefMixing", 5, MIX), ("KelpForestMixing", 5, MIX), ("AbyssMixing", 5, MIX),
    ("AquaticAsteroidMixing", 5, MIX),
];

/// Decoded coordinate options. Maps hold every coordinated setting.
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize)]
pub struct CustomSettings {
    pub quality: BTreeMap<String, String>,
    pub stories: Vec<String>,
    pub mixing: BTreeMap<String, String>,
}

impl CustomSettings {
    /// True when every quality setting is at its default (value 0) level.
    pub fn quality_is_default(&self) -> bool {
        QUALITY.iter().all(|(id, _, levels)| self.quality.get(*id).map(String::as_str) == Some(levels[0]))
    }
    /// True when no mixing setting is enabled.
    pub fn mixing_is_default(&self) -> bool {
        MIXING.iter().all(|(id, _, levels)| self.mixing.get(*id).map(String::as_str) == Some(levels[0]))
    }
    pub fn level(&self, id: &str) -> Option<&str> {
        self.quality.get(id).or_else(|| self.mixing.get(id)).map(String::as_str)
    }
    /// The content (DLC ids) that the non-default quality and mixing levels
    /// need, in registration order without repeats.
    pub fn required_content(&self) -> Vec<&'static str> {
        let mut out: Vec<&'static str> = Vec::new();
        for (id, _, levels) in QUALITY {
            if self.quality.get(*id).is_some_and(|l| l != levels[0]) {
                if let Some((_, content)) = QUALITY_REQUIRED_CONTENT.iter().find(|(q, _)| q == id) { out.push(content); }
            }
        }
        for (id, _, levels) in MIXING {
            if self.mixing.get(*id).is_some_and(|l| l != levels[0]) {
                if let Some(content) = crate::mixing::required_content(id) { out.push(content); }
            }
        }
        let mut seen = Vec::new();
        out.retain(|c| if seen.contains(c) { false } else { seen.push(*c); true });
        out
    }
}

/// Little-endian base36 as written by `BinarytoBase36` ("0" for zero).
fn parse_base36(code: &str) -> Result<Vec<u32>, String> {
    // Arbitrary precision: little-endian base-2^32 limbs.
    let mut limbs: Vec<u32> = vec![0];
    for ch in code.chars().rev() {
        let digit = ch.to_digit(36).ok_or_else(|| format!("invalid base36 digit {ch:?} in {code:?}"))?;
        let mut carry = digit as u64;
        for limb in limbs.iter_mut() {
            let v = *limb as u64 * 36 + carry;
            *limb = v as u32;
            carry = v >> 32;
        }
        if carry > 0 { limbs.push(carry as u32); }
    }
    Ok(limbs)
}

fn div_rem(limbs: &mut Vec<u32>, divisor: u32) -> u32 {
    let mut rem = 0u64;
    for limb in limbs.iter_mut().rev() {
        let v = (rem << 32) | *limb as u64;
        *limb = (v / divisor as u64) as u32;
        rem = v % divisor as u64;
    }
    while limbs.len() > 1 && *limbs.last().unwrap() == 0 { limbs.pop(); }
    rem as u32
}

fn decode_table(code: &str, table: Table) -> Result<BTreeMap<String, String>, String> {
    let mut value = parse_base36(code)?;
    let mut out = BTreeMap::new();
    for (id, range, levels) in table.iter().rev() {
        let v = div_rem(&mut value, *range) as usize;
        let level = levels.get(v).ok_or_else(|| format!("{id}: no level with coordinate value {v}"))?;
        out.insert((*id).to_string(), (*level).to_string());
    }
    if value.iter().any(|&l| l != 0) { return Err(format!("code {code:?} has more digits than its settings table")); }
    Ok(out)
}

/// Content each coordinated quality setting requires (`SettingConfig.required_content`).
/// `CustomGameSettings.GetCurrentQualitySetting` returns the default level when
/// that content is not subscribed, whatever the coordinate says.
pub const QUALITY_REQUIRED_CONTENT: &[(&str, &str)] = &[
    ("Radiation", "EXPANSION1_ID"), ("Teleporters", "EXPANSION1_ID"),
    ("BionicWattage", "DLC3_ID"), ("DemoliorDifficulty", "DLC4_ID"),
];

/// Default level id of a quality setting. Durability's default is
/// "Indestructible" under pure vanilla (`CustomGameSettingConfigs.Durability`).
pub fn default_quality_level(id: &str, pure_vanilla: bool) -> Option<&'static str> {
    if id == "Durability" && pure_vanilla { return Some("Indestructible"); }
    QUALITY.iter().find(|(q, _, _)| *q == id).map(|(_, _, levels)| levels[0])
}

impl CustomSettings {
    /// Quality levels the game actually uses for `dlc`: settings whose
    /// required content is inactive fall back to their default level.
    pub fn effective_quality(&self, dlc: &ActiveDlc) -> BTreeMap<String, String> {
        self.quality.iter().map(|(id, level)| {
            let inactive = QUALITY_REQUIRED_CONTENT.iter().any(|(q, content)| q == id && !dlc.is_active(content));
            let level = if inactive { default_quality_level(id, dlc.is_pure_vanilla()).unwrap().to_string() } else { level.clone() };
            (id.clone(), level)
        }).collect()
    }
    /// `GameSpawnData.AddTemplate`: warp portal, receiver and conduit
    /// sender/receiver entities are dropped only when Spaced Out is active and
    /// the effective Teleporters level is not "Enabled".
    pub fn teleporters_enabled(&self, dlc: &ActiveDlc) -> bool {
        !dlc.expansion1 || self.effective_quality(dlc).get("Teleporters").map(String::as_str) != Some("Disabled")
    }
}

/// Decode `other`, `story` and `mixing` for an active DLC set. Identical to
/// [`decode`] except that pure vanilla uses its Durability coordinate values
/// (Indestructible = 0, Default = 2).
pub fn decode_for(other: &str, story: &str, mixing: &str, dlc: &ActiveDlc) -> Result<CustomSettings, String> {
    let mut settings = decode(other, story, mixing)?;
    if dlc.is_pure_vanilla() {
        if let Some(level) = settings.quality.get_mut("Durability") {
            *level = match level.as_str() { "Default" => "Indestructible", "Indestructible" => "Default", other => other }.to_string();
        }
    }
    Ok(settings)
}

/// Decode `other`, `story` and `mixing` fields of a parsed coordinate, using
/// the Spaced Out / non-vanilla Durability values.
pub fn decode(other: &str, story: &str, mixing: &str) -> Result<CustomSettings, String> {
    let quality = decode_table(other, QUALITY)?;
    let story_table: Vec<(&'static str, u32, &'static [&'static str])> =
        STORIES.iter().map(|id| (*id, 3u32, &["Disabled", "Guaranteed"][..])).collect();
    let story_table: Table = Box::leak(story_table.into_boxed_slice());
    let stories = decode_table(story, story_table)?
        .into_iter().filter(|(_, level)| level == "Guaranteed").map(|(id, _)| id).collect::<Vec<_>>();
    // Keep the game's coordinate order rather than BTreeMap order.
    let stories = STORIES.iter().filter(|id| stories.iter().any(|s| s == *id)).map(|s| s.to_string()).collect();
    Ok(CustomSettings { quality, stories, mixing: decode_table(mixing, MIXING)? })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn defaults_decode_to_default_levels() {
        let s = decode("0", "0", "0").unwrap();
        assert!(s.quality_is_default() && s.mixing_is_default() && s.stories.is_empty());
    }
    #[test]
    fn codes_match_the_js_encoder() {
        // node scripts/coordinate-codes.js SNDST-C 1 story:<all six>  -> 4A
        assert_eq!(decode("0", "4A", "0").unwrap().stories, STORIES.to_vec());
        // ... Teleporters=Disabled -> S1
        let s = decode("S1", "0", "0").unwrap();
        assert_eq!(s.level("Teleporters"), Some("Disabled"));
        assert_eq!(QUALITY.iter().filter(|(id, _, l)| s.level(id) != Some(l[0])).count(), 1);
        // ... mix:DLC2_ID=Enabled mix:IceCavesMixing=Guarantee -> BW9NX4Q2
        let m = decode("0", "0", "BW9NX4Q2").unwrap();
        assert_eq!((m.level("DLC2_ID"), m.level("IceCavesMixing")), (Some("Enabled"), Some("GuranteeMixing")));
        assert_eq!(m.level("CeresAsteroidMixing"), Some("Disabled"));
    }
    // Code-derived (CustomGameSettingConfigs.Durability, IsPureVanilla); the
    // vanilla mapping has not been confirmed against the game yet.
    #[test]
    fn durability_values_depend_on_pure_vanilla() {
        let e1 = ActiveDlc { expansion1: true, ..ActiveDlc::VANILLA };
        // Coordinate value 0 everywhere: Default in Spaced Out, Indestructible in vanilla.
        assert_eq!(decode_for("0", "0", "0", &e1).unwrap().level("Durability"), Some("Default"));
        assert_eq!(decode_for("0", "0", "0", &ActiveDlc::VANILLA).unwrap().level("Durability"), Some("Indestructible"));
        assert_eq!(default_quality_level("Durability", true), Some("Indestructible"));
        assert_eq!(default_quality_level("Durability", false), Some("Default"));
    }
    // Code-derived (GetCurrentQualitySetting required_content fallback and
    // GameSpawnData.AddTemplate's IsExpansion1Active gate).
    #[test]
    fn teleporters_only_matter_with_spaced_out() {
        let s = decode("S1", "0", "0").unwrap();
        let e1 = ActiveDlc { expansion1: true, dlc2: true, ..ActiveDlc::VANILLA };
        assert!(!s.teleporters_enabled(&e1));
        assert!(s.teleporters_enabled(&ActiveDlc::VANILLA));
        assert!(s.teleporters_enabled(&ActiveDlc { dlc2: true, ..ActiveDlc::VANILLA }));
        assert_eq!(s.effective_quality(&ActiveDlc::VANILLA).get("Teleporters").map(String::as_str), Some("Enabled"));
        assert!(decode("0", "0", "0").unwrap().teleporters_enabled(&e1));
    }
    #[test]
    fn rejects_overlong_and_invalid_codes() {
        assert!(decode("ZZZZZZZZZZZZZZZZZZZZZZZZZZZZZ", "0", "0").is_err());
        assert!(decode("0", "!", "0").is_err());
    }
}
