//! Native driver and benchmark. No WASM engine or running game is involved.
use oni_worldgen::{compact::{self, Tier}, coord::CoordinateCheck, data_source::{DataSource, Layer, Overlay}, diagnostics::RunError, dlc::ActiveDlc,
    generate::WorldSelector, pipeline::{run_cluster, run_partial, validate}};
use serde_json::json;
use std::{collections::BTreeMap, path::{Path, PathBuf}, time::Instant};

/// `--failure-json`: the run's failure (or null) and warnings.
fn write_diagnostics(path: Option<&Path>, diagnostics: &serde_json::Value) -> Result<(), String> {
    let Some(path) = path else { return Ok(()) };
    std::fs::write(path, serde_json::to_string_pretty(diagnostics).map_err(|e| e.to_string())?).map_err(|e| format!("{}: {e}", path.display()))
}

fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let mut options = BTreeMap::new();
    let (mut overlays, mut mods): (Vec<PathBuf>, Vec<PathBuf>) = (Vec::new(), Vec::new());
    let mut tier = None;
    while let Some(key) = args.next() {
        if key == "--help" {
            println!("worldgen --assets STREAMING_ASSETS --coordinate FULL_COORD --output FILE [--dlc IDS|none] [--game-build BUILD] [--repeat COUNT] [--accept-rewritten]");
            println!("         [--summary|--compact [--ambient-oil]]");
            println!("  --dlc  active DLC set, comma-separated (EXPANSION1_ID,DLC2_ID,DLC3_ID,DLC4_ID,DLC5_ID) or none for the");
            println!("         base game; omitted: inferred from the cluster definition and the DLC packs the coordinate's");
            println!("         settings and mixing need");
            println!("  --accept-rewritten  when the game would replace the coordinate (e.g. a fixed-seed cluster),");
            println!("         generate the coordinate the game uses instead of failing; without --dlc the check");
            println!("         assumes DLC2 is active if installed and allowed by the cluster");
            println!("         [--worlds SELECTORS [--partial-mode templates|notemplates]] [--overlay DIR]... [--mod DIR]... [--failure-json FILE]");
            println!("  --worlds  generate only these worlds: comma-separated placement indices or world paths (full, or the");
            println!("         last segment, ignoring case, after world mixing); story traits and the starmap are skipped");
            println!("  --partial-mode  notemplates also skips template placement (default templates)");
            println!("  --overlay  a directory laid out like StreamingAssets whose files replace or add to the game's own, as");
            println!("         editing StreamingAssets would; repeat for several (later wins)");
            println!("  --mod  a Workshop-style mod directory (mod_info.yaml, archived_versions), layered over the game data as the");
            println!("         game loads mod content; repeat for several, in load order");
            println!("  --failure-json  write {{\"failure\": ..., \"telemetry\": [...]}} for the run, failed or not");
            println!("  --summary  write the search summary (JSON, no cell grids) instead of the full document");
            println!("  --compact  write the compact container (summary JSON plus deflated cell grids; see compact.rs)");
            println!("         both work with --worlds, --overlay, --mod and --failure-json");
            println!("  --ambient-oil  with --summary or --compact: add each world's estimated ambient OilWells, placed on the");
            println!("         unsettled grid (ambient_oil.rs), as ambient_oil_wells; positions are approximate");
            println!("  To settle the worlds and place ambient mobs, use worldgen-settle --settle.");
            return Ok(());
        }
        if key == "--overlay" || key == "--mod" {
            let dir = PathBuf::from(args.next().ok_or_else(|| format!("missing value for {key}"))?);
            if !dir.is_dir() { return Err(format!("{key} {} is not a directory", dir.display())); }
            if key == "--mod" { mods.push(dir) } else { overlays.push(dir) }
            continue;
        }
        if key == "--summary" || key == "--compact" {
            if tier.replace(if key == "--summary" { Tier::Summary } else { Tier::Compact }).is_some() { return Err("use one of --summary and --compact".into()); }
            continue;
        }
        if key == "--accept-rewritten" || key == "--ambient-oil" {
            if options.insert(key.clone(), String::new()).is_some() { return Err(format!("duplicate option {key}")); }
            continue;
        }
        if !["--assets", "--coordinate", "--output", "--dlc", "--game-build", "--repeat",
            "--worlds", "--partial-mode", "--failure-json"].contains(&key.as_str()) {
            return Err(format!("unknown option {key}; use --help"));
        }
        let value = args.next().ok_or_else(|| format!("missing value for {key}"))?;
        if options.insert(key.clone(), value).is_some() { return Err(format!("duplicate option {key}")); }
    }
    let required = |key: &str| options.get(key).ok_or_else(|| format!("{key} is required"));
    let assets = PathBuf::from(required("--assets")?);
    let requested = required("--coordinate")?;
    let output = PathBuf::from(required("--output")?);
    if !assets.join("worldgen/clusters").is_dir() { return Err("--assets must point to OxygenNotIncluded_Data/StreamingAssets".into()); }
    let explicit_dlc = options.get("--dlc").map(|s| ActiveDlc::parse(s)).transpose()?;
    let content: Vec<(&str, String)> = std::iter::once(("EXPANSION1_ID", "expansion1")).chain(oni_worldgen::dlc::CONTENT_DLCS)
        .map(|(id, dir)| (id, format!("dlc/{dir}"))).collect();
    for (id, dir) in &content {
        if explicit_dlc.is_some_and(|d| d.is_active(id)) && !assets.join(dir).is_dir() {
            return Err(format!("--dlc includes {id} but {} is missing", assets.join(dir).display()));
        }
    }
    if output.exists() { return Err(format!("output already exists: {}", output.display())); }
    let repeats: usize = options.get("--repeat").map(String::as_str).unwrap_or("1").parse().map_err(|_| "invalid repeat count")?;
    if repeats == 0 || repeats > 10000 { return Err("repeat must be 1..10000".into()); }
    let game_version = options.get("--game-build").map(String::as_str).unwrap_or("unknown");
    let selectors: Option<Vec<WorldSelector>> = options.get("--worlds").map(|s| s.split(',').filter(|s| !s.trim().is_empty()).map(WorldSelector::parse).collect());
    if selectors.as_ref().is_some_and(|s| s.is_empty()) { return Err("--worlds needs at least one world".into()); }
    let templates = match options.get("--partial-mode").map(String::as_str) {
        None | Some("templates") => true,
        Some("notemplates") => false,
        Some(other) => return Err(format!("--partial-mode must be templates or notemplates, not {other}")),
    };
    if options.contains_key("--partial-mode") && selectors.is_none() { return Err("--partial-mode requires --worlds".into()); }
    let failure_json = options.get("--failure-json").map(PathBuf::from);
    let start = Instant::now();
    let ds = if overlays.is_empty() && mods.is_empty() { DataSource::load(&assets)? } else {
        // Mods load for the DLC the game runs with (--dlc), else what is installed.
        let installed: Vec<&str> = match explicit_dlc {
            Some(dlc) => content.iter().map(|(id, _)| *id).filter(|id| dlc.is_active(id)).collect(),
            None => content.iter().filter(|(_, d)| assets.join(d).is_dir()).map(|(id, _)| *id).collect(),
        };
        let build: u32 = game_version.parse().unwrap_or(744825);
        let mut overlay = Overlay { mods: mods.iter().map(|dir| Layer::from_dir(dir, &installed, build)).collect::<Result<Vec<_>, _>>()?, ..Overlay::default() };
        for dir in &overlays { overlay.edits.extend(Layer::from_dir(dir, &installed, build)?.files); }
        DataSource::load_with(&assets, &overlay)?
    };
    let load_ms = start.elapsed().as_secs_f64() * 1000.0;
    let checked = validate(requested, &ds, explicit_dlc, assets.join("dlc/dlc2").is_dir())?;
    let coordinate = checked.coordinate(options.contains_key("--accept-rewritten"), "--accept-rewritten")?;
    if let CoordinateCheck::Rewritten { to, reason } = &checked.check { eprintln!("note: the game uses {to} ({reason}); generating that"); }
    let ambient_oil = options.contains_key("--ambient-oil");
    if ambient_oil && tier.is_none() { return Err("--ambient-oil requires --summary or --compact".into()); }
    if let Some(tier) = tier {
        let overlay = (!overlays.is_empty() || !mods.is_empty()).then(|| json!({"overlays": overlays, "mods": mods, "checksum": ds.checksum}));
        let partial = selectors.as_deref().map(|s| (s, templates));
        return write_tier(TierJob { tier, coordinate, ds: &ds, explicit_dlc, partial, repeats, checked: &checked, game_version, assets: &assets,
            load_ms, overlay, output: &output, failure_json: failure_json.as_deref(), ambient_oil });
    }
    let mut elapsed = Vec::new();
    let (mut first, mut worlds) = (None, 0);
    for _ in 0..repeats {
        let start = Instant::now();
        let failed = |e: RunError| -> Result<String, String> { write_diagnostics(failure_json.as_deref(), &e.to_json())?; Ok(e.text) };
        let doc = if let Some(selectors) = &selectors {
            let run = match run_partial(coordinate, &ds, explicit_dlc, selectors, templates) { Ok(run) => run, Err(e) => return Err(failed(e)?) };
            elapsed.push(start.elapsed().as_secs_f64() * 1000.0);
            worlds = run.cluster.worlds.len();
            write_diagnostics(failure_json.as_deref(), &json!({"failure": null, "telemetry": run.telemetry}))?;
            run.into_document("cli")?
        } else {
            let run = match run_cluster(coordinate, &ds, explicit_dlc) { Ok(run) => run, Err(e) => return Err(failed(e)?) };
            elapsed.push(start.elapsed().as_secs_f64() * 1000.0);
            worlds = run.cluster.worlds.len();
            write_diagnostics(failure_json.as_deref(), &json!({"failure": null, "telemetry": run.telemetry}))?;
            run.into_document("cli")?
        };
        if let Some(ref reference) = first {
            if reference != &doc { return Err("nondeterministic output across repeated calls".into()); }
        } else { first = Some(doc); }
    }
    let mut result = first.unwrap();
    result.set("coordinateCheck", &checked.to_json())?;
    result.set("gameVersion", game_version)?;
    let mut provenance = json!({"assetsDirectory": assets, "buildSource": "caller-declared target; local game YAML used", "dataLoadMs": load_ms, "generationTimesMs": elapsed});
    if !overlays.is_empty() || !mods.is_empty() { provenance["overlay"] = json!({"overlays": overlays, "mods": mods, "checksum": ds.checksum}); }
    result.set("provenance", &provenance)?;
    let text = result.finish();
    if let Some(parent) = output.parent().filter(|p| !p.as_os_str().is_empty()) { std::fs::create_dir_all(parent).map_err(|e| e.to_string())?; }
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new().create_new(true).write(true).open(&output).map_err(|e| e.to_string())?;
    file.write_all(&text).map_err(|e| e.to_string())?;
    eprintln!("{} worlds; load {:.1} ms; generate {:?} ms; output {}", worlds, load_ms, elapsed, output.display());
    Ok(())
}

/// What `--summary` / `--compact` writes and from what.
struct TierJob<'a> {
    tier: Tier,
    coordinate: &'a str,
    ds: &'a DataSource,
    explicit_dlc: Option<ActiveDlc>,
    partial: Option<(&'a [WorldSelector], bool)>,
    repeats: usize,
    checked: &'a oni_worldgen::pipeline::CheckedCoordinate,
    game_version: &'a str,
    assets: &'a Path,
    load_ms: f64,
    /// `provenance.overlay`, when overlays or mods are loaded.
    overlay: Option<serde_json::Value>,
    output: &'a Path,
    failure_json: Option<&'a Path>,
    /// `--ambient-oil`: estimate ambient OilWells (`TierRun::estimate_ambient_oil`).
    ambient_oil: bool,
}

/// `--summary` / `--compact`: the same coordinate handling, failures and
/// `--failure-json` as the full dump, with the tier document instead.
fn write_tier(job: TierJob) -> Result<(), String> {
    let TierJob { tier, coordinate, ds, explicit_dlc, partial, repeats, checked, game_version, assets, load_ms, overlay, output, failure_json, ambient_oil } = job;
    let mut elapsed = Vec::new();
    let mut run = None;
    for _ in 0..repeats {
        let start = Instant::now();
        let mut this = match compact::run_selected(coordinate, ds, explicit_dlc, tier, partial) {
            Ok(run) => run,
            Err(e) => { write_diagnostics(failure_json, &e.to_json())?; return Err(e.text); }
        };
        if ambient_oil { this.estimate_ambient_oil(ds); }
        run = Some(this);
        elapsed.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    let run = run.unwrap();
    write_diagnostics(failure_json, &json!({"failure": null, "telemetry": run.telemetry}))?;
    let mut provenance = json!({"assetsDirectory": assets, "buildSource": "caller-declared target; local game YAML used", "dataLoadMs": load_ms, "generationTimesMs": elapsed});
    if let Some(overlay) = overlay { provenance["overlay"] = overlay; }
    let extra = [("coordinateCheck", checked.to_json()), ("gameVersion", json!(game_version)), ("provenance", provenance)];
    let bytes = match tier {
        Tier::Summary => {
            let mut doc = run.summary(ds);
            for (k, v) in extra { doc[k] = v; }
            serde_json::to_vec(&doc).map_err(|e| e.to_string())?
        }
        Tier::Compact => compact::encode(&run, ds, &extra)?,
    };
    if let Some(parent) = output.parent().filter(|p| !p.as_os_str().is_empty()) { std::fs::create_dir_all(parent).map_err(|e| e.to_string())?; }
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new().create_new(true).write(true).open(output).map_err(|e| e.to_string())?;
    file.write_all(&bytes).map_err(|e| e.to_string())?;
    eprintln!("{} worlds; load {:.1} ms; generate {:?} ms; output {}", run.worlds.len(), load_ms, elapsed, output.display());
    Ok(())
}

fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => { eprintln!("worldgen: {error}"); std::process::ExitCode::FAILURE }
    }
}
