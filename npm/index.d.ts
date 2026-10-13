// Types for oniworldgen (game build 744825, generation only).

export interface InitOptions {
  /** URL, Request, Response, bytes or compiled module of `worldgen_bg.wasm`. */
  module_or_path?: string | URL | Request | Response | BufferSource | WebAssembly.Module
    | Promise<string | URL | Request | Response | BufferSource | WebAssembly.Module>;
}

export interface GenerateOptions {
  /**
   * `"all"` (default): every template-placed building and entity.
   * `"map"`: only points of interest, `OilWell` and `UnderwaterVent`.
   */
  entities?: 'all' | 'map';
  /** Generate the coordinate the game substitutes for one it rewrites (e.g. KF23 seeds, dropped mixing). */
  acceptRewritten?: boolean;
  /** Append ambient `OilWell`s to `other_entities` (default `true`). */
  ambientOil?: boolean;
}

export interface MapData {
  coordinate: string;
  seed: number;
  /** Cluster YAML path, e.g. `expansion1::clusters/SandstoneStartCluster`. */
  cluster_id: string;
  /** e.g. `SNDST-C`, `V-BAD-C`, `M-FRZ-C`, `VOLCA`. */
  coordinate_prefix: string;
  /** One entry per asteroid, in world order (Spaced Out hexes; base game: one entry at 0,0). */
  starmap: { world_index: number; q: number; r: number }[];
  /** Spaced Out space POIs. */
  starmap_pois: { poi_type: string; q: number; r: number }[];
  /** Base-game rocket destinations, in the game's order; empty on Spaced Out. */
  vanilla_starmap: { type: string; distance: number }[];
  worlds: WorldMapData[];
  /** Set when the game's worldgen fails for this coordinate; the lists are then empty. */
  failure: { stage: string; world_index: number; message: string } | null;
  /** Warnings the game logs during worldgen; world entries are prefixed `world[N]: `. */
  telemetry: { category: string; message: string }[];
}

export interface WorldMapData {
  /** World path after world mixing, e.g. `expansion1::worlds/TerraMoonlet`. */
  name: string;
  width: number;
  height: number;
  is_starting: boolean;
  world_traits: string[];
  /** Overworld biome cells; `y` grows upwards (the game's world coordinates). */
  biome_cells: { id: number; zone_type: string | null; x: number; y: number; poly: number[] }[];
  geysers: GeyserSpawn[];
  /** Template-placed buildings. */
  buildings: EntitySpawn[];
  /**
   * Template-placed entities other than geysers, then ambient `OilWell`s in spawn order (see
   * `ambientOil`), placed without simulation: positions approximate, counts rarely off by one. No other
   * ambient (critter/plant) spawns: they need the settle simulation.
   */
  other_entities: EntitySpawn[];
}

export interface EntitySpawn { tag: string; x: number; y: number }

export interface GeyserSpawn extends EntitySpawn {
  /** Resolved geyser type, e.g. `steam`, `molten_iron`. */
  type: string;
  scaled_rate: number;
  scaled_iter_len: number;
  scaled_iter_pct: number;
  scaled_year_len: number;
  scaled_year_pct: number;
}

export default function init(options?: InitOptions): Promise<void>;

export const worldgen: {
  /** `"744825+<package version>"`. */
  version(): string;
  generate(coordinate: string, options?: GenerateOptions): MapData | { error: string };
};
