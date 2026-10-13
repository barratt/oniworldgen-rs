#!/usr/bin/env bash
# Build the `oniworldgen` npm package into dist/npm/.
#
#   scripts/build-npm.sh --assets STREAMING_ASSETS [--version X.Y.Z] [--out DIR] [--pack]
#
# --assets   the game's OxygenNotIncluded_Data/StreamingAssets (or set
#            ONI_STREAMING_ASSETS). The game data is read from it at build time
#            and embedded in the module; it is never written into the repository.
# --version  package version (default: the version in wasm/Cargo.toml).
# --out      output directory (default: dist/npm).
# --pack     run `npm pack --dry-run` on the result.
#
# Needs: the wasm32-unknown-unknown Rust target, wasm-bindgen-cli at the
# version wasm/Cargo.toml pins, wasm-opt from binaryen (on PATH or WASM_OPT)
# and Node.js. This script does not publish; see the README.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

assets="${ONI_STREAMING_ASSETS:-}"
version=""
out="$root/dist/npm"
pack=0
while [ $# -gt 0 ]; do
  case "$1" in
    --assets) assets="${2:?--assets needs a value}"; shift 2 ;;
    --version) version="${2:?--version needs a value}"; shift 2 ;;
    --out) out="${2:?--out needs a value}"; shift 2 ;;
    --pack) pack=1; shift ;;
    -h|--help) sed -n '2,17p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

die() { echo "build-npm: $*" >&2; exit 1; }

# ---- tools
[ -n "$assets" ] || die "pass --assets <StreamingAssets> (or set ONI_STREAMING_ASSETS)"
[ -d "$assets/worldgen/clusters" ] || die "$assets is not OxygenNotIncluded_Data/StreamingAssets"
command -v cargo >/dev/null || die "cargo not found"
command -v node >/dev/null || die "node not found"
if command -v rustup >/dev/null; then
  rustup target list --installed | grep -qx wasm32-unknown-unknown \
    || die "missing Rust target: rustup target add wasm32-unknown-unknown"
fi
bindgen_want="$(sed -n 's/^wasm-bindgen = "=\([0-9.]*\)".*/\1/p' wasm/Cargo.toml)"
WASM_BINDGEN="${WASM_BINDGEN:-wasm-bindgen}"
command -v "$WASM_BINDGEN" >/dev/null || die "wasm-bindgen not found: cargo install wasm-bindgen-cli --version $bindgen_want"
bindgen_have="$("$WASM_BINDGEN" --version | awk '{print $2}')"
[ "$bindgen_have" = "$bindgen_want" ] || die "wasm-bindgen $bindgen_have found, $bindgen_want needed: cargo install wasm-bindgen-cli --version $bindgen_want"
WASM_OPT="${WASM_OPT:-$(command -v wasm-opt || true)}"
[ -n "$WASM_OPT" ] || die "wasm-opt (binaryen) not found; put it on PATH or set WASM_OPT"

crate_version="$(sed -n 's/^version = "\(.*\)"/\1/p' wasm/Cargo.toml | head -1)"
version="${version:-$crate_version}"
game_build="$(node -p 'require("./npm/package.json").gameBuild')"
echo "oniworldgen $version (game build $game_build)"

target_dir="${CARGO_TARGET_DIR:-$root/target}"
work="$target_dir/npm"
mkdir -p "$work"

# ---- data snapshot from the local game install
snapshot="$work/worldgen-snapshot.bin"
cargo run --release --quiet -p worldgen-snapshot -- --assets "$assets" --output "$snapshot" --game-build "$game_build"

# ---- WebAssembly
# Panic locations name source files; keep local paths out of the module.
win() { cygpath -w "$1" 2>/dev/null || echo "$1"; }
cargo_home="${CARGO_HOME:-$HOME/.cargo}"
rustup_home="${RUSTUP_HOME:-$HOME/.rustup}"
# CARGO_ENCODED_RUSTFLAGS separates flags with 0x1f, so paths may hold spaces;
# with --target it applies to the WebAssembly artifacts only.
sep="$(printf '\037')"
flags="-Cstrip=symbols"
for pair in "$root=oniworldgen-rs" "$cargo_home=cargo" "$rustup_home=rustup"; do
  dir="${pair%=*}"; name="${pair##*=}"
  flags="$flags$sep--remap-path-prefix=$dir=$name"
  [ "$(win "$dir")" = "$dir" ] || flags="$flags$sep--remap-path-prefix=$(win "$dir")=$name"
done
export ONIWORLDGEN_SNAPSHOT="$(win "$snapshot")"
if [ "$version" != "$crate_version" ]; then export ONIWORLDGEN_PKG_VERSION="$version"; else unset ONIWORLDGEN_PKG_VERSION; fi
CARGO_ENCODED_RUSTFLAGS="$flags" \
  cargo build -p oniworldgen-wasm --profile wasm-release --target wasm32-unknown-unknown

rm -rf "$out"
mkdir -p "$out"
"$WASM_BINDGEN" --target web --out-dir "$out" --out-name worldgen --no-typescript \
  "$target_dir/wasm32-unknown-unknown/wasm-release/oniworldgen_wasm.wasm"
"$WASM_OPT" -O3 --strip-debug --strip-producers \
  --enable-bulk-memory --enable-nontrapping-float-to-int --enable-sign-ext --enable-mutable-globals \
  --enable-reference-types --enable-multivalue \
  "$out/worldgen_bg.wasm" -o "$out/worldgen_bg.wasm"

# ---- package
cp npm/index.js npm/index.d.ts npm/README.md npm/NOTICE "$out/"
cp LICENSE "$out/LICENSE"
node scripts/npm-package.mjs write npm/package.json "$(win "$out")" "$version"
# The module must report the version written into package.json.
node scripts/npm-package.mjs check "$(win "$out")"

ls -l "$out"
if [ "$pack" = 1 ]; then (cd "$out" && npm pack --dry-run); fi
echo "built $out; publish with: cd $out && npm publish --access public"
