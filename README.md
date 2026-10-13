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

## Licence

GNU Affero General Public License v3.0 only; see [LICENSE](LICENSE).

This project is not affiliated with or endorsed by Klei Entertainment.
Oxygen Not Included is a trademark of Klei Entertainment.
