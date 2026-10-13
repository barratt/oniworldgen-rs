//! Germ parameters (the diseases message), transcribed from the game's
//! managed disease definitions for build 744825, and the rule engine that
//! expands per-element growth rules over the element table.
//!
//! Sources: Klei.AI.Disease (defaults, Polypropylene/Vacuum overrides, rule
//! application), Klei.AI.DiseaseGrowthRules.{GrowthRule, StateGrowthRule,
//! ElementGrowthRule, TagGrowthRule}, and the five disease classes cited per
//! entry below. Display names are STRINGS.DUPLICANTS.DISEASES.<ID>.NAME.
use crate::element_yaml::Entry;
use crate::tables::element_hash;

const INF: f32 = f32::INFINITY;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Growth {
    pub under_population_death_rate: f32,
    pub population_half_life: f32,
    pub over_population_half_life: f32,
    pub diffusion_scale: f32,
    pub min_count_per_kg: f32,
    pub max_count_per_kg: f32,
    pub min_diffusion_count: i32,
    pub min_diffusion_infestation_tick_count: u8,
}

/// Disease.DEFAULT_GROWTH_INFO.
const DEFAULT: Growth = Growth {
    under_population_death_rate: 0.0,
    population_half_life: INF,
    over_population_half_life: INF,
    diffusion_scale: 1.0,
    min_count_per_kg: 0.0,
    max_count_per_kg: INF,
    min_diffusion_count: 0,
    min_diffusion_infestation_tick_count: 255,
};

/// Which elements a rule applies to.
#[derive(Clone, Copy, Debug)]
pub enum Test {
    All,
    /// Element.State: 1 gas, 2 liquid, 3 solid.
    State(u8),
    Element(&'static str),
    Tag(&'static str),
}

/// A growth rule: fields left `None` keep their current value.
#[derive(Clone, Copy, Debug)]
pub struct Rule {
    pub test: Test,
    pub under_population_death_rate: Option<f32>,
    pub population_half_life: Option<f32>,
    pub over_population_half_life: Option<f32>,
    pub diffusion_scale: Option<f32>,
    pub min_count_per_kg: Option<f32>,
    pub max_count_per_kg: Option<f32>,
    pub min_diffusion_count: Option<i32>,
    pub min_diffusion_infestation_tick_count: Option<u8>,
}

const R: Rule = Rule {
    test: Test::All,
    under_population_death_rate: None,
    population_half_life: None,
    over_population_half_life: None,
    diffusion_scale: None,
    min_count_per_kg: None,
    max_count_per_kg: None,
    min_diffusion_count: None,
    min_diffusion_infestation_tick_count: None,
};

/// (minViable, minGrowth, maxGrowth, maxViable).
pub type Range = [f32; 4];
const IDEMPOTENT: Range = [INF, INF, INF, INF];

pub struct DiseaseDef {
    pub id: &'static str,
    pub name: &'static str,
    /// The Disease constructor takes a strength but never stores it, so the
    /// managed side sends 0 for every disease.
    pub strength_sent: f32,
    pub temperature_range: Range,
    pub temperature_half_lives: Range,
    pub pressure_range: Range,
    pub pressure_half_lives: Range,
    pub radiation_kill_rate: f32,
    pub rules: &'static [Rule],
}

/// Klei.AI.FoodGerms ("FoodPoisoning").
const FOOD_GERMS: &[Rule] = &[
    Rule { under_population_death_rate: Some(2.6666667), min_count_per_kg: Some(0.4), population_half_life: Some(12000.0), max_count_per_kg: Some(1000.0), over_population_half_life: Some(3000.0), min_diffusion_count: Some(1000), diffusion_scale: Some(0.001), min_diffusion_infestation_tick_count: Some(1), ..R },
    Rule { test: Test::State(3), min_count_per_kg: Some(0.4), population_half_life: Some(300.0), over_population_half_life: Some(10.0), min_diffusion_count: Some(1000000), ..R },
    Rule { test: Test::Element("ToxicSand"), population_half_life: Some(INF), over_population_half_life: Some(12000.0), ..R },
    Rule { test: Test::Element("Creature"), population_half_life: Some(INF), max_count_per_kg: Some(4000.0), over_population_half_life: Some(3000.0), ..R },
    Rule { test: Test::Element("BleachStone"), population_half_life: Some(10.0), over_population_half_life: Some(10.0), diffusion_scale: Some(0.001), ..R },
    Rule { test: Test::State(1), min_count_per_kg: Some(250.0), population_half_life: Some(1200.0), over_population_half_life: Some(300.0), diffusion_scale: Some(0.01), ..R },
    Rule { test: Test::Element("ContaminatedOxygen"), population_half_life: Some(12000.0), max_count_per_kg: Some(10000.0), over_population_half_life: Some(3000.0), diffusion_scale: Some(0.05), ..R },
    Rule { test: Test::Element("ChlorineGas"), population_half_life: Some(10.0), over_population_half_life: Some(10.0), min_diffusion_count: Some(1000000), ..R },
    Rule { test: Test::State(2), min_count_per_kg: Some(0.4), population_half_life: Some(12000.0), max_count_per_kg: Some(5000.0), diffusion_scale: Some(0.2), ..R },
    Rule { test: Test::Element("DirtyWater"), population_half_life: Some(-12000.0), over_population_half_life: Some(12000.0), ..R },
    Rule { test: Test::Tag("Edible"), population_half_life: Some(-12000.0), over_population_half_life: Some(INF), ..R },
    Rule { test: Test::Tag("Pickled"), population_half_life: Some(10.0), over_population_half_life: Some(10.0), ..R },
];

/// Klei.AI.SlimeGerms ("SlimeLung").
const SLIME_GERMS: &[Rule] = &[
    Rule { under_population_death_rate: Some(2.6666667), min_count_per_kg: Some(0.4), population_half_life: Some(12000.0), max_count_per_kg: Some(500.0), over_population_half_life: Some(1200.0), min_diffusion_count: Some(1000), diffusion_scale: Some(0.001), min_diffusion_infestation_tick_count: Some(1), ..R },
    Rule { test: Test::State(3), min_count_per_kg: Some(0.4), population_half_life: Some(3000.0), over_population_half_life: Some(1200.0), diffusion_scale: Some(1e-6), min_diffusion_count: Some(1000000), ..R },
    Rule { test: Test::Element("SlimeMold"), under_population_death_rate: Some(0.0), population_half_life: Some(-3000.0), over_population_half_life: Some(3000.0), max_count_per_kg: Some(4500.0), diffusion_scale: Some(0.05), ..R },
    Rule { test: Test::Element("BleachStone"), population_half_life: Some(10.0), over_population_half_life: Some(10.0), min_diffusion_count: Some(100000), diffusion_scale: Some(0.001), ..R },
    Rule { test: Test::State(1), min_count_per_kg: Some(250.0), population_half_life: Some(12000.0), over_population_half_life: Some(1200.0), max_count_per_kg: Some(10000.0), min_diffusion_count: Some(5100), diffusion_scale: Some(0.005), ..R },
    Rule { test: Test::Element("ContaminatedOxygen"), under_population_death_rate: Some(0.0), population_half_life: Some(-300.0), over_population_half_life: Some(1200.0), ..R },
    Rule { test: Test::Element("Oxygen"), population_half_life: Some(1200.0), over_population_half_life: Some(10.0), ..R },
    Rule { test: Test::Element("ChlorineGas"), population_half_life: Some(10.0), over_population_half_life: Some(10.0), min_diffusion_count: Some(100000), diffusion_scale: Some(0.001), ..R },
    Rule { test: Test::State(2), min_count_per_kg: Some(0.4), population_half_life: Some(1200.0), over_population_half_life: Some(300.0), max_count_per_kg: Some(100.0), diffusion_scale: Some(0.01), ..R },
];

/// Klei.AI.PollenGerms ("PollenGerms").
const POLLEN_GERMS: &[Rule] = &[
    Rule { under_population_death_rate: Some(2.0 / 3.0), min_count_per_kg: Some(0.4), population_half_life: Some(3000.0), max_count_per_kg: Some(500.0), over_population_half_life: Some(10.0), min_diffusion_count: Some(3000), diffusion_scale: Some(0.001), min_diffusion_infestation_tick_count: Some(1), ..R },
    Rule { test: Test::State(3), min_count_per_kg: Some(0.4), population_half_life: Some(10.0), over_population_half_life: Some(10.0), diffusion_scale: Some(1e-6), min_diffusion_count: Some(1000000), ..R },
    Rule { test: Test::State(1), min_count_per_kg: Some(500.0), under_population_death_rate: Some(2.6666667), population_half_life: Some(10.0), over_population_half_life: Some(10.0), max_count_per_kg: Some(1000000.0), min_diffusion_count: Some(1000), diffusion_scale: Some(0.015), ..R },
    Rule { test: Test::Element("Oxygen"), population_half_life: Some(200.0), over_population_half_life: Some(10.0), ..R },
    Rule { test: Test::State(2), min_count_per_kg: Some(0.4), population_half_life: Some(10.0), over_population_half_life: Some(10.0), max_count_per_kg: Some(100.0), diffusion_scale: Some(0.01), ..R },
];

const ZOMBIE_SOLID_FUEL: Rule = Rule { under_population_death_rate: Some(0.0), population_half_life: Some(INF), over_population_half_life: Some(3000.0), max_count_per_kg: Some(1000.0), diffusion_scale: Some(0.005), ..R };
const ZOMBIE_GAS: Rule = Rule { under_population_death_rate: Some(0.0), population_half_life: Some(INF), over_population_half_life: Some(6000.0), ..R };
const ZOMBIE_OIL: Rule = Rule { population_half_life: Some(INF), over_population_half_life: Some(6000.0), max_count_per_kg: Some(1000.0), diffusion_scale: Some(0.005), ..R };

/// Klei.AI.ZombieSpores ("ZombieSpores").
const ZOMBIE_SPORES: &[Rule] = &[
    Rule { under_population_death_rate: Some(2.6666667), min_count_per_kg: Some(0.4), population_half_life: Some(12000.0), max_count_per_kg: Some(500.0), over_population_half_life: Some(1200.0), min_diffusion_count: Some(1000), diffusion_scale: Some(0.001), min_diffusion_infestation_tick_count: Some(1), ..R },
    Rule { test: Test::State(3), min_count_per_kg: Some(0.4), population_half_life: Some(3000.0), over_population_half_life: Some(1200.0), diffusion_scale: Some(1e-6), min_diffusion_count: Some(1000000), ..R },
    Rule { test: Test::Element("Carbon"), ..ZOMBIE_SOLID_FUEL },
    Rule { test: Test::Element("Diamond"), ..ZOMBIE_SOLID_FUEL },
    Rule { test: Test::Element("BleachStone"), population_half_life: Some(10.0), over_population_half_life: Some(10.0), min_diffusion_count: Some(100000), diffusion_scale: Some(0.001), ..R },
    Rule { test: Test::State(1), min_count_per_kg: Some(250.0), population_half_life: Some(12000.0), over_population_half_life: Some(1200.0), max_count_per_kg: Some(10000.0), min_diffusion_count: Some(5100), diffusion_scale: Some(0.005), ..R },
    Rule { test: Test::Element("CarbonDioxide"), ..ZOMBIE_GAS },
    Rule { test: Test::Element("Methane"), ..ZOMBIE_GAS },
    Rule { test: Test::Element("SourGas"), ..ZOMBIE_GAS },
    Rule { test: Test::Element("ChlorineGas"), population_half_life: Some(10.0), over_population_half_life: Some(10.0), min_diffusion_count: Some(100000), diffusion_scale: Some(0.001), ..R },
    Rule { test: Test::State(2), min_count_per_kg: Some(0.4), population_half_life: Some(1200.0), over_population_half_life: Some(300.0), max_count_per_kg: Some(100.0), diffusion_scale: Some(0.01), ..R },
    Rule { test: Test::Element("CrudeOil"), ..ZOMBIE_OIL },
    Rule { test: Test::Element("Petroleum"), ..ZOMBIE_OIL },
    Rule { test: Test::Element("Naphtha"), ..ZOMBIE_OIL },
    Rule { test: Test::Element("LiquidMethane"), ..ZOMBIE_OIL },
    Rule { test: Test::Element("Chlorine"), population_half_life: Some(10.0), over_population_half_life: Some(10.0), min_diffusion_count: Some(100000), diffusion_scale: Some(0.001), ..R },
];

/// Klei.AI.RadiationPoisoning ("RadiationSickness"); registered only when
/// radiation is enabled.
const RADIATION_SICKNESS: &[Rule] = &[Rule {
    under_population_death_rate: Some(0.0),
    min_count_per_kg: Some(0.0),
    population_half_life: Some(600.0),
    max_count_per_kg: Some(INF),
    over_population_half_life: Some(600.0),
    min_diffusion_count: Some(10000),
    diffusion_scale: Some(0.0),
    min_diffusion_infestation_tick_count: Some(1),
    ..R
}];

/// In Database.Diseases order.
pub const DISEASES: &[DiseaseDef] = &[
    DiseaseDef { id: "FoodPoisoning", name: "Food Poisoning", strength_sent: 0.0, temperature_range: [248.15, 278.15, 313.15, 348.15], temperature_half_lives: [10.0, 1200.0, 1200.0, 10.0], pressure_range: [0.0, 0.0, 1000.0, 1000.0], pressure_half_lives: IDEMPOTENT, radiation_kill_rate: 2.5, rules: FOOD_GERMS },
    DiseaseDef { id: "SlimeLung", name: "Slimelung", strength_sent: 0.0, temperature_range: [283.15, 293.15, 363.15, 373.15], temperature_half_lives: [10.0, 1200.0, 1200.0, 10.0], pressure_range: [0.0, 0.0, 1000.0, 1000.0], pressure_half_lives: IDEMPOTENT, radiation_kill_rate: 2.5, rules: SLIME_GERMS },
    DiseaseDef { id: "PollenGerms", name: "Floral Scent", strength_sent: 0.0, temperature_range: [263.15, 273.15, 363.15, 373.15], temperature_half_lives: [10.0, 100.0, 100.0, 10.0], pressure_range: [0.0, 0.0, 1000.0, 1000.0], pressure_half_lives: IDEMPOTENT, radiation_kill_rate: 0.0, rules: POLLEN_GERMS },
    DiseaseDef { id: "ZombieSpores", name: "Zombie Spores", strength_sent: 0.0, temperature_range: [168.15, 258.15, 513.15, 563.15], temperature_half_lives: [10.0, 1200.0, 1200.0, 10.0], pressure_range: [0.0, 0.0, 1000.0, 1000.0], pressure_half_lives: IDEMPOTENT, radiation_kill_rate: 1.0, rules: ZOMBIE_SPORES },
    DiseaseDef { id: "RadiationSickness", name: "Radioactive Contaminants", strength_sent: 0.0, temperature_range: IDEMPOTENT, temperature_half_lives: IDEMPOTENT, pressure_range: IDEMPOTENT, pressure_half_lives: IDEMPOTENT, radiation_kill_rate: 0.0, rules: RADIATION_SICKNESS },
];

fn applies(test: Test, e: &Entry) -> bool {
    match test {
        Test::All => true,
        Test::State(s) => e.state & 3 == s,
        Test::Element(id) => e.hash == element_hash(id),
        Test::Tag(t) => e.id == t || e.tags.iter().any(|x| x == t),
    }
}

/// Per-element growth info for one disease over the final element order.
pub fn growth_info(def: &DiseaseDef, elements: &[Entry]) -> Vec<Growth> {
    let mut info = vec![DEFAULT; elements.len()];
    let index = |id: &str| elements.iter().position(|e| e.hash == element_hash(id));
    if let Some(i) = index("Polypropylene") {
        info[i] = Growth {
            under_population_death_rate: 2.6666667,
            population_half_life: 10.0,
            over_population_half_life: 10.0,
            min_count_per_kg: 0.0,
            max_count_per_kg: INF,
            min_diffusion_count: i32::MAX,
            diffusion_scale: 1.0,
            min_diffusion_infestation_tick_count: 255,
        };
    }
    let vacuum = element_hash("Vacuum");
    if let Some(i) = index("Vacuum") {
        info[i] = Growth {
            under_population_death_rate: 0.0,
            population_half_life: 0.0,
            over_population_half_life: 0.0,
            min_count_per_kg: 0.0,
            max_count_per_kg: INF,
            min_diffusion_count: 0,
            diffusion_scale: 0.0,
            min_diffusion_infestation_tick_count: 255,
        };
    }
    for rule in def.rules {
        for (i, e) in elements.iter().enumerate() {
            if e.hash == vacuum || !applies(rule.test, e) {
                continue;
            }
            let g = &mut info[i];
            if let Some(v) = rule.under_population_death_rate {
                g.under_population_death_rate = v;
            }
            if let Some(v) = rule.population_half_life {
                g.population_half_life = v;
            }
            if let Some(v) = rule.over_population_half_life {
                g.over_population_half_life = v;
            }
            if let Some(v) = rule.diffusion_scale {
                g.diffusion_scale = v;
            }
            if let Some(v) = rule.min_count_per_kg {
                g.min_count_per_kg = v;
            }
            if let Some(v) = rule.max_count_per_kg {
                g.max_count_per_kg = v;
            }
            if let Some(v) = rule.min_diffusion_count {
                g.min_diffusion_count = v;
            }
            if let Some(v) = rule.min_diffusion_infestation_tick_count {
                g.min_diffusion_infestation_tick_count = v;
            }
        }
    }
    info
}

/// The diseases message payload (SimMessages.CreateDiseaseTable).
pub fn payload(elements: &[Entry], radiation_enabled: bool) -> Vec<u8> {
    let defs: Vec<&DiseaseDef> = DISEASES.iter().filter(|d| radiation_enabled || d.id != "RadiationSickness").collect();
    let mut out = Vec::new();
    out.extend((defs.len() as u32).to_le_bytes());
    out.extend((elements.len() as u32).to_le_bytes());
    for d in defs {
        out.extend((d.name.len() as i32).to_le_bytes());
        out.extend(d.name.as_bytes());
        out.extend(element_hash(d.id).to_le_bytes());
        out.extend(d.strength_sent.to_le_bytes());
        for r in [d.temperature_range, d.temperature_half_lives, d.pressure_range, d.pressure_half_lives] {
            for v in r {
                out.extend(v.to_le_bytes());
            }
        }
        out.extend(d.radiation_kill_rate.to_le_bytes());
        for g in growth_info(d, elements) {
            for v in [g.under_population_death_rate, g.population_half_life, g.over_population_half_life, g.diffusion_scale, g.min_count_per_kg, g.max_count_per_kg] {
                out.extend(v.to_le_bytes());
            }
            out.extend(g.min_diffusion_count.to_le_bytes());
            out.push(g.min_diffusion_infestation_tick_count);
        }
    }
    out
}
