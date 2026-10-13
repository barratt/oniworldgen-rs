//! WASM exports: `generate`, `version` and `warmUp`. The JavaScript wrapper
//! (`npm/index.js`) turns thrown errors into `{error}` results.

use std::cell::OnceCell;

use wasm_bindgen::prelude::*;

use crate::{Engine, Entities, Options};

/// The game data, written by `worldgen-snapshot` from the game's
/// StreamingAssets at build time (scripts/build-npm.sh sets
/// ONIWORLDGEN_SNAPSHOT to its path). Not part of the repository.
const SNAPSHOT: &[u8] = include_bytes!(env!("ONIWORLDGEN_SNAPSHOT"));

#[cfg(not(target_feature = "atomics"))]
#[global_allocator]
static TALC: talc::cell::TalcSyncCell<ChunkedGrowth, talc::wasm::WasmBinning> = talc::cell::TalcSyncCell::new_wasm(ChunkedGrowth { end: None });

/// talc's `WasmGrowAndExtend`, growing memory by at least 1 MiB (or an
/// eighth of the current memory) at a time: growing by the page for each
/// allocation that does not fit makes thousands of `memory.grow` calls
/// while the data snapshot and a cluster are built.
#[derive(Debug)]
struct ChunkedGrowth { end: Option<std::ptr::NonNull<u8>> }

// SAFETY: as `WasmGrowAndExtend`: no allocation, and memory past the
// previous end is only handed to talc once grown.
unsafe impl talc::source::Source for ChunkedGrowth {
    fn acquire<B: talc::base::binning::Binning>(talc: &mut talc::base::Talc<Self, B>, layout: std::alloc::Layout) -> Result<(), ()> {
        const PAGE: usize = 65536;
        let needed = (layout.size() + layout.align() + 4 * PAGE).div_ceil(PAGE);
        let current = core::arch::wasm32::memory_size::<0>();
        let delta = needed.max(16).max(current / 8);
        let previous = match core::arch::wasm32::memory_grow::<0>(delta) {
            usize::MAX => match core::arch::wasm32::memory_grow::<0>(needed) { usize::MAX => return Err(()), p => p },
            p => p,
        };
        let grown = if core::arch::wasm32::memory_size::<0>() - previous == delta { delta } else { needed };
        let base = (previous * PAGE) as *mut u8;
        let end = base.wrapping_add(grown * PAGE);
        if let Some(old_end) = talc.source.end.take() {
            if old_end.as_ptr() == base {
                talc.source.end = Some(unsafe { talc.extend(old_end, end) });
                return Ok(());
            }
        }
        talc.source.end = unsafe { talc.claim(base, grown * PAGE) };
        Ok(())
    }
}

thread_local! {
    // A OnceCell holds no borrow flag, so a call that traps (a panic aborts in
    // WASM) leaves the engine usable for the next call.
    static ENGINE: OnceCell<Result<Engine, String>> = const { OnceCell::new() };
}

fn with_engine<T>(f: impl FnOnce(&Engine) -> Result<T, JsValue>) -> Result<T, JsValue> {
    ENGINE.with(|cell| match cell.get_or_init(|| Engine::from_snapshot(SNAPSHOT)) {
        Ok(e) => f(e),
        Err(e) => Err(JsValue::from_str(e)),
    })
}

fn options(opts: &JsValue) -> Result<Options, JsValue> {
    let mut o = Options::default();
    if opts.is_undefined() || opts.is_null() { return Ok(o); }
    let get = |k: &str| js_sys::Reflect::get(opts, &JsValue::from_str(k));
    let entities = get("entities")?;
    if let Some(s) = entities.as_string() {
        o.entities = match s.as_str() {
            "all" => Entities::All,
            "map" => Entities::Map,
            other => return Err(JsValue::from_str(&format!("entities must be \"all\" or \"map\", not {other}"))),
        };
    } else if !entities.is_undefined() {
        return Err(JsValue::from_str("entities must be \"all\" or \"map\""));
    }
    o.accept_rewritten = get("acceptRewritten")?.as_bool().unwrap_or(false);
    let ambient_oil = get("ambientOil")?;
    if let Some(b) = ambient_oil.as_bool() { o.ambient_oil = b; } else if !ambient_oil.is_undefined() {
        return Err(JsValue::from_str("ambientOil must be true or false"));
    }
    Ok(o)
}

fn run(coordinate: &str, opts: &JsValue) -> Result<String, JsValue> {
    let o = options(opts)?;
    with_engine(|e| crate::generate(e, coordinate, o).map(|v| v.to_string()).map_err(|r| JsValue::from_str(&r.0)))
}

/// The document as a JavaScript object. Throws the error text for
/// coordinates that cannot be generated; a worldgen failure is a document
/// with `failure`.
#[wasm_bindgen]
pub fn generate(coordinate: &str, opts: JsValue) -> Result<JsValue, JsValue> {
    js_sys::JSON::parse(&run(coordinate, &opts)?)
}

/// `"744825+<package version>"`.
#[wasm_bindgen]
pub fn version() -> String { crate::version() }

/// Restore the embedded data now instead of on the first `generate`.
#[wasm_bindgen(js_name = warmUp)]
pub fn warm_up() -> Result<(), JsValue> { with_engine(|_| Ok(())) }
