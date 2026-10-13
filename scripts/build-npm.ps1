# Build the `oniworldgen` npm package into dist/npm/ (PowerShell version of
# build-npm.sh).
#
#   scripts/build-npm.ps1 -Assets STREAMING_ASSETS [-Version X.Y.Z] [-Out DIR] [-Pack]
#
# -Assets   the game's OxygenNotIncluded_Data/StreamingAssets (or set
#           ONI_STREAMING_ASSETS). The game data is read from it at build time
#           and embedded in the module; it is never written into the repository.
# -Version  package version (default: the version in wasm/Cargo.toml).
# -Out      output directory (default: dist/npm).
# -Pack     run `npm pack --dry-run` on the result.
#
# Needs: the wasm32-unknown-unknown Rust target, wasm-bindgen-cli at the
# version wasm/Cargo.toml pins, wasm-opt from binaryen (on PATH or WASM_OPT)
# and Node.js. This script does not publish; see the README.
param(
    [string]$Assets = $env:ONI_STREAMING_ASSETS,
    [string]$Version = "",
    [string]$Out = "",
    [switch]$Pack
)
$ErrorActionPreference = "Stop"

$root = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
Set-Location $root
if (-not $Out) { $Out = Join-Path $root "dist/npm" }

function Die([string]$message) { Write-Error "build-npm: $message"; exit 1 }
function Run([string]$exe, [string[]]$arguments) {
    # Native tools log progress on stderr; only the exit code means failure.
    $ErrorActionPreference = "Continue"
    & $exe @arguments
    if ($LASTEXITCODE -ne 0) { Die "$exe failed with exit code $LASTEXITCODE" }
}

# ---- tools
if (-not $Assets) { Die "pass -Assets <StreamingAssets> (or set ONI_STREAMING_ASSETS)" }
if (-not (Test-Path (Join-Path $Assets "worldgen/clusters") -PathType Container)) { Die "$Assets is not OxygenNotIncluded_Data/StreamingAssets" }
foreach ($tool in "cargo", "node") { if (-not (Get-Command $tool -ErrorAction SilentlyContinue)) { Die "$tool not found" } }
if (Get-Command rustup -ErrorAction SilentlyContinue) {
    if (-not ((rustup target list --installed) -contains "wasm32-unknown-unknown")) { Die "missing Rust target: rustup target add wasm32-unknown-unknown" }
}
$manifest = Get-Content (Join-Path $root "wasm/Cargo.toml")
$bindgenWant = ($manifest | Select-String '^wasm-bindgen = "=([0-9.]+)"').Matches[0].Groups[1].Value
$wasmBindgen = if ($env:WASM_BINDGEN) { $env:WASM_BINDGEN } else { "wasm-bindgen" }
if (-not (Get-Command $wasmBindgen -ErrorAction SilentlyContinue)) { Die "wasm-bindgen not found: cargo install wasm-bindgen-cli --version $bindgenWant" }
$bindgenHave = ((& $wasmBindgen --version) -split " ")[1]
if ($bindgenHave -ne $bindgenWant) { Die "wasm-bindgen $bindgenHave found, $bindgenWant needed: cargo install wasm-bindgen-cli --version $bindgenWant" }
$wasmOpt = if ($env:WASM_OPT) { $env:WASM_OPT } else { (Get-Command wasm-opt -ErrorAction SilentlyContinue).Source }
if (-not $wasmOpt) { Die "wasm-opt (binaryen) not found; put it on PATH or set WASM_OPT" }

$crateVersion = ($manifest | Select-String '^version = "(.+)"').Matches[0].Groups[1].Value
if (-not $Version) { $Version = $crateVersion }
$gameBuild = (Get-Content (Join-Path $root "npm/package.json") -Raw | ConvertFrom-Json).gameBuild
Write-Host "oniworldgen $Version (game build $gameBuild)"

$targetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $root "target" }
$work = Join-Path $targetDir "npm"
New-Item -ItemType Directory -Force $work | Out-Null

# ---- data snapshot from the local game install
$snapshot = Join-Path $work "worldgen-snapshot.bin"
Run cargo @("run", "--release", "--quiet", "-p", "worldgen-snapshot", "--", "--assets", $Assets, "--output", $snapshot, "--game-build", $gameBuild)

# ---- WebAssembly
# Panic locations name source files; keep local paths out of the module.
# CARGO_ENCODED_RUSTFLAGS separates flags with 0x1f, so paths may hold spaces;
# with --target it applies to the WebAssembly artifacts only.
$cargoHome = if ($env:CARGO_HOME) { $env:CARGO_HOME } else { Join-Path $HOME ".cargo" }
$rustupHome = if ($env:RUSTUP_HOME) { $env:RUSTUP_HOME } else { Join-Path $HOME ".rustup" }
$flags = @("-Cstrip=symbols")
foreach ($pair in @(@($root, "oniworldgen-rs"), @($cargoHome, "cargo"), @($rustupHome, "rustup"))) {
    $flags += "--remap-path-prefix=$($pair[0])=$($pair[1])"
}
$saved = @{ flags = $env:CARGO_ENCODED_RUSTFLAGS; snapshot = $env:ONIWORLDGEN_SNAPSHOT; version = $env:ONIWORLDGEN_PKG_VERSION }
try {
    $env:CARGO_ENCODED_RUSTFLAGS = $flags -join [char]0x1f
    $env:ONIWORLDGEN_SNAPSHOT = $snapshot
    $env:ONIWORLDGEN_PKG_VERSION = if ($Version -ne $crateVersion) { $Version } else { $null }
    Run cargo @("build", "-p", "oniworldgen-wasm", "--profile", "wasm-release", "--target", "wasm32-unknown-unknown")
} finally {
    $env:CARGO_ENCODED_RUSTFLAGS = $saved.flags
    $env:ONIWORLDGEN_SNAPSHOT = $saved.snapshot
    $env:ONIWORLDGEN_PKG_VERSION = $saved.version
}

if (Test-Path $Out) { Remove-Item -Recurse -Force $Out }
New-Item -ItemType Directory -Force $Out | Out-Null
Run $wasmBindgen @("--target", "web", "--out-dir", $Out, "--out-name", "worldgen", "--no-typescript",
    (Join-Path $targetDir "wasm32-unknown-unknown/wasm-release/oniworldgen_wasm.wasm"))
$wasm = Join-Path $Out "worldgen_bg.wasm"
Run $wasmOpt @("-O3", "--strip-debug", "--strip-producers",
    "--enable-bulk-memory", "--enable-nontrapping-float-to-int", "--enable-sign-ext", "--enable-mutable-globals",
    "--enable-reference-types", "--enable-multivalue", $wasm, "-o", $wasm)

# ---- package
foreach ($f in "index.js", "index.d.ts", "README.md", "NOTICE") { Copy-Item (Join-Path $root "npm/$f") $Out }
Copy-Item (Join-Path $root "LICENSE") (Join-Path $Out "LICENSE")
Run node @((Join-Path $root "scripts/npm-package.mjs"), "write", (Join-Path $root "npm/package.json"), $Out, $Version)
# The module must report the version written into package.json.
Run node @((Join-Path $root "scripts/npm-package.mjs"), "check", $Out)

Get-ChildItem $Out | Format-Table Name, Length -AutoSize
if ($Pack) { Push-Location $Out; try { Run npm @("pack", "--dry-run") } finally { Pop-Location } }
Write-Host "built $Out; publish with: cd $Out; npm publish --access public"
