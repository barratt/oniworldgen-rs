//! Write the game data snapshot the WASM build embeds: the `DataSource` the
//! game's worldgen files load to, pre-parsed and serialized
//! (`oni_worldgen::snapshot`), so the module starts without parsing YAML.
//!
//! The file set is what the native loader reads from StreamingAssets
//! (`DataSource::bundle_files`: elements, worldgen trees, story traits,
//! mixing, templates, ...), so the snapshot holds the same data the CLI
//! loads with `--assets`. It is game data: build it from your own install
//! and do not commit it.
//!
//! Usage: worldgen-snapshot --assets STREAMING_ASSETS --output FILE [--game-build BUILD]
//! (`--assets` defaults to the ONI_STREAMING_ASSETS environment variable.)

use std::path::PathBuf;

use oni_worldgen::data_source::DataSource;

const USAGE: &str = "usage: worldgen-snapshot --assets STREAMING_ASSETS --output FILE [--game-build BUILD]";

fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => { eprintln!("worldgen-snapshot: {error}"); std::process::ExitCode::FAILURE }
    }
}

fn run() -> Result<(), String> {
    let (mut assets, mut output, mut game_build) = (std::env::var("ONI_STREAMING_ASSETS").ok(), None, None);
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or_else(|| format!("missing value for {arg}"));
        match arg.as_str() {
            "--assets" => assets = Some(value()?),
            "--output" => output = Some(value()?),
            "--game-build" => game_build = Some(value()?),
            "-h" | "--help" => { println!("{USAGE}"); return Ok(()); }
            other => return Err(format!("unknown argument {other}\n{USAGE}")),
        }
    }
    let assets = PathBuf::from(assets.ok_or(USAGE)?);
    let output = PathBuf::from(output.ok_or(USAGE)?);
    if !assets.join("worldgen/clusters").is_dir() {
        return Err(format!("{} is not OxygenNotIncluded_Data/StreamingAssets", assets.display()));
    }
    let files = DataSource::bundle_files(&assets)?;
    let yaml: usize = files.values().map(String::len).sum();
    let data = DataSource::from_files(&files)?;
    let dlc2_installed = files.keys().any(|k| k.starts_with("dlc/dlc2/"));
    let snapshot = oni_worldgen::snapshot::encode(&data, game_build.as_deref(), dlc2_installed)?;
    if let Some(parent) = output.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    std::fs::write(&output, &snapshot).map_err(|e| format!("{}: {e}", output.display()))?;
    eprintln!("wrote {} ({} files, {:.2} MB of YAML, {:.2} MB snapshot)", output.display(), files.len(), yaml as f64 / 1e6, snapshot.len() as f64 / 1e6);
    Ok(())
}
