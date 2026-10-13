//! Coordinate parser.
//!
//! Format: `PREFIX-SEED-OTHER-STORYCODE-MIXCODE`
//!
//! - **PREFIX**: cluster identifier, may contain hyphens (e.g. `V-SNDST-C`,
//!   `M-FRZ-C`). Maps to a cluster YAML via `coordinatePrefix` field.
//! - **SEED**: 32-bit signed decimal. Used as `globalWorldSeed`.
//! - **OTHER**: base36-encoded BigInt of "other settings" (game mode, etc.)
//!   Usually `"0"` in DB coords.
//! - **STORYCODE**: base36 bitmask of story traits. `"0"` = none, `"4A"`
//!   = all stories in full SpacedOut.
//! - **MIXCODE**: base36-ish code mixing DLC and subworld mixing toggles.
//!   `"0"` = no mixing, `"MUWF1"` = all mixing on.
//!
//! Port of `CustomGameSettings.ParseSettingCoordinate`.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Coord {
    pub prefix: String,       // e.g. "V-SNDST-C"
    pub seed: i32,
    pub other: String,        // usually "0"
    pub story: String,        // base36 bitmask
    pub mixing: String,       // base36 mixing code
}

impl Coord {
    /// Greedy parse mirroring the game regex `(.*)-(\d*)-(.*)-(.*)-(.*)`.
    /// Missing trailing sections default to "0".
    pub fn parse(coord: &str) -> Option<Coord> {
        // Work right-to-left: mixing is after the last '-', story after
        // the second-to-last '-', other after the third, seed after the
        // fourth, prefix is everything before that.
        //
        // But C# falls back to shorter regex forms if the full one
        // doesn't match — meaning coords without all 5 fields are
        // still accepted. We re-emit those missing fields as "0".

        let parts: Vec<&str> = coord.splitn(5, |_| false).collect();
        // Find the rightmost hyphens that separate fields. The SEED is
        // the rightmost purely-numeric substring between two dashes —
        // everything to its left is the prefix.
        let mut tokens: Vec<&str> = coord.split('-').collect();
        if tokens.len() < 2 {
            return None;
        }
        // Scan from the end looking for the seed token.
        // The coord has the shape: prefix_parts... , seed , other , story , mixing
        // So the last 4 tokens are [seed, other, story, mixing].
        // But if we have fewer than 5 tokens total, we have a truncated
        // coord and zero-fill trailing fields.

        let n = tokens.len();
        let _ = parts; // silence

        // Try full 5-field form.
        if n >= 5 {
            let mixing = tokens.pop()?;
            let story = tokens.pop()?;
            let other = tokens.pop()?;
            let seed_s = tokens.pop()?;
            let prefix = tokens.join("-");
            return Some(Coord {
                prefix,
                seed: seed_s.parse().ok()?,
                other: other.to_string(),
                story: story.to_string(),
                mixing: mixing.to_string(),
            });
        }
        // n == 4: no mixing code.
        if n == 4 {
            let story = tokens.pop()?;
            let other = tokens.pop()?;
            let seed_s = tokens.pop()?;
            let prefix = tokens.join("-");
            return Some(Coord {
                prefix,
                seed: seed_s.parse().ok()?,
                other: other.to_string(),
                story: story.to_string(),
                mixing: "0".to_string(),
            });
        }
        // n == 3: no story or mixing.
        if n == 3 {
            let other = tokens.pop()?;
            let seed_s = tokens.pop()?;
            let prefix = tokens.join("-");
            return Some(Coord {
                prefix,
                seed: seed_s.parse().ok()?,
                other: other.to_string(),
                story: "0".to_string(),
                mixing: "0".to_string(),
            });
        }
        // n == 2: prefix + seed.
        let seed_s = tokens.pop()?;
        let prefix = tokens.join("-");
        Some(Coord {
            prefix,
            seed: seed_s.parse().ok()?,
            other: "0".to_string(),
            story: "0".to_string(),
            mixing: "0".to_string(),
        })
    }

    /// Re-serialize in canonical form.
    pub fn canonical(&self) -> String {
        format!("{}-{}-{}-{}-{}", self.prefix, self.seed, self.other, self.story, self.mixing)
    }
}

/// Decode a base36 string (lowercase or mixed-case, plus digits) to
/// an arbitrary-precision value as u128. The game uses BigInteger
/// because story masks can be many bits, but in practice the MIX code
/// fits easily in u128.
pub fn base36_to_u128(s: &str) -> Option<u128> {
    if s == "0" { return Some(0); }
    let mut v: u128 = 0;
    // Klei's hexChars order: 0-9, then A-Z (uppercase).
    for c in s.chars().rev() {
        let d = match c {
            '0'..='9' => c as u32 - '0' as u32,
            'A'..='Z' => c as u32 - 'A' as u32 + 10,
            'a'..='z' => c as u32 - 'a' as u32 + 10,
            _ => return None,
        };
        v = v.checked_mul(36)?.checked_add(d as u128)?;
    }
    Some(v)
}

// ---------------------------------------------------------------------------
// In-game reproducibility (build 744825)
//
// The new-game screen applies a coordinate in
// `ColonyDestinationSelectScreen.CoordinateChanged`:
//
// 1. `ParseSettingCoordinate` splits it; fewer than 3 fields or a seed that
//    `int.TryParse` rejects makes it return without changing anything.
// 2. The cluster whose `coordinatePrefix` equals the prefix is selected. Only
//    clusters in `SettingsCache.clusterLayouts` are searched (DLC-compatible,
//    not `skip`); an unknown prefix leaves the previous cluster selected.
// 3. `WorldgenSeed` is set to the seed text verbatim (`SeedSettingConfig`
//    keeps leading zeros), then the three codes are applied by
//    `ParseAndApply{Settings,StoryTraitSettings,MixingSettings}Code`.
//
// Every setting change fires `QualitySettingChanged` ->
// `RefreshRowsAndDescriptions`, which replaces the seed with the cluster's
// `fixedCoordinate` when it is not -1 (KF23-A/KF23-C: 7) and selects the
// cluster in `DestinationSelectPanel` (throws for `clusters/SandstoneDefault`
// under Spaced Out, which the panel hides). `GetSettingsCoordinate` then
// re-encodes the *effective* levels, and `LaunchClicked` runs
// `RemoveInvalidMixingSettings` before worldgen.
// ---------------------------------------------------------------------------

use crate::custom_settings::{CustomSettings, MIXING, QUALITY, QUALITY_REQUIRED_CONTENT, STORIES};
use crate::data_source::DataSource;
use crate::dlc::ActiveDlc;

/// Outcome of [`validate_coordinate`] for a coordinate the game applies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoordinateCheck {
    /// `GetSettingsCoordinate` returns the input unchanged and launch keeps it.
    Accepted(String),
    /// The game silently applies, and generates, `to` instead.
    Rewritten { to: String, reason: String },
}

/// `CustomGameSettings.ParseSettingCoordinate`: the regex
/// `(.*)-(\d*)-(.*)-(.*)-(.*)`, then the same with one and two trailing
/// `-(.*)` removed. Returns the matched groups (3 to 5 of them), or `None`
/// when no form matches. Group 1 is greedy, so the seed is the last digit run
/// that is followed by `-` and leaves enough hyphens for the later groups.
pub fn parse_setting_coordinate(coord: &str) -> Option<Vec<&str>> {
    let bytes = coord.as_bytes();
    for trailing in [2usize, 1, 0] {
        for p in (0..bytes.len()).rev().filter(|&i| bytes[i] == b'-') {
            let seed_end = p + 1 + bytes[p + 1..].iter().take_while(|b| b.is_ascii_digit()).count();
            if bytes.get(seed_end) != Some(&b'-') { continue; }
            let rest = &coord[seed_end + 1..];
            let hyphens: Vec<usize> = rest.match_indices('-').map(|(i, _)| i).collect();
            if hyphens.len() < trailing { continue; }
            let mut groups = vec![&coord[..p], &coord[p + 1..seed_end]];
            let mut start = 0;
            for &cut in &hyphens[hyphens.len() - trailing..] {
                groups.push(&rest[start..cut]);
                start = cut + 1;
            }
            groups.push(&rest[start..]);
            return Some(groups);
        }
    }
    None
}

const HEX_CHARS: &str = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ";

/// `Base36toBinary` as a sign and little-endian u32 magnitude. Digits are
/// `hexChars.IndexOf(c)`, so any character outside 0-9A-Z (lowercase too)
/// counts as -1 and can make the value negative.
fn base36_signed(code: &str) -> (bool, Vec<u32>) {
    let (mut negative, mut mag) = (false, vec![0u32]);
    if code == "0" { return (negative, mag); }
    for ch in code.chars().rev() {
        let digit = HEX_CHARS.find(ch).map_or(-1, |i| i as i64);
        let was_zero = mag.iter().all(|&l| l == 0);
        limbs_mul_add(&mut mag, 36, 0);
        if was_zero {
            negative = digit < 0;
            mag = vec![digit.unsigned_abs() as u32];
        } else if (digit >= 0) != negative {
            limbs_mul_add(&mut mag, 1, digit.unsigned_abs() as u32);
        } else {
            // |value| >= 36 > |digit|, so the sign cannot flip.
            let mut borrow = digit.unsigned_abs();
            for limb in mag.iter_mut() {
                let (v, under) = (*limb as u64).overflowing_sub(borrow);
                *limb = v as u32;
                borrow = under as u64;
                if !under { break; }
            }
        }
    }
    (negative, mag)
}

fn limbs_mul_add(mag: &mut Vec<u32>, mul: u32, add: u32) {
    let mut carry = add as u64;
    for limb in mag.iter_mut() {
        let v = *limb as u64 * mul as u64 + carry;
        *limb = v as u32;
        carry = v >> 32;
    }
    if carry > 0 { mag.push(carry as u32); }
}

fn limbs_div_rem(mag: &mut [u32], divisor: u32) -> u32 {
    let mut rem = 0u64;
    for limb in mag.iter_mut().rev() {
        let v = (rem << 32) | *limb as u64;
        *limb = (v / divisor as u64) as u32;
        rem = v % divisor as u64;
    }
    rem as u32
}

/// One `ParseAndApply*Code` pass over `(range, level count)` in registration
/// order. C# BigInteger `%` and `/` truncate toward zero, so a negative value
/// peels non-positive remainders. `None` marks a remainder no level has (the
/// game leaves that setting unchanged). The flag reports digits left over
/// after the last setting, which the game drops.
fn peel(code: &str, table: &[(u32, usize)]) -> (Vec<Option<u32>>, bool) {
    let (negative, mut mag) = base36_signed(code);
    let mut values = vec![None; table.len()];
    for (i, &(range, levels)) in table.iter().enumerate().rev() {
        let rem = limbs_div_rem(&mut mag, range);
        values[i] = (rem == 0 || (!negative && (rem as usize) < levels)).then_some(if negative { 0 } else { rem });
    }
    (values, mag.iter().any(|&l| l != 0))
}

/// `Get*SettingsCode` + `BinarytoBase36`.
fn encode(values: &[u32], ranges: impl Iterator<Item = u32>) -> String {
    let mut v = values.iter().zip(ranges).fold(0u128, |acc, (&x, r)| acc * r as u128 + x as u128);
    if v == 0 { return "0".into(); }
    let mut out = String::new();
    while v > 0 {
        out.push(HEX_CHARS.as_bytes()[(v % 36) as usize] as char);
        v /= 36;
    }
    out
}

/// No Sweat level coordinate values in `QUALITY` order (`nosweat_default_level_id`).
/// Durability's is Reinforced (1), or Indestructible (0) under pure vanilla.
fn nosweat_values(pure_vanilla: bool) -> [u32; 12] {
    [1, 1, 1, if pure_vanilla { 0 } else { 1 }, 1, 1, 1, 0, 0, 0, 1, 1]
}

fn quality_level(index: usize, value: u32, pure_vanilla: bool) -> &'static str {
    let (id, _, levels) = QUALITY[index];
    match (id, value, pure_vanilla) {
        ("Durability", 0, true) => "Indestructible",
        ("Durability", 2, true) => "Default",
        _ => levels[value as usize],
    }
}

/// Predict what the game does with `coord` when the new-game screen applies
/// it with `active_dlc` subscribed (the cluster's own category selected,
/// sandbox and debug fast-workers off).
///
/// `Err` when the game ignores part of the coordinate (so the result depends
/// on what was selected before) or throws. `Rewritten` when it silently
/// normalises: fixed-seed clusters, codes that are not in canonical form,
/// settings or mixing whose DLC is not subscribed, and mixing that
/// `RemoveInvalidMixingSettings` drops at launch.
pub fn validate_coordinate(coord: &str, ds: &DataSource, active_dlc: ActiveDlc) -> Result<CoordinateCheck, String> {
    if coord.chars().any(|c| !c.is_ascii() || c.is_ascii_control()) {
        return Err("coordinate contains non-ASCII or control characters".into());
    }
    let fields = parse_setting_coordinate(coord)
        .ok_or("coordinate does not match PREFIX-SEED-SETTINGS[-STORY[-MIXING]]; CoordinateChanged ignores it")?;
    let (prefix, seed_text, other) = (fields[0], fields[1], fields[2]);
    let story = fields.get(3).copied().unwrap_or("0");
    let mixing = fields.get(4).copied().unwrap_or("0");
    if seed_text.parse::<i32>().is_err() {
        return Err(format!("seed {seed_text:?} is not a 32-bit integer; CoordinateChanged ignores the coordinate"));
    }
    let keeps = "so CoordinateChanged keeps the previously selected cluster";
    let cluster = ds.clusters.get(prefix).ok_or_else(|| format!("unknown cluster prefix {prefix:?}, {keeps}"))?;
    if cluster.skip { return Err(format!("cluster {prefix} is editor-only (skip) and not loaded by the game, {keeps}")); }
    active_dlc.check_cluster(cluster).map_err(|e| format!("{e}; the game does not load it, {keeps}"))?;
    if active_dlc.expansion1 && cluster.id == "clusters/SandstoneDefault" {
        return Err(format!("with Spaced Out active DestinationSelectPanel hides {} ({prefix}); selecting it throws", cluster.id));
    }

    let mut reasons = Vec::new();
    let seed = if cluster.fixed_coordinate != -1 {
        let fixed = cluster.fixed_coordinate.to_string();
        if fixed != seed_text {
            reasons.push(format!("cluster {prefix} has fixedCoordinate {fixed}, which RefreshRowsAndDescriptions forces as the seed"));
        }
        fixed
    } else {
        seed_text.to_string()
    };

    let vanilla = active_dlc.is_pure_vanilla();
    let mut peel_field = |name: &str, code: &str, table: &[(u32, usize)], ids: &dyn Fn(usize) -> &'static str| -> Result<Vec<u32>, String> {
        let (values, leftover) = peel(code, table);
        if leftover { reasons.push(format!("{name} code {code} has digits beyond its settings, which the game drops")); }
        values.iter().enumerate().map(|(i, v)| v.ok_or_else(|| format!(
            "{name} code {code} gives {} a coordinate value with no level; the game keeps the previously selected level", ids(i)
        ))).collect()
    };
    let quality_table: Vec<(u32, usize)> = QUALITY.iter().map(|q| (q.1, q.2.len())).collect();
    let mut quality = peel_field("settings", other, &quality_table, &|i| QUALITY[i].0)?;
    let story_values = peel_field("story", story, &[(3, 2); STORIES.len()], &|i| STORIES[i])?;
    let mixing_table: Vec<(u32, usize)> = MIXING.iter().map(|m| (m.1, m.2.len())).collect();
    let mut mixing_values = peel_field("mixing", mixing, &mixing_table, &|i| MIXING[i].0)?;

    // GetCurrentQualitySetting: in Survival or No Sweat mode (every
    // triggers_custom_game setting at that preset) the preset level is
    // returned; otherwise settings whose required_content is not subscribed
    // read as their default (coordinate value 0).
    let survival = quality.iter().all(|&v| v == 0);
    let nosweat = quality.iter().zip(nosweat_values(vanilla)).all(|(&v, n)| v == n);
    if !survival && !nosweat {
        for (i, v) in quality.iter_mut().enumerate() {
            let id = QUALITY[i].0;
            let Some((_, content)) = QUALITY_REQUIRED_CONTENT.iter().find(|(q, _)| *q == id) else { continue };
            if *v != 0 && !active_dlc.is_active(content) {
                reasons.push(format!("{id}={} needs {content}, which is not active; the game uses {}",
                    quality_level(i, *v, vanilla), quality_level(i, 0, vanilla)));
                *v = 0;
            }
        }
    }
    // GetCurrentMixingSettingLevel: the same fallback, without presets.
    for (i, v) in mixing_values.iter_mut().enumerate() {
        let (id, _, levels) = MIXING[i];
        let content = crate::mixing::required_content(id).unwrap_or("");
        if *v != 0 && !active_dlc.is_active(content) {
            reasons.push(format!("mixing {id}={} needs {content}, which is not active", levels[*v as usize]));
            *v = 0;
        }
    }
    // LaunchClicked -> RemoveInvalidMixingSettings, after the check above.
    let settings = CustomSettings {
        mixing: MIXING.iter().zip(&mixing_values).map(|(m, &v)| (m.0.to_string(), m.2[v as usize].to_string())).collect(),
        ..Default::default()
    };
    for id in crate::mixing::active(&settings, cluster, ds, Some(active_dlc))?.removed {
        let i = MIXING.iter().position(|m| m.0 == id).ok_or_else(|| format!("unknown mixing setting {id}"))?;
        reasons.push(format!("at launch RemoveInvalidMixingSettings disables mixing {id} for {prefix}"));
        mixing_values[i] = 0;
    }

    let to = format!("{prefix}-{seed}-{}-{}-{}",
        encode(&quality, QUALITY.iter().map(|q| q.1)),
        encode(&story_values, std::iter::repeat(3)),
        encode(&mixing_values, MIXING.iter().map(|m| m.1)));
    if to == coord { return Ok(CoordinateCheck::Accepted(to)); }
    if reasons.is_empty() {
        reasons.push(if fields.len() < 5 { "missing fields are written as 0".into() } else { "its codes are not in the game's canonical form".into() });
    }
    Ok(CoordinateCheck::Rewritten { to, reason: reasons.join("; ") })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_basegame() {
        let c = Coord::parse("SNDST-A-42-0-0-0").unwrap();
        assert_eq!(c.prefix, "SNDST-A");
        assert_eq!(c.seed, 42);
        assert_eq!(c.other, "0");
        assert_eq!(c.story, "0");
        assert_eq!(c.mixing, "0");
    }

    // Negative-seed coords do not occur in DB. If needed, the parser
    // can be extended; we keep the common case fast.

    #[test]
    fn parse_spacedout_with_mixing() {
        let c = Coord::parse("V-SNDST-C-42-0-4A-MUWF1").unwrap();
        assert_eq!(c.prefix, "V-SNDST-C");
        assert_eq!(c.seed, 42);
        assert_eq!(c.story, "4A");
        assert_eq!(c.mixing, "MUWF1");
    }

    #[test]
    fn parse_moonlet() {
        let c = Coord::parse("M-FRZ-C-100-0-0-MUWF1").unwrap();
        assert_eq!(c.prefix, "M-FRZ-C");
        assert_eq!(c.seed, 100);
        assert_eq!(c.mixing, "MUWF1");
    }

    #[test]
    fn roundtrip() {
        for s in ["SNDST-A-42-0-0-0", "V-SNDST-C-42-0-4A-MUWF1", "M-FRZ-C-100-0-0-MUWF1"] {
            assert_eq!(Coord::parse(s).unwrap().canonical(), s);
        }
    }

    #[test]
    fn base36_roundtrips() {
        // Klei's encoding is LITTLE-ENDIAN base36 (first char = LSB).
        // Verified by reading CustomGameSettings.Base36toBinary in the
        // decompiled source.
        assert_eq!(base36_to_u128("0"), Some(0));
        assert_eq!(base36_to_u128("Z"), Some(35));
        assert_eq!(base36_to_u128("10"), Some(1));   // digit[0]='1'=LSB → 1
        assert_eq!(base36_to_u128("01"), Some(36));  // digit[0]='0', digit[1]='1'*36 → 36
        // Full mixing code is valid base36.
        assert!(base36_to_u128("MUWF1").is_some());
    }
}

/// Code-derived from `ParseSettingCoordinate`, `Base36toBinary` and
/// `ParseAndApply*Code`.
#[cfg(test)]
mod validity_tests {
    use super::*;

    #[test]
    fn splits_like_the_game_regex() {
        assert_eq!(parse_setting_coordinate("V-SNDST-C-42-0-4A-MUWF1").unwrap(), ["V-SNDST-C", "42", "0", "4A", "MUWF1"]);
        assert_eq!(parse_setting_coordinate("KF23-C-1-0-0").unwrap(), ["KF23-C", "1", "0", "0"]);
        // Coord::parse would read this as prefix SNDST, seed A.
        assert_eq!(parse_setting_coordinate("SNDST-A-42-0").unwrap(), ["SNDST-A", "42", "0"]);
        // (\d*) may be empty; int.TryParse then rejects it.
        assert_eq!(parse_setting_coordinate("SNDST-C--0-0-0").unwrap(), ["SNDST-C", "", "0", "0", "0"]);
        assert_eq!(parse_setting_coordinate("A-B"), None);
        assert_eq!(parse_setting_coordinate("SNDST-C"), None);
    }

    #[test]
    fn peels_and_encodes_like_the_game() {
        let quality: Vec<(u32, usize)> = QUALITY.iter().map(|q| (q.1, q.2.len())).collect();
        let (values, leftover) = peel("S1", &quality);
        assert!(!leftover);
        assert_eq!(values.iter().position(|v| *v == Some(1)), Some(9)); // Teleporters=Disabled
        assert_eq!(encode(&values.iter().map(|v| v.unwrap()).collect::<Vec<_>>(), QUALITY.iter().map(|q| q.1)), "S1");
        // Lowercase is IndexOf == -1: "s1" = 1*36 - 1 = 35 = "Z".
        assert_eq!(peel("s1", &quality), peel("Z", &quality));
        // A negative value peels -1 for DemoliorDifficulty: no level matches.
        assert_eq!(peel("a", &quality).0[11], None);
        // Trailing zeros are high digits; extra digits are left over.
        assert_eq!(peel("S100", &quality), peel("S1", &quality));
        assert!(peel("ZZZZZZZZZZZZZZZZZZZZ", &quality).1);
        // Values 5..7 of a range-8 setting have no level.
        assert_eq!(peel("5", &quality).0[11], None);
    }
}
