# oniworldgen-rs

Oxygen Not Included world generation in Rust. Given a world coordinate (seed),
it generates the cluster the game would generate for game build 744825:
worlds, cell grids, templates, geysers, story traits and the starmap. A
pure-Rust port of the game's cell simulation can then settle the generated
worlds and place ambient creatures and plants.

Supported content: the base game, Spaced Out!, Frosty Planet, Bionic Booster,
Prehistoric Planet and Aquatic Planet, including world and subworld mixing.

## Crates

- `rust-worldgen`: the generator library (`oni_worldgen`) and the `worldgen` CLI.
- `simcore`: the cell simulation used to settle worlds.
- `settle`: settles generated worlds with `simcore`, places ambient mobs, and
  provides the `worldgen-settle` CLI.
- `wasm`: the WebAssembly build published as the [`oniworldgen`](https://www.npmjs.com/package/oniworldgen)
  npm package (see below); its JavaScript wrapper and package files are in `npm/`.
- `tools/snapshot`: `worldgen-snapshot`, which writes the pre-parsed game data the
  WebAssembly build embeds.

## Requirements

- Rust stable.
- An installed copy of Oxygen Not Included (build 744825). The game data is not
  included; pass its `StreamingAssets` directory with `--assets`, e.g.
  `.../steamapps/common/OxygenNotIncluded/OxygenNotIncluded_Data/StreamingAssets`.
  DLC content is read from `StreamingAssets/dlc/`, so the DLC you want to
  generate must be installed.

## Build

```sh
cargo build --release
```

The binaries are `target/release/worldgen` and `target/release/worldgen-settle`.

## Usage

```sh
ASSETS="$HOME/.steam/steam/steamapps/common/OxygenNotIncluded/OxygenNotIncluded_Data/StreamingAssets"

# Full document: every world with its cell grids (large JSON).
worldgen --assets "$ASSETS" --coordinate SNDST-C-43-0-0-0 --game-build 744825 --output sndst.json

# Summary: worlds, geysers, traits and starmap as JSON, without cell grids.
worldgen --assets "$ASSETS" --coordinate AQU-C-4-0-0-0 --game-build 744825 --summary --output aqu-summary.json

# Compact: the summary plus deflated cell grids in one binary container.
worldgen --assets "$ASSETS" --coordinate SNDST-C-43-0-0-0 --game-build 744825 --compact --output sndst.onic

# Generate and settle every world, then place ambient mobs (adds a `simulation` section).
worldgen-settle --assets "$ASSETS" --coordinate SNDST-C-43-0-0-0 --game-build 744825 --settle --output sndst-settled.json
```

`--output` must not already exist. Other useful options (see `--help`):

- `--dlc IDS|none`: the active DLC set (`EXPANSION1_ID,DLC2_ID,DLC3_ID,DLC4_ID,DLC5_ID`),
  otherwise inferred from the cluster and the coordinate's settings.
- `--accept-rewritten`: generate the coordinate the game would actually use when
  it rewrites the requested one (e.g. fixed-seed clusters).
- `--worlds`: generate only some worlds of the cluster.
- `--ambient-oil`: with `--summary` or `--compact`, add an estimate of each world's ambient oil wells.
- `--overlay DIR`, `--mod DIR`: layer modified or modded worldgen files over the game data.
- `--frames 499|500` (`worldgen-settle`): the observation frame for ambient placement.

## Coordinates

A coordinate is `PREFIX-SEED-SETTINGS-STORY-MIXING`, as shown in the game's
new-game screen:

- `PREFIX`: the cluster, e.g. `SNDST-A` (base game Terra), `SNDST-C` (Spaced Out
  Terra), `V-SNDST-C` (classic-style Terra in Spaced Out), `AQU-C`.
- `SEED`: the world seed.
- `SETTINGS`, `STORY`, `MIXING`: base-36 codes for the game settings, story
  traits and DLC/biome mixing; `0` means defaults / none.

## npm package (WebAssembly)

[`oniworldgen`](https://www.npmjs.com/package/oniworldgen) runs the generator in
browsers and Node. It returns each cluster's worlds, biome polygons, geysers,
points of interest, world traits and starmap (no cell grids, no simulation; ambient
oil reservoirs are estimated).

```sh
npm install oniworldgen
```

```js
import init, { worldgen } from 'oniworldgen';

await init();
const map = worldgen.generate('SNDST-A-1-0-0-0');
```

See [npm/README.md](npm/README.md) for the API.

### Building the package

The module embeds the game's worldgen data, which is not in this repository: the
build reads it from your own installation.

Requirements: Rust with the `wasm32-unknown-unknown` target, `wasm-bindgen-cli`
at the version pinned in `wasm/Cargo.toml`, `wasm-opt` from
[binaryen](https://github.com/WebAssembly/binaryen/releases) (on `PATH`, or set
`WASM_OPT`) and Node.js.

```sh
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version 0.2.118

scripts/build-npm.sh --assets "$ASSETS" --pack
```

On Windows PowerShell:

```powershell
scripts/build-npm.ps1 -Assets "C:/Program Files (x86)/Steam/steamapps/common/OxygenNotIncluded/OxygenNotIncluded_Data/StreamingAssets" -Pack
```

The package is written to `dist/npm/`. Its version is the one in `wasm/Cargo.toml`
unless `--version` (`-Version`) is given; `worldgen.version()` returns
`"<game build>+<package version>"`. `--pack` lists the files `npm publish` would
include.

### Publishing

The build scripts never publish. After building and checking `dist/npm/`:

```sh
cd dist/npm && npm publish --access public
```

## Licence

GNU Affero General Public License v3.0 only; see [LICENSE](LICENSE). This
covers the npm package too. The game data the package embeds belongs to Klei
Entertainment; see [npm/NOTICE](npm/NOTICE).

This project is not affiliated with or endorsed by Klei Entertainment.
Oxygen Not Included is a trademark of Klei Entertainment.
