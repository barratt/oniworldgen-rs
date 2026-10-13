// Oxygen Not Included worldgen (game build 744825), generation only.
//
// `await init()`, then `worldgen.generate(coordinate)` returns the cluster,
// `{error}` when the coordinate cannot be generated, or a document whose
// `failure` is set when the game's worldgen fails for that coordinate.

import initWasm, * as wasm from './worldgen.js';

let ready = null;

/**
 * Load the WASM module. Memoized; extra calls return the same promise.
 * @param {{module_or_path?: any}} [opts] Custom URL, Response, bytes or
 *   compiled module for `worldgen_bg.wasm`. Omit in browsers and
 *   bundlers (it resolves next to this file); in Node pass the bytes.
 */
export default function init(opts) {
  if (!ready) {
    ready = initWasm(opts?.module_or_path !== undefined ? { module_or_path: opts.module_or_path } : undefined)
      .then(() => { wasm.warmUp(); });
  }
  return ready;
}

/** `true` once `init()` has resolved. */
let loaded = false;
const guard = (name) => {
  if (!loaded) {
    try { wasm.version(); loaded = true; } catch {
      throw new Error(`worldgen.${name}: call \`await init()\` first`);
    }
  }
};

// Errors from the generator are strings; a WebAssembly.RuntimeError is an
// internal fault in that one call (the module stays usable).
const errorText = (e) => (typeof e === 'string' ? e : `internal error: ${e?.message ?? String(e)}`);

export const worldgen = {
  /** `"744825+<package version>"`: the game build, then the package. */
  version() { guard('version'); return wasm.version(); },

  /**
   * Generate a cluster. `options.entities` is `"all"` (default: every
   * template-placed building and entity) or `"map"` (only points of
   * interest, OilWell and UnderwaterVent); `options.acceptRewritten`
   * generates the coordinate the game substitutes for one it rewrites;
   * `options.ambientOil: false` leaves out ambient oil reservoirs.
   * @returns {object} MapData, or `{error}`.
   */
  generate(coordinate, options) {
    guard('generate');
    try { return wasm.generate(coordinate, options); } catch (e) { return { error: errorText(e) }; }
  },
};
