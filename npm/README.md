# oniworldgen

Oxygen Not Included world generation for game build 744825, as WebAssembly.
Give it a world coordinate and it returns the cluster: asteroids, biome polygons,
geysers, points of interest, world traits and the starmap.

Supports the base game, Spaced Out!, Frosty Planet, Bionic Booster, Prehistoric
Planet and Aquatic Planet, including world and subworld mixing.

## Install

```sh
npm install oniworldgen
```

## Use

```js
import init, { worldgen } from 'oniworldgen';

await init();

const map = worldgen.generate('SNDST-C-44-0-0-0');
if (map.error) {
  // the coordinate is invalid or cannot be generated
} else if (map.failure) {
  // the game's own world generation fails for this coordinate
} else {
  for (const world of map.worlds) console.log(world.name, world.geysers.length);
}
```

In Node, pass the module bytes to `init`:

```js
import fs from 'node:fs';
import { createRequire } from 'node:module';
const require = createRequire(import.meta.url);
await init({ module_or_path: fs.readFileSync(require.resolve('oniworldgen/worldgen_bg.wasm')) });
```

`generate` is synchronous, so run it in a Web Worker in browsers.

## API

- `init(options?)` loads the module. Call it once before anything else.
  - `options.module_or_path`: a URL, `Response`, bytes or compiled module for `worldgen_bg.wasm`.
    Omit it in browsers and bundlers (it resolves next to `index.js`).
- `worldgen.version()` returns `"<game build>+<package version>"`, e.g. `"744825+2.1.0"`.
- `worldgen.generate(coordinate, options?)` returns the cluster, or `{error}`.
  - `options.entities`: `"all"` (default) or `"map"` (points of interest, oil reservoirs and vents only).
  - `options.acceptRewritten`: generate the coordinate the game substitutes when it rewrites one
    (for example fixed-seed clusters).
  - `options.ambientOil`: `true` (default) or `false` (leave out ambient oil reservoirs).

The result's fields are typed in `index.d.ts`.

Ambient critters and plants are not included: the game places them only after its settle simulation.
Oil reservoirs are included; ambient ones (after the template-placed ones in `other_entities`) are
placed without that simulation, so their positions are approximate and, rarely, their count is off by one.

## Coordinates

A coordinate is `PREFIX-SEED-SETTINGS-STORY-MIXING`, as shown in the game's new-game screen,
e.g. `SNDST-A-1-0-0-0` (base game Terra) or `AQU-C-4-0-0-0` (Aquatic Planet).

## Source

Built from [barratt/oniworldgen-rs](https://github.com/barratt/oniworldgen-rs), which also provides
command-line tools that generate full cell grids and settle the worlds.

## License

AGPL-3.0-only; see `LICENSE`.

The embedded worldgen data is derived from Oxygen Not Included; see `NOTICE`.
Not affiliated with Klei Entertainment. Oxygen Not Included is a trademark of Klei Entertainment.
