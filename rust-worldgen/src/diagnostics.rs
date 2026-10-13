//! Worldgen failures and warnings as data, alongside the error text.
//!
//! A worldgen failure keeps its exact message (`Worldgen failure: <the
//! game's message>` for the stages the game reports) in [`RunError::text`];
//! [`Failure`] says which stage failed, for which world, and for template
//! failures which rule. [`Event`]s are the non-fatal warnings the game logs
//! during worldgen that this port can detect.

use serde::Serialize;

/// A worldgen failure the game reports (`WorldGen.ReportWorldGenError`, or
/// the world mixing check).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Failure {
    /// `WorldMixing`, `SubworldMixing`, `ConvertUnknownCells`, `GenerateLayout`
    /// (a child sampler shortfall), `TemplateSpawning`, `StoryTraits` or
    /// `AssignClusterLocations`.
    pub stage: &'static str,
    /// Placement index of the failing world; -1 for cluster-level stages.
    pub world_index: i32,
    /// The world whose `ReportWorldGenError` the game calls: the failing
    /// world, the first world for story traits, the starting world for
    /// subworld mixing and cluster locations; `None` for world mixing, which
    /// fails in the `Cluster` constructor.
    pub reporting_world_index: Option<i32>,
    /// World file path (`expansion1::worlds/MiniBadlands`) the stage was
    /// handling, when there is one.
    pub world: Option<String>,
    /// The game's message, without the `Worldgen failure: ` prefix.
    pub message: String,
    /// The guaranteed template rule that failed (`TemplateSpawning`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin: Option<RuleOrigin>,
}

/// Where a template rule is authored. `file` is the StreamingAssets-relative
/// YAML path (the overlay key); `index` is the position in the authored list
/// (before trait removals and the priority sort), so `(file, index)` joins
/// against the YAML as read.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RuleOrigin {
    /// `world` (`worldTemplateRules`), `worldTrait` / `storyTrait`
    /// (`additionalWorldTemplateRules`), `subworld` (`subworldTemplateRules`),
    /// `clusterMixing` (`worldPlacements[placement].worldMixing.additionalWorldTemplateRules`)
    /// or `subworldMixing` (`additionalWorldTemplateRules` of the settings).
    pub kind: &'static str,
    pub file: String,
    pub index: usize,
    #[serde(rename = "ruleId")]
    pub rule_id: Option<String>,
    /// World file path whose template pass ran the rule (not for `world`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub world: Option<String>,
    /// `worldPlacements` index (`clusterMixing` only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub placement: Option<usize>,
}

impl RuleOrigin {
    pub fn new(kind: &'static str, qname: &str, index: usize, rule: &serde_yaml::Value) -> Self {
        RuleOrigin { kind, file: yaml_path(qname), index, rule_id: rule.get("ruleId").and_then(|v| v.as_str()).map(str::to_string), world: None, placement: None }
    }
    pub fn in_world(mut self, world: &str) -> Self { self.world = Some(world.to_string()); self }
}

/// A warning the game logs without failing.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Event {
    /// `traits`, `layout`, `template_rules` or `story_traits`.
    pub category: &'static str,
    /// Placement index; -1 for cluster-level events.
    pub world_index: i32,
    pub message: String,
}

/// An error from a cluster run: the text the CLI prints, plus the failure
/// and the warnings logged before it.
#[derive(Debug, Clone, PartialEq)]
pub struct RunError {
    pub text: String,
    pub failure: Option<Failure>,
    pub telemetry: Vec<Event>,
}

impl RunError {
    pub fn failure(text: String, failure: Failure) -> Self { RunError { text, failure: Some(failure), telemetry: Vec::new() } }
    /// Prepend warnings logged before the stage that failed.
    pub fn after(mut self, mut telemetry: Vec<Event>) -> Self {
        telemetry.append(&mut self.telemetry);
        self.telemetry = telemetry;
        self
    }
    /// `{"failure": ..., "telemetry": [...]}`.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({"failure": self.failure, "telemetry": self.telemetry})
    }
}

impl From<String> for RunError {
    fn from(text: String) -> Self { RunError { text, failure: None, telemetry: Vec::new() } }
}
impl From<&str> for RunError {
    fn from(text: &str) -> Self { text.to_string().into() }
}
impl From<RunError> for String {
    fn from(e: RunError) -> Self { e.text }
}
impl std::fmt::Display for RunError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result { f.write_str(&self.text) }
}

/// The `Worldgen failure: ` prefix the CLI's failure text carries.
pub const PREFIX: &str = "Worldgen failure: ";

/// StreamingAssets-relative YAML path of a worldgen file path:
/// `expansion1::worlds/X` -> `dlc/expansion1/worldgen/worlds/X.yaml`.
pub fn yaml_path(qname: &str) -> String {
    match qname.split_once("::") {
        Some((dlc, rest)) => format!("dlc/{dlc}/worldgen/{rest}.yaml"),
        None => format!("worldgen/{qname}.yaml"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn paths_and_text_conversion() {
        assert_eq!(yaml_path("expansion1::worlds/MiniBadlands"), "dlc/expansion1/worldgen/worlds/MiniBadlands.yaml");
        assert_eq!(yaml_path("traits/GeoActive"), "worldgen/traits/GeoActive.yaml");
        let e: RunError = "boom".into();
        assert_eq!(String::from(e.clone()), "boom");
        assert_eq!(e.to_json(), serde_json::json!({"failure": null, "telemetry": []}));
    }
}
