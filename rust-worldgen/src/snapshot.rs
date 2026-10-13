//! Pre-parsed game data: a loaded [`DataSource`] serialized as MessagePack,
//! so the WASM build can start without parsing several MB of YAML.
//! `tools/yaml_to_json` writes it next to the JSON bundle
//! (`data/worldgen-snapshot.bin`), built from the bundle's own files, and
//! [`decode`] restores the same `DataSource` that `DataSource::from_files`
//! builds from them. MessagePack is self-describing, which the embedded
//! `serde_yaml::Value` fields need.
//!
//! Layout: `ONIS`, u32 little-endian format version, then the MessagePack
//! encoding of `{game_build, dlc2_installed, data}` (structs as arrays).
use serde::{Deserialize, Serialize};

use crate::data_source::DataSource;

const MAGIC: &[u8; 4] = b"ONIS";
const VERSION: u32 = 2;

#[derive(Serialize)]
struct SnapshotRef<'a> { game_build: Option<&'a str>, dlc2_installed: bool, data: &'a DataSource }

#[derive(Deserialize)]
pub struct Snapshot {
    pub game_build: Option<String>,
    /// The bundle holds `dlc/dlc2/` files (the CLI's `assets/dlc/dlc2` test).
    pub dlc2_installed: bool,
    pub data: DataSource,
}

pub fn encode(data: &DataSource, game_build: Option<&str>, dlc2_installed: bool) -> Result<Vec<u8>, String> {
    let mut out = MAGIC.to_vec();
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.extend(rmp_serde::to_vec(&SnapshotRef { game_build, dlc2_installed, data }).map_err(|e| format!("snapshot encode: {e}"))?);
    Ok(out)
}

pub fn decode(bytes: &[u8]) -> Result<Snapshot, String> {
    if bytes.len() < 8 || &bytes[..4] != MAGIC { return Err("not a worldgen data snapshot".into()); }
    if u32::from_le_bytes(bytes[4..8].try_into().unwrap()) != VERSION { return Err("unsupported worldgen data snapshot version".into()); }
    rmp_serde::from_slice(&bytes[8..]).map_err(|e| format!("snapshot decode: {e}"))
}
