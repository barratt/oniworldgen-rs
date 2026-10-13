//! `worldgen` with `--settle`: generate a cluster exactly like the worldgen
//! CLI and, with `--settle`, settle every world with the Rust simulation port
//! and place ambient mobs, adding a `simulation` section to the document.
use oni_worldgen::{coord::CoordinateCheck, data_source::DataSource, dlc::ActiveDlc, pipeline::{run_cluster, validate}};
use serde_json::json;
use std::{collections::BTreeMap, path::PathBuf, time::Instant};

fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let mut options = BTreeMap::new();
    let mut settle = false;
    while let Some(key) = args.next() {
        if key == "--help" {
            println!("worldgen-settle --assets STREAMING_ASSETS --coordinate FULL_COORD --output FILE [--dlc IDS|none] [--game-build BUILD] [--accept-rewritten]");
            println!("                [--settle [--frames 499|500]]");
            println!("  --settle  settle each world with the Rust simulation port and place ambient mobs after --frames");
            println!("            observation frames (default 499); requires --game-build 744825");
            return Ok(());
        }
        if key == "--settle" {
            settle = true;
            continue;
        }
        if key == "--accept-rewritten" {
            options.insert(key.clone(), String::new());
            continue;
        }
        if !["--assets", "--coordinate", "--output", "--dlc", "--game-build", "--frames"].contains(&key.as_str()) {
            return Err(format!("unknown option {key}; use --help"));
        }
        let value = args.next().ok_or_else(|| format!("missing value for {key}"))?;
        if options.insert(key.clone(), value).is_some() {
            return Err(format!("duplicate option {key}"));
        }
    }
    let required = |key: &str| options.get(key).ok_or_else(|| format!("{key} is required"));
    let assets = PathBuf::from(required("--assets")?);
    let requested = required("--coordinate")?;
    let output = PathBuf::from(required("--output")?);
    if output.exists() {
        return Err(format!("output already exists: {}", output.display()));
    }
    let explicit_dlc = options.get("--dlc").map(|s| ActiveDlc::parse(s)).transpose()?;
    let frames: i32 = options.get("--frames").map(String::as_str).unwrap_or("499").parse().map_err(|_| "--frames must be 499 or 500")?;
    let game_version = options.get("--game-build").map(String::as_str).unwrap_or("unknown");
    if settle && game_version != settle::GAME_VERSION {
        return Err(format!("--settle requires --game-build {}", settle::GAME_VERSION));
    }
    let start = Instant::now();
    let ds = DataSource::load(&assets)?;
    let load_ms = start.elapsed().as_secs_f64() * 1000.0;
    let checked = validate(requested, &ds, explicit_dlc, assets.join("dlc/dlc2").is_dir())?;
    let coordinate = checked.coordinate(options.contains_key("--accept-rewritten"), "--accept-rewritten")?.to_string();
    if let CoordinateCheck::Rewritten { to, reason } = &checked.check {
        eprintln!("note: the game uses {to} ({reason}); generating that");
    }
    let started = Instant::now();
    let run = run_cluster(&coordinate, &ds, explicit_dlc)?;
    let generate_ms = started.elapsed().as_secs_f64() * 1000.0;
    let worlds = run.cluster.worlds.len();
    let scope = run.scope;
    let prediction = if settle { Some(oni_worldgen::simulate::prediction(&run, game_version)) } else { None };
    let mut result = run.into_document("cli")?;
    result.set("coordinateCheck", &checked.to_json())?;
    result.set("gameVersion", game_version)?;
    result.set("provenance", &json!({"assetsDirectory": assets, "buildSource": "caller-declared target; local game YAML used", "dataLoadMs": load_ms, "generationTimesMs": [generate_ms]}))?;
    if let Some(prediction) = prediction {
        let t = Instant::now();
        let yaml = settle::element_yaml_from_assets(&assets)?;
        let refs: Vec<&str> = yaml.iter().map(String::as_str).collect();
        let tables = settle::Tables::new(&refs, &scope)?;
        let inputs = settle::world_inputs(&prediction, &tables)?;
        let transport_ms = t.elapsed().as_secs_f64() * 1000.0;
        let t = Instant::now();
        let settled = settle::settle(inputs, frames)?;
        let settle_ms = t.elapsed().as_secs_f64() * 1000.0;
        let t = Instant::now();
        let ambient = settle::ambient(&ds, &coordinate, explicit_dlc, &prediction, &settled, &tables)?;
        let ambient_ms = t.elapsed().as_secs_f64() * 1000.0;
        let section = settle::simulation_section(&coordinate, frames, &settled, &ambient, &tables,
            json!({"transport": transport_ms, "settle": settle_ms, "ambient": ambient_ms}));
        result.set("simulation", &section)?;
        let mut coverage: serde_json::Value = serde_json::from_slice(result.get("coverage").ok_or("missing coverage")?).map_err(|e| e.to_string())?;
        coverage["settledPhysics"] = json!(true);
        coverage["ambientMobs"] = json!(true);
        result.set("coverage", &coverage)?;
    }
    let text = result.finish();
    if let Some(parent) = output.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new().create_new(true).write(true).open(&output).map_err(|e| e.to_string())?;
    file.write_all(&text).map_err(|e| e.to_string())?;
    eprintln!("{worlds} worlds; load {load_ms:.1} ms; generate {generate_ms:.1} ms; total {:.1} ms; output {}", start.elapsed().as_secs_f64() * 1000.0, output.display());
    Ok(())
}

fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("worldgen: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}
